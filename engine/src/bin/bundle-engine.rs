//! `just engine`: builds figaro-engine and copies it, with the CEF runtime
//! (libcef.so, resources, locales), into `engine/bundle/`.
//! Usage: bundle-engine <output dir> [--release]

#[cfg(target_os = "linux")]
fn main() {
    let mut args = std::env::args().skip(1);
    let output = std::path::PathBuf::from(args.next().unwrap_or_else(|| "engine/bundle".into()));
    let release = args.any(|a| a == "--release");
    match cef::build_util::linux::build_bundle(&output, "figaro-engine", release) {
        Ok(path) => println!("engine bundled at {}", path.display()),
        Err(e) => {
            eprintln!("bundling failed: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("the playback engine is bundled on Linux only");
    std::process::exit(2);
}
