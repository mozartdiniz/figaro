mod config;
mod playback;
mod player_bar;
mod secrets;
mod song_table;
mod window;

use adw::prelude::*;
use gtk::glib;

/// Tokio runtime for network and process I/O; GTK owns the main thread.
static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get().expect("tokio runtime not initialised")
}

/// Runs `fut` on tokio and awaits it from any executor (GTK's included).
pub async fn bg<F>(fut: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(fut).await.expect("background task panicked")
}

fn main() -> glib::ExitCode {
    pretty_env_logger::init();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    assert!(RUNTIME.set(rt).is_ok(), "runtime set once");

    glib::set_application_name("Figaro");
    let app = adw::Application::builder().application_id(config::APP_ID).build();
    app.connect_startup(|app| {
        // GNOME Music's shortcuts.
        app.set_accels_for_action("win.toggle", &["<Control>space"]);
        app.set_accels_for_action("win.next", &["<Control>n"]);
        app.set_accels_for_action("win.previous", &["<Control>b"]);
        app.set_accels_for_action("win.filter", &["<Control>f"]);
        app.set_accels_for_action("win.refresh", &["<Control>r"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        let quit = gtk::gio::SimpleAction::new("quit", None);
        let a = app.clone();
        quit.connect_activate(move |_, _| a.quit());
        app.add_action(&quit);
    });
    app.connect_activate(|app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        let win = window::build(app);
        win.present();
        if let Some(path) = std::env::var_os("FIGARO_SNAPSHOT") {
            snapshot_later(win.upcast(), path.into());
        }
    });
    app.run()
}

/// Dev aid: `FIGARO_SNAPSHOT=out.png` renders the window to a PNG after
/// two seconds (for checking layouts where screen capture isn't available).
/// `FIGARO_SNAPSHOT_SCRIPT` runs window actions first, comma-separated.
fn snapshot_later(win: gtk::Window, path: std::path::PathBuf) {
    glib::timeout_add_seconds_local_once(2, move || {
        if let Ok(script) = std::env::var("FIGARO_SNAPSHOT_SCRIPT") {
            for action in script.split(',').filter(|a| !a.is_empty()) {
                let _ = win.activate_action(action, None);
            }
        }
        glib::timeout_add_seconds_local_once(1, move || {
            let (w, h) = (win.width(), win.height());
            let paintable = gtk::WidgetPaintable::new(Some(&win));
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, w as f64, h as f64);
            let (Some(node), Some(renderer)) = (snapshot.to_node(), win.renderer()) else { return };
            let texture = renderer.render_texture(node, None);
            match texture.save_to_png(&path) {
                Ok(()) => log::info!("snapshot written to {}", path.display()),
                Err(e) => log::warn!("snapshot failed: {e}"),
            }
        });
    });
}
