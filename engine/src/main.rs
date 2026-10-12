//! figaro-engine: the playback half of Figaro.
//!
//! Apple Music catalog audio is FairPlay/Widevine protected, so it only plays
//! through MusicKit JS in a browser engine with a licensed CDM. This process
//! is that engine: CEF (Chromium) with one hidden window, driven by the GTK
//! app through JSON lines.
//!
//! stdin  (app → engine): `{"cmd": "configure" | "authorize" | "play" | ...}`
//! stdout (engine → app): `{"event": "loaded" | "authorized" | "state" | ...}`
//! See `docs/ENGINE_PROTOCOL.md`.
//!
//! Chromium's helper processes re-run this same executable with `--type=…`;
//! they return from `execute_process` immediately and never read stdin.

mod host;
mod page;

use std::io::BufRead;
use std::path::PathBuf;

use cef::*;

fn main() {
    if !cfg!(target_os = "linux") {
        // macOS needs an app bundle, helper apps and NSApplication glue for
        // CEF, and has Music.app anyway. Windows is untested.
        eprintln!("figaro-engine currently runs on Linux only");
        std::process::exit(2);
    }

    let args = cef::args::Args::new();
    let Some(cmd_line) = args.as_cmd_line() else {
        eprintln!("failed to parse command line");
        std::process::exit(1);
    };
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let is_browser_process = cmd_line.has_switch(Some(&CefString::from("type"))) != 1;
    let ret = execute_process(Some(args.as_main_args()), None::<&mut App>, std::ptr::null_mut());
    if !is_browser_process {
        std::process::exit(ret);
    }

    let profile = profile_dir();
    let _ = std::fs::create_dir_all(&profile);
    let profile = profile.to_string_lossy().into_owned();
    let settings = Settings {
        // chrome-sandbox needs a setuid helper on most distros. The engine
        // only ever loads its own page and Apple's sign-in.
        no_sandbox: 1,
        root_cache_path: CefString::from(profile.as_str()),
        // Same directory: keeps MusicKit's localStorage (the sign-in),
        // cookies and the downloaded Widevine CDM across launches.
        cache_path: CefString::from(profile.as_str()),
        persist_session_cookies: 1,
        log_severity: LogSeverity::WARNING,
        ..Default::default()
    };
    let show = std::env::args().any(|a| a == "--show");
    let mut app = host::EngineApp::new(page::URL.to_owned(), show);
    if initialize(Some(args.as_main_args()), Some(&settings), Some(&mut app), std::ptr::null_mut()) != 1 {
        host::emit_error("CEF failed to initialise");
        std::process::exit(1);
    }

    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if !line.trim().is_empty() {
                host::post_command(line);
            }
        }
        // The app went away: shut down with it.
        host::post_command(r#"{"cmd":"quit"}"#.to_owned());
    });

    run_message_loop();
    shutdown();
}

fn profile_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("FIGARO_ENGINE_PROFILE") {
        return p.into();
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    data.join("figaro").join("engine")
}
