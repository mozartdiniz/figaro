//! Settings and the on-disk library cache, both plain JSON.
//!
//! - `~/.config/figaro/config.json`: optional MusicKit key location, storefront
//! - `~/.cache/figaro/web-token.json`: web player token, when there's no key
//! - `~/.cache/figaro/library.json`: last fetched songs and playlists, so the
//!   window fills instantly and works offline (playback still needs network)
//!
//! The Music User Token is a credential and goes to the keyring instead
//! (see `secrets.rs`).

use std::collections::HashMap;
use std::path::PathBuf;

use applemusic::{Playlist, Song};
use gtk::glib;
use serde::{Deserialize, Serialize};

pub const APP_ID: &str = "io.github.mozartdiniz.Figaro";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub team_id: String,
    #[serde(default)]
    pub key_id: String,
    /// Path to `AuthKey_<KEYID>.p8`. The file is read at startup, never copied.
    #[serde(default)]
    pub key_path: String,
    /// Learned from `/v1/me/storefront` after sign-in.
    #[serde(default)]
    pub storefront: Option<String>,
}

impl Config {
    fn path() -> PathBuf {
        glib::user_config_dir().join("figaro").join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        write_json(&Self::path(), self);
    }

    pub fn has_key(&self) -> bool {
        !self.team_id.is_empty() && !self.key_id.is_empty() && !self.key_path.is_empty()
    }

    /// Signs a fresh developer token (valid 30 days; we sign one per launch).
    fn own_developer_token(&self) -> anyhow::Result<String> {
        let pem = std::fs::read_to_string(&self.key_path)
            .map_err(|e| anyhow::anyhow!("can't read {}: {e}", self.key_path))?;
        let key = applemusic::DeveloperKey::from_pem(&self.team_id, &self.key_id, &pem)?;
        Ok(key.sign(now(), 30 * 24 * 3600))
    }

    /// The user's own MusicKit key when configured, otherwise the web
    /// player's token (cached; `refresh` forces a new download).
    pub async fn developer_token(&self, refresh: bool) -> anyhow::Result<DevToken> {
        if self.has_key() {
            return self.own_developer_token().map(DevToken::Own);
        }
        let cache = glib::user_cache_dir().join("figaro").join("web-token.json");
        if !refresh {
            let cached: Option<applemusic::WebToken> =
                std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice(&b).ok());
            // Renew a few days early rather than fail mid-session.
            if let Some(t) = cached.filter(|t| t.expires_at > now() + 3 * 24 * 3600) {
                return Ok(DevToken::Web(t.token));
            }
        }
        let fresh = crate::bg(applemusic::fetch_web_token())
            .await
            .map_err(|e| anyhow::anyhow!("couldn't get Apple Music's web token: {e}"))?;
        write_json(&cache, &fresh);
        Ok(DevToken::Web(fresh.token))
    }
}

/// Where the developer token came from, which decides how the API is called.
#[derive(Debug, Clone)]
pub enum DevToken {
    /// Signed from the user's own `.p8` key.
    Own(String),
    /// music.apple.com's web player token.
    Web(String),
}

impl DevToken {
    pub fn as_str(&self) -> &str {
        match self {
            DevToken::Own(t) | DevToken::Web(t) => t,
        }
    }

    pub fn is_web(&self) -> bool {
        matches!(self, DevToken::Web(_))
    }

    /// `user_token` may be empty for catalog-only use.
    pub fn client(&self, user_token: &str, storefront: &str) -> applemusic::Client {
        match self {
            DevToken::Own(t) => applemusic::Client::new(t, user_token, storefront),
            DevToken::Web(t) => applemusic::Client::web(t, user_token, storefront),
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LibraryCache {
    pub songs: Vec<Song>,
    pub playlists: Vec<Playlist>,
    /// Playlist ID → tracks, filled as playlists are opened.
    #[serde(default)]
    pub playlist_tracks: HashMap<String, Vec<Song>>,
}

impl LibraryCache {
    fn path() -> PathBuf {
        glib::user_cache_dir().join("figaro").join("library.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        write_json(&Self::path(), self);
    }

    pub fn clear() {
        let _ = std::fs::remove_file(Self::path());
    }
}

/// Writes via a temp file so a crash never leaves half a file behind.
fn write_json<T: Serialize>(path: &PathBuf, value: &T) {
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(value)?)?;
        std::fs::rename(tmp, path)
    })();
    if let Err(e) = result {
        log::warn!("could not write {}: {e}", path.display());
    }
}
