//! The CEF side: one hidden browser window running `player.html`.
//!
//! All browser state lives on CEF's UI thread in [`STATE`]. The stdin
//! reader thread never touches it directly; it posts a [`Command`] task.

use std::cell::RefCell;
use std::io::Write;

use cef::*;

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

#[derive(Default)]
struct State {
    main: Option<Browser>,
    open_browsers: usize,
    /// `musickitloaded` fired; commands can run.
    page_ready: bool,
    /// Commands that arrived before the page was ready.
    pending: Vec<String>,
    quitting: bool,
}

/// Writes one protocol line. Only lines that are JSON objects with an
/// `event` key are protocol; the app ignores anything else on stdout.
pub fn emit(json: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{json}");
    let _ = out.flush();
}

pub fn emit_error(message: &str) {
    emit(&serde_json::json!({ "event": "error", "message": message }).to_string());
}

fn run_js(code: &str) {
    STATE.with_borrow(|s| {
        if let Some(frame) = s.main.as_ref().and_then(|b| b.main_frame()) {
            frame.execute_java_script(Some(&CefString::from(code)), None, 0);
        }
    });
}

/// Handles one stdin line on the UI thread.
fn handle_command(line: &str) {
    let Ok(cmd) = serde_json::from_str::<serde_json::Value>(line) else {
        emit_error(&format!("bad command: {line}"));
        return;
    };
    match cmd["cmd"].as_str() {
        Some("quit") => quit(),
        Some("show") => set_main_window_visible(true),
        Some("hide") => set_main_window_visible(false),
        Some(_) => {
            // Re-serialised JSON is a valid JS expression: no injection.
            let js = format!("figaro.dispatch({cmd})");
            let ready = STATE.with_borrow_mut(|s| {
                if !s.page_ready {
                    s.pending.push(js.clone());
                }
                s.page_ready
            });
            if ready {
                run_js(&js);
            }
        }
        None => emit_error(&format!("command without cmd: {line}")),
    }
}

fn on_page_message(json: &str) {
    if json.contains(r#""event":"loaded""#) {
        let pending = STATE.with_borrow_mut(|s| {
            s.page_ready = true;
            std::mem::take(&mut s.pending)
        });
        for js in pending {
            run_js(&js);
        }
    }
    emit(json);
}

fn set_main_window_visible(visible: bool) {
    let mut main = STATE.with_borrow(|s| s.main.clone());
    if let Some(window) = browser_view_get_for_browser(main.as_mut()).and_then(|v| v.window()) {
        if visible {
            window.show();
            window.activate();
        } else {
            window.hide();
        }
    }
}

pub fn quit() {
    let browsers = STATE.with_borrow_mut(|s| {
        s.quitting = true;
        s.open_browsers
    });
    if browsers == 0 {
        quit_message_loop();
        return;
    }
    let main = STATE.with_borrow(|s| s.main.clone());
    if let Some(host) = main.and_then(|b| b.host()) {
        host.close_browser(1);
    }
}

/// Called from the stdin thread.
pub fn post_command(line: String) {
    let mut task = Command::new(line);
    post_task(ThreadId::UI, Some(&mut task));
}

wrap_task! {
    struct Command {
        line: String,
    }

    impl Task {
        fn execute(&self) {
            handle_command(&self.line);
        }
    }
}

wrap_app! {
    pub struct EngineApp {
        url: String,
        show: bool,
    }

    impl App {
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let Some(cl) = command_line else { return };
            // The window is hidden, so there is never a user gesture.
            cl.append_switch_with_value(
                Some(&CefString::from("autoplay-policy")),
                Some(&CefString::from("no-user-gesture-required")),
            );
            // Widevine arrives through Chromium's component updater; keep it on.
            cl.append_switch(Some(&CefString::from("enable-widevine-cdm")));
            // No GPU needed for an invisible audio page.
            cl.append_switch(Some(&CefString::from("disable-gpu")));
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(EngineBrowserProcessHandler::new(self.url.clone(), self.show))
        }
    }
}

wrap_browser_process_handler! {
    struct EngineBrowserProcessHandler {
        url: String,
        show: bool,
    }

    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            crate::page::register();
            let mut client = EngineClient::new();
            let mut delegate = EngineBrowserViewDelegate::new();
            let browser_view = browser_view_create(
                Some(&mut client),
                Some(&CefString::from(self.url.as_str())),
                Some(&BrowserSettings::default()),
                None,
                None,
                Some(&mut delegate),
            );
            let mut window = EngineWindowDelegate::new(RefCell::new(browser_view), self.show, true);
            window_create_top_level(Some(&mut window));
        }
    }
}

wrap_window_delegate! {
    struct EngineWindowDelegate {
        browser_view: RefCell<Option<BrowserView>>,
        show: bool,
        // The player window hides instead of closing; popups close normally.
        is_main: bool,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            Size { width: 520, height: 640 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let browser_view = self.browser_view.borrow();
            let (Some(window), Some(browser_view)) = (window, browser_view.as_ref()) else { return };
            window.set_title(Some(&CefString::from(if self.is_main { "Figaro Engine" } else { "Apple Music" })));
            let mut view = View::from(browser_view);
            window.add_child_view(Some(&mut view));
            if self.show {
                window.show();
            }
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            *self.browser_view.borrow_mut() = None;
        }

        fn can_close(&self, window: Option<&mut Window>) -> i32 {
            if self.is_main && !STATE.with_borrow(|s| s.quitting) {
                if let Some(w) = window {
                    w.hide();
                }
                return 0;
            }
            let browser_view = self.browser_view.borrow();
            match browser_view.as_ref().and_then(|v| v.browser()).and_then(|b| b.host()) {
                Some(host) => host.try_close_browser(),
                None => 1,
            }
        }
    }
}

wrap_browser_view_delegate! {
    struct EngineBrowserViewDelegate {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        // MusicKit's `authorize()` opens a popup to Apple's sign-in page.
        // That one the user must see.
        fn on_popup_browser_view_created(
            &self,
            _browser_view: Option<&mut BrowserView>,
            popup_browser_view: Option<&mut BrowserView>,
            _is_devtools: i32,
        ) -> i32 {
            let mut delegate =
                EngineWindowDelegate::new(RefCell::new(popup_browser_view.cloned()), true, false);
            window_create_top_level(Some(&mut delegate));
            1
        }
    }
}

wrap_client! {
    struct EngineClient {}

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(EngineDisplayHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(EngineLifeSpanHandler::new())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(EngineLoadHandler::new())
        }
    }
}

wrap_display_handler! {
    struct EngineDisplayHandler {}

    impl DisplayHandler {
        fn on_console_message(
            &self,
            _browser: Option<&mut Browser>,
            _level: LogSeverity,
            message: Option<&CefString>,
            source: Option<&CefString>,
            line: i32,
        ) -> i32 {
            let message = message.map(CefString::to_string).unwrap_or_default();
            if let Some(json) = message.strip_prefix("FIGARO:") {
                on_page_message(json);
                return 1;
            }
            let source = source.map(CefString::to_string).unwrap_or_default();
            log::debug!("console {source}:{line}: {message}");
            1
        }
    }
}

wrap_life_span_handler! {
    struct EngineLifeSpanHandler {}

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            STATE.with_borrow_mut(|s| {
                s.open_browsers += 1;
                if s.main.is_none() {
                    s.main = browser.cloned();
                }
            });
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let mut browser = browser.cloned();
            let done = STATE.with_borrow_mut(|s| {
                s.open_browsers = s.open_browsers.saturating_sub(1);
                let is_main = s.main.as_ref().is_some_and(|m| m.is_same(browser.as_mut()) != 0);
                if is_main {
                    s.main = None;
                    s.quitting = true;
                }
                s.quitting && (is_main || s.open_browsers == 0)
            });
            if done {
                quit_message_loop();
            }
        }
    }
}

wrap_load_handler! {
    struct EngineLoadHandler {}

    impl LoadHandler {
        fn on_load_error(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            error_code: Errorcode,
            error_text: Option<&CefString>,
            failed_url: Option<&CefString>,
        ) {
            if frame.is_some_and(|f| f.is_main() == 0) {
                return;
            }
            let code = sys::cef_errorcode_t::from(error_code);
            if code == sys::cef_errorcode_t::ERR_ABORTED {
                return;
            }
            let text = error_text.map(CefString::to_string).unwrap_or_default();
            let url = failed_url.map(CefString::to_string).unwrap_or_default();
            emit_error(&format!("could not load {url}: {text}"));
        }
    }
}
