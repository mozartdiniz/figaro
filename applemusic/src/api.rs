//! Apple Music API (`api.music.apple.com`) — plain JSON over HTTPS.
//!
//! With the web player's token ([`Client::web`]) requests go where
//! music.apple.com sends them, `amp-api.music.apple.com`, with its origin.
//!
//! Every request carries `Authorization: Bearer <developer token>`; anything
//! under `/v1/me/` also needs `Music-User-Token`, which MusicKit JS hands out
//! after the user signs in (see the engine).

use std::time::Duration;

use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::model::{
    Page, Playlist, PlaylistAttributes, Resource, Song, SongAttributes, Storefront, TrackRef,
};
use crate::Error;

const BASE: &str = "https://api.music.apple.com";
const WEB_BASE: &str = "https://amp-api.music.apple.com";
/// Largest page the library endpoints accept.
const LIBRARY_PAGE: usize = 100;
/// Largest page the search endpoint accepts per type.
const SEARCH_PAGE: usize = 25;
const MAX_RETRIES: u32 = 4;

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    developer_token: String,
    user_token: String,
    storefront: String,
    /// Developer token is the web player's: use its host and origin.
    web: bool,
}

impl Client {
    /// `storefront` is the two-letter catalog region (`us`, `br`, …); use
    /// [`Client::fetch_storefront`] to learn the user's. `user_token` may be
    /// empty, which limits the client to catalog requests.
    pub fn new(developer_token: &str, user_token: &str, storefront: &str) -> Self {
        Client {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("TLS backend available"),
            developer_token: developer_token.to_owned(),
            user_token: user_token.to_owned(),
            storefront: storefront.to_owned(),
            web: false,
        }
    }

    /// A client using the web player's token (see [`crate::webtoken`]).
    pub fn web(web_token: &str, user_token: &str, storefront: &str) -> Self {
        Client { web: true, ..Self::new(web_token, user_token, storefront) }
    }

    pub fn storefront(&self) -> &str {
        &self.storefront
    }

    pub fn set_storefront(&mut self, storefront: &str) {
        self.storefront = storefront.to_owned();
    }

    pub async fn fetch_storefront(&self) -> Result<String, Error> {
        let page: Page<Storefront> = self.get_json("/v1/me/storefront").await?;
        page.data.into_iter().next().map(|s| s.id).ok_or(Error::Unexpected("no storefront"))
    }

    /// Searches the Apple Music catalog for songs.
    pub async fn search_catalog(&self, term: &str, max: usize) -> Result<Vec<Song>, Error> {
        #[derive(serde::Deserialize)]
        struct Results {
            #[serde(default)]
            results: SearchResults,
        }
        #[derive(Default, serde::Deserialize)]
        struct SearchResults {
            songs: Option<Page<Resource<SongAttributes>>>,
        }

        let mut out = Vec::new();
        let mut offset = 0;
        while out.len() < max {
            let path = format!(
                "/v1/catalog/{}/search?types=songs&limit={SEARCH_PAGE}&offset={offset}&term={}",
                self.storefront,
                encode(term)
            );
            let res: Results = self.get_json(&path).await?;
            let Some(page) = res.results.songs else { break };
            let n = page.data.len();
            out.extend(page.data.into_iter().filter_map(Song::from_resource));
            if page.next.is_none() || n == 0 {
                break;
            }
            offset += n;
        }
        out.truncate(max);
        Ok(out)
    }

    /// Every song in the user's library. `progress` gets the running count
    /// after each page; large libraries take one request per 100 songs.
    pub async fn library_songs(&self, mut progress: impl FnMut(usize)) -> Result<Vec<Song>, Error> {
        let first = format!("/v1/me/library/songs?limit={LIBRARY_PAGE}");
        let mut out = Vec::new();
        self.paginate::<SongAttributes>(&first, |r| {
            out.extend(r.into_iter().filter_map(Song::from_resource));
            progress(out.len());
        })
        .await?;
        Ok(out)
    }

    pub async fn library_playlists(&self) -> Result<Vec<Playlist>, Error> {
        let first = format!("/v1/me/library/playlists?limit={LIBRARY_PAGE}");
        let mut out = Vec::new();
        self.paginate::<PlaylistAttributes>(&first, |r| {
            out.extend(r.into_iter().map(Playlist::from_resource))
        })
        .await?;
        Ok(out)
    }

    pub async fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Song>, Error> {
        let first = format!("/v1/me/library/playlists/{}/tracks?limit={LIBRARY_PAGE}", encode(playlist_id));
        let mut out = Vec::new();
        match self
            .paginate::<SongAttributes>(&first, |r| out.extend(r.into_iter().filter_map(Song::from_resource)))
            .await
        {
            // An empty playlist answers 404 rather than an empty page.
            Err(Error::Status { status: 404, .. }) => Ok(Vec::new()),
            other => other.map(|()| out),
        }
    }

    /// Adds catalog songs to the user's library. Apple answers 202 and
    /// applies it asynchronously, so a library fetch right after may not
    /// show them yet.
    pub async fn add_to_library(&self, catalog_song_ids: &[&str]) -> Result<(), Error> {
        if catalog_song_ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = catalog_song_ids.iter().map(|id| encode(id)).collect();
        let path = format!("/v1/me/library?ids[songs]={}", ids.join(","));
        self.send(Method::POST, &path, None).await.map(drop)
    }

    pub async fn create_playlist(
        &self,
        name: &str,
        description: Option<&str>,
        tracks: &[TrackRef],
    ) -> Result<Playlist, Error> {
        let mut body = json!({ "attributes": { "name": name } });
        if let Some(d) = description {
            body["attributes"]["description"] = json!(d);
        }
        if !tracks.is_empty() {
            body["relationships"] = json!({ "tracks": { "data": tracks } });
        }
        let text = self.send(Method::POST, "/v1/me/library/playlists", Some(body)).await?;
        let page: Page<Resource<PlaylistAttributes>> = serde_json::from_str(&text)?;
        page.data
            .into_iter()
            .next()
            .map(Playlist::from_resource)
            .ok_or(Error::Unexpected("no playlist returned"))
    }

    pub async fn add_to_playlist(&self, playlist_id: &str, tracks: &[TrackRef]) -> Result<(), Error> {
        if tracks.is_empty() {
            return Ok(());
        }
        let path = format!("/v1/me/library/playlists/{}/tracks", encode(playlist_id));
        self.send(Method::POST, &path, Some(json!({ "data": tracks }))).await.map(drop)
    }

    // ── plumbing ──────────────────────────────────────────────────────────

    async fn paginate<A: DeserializeOwned>(
        &self,
        first: &str,
        mut sink: impl FnMut(Vec<Resource<A>>),
    ) -> Result<(), Error> {
        let mut next = Some(first.to_owned());
        while let Some(path) = next {
            let page: Page<Resource<A>> = self.get_json(&path).await?;
            next = page.next;
            // `next` drops the limit we asked for; keep pages large.
            if let Some(n) = next.as_mut() {
                if !n.contains("limit=") {
                    n.push(if n.contains('?') { '&' } else { '?' });
                    n.push_str(&format!("limit={LIBRARY_PAGE}"));
                }
            }
            sink(page.data);
        }
        Ok(())
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let text = self.send(Method::GET, path, None).await?;
        Ok(serde_json::from_str(&text)?)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let base = if self.web { WEB_BASE } else { BASE };
        let mut req = self.http.request(method, format!("{base}{path}")).bearer_auth(&self.developer_token);
        if self.web {
            // The token is only valid from apple.com origins.
            req = req
                .header("Origin", crate::webtoken::WEB_ORIGIN)
                .header("Referer", format!("{}/", crate::webtoken::WEB_ORIGIN));
        }
        // Catalog requests work without a user token (before sign-in).
        if self.user_token.is_empty() {
            req
        } else {
            req.header("Music-User-Token", &self.user_token)
        }
    }

    pub fn has_user_token(&self) -> bool {
        !self.user_token.is_empty()
    }

    /// Downloads artwork from a template URL (see [`crate::artwork_url`]).
    pub async fn fetch_artwork(&self, template: &str, size: u32) -> Result<Vec<u8>, Error> {
        let resp = self.http.get(crate::artwork_url(template, size)).send().await?.error_for_status()?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Sends with retries on rate limiting (429) and server errors (5xx).
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<String, Error> {
        let mut attempt = 0;
        loop {
            let mut req = self.request(method.clone(), path);
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            if status.is_success() {
                return Ok(resp.text().await?);
            }
            let retryable = status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if retryable && attempt < MAX_RETRIES {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
                    .unwrap_or(1 << attempt);
                log::warn!("{method} {path}: {status}, retrying in {wait}s");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                attempt += 1;
                continue;
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Error::Unauthorized(body),
                _ => Error::Status { status: status.as_u16(), body },
            });
        }
    }
}

/// Percent-encodes a query/path component (RFC 3986 unreserved chars kept).
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn encodes_search_terms() {
        assert_eq!(encode("Björk & co"), "Bj%C3%B6rk%20%26%20co");
        assert_eq!(encode("i.AbC-1_~"), "i.AbC-1_~");
    }
}
