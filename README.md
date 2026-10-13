# Figaro

Your Apple Music library and playlists on GNOME.

- Search the Apple Music catalog and add songs to your library
- Songs: your whole library in one table. Sort by title, artist, album,
  genre, year or time; filter as you type
- Playlists: browse, create, add songs (right-click → Add to Playlist)
- Plays your subscription's full-length songs

## Setup

1. Figaro uses the developer token of Apple's own web player, which it
   fetches from music.apple.com. That goes against Apple's developer terms
   and can break if Apple changes it. If you have an Apple Developer
   membership, set your own MusicKit key from the menu → *MusicKit Key…*
   instead.
2. **Playback engine** (Linux):
   ```sh
   sudo apt install libgtk-4-dev libadwaita-1-dev curl unzip   # or your distro’s equivalent
   just engine                          # downloads castlabs Electron into engine/bundle/
   ```
3. `cargo run`, then *Sign In* with your Apple ID.

## How it works

The window and every library operation are native GTK and Rust, calling the
[Apple Music API](https://developer.apple.com/documentation/applemusicapi)
directly. Apple Music audio is DRM-protected, so playback runs in a hidden
castlabs Electron process with MusicKit JS, just as on music.apple.com. See
`docs/APPLE_MUSIC_PROTOCOL.md`.

Data lives in `~/.config/figaro/config.json` (key location),
`~/.cache/figaro/library.json` (library cache),
`~/.local/share/figaro/electron` (engine profile: MusicKit sign-in, Widevine),
and the system keyring (Music User Token).
