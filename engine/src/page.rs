//! Serves `player.html` as `https://music.apple.com/figaro-engine`.
//!
//! Without the user's own MusicKit key, the developer token is the web
//! player's, which Apple only accepts from `apple.com` origins. So the page
//! has to *be* on music.apple.com: CEF answers this one URL from memory, and
//! every other music.apple.com request goes to the network as usual. It's
//! also a secure context (EME needs one), and MusicKit keeps its sign-in in
//! this origin's localStorage in the engine profile.

use cef::wrapper::stream_resource_handler::StreamResourceHandler;
use cef::*;

const PAGE: &str = include_str!("../web/player.html");
const HOST: &str = "music.apple.com";
pub const URL: &str = "https://music.apple.com/figaro-engine";

/// Call on the UI thread before creating the browser.
pub fn register() {
    let mut factory = PageFactory::new();
    register_scheme_handler_factory(
        Some(&CefString::from("https")),
        Some(&CefString::from(HOST)),
        Some(&mut factory),
    );
}

fn is_page(url: &str) -> bool {
    let rest = url.strip_prefix(URL).unwrap_or("-");
    rest.is_empty() || rest.starts_with('?') || rest.starts_with('#')
}

wrap_scheme_handler_factory! {
    struct PageFactory {}

    impl SchemeHandlerFactory {
        fn create(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _scheme_name: Option<&CefString>,
            request: Option<&mut Request>,
        ) -> Option<ResourceHandler> {
            let url = CefString::from(&request?.url()).to_string();
            if !is_page(&url) {
                // Not ours: let CEF fetch it from the network.
                return None;
            }
            // PAGE is 'static and CEF only reads from the pointer.
            let stream = stream_reader_create_for_data(PAGE.as_ptr() as *mut u8, PAGE.len())?;
            Some(StreamResourceHandler::new_with_stream("text/html".to_owned(), stream))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_page;

    #[test]
    fn only_intercepts_our_page() {
        assert!(is_page("https://music.apple.com/figaro-engine"));
        assert!(is_page("https://music.apple.com/figaro-engine?x=1"));
        assert!(!is_page("https://music.apple.com/figaro-engine-x"));
        assert!(!is_page("https://music.apple.com/us/browse"));
    }
}
