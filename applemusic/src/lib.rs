//! Apple Music for Figaro: developer token signing and the Apple Music API.
//!
//! Playback is not here. Catalog audio is DRM-protected and only plays
//! through MusicKit JS in a Widevine-capable browser engine (see `engine/`).

pub mod api;
pub mod model;
pub mod token;
pub mod webtoken;

pub use api::Client;
pub use model::{artwork_url, Playlist, Song, Source, TrackRef};
pub use token::DeveloperKey;
pub use webtoken::{fetch_web_token, WebToken};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected response: {0}")]
    Json(#[from] serde_json::Error),
    /// 401/403: the developer token is invalid or the Music User Token has
    /// expired or been revoked. The user has to sign in again.
    #[error("not authorized: {0}")]
    Unauthorized(String),
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("invalid MusicKit key: {0}")]
    Key(String),
    #[error("{0}")]
    Unexpected(&'static str),
}
