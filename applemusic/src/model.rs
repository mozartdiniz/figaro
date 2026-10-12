//! Apple Music API resources, flattened into what the UI needs.
//!
//! The API wraps everything as `{ id, type, href, attributes: {...} }` inside
//! `{ data: [...], next: "/v1/...?offset=N" }` pages. Catalog songs (`songs`)
//! and library songs (`library-songs`) share most attribute names, so both
//! become a [`Song`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(crate) struct Page<T> {
    #[serde(default = "Vec::new")]
    pub data: Vec<T>,
    pub next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Resource<A> {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub attributes: Option<A>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SongAttributes {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub album_name: String,
    pub duration_in_millis: Option<u64>,
    #[serde(default)]
    pub genre_names: Vec<String>,
    pub release_date: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub date_added: Option<String>,
    pub artwork: Option<Artwork>,
    pub play_params: Option<PlayParams>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlayParams {
    pub catalog_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Artwork {
    pub url: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaylistAttributes {
    #[serde(default)]
    pub name: String,
    pub description: Option<Description>,
    #[serde(default)]
    pub can_edit: bool,
    pub date_added: Option<String>,
    pub artwork: Option<Artwork>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Description {
    pub standard: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Storefront {
    pub id: String,
}

/// Where a song lives, which decides how it is referenced in requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// `songs` resource: Apple Music catalog, numeric ID.
    Catalog,
    /// `library-songs` resource: the user's library, `i.` ID.
    Library,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Song {
    pub id: String,
    pub source: Source,
    /// Catalog ID of a library song, when it is (matched to) a catalog song.
    pub catalog_id: Option<String>,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub genre: String,
    /// `YYYY-MM-DD` or `YYYY`; may be missing for uploaded songs.
    pub release_date: Option<String>,
    pub track_number: u32,
    pub disc_number: u32,
    pub date_added: Option<String>,
    /// Template URL with `{w}` and `{h}` placeholders.
    pub artwork_url: Option<String>,
}

impl Song {
    pub(crate) fn from_resource(r: Resource<SongAttributes>) -> Option<Self> {
        let source = match r.kind.as_str() {
            "songs" => Source::Catalog,
            "library-songs" => Source::Library,
            _ => return None, // music videos, uploaded videos, …
        };
        let a = r.attributes.unwrap_or_default();
        let catalog_id = match source {
            Source::Catalog => Some(r.id.clone()),
            Source::Library => a.play_params.and_then(|p| p.catalog_id),
        };
        Some(Song {
            id: r.id,
            source,
            catalog_id,
            name: a.name,
            artist: a.artist_name,
            album: a.album_name,
            duration_ms: a.duration_in_millis.unwrap_or(0),
            genre: a.genre_names.into_iter().find(|g| g != "Music").unwrap_or_default(),
            release_date: a.release_date,
            track_number: a.track_number.unwrap_or(0),
            disc_number: a.disc_number.unwrap_or(1),
            date_added: a.date_added,
            artwork_url: a.artwork.map(|a| a.url),
        })
    }

    /// The ID to hand to MusicKit for playback. Catalog IDs play most
    /// reliably; uploaded songs only have their library ID.
    pub fn playback_id(&self) -> &str {
        self.catalog_id.as_deref().unwrap_or(&self.id)
    }

    pub fn year(&self) -> Option<u32> {
        self.release_date.as_deref()?.get(..4)?.parse().ok()
    }

    /// Reference used when adding this song to a playlist.
    pub fn track_ref(&self) -> TrackRef {
        match self.source {
            Source::Catalog => TrackRef { id: self.id.clone(), kind: "songs" },
            Source::Library => TrackRef { id: self.id.clone(), kind: "library-songs" },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackRef {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Only playlists the user made are editable; subscribed Apple Music
    /// playlists in the library are not.
    pub can_edit: bool,
    pub date_added: Option<String>,
    pub artwork_url: Option<String>,
}

impl Playlist {
    pub(crate) fn from_resource(r: Resource<PlaylistAttributes>) -> Self {
        let a = r.attributes.unwrap_or_default();
        Playlist {
            id: r.id,
            name: a.name,
            description: a.description.and_then(|d| d.standard),
            can_edit: a.can_edit,
            date_added: a.date_added,
            artwork_url: a.artwork.map(|a| a.url),
        }
    }
}

/// Fills an artwork template URL for a square image of `size` pixels.
pub fn artwork_url(template: &str, size: u32) -> String {
    template.replace("{w}", &size.to_string()).replace("{h}", &size.to_string()).replace("{f}", "jpg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_library_and_catalog_songs() {
        let page: Page<Resource<SongAttributes>> = serde_json::from_str(
            r#"{
              "next": "/v1/me/library/songs?offset=100",
              "data": [
                {"id": "i.abc", "type": "library-songs", "attributes": {
                  "name": "Song A", "artistName": "Artist", "albumName": "Album",
                  "durationInMillis": 201000, "genreNames": ["Rock", "Music"],
                  "releaseDate": "1999-04-01", "trackNumber": 3, "discNumber": 1,
                  "artwork": {"url": "https://x/{w}x{h}bb.jpg", "width": 1200},
                  "playParams": {"id": "i.abc", "kind": "song", "isLibrary": true, "catalogId": "12345"}
                }},
                {"id": "999", "type": "songs", "attributes": {"name": "Song B", "genreNames": ["Music", "Jazz"]}},
                {"id": "i.vid", "type": "library-music-videos", "attributes": {"name": "Video"}}
              ]
            }"#,
        )
        .unwrap();
        assert_eq!(page.next.as_deref(), Some("/v1/me/library/songs?offset=100"));
        let songs: Vec<Song> = page.data.into_iter().filter_map(Song::from_resource).collect();
        assert_eq!(songs.len(), 2);

        let a = &songs[0];
        assert_eq!(a.source, Source::Library);
        assert_eq!(a.playback_id(), "12345");
        assert_eq!(a.genre, "Rock");
        assert_eq!(a.year(), Some(1999));
        assert_eq!(a.track_ref().kind, "library-songs");

        let b = &songs[1];
        assert_eq!(b.source, Source::Catalog);
        assert_eq!(b.playback_id(), "999");
        assert_eq!(b.genre, "Jazz");
        assert_eq!(b.year(), None);
        assert_eq!(b.track_ref().kind, "songs");
    }

    #[test]
    fn parses_playlists() {
        let page: Page<Resource<PlaylistAttributes>> = serde_json::from_str(
            r#"{"data": [{"id": "p.1", "type": "library-playlists", "attributes": {
                "name": "Mine", "canEdit": true, "description": {"standard": "desc"},
                "dateAdded": "2024-01-01T00:00:00Z"}}]}"#,
        )
        .unwrap();
        let p = Playlist::from_resource(page.data.into_iter().next().unwrap());
        assert_eq!(p.name, "Mine");
        assert!(p.can_edit);
        assert_eq!(p.description.as_deref(), Some("desc"));
    }

    #[test]
    fn fills_artwork_template() {
        assert_eq!(artwork_url("https://x/{w}x{h}bb.{f}", 64), "https://x/64x64bb.jpg");
    }
}
