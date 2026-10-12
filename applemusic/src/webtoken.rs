//! The developer token Apple's own web player uses, for users without an
//! Apple Developer membership.
//!
//! music.apple.com ships it in its main script (`/assets/index~<hash>.js`)
//! next to a couple of other Apple tokens. The web player's has
//! `kid: "WebPlayKid"`, `iss: "AMPWebPlay"`, lives ~70 days and is
//! restricted to `apple.com` origins (`root_https_origin`), so requests must
//! carry `Origin: https://music.apple.com` (see [`crate::Client::web`]).
//! Apple rotates it with each web player release; fetch again on expiry or 401.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::Error;

pub const WEB_ORIGIN: &str = "https://music.apple.com";
const PAGE: &str = "https://music.apple.com/us/browse";
const USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebToken {
    pub token: String,
    /// Unix seconds (`exp` claim).
    pub expires_at: u64,
}

/// Downloads music.apple.com and extracts the web player token.
pub async fn fetch_web_token() -> Result<WebToken, Error> {
    let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let html = http.get(PAGE).send().await?.error_for_status()?.text().await?;
    let mut scripts = script_paths(&html);
    // The main bundle is the likeliest home; try it first.
    scripts.sort_by_key(|p| !p.contains("/index"));
    for path in scripts {
        let js = http.get(format!("{WEB_ORIGIN}{path}")).send().await?.error_for_status()?.text().await?;
        if let Some(t) = find_web_token(&js) {
            return Ok(t);
        }
    }
    Err(Error::Unexpected("no web player token found on music.apple.com"))
}

/// `/assets/….js` paths referenced by the page.
fn script_paths(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in html.match_indices("/assets/") {
        let rest = &html[i..];
        let end = rest.find(['"', '\'', ' ', '>']).unwrap_or(rest.len());
        let path = &rest[..end];
        if path.ends_with(".js") && !out.iter().any(|p| p == path) {
            out.push(path.to_owned());
        }
    }
    out
}

/// Picks the web player token out of a script. Other Apple tokens in the
/// same bundle are ignored.
pub fn find_web_token(js: &str) -> Option<WebToken> {
    let mut fallback = None;
    for jwt in jwts(js) {
        let Some((header, claims)) = decode(jwt) else { continue };
        if header["alg"] != "ES256" {
            continue;
        }
        let Some(exp) = claims["exp"].as_u64() else { continue };
        let token = WebToken { token: jwt.to_owned(), expires_at: exp };
        if header["kid"] == "WebPlayKid" || claims["iss"] == "AMPWebPlay" {
            return Some(token);
        }
        // An unrecognised ES256 token without extra restrictions beyond
        // apple.com would probably work too; keep it as a last resort.
        if fallback.is_none() && claims.get("origin").is_none() {
            fallback = Some(token);
        }
    }
    fallback
}

fn jwts(text: &str) -> impl Iterator<Item = &str> {
    let b64url = |c: u8| c.is_ascii_alphanumeric() || c == b'-' || c == b'_';
    text.match_indices("eyJ").filter_map(move |(start, _)| {
        let bytes = text.as_bytes();
        let mut end = start;
        while end < bytes.len() && (b64url(bytes[end]) || bytes[end] == b'.') {
            end += 1;
        }
        let candidate = text[start..end].trim_end_matches('.');
        let parts: Vec<&str> = candidate.split('.').collect();
        (parts.len() == 3 && parts[1].starts_with("eyJ") && parts[2].len() >= 40).then_some(candidate)
    })
}

fn decode(jwt: &str) -> Option<(serde_json::Value, serde_json::Value)> {
    let mut parts = jwt.split('.');
    let part = |p: &str| -> Option<serde_json::Value> {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok()?).ok()
    };
    Some((part(parts.next()?)?, part(parts.next()?)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(header: &str, claims: &str) -> String {
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(header),
            URL_SAFE_NO_PAD.encode(claims),
            URL_SAFE_NO_PAD.encode([7u8; 64])
        )
    }

    #[test]
    fn picks_the_web_player_token() {
        let other = jwt(
            r#"{"typ":"JWT","alg":"ES256","kid":"AAAA"}"#,
            r#"{"iss":"X","iat":1,"exp":2,"root_https_origin":["apple.com"],"origin":"*.apple.com"}"#,
        );
        let web = jwt(
            r#"{"typ":"JWT","alg":"ES256","kid":"WebPlayKid"}"#,
            r#"{"iss":"AMPWebPlay","iat":1,"exp":6048001,"root_https_origin":["apple.com"]}"#,
        );
        let js = format!(r#"const a="{other}",b={{token:"{web}"}};x.eyJ.notatoken;"#);
        let found = find_web_token(&js).expect("found");
        assert_eq!(found.token, web);
        assert_eq!(found.expires_at, 6048001);
    }

    #[test]
    fn finds_nothing_in_plain_code() {
        assert!(find_web_token("function eyJunk(){return 'eyJ.eyJ.x'}").is_none());
    }

    #[test]
    fn lists_scripts_once() {
        let html = r#"<script src="/assets/index~abc.js"></script><link href="/assets/x.css"><script src="/assets/index~abc.js">"#;
        assert_eq!(script_paths(html), vec!["/assets/index~abc.js"]);
    }
}
