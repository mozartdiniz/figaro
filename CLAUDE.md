# Figaro — Project Brief for Claude

## Vision

Listen to an Apple Music subscription on Linux with a native GNOME app, in
the spirit of Music.app. Scope: search the catalog, add to library, song
list with sorting and filtering, playlists (browse, create, add to).
Sibling of amadeuz (iCloud Notes) and scrawl (Preview). Same stack, same
approach: study Apple's app, then use the web-services API directly.

Read `docs/APPLE_MUSIC_PROTOCOL.md` before changing the architecture.

## Stack

- Rust 2021, `gtk4` 0.10 (`gnome_46`), `libadwaita` 0.8 (`v1_5`); UI built in
  Rust, no Blueprint/Meson. `cargo run` works on macOS with Homebrew GTK too
- tokio for network and the engine process; GTK awaits tokio `JoinHandle`s
  through `crate::bg`, so UI code stays on the main thread
- `keyring` for the Music User Token

## Layout

```
applemusic/   API crate, no GTK: token.rs (ES256 developer token),
              webtoken.rs (web player token),
              api.rs (Apple Music API client), model.rs (Song/Playlist);
              examples/probe.rs checks a live account from the CLI,
              examples/writetest.rs the write endpoints (changes the account)
engine/       figaro-engine: CEF host (host.rs), page.rs/server.rs (see Status),
              web/player.html (MusicKit JS). Linux only. Not a default
              workspace member: CEF downloads ~300 MB at build time
src/
  main.rs       app, tokio runtime, accelerators, FIGARO_SNAPSHOT dev aid
  window.rs     window + controller (session, library, requests, dialogs)
  song_table.rs sortable/filterable ColumnView used by every page
  player_bar.rs transport, position, volume
  playback.rs   engine child process client (JSON lines)
  config.rs     config.json + library cache
  secrets.rs    keyring
docs/         protocol research, engine protocol
```

## Key decisions

- **Two halves.** All UI and every library/search/playlist operation is
  native Rust → `api.music.apple.com`. Only DRM playback and the sign-in run
  in MusicKit JS, in a hidden CEF window in a separate process. A crash or
  hang in the engine never takes the UI with it.
- **Developer token:** the web player's token from music.apple.com by
  default (the user has no developer membership; they chose this knowing
  it's against Apple's developer terms and can break). The user's own
  MusicKit `.p8` key is optional, set from the menu.
- **No DRM circumvention.** No key extraction, and no loading Widevine
  outside a licensed host.
- **Queue = visible list.** Playing a row queues the table as sorted and
  filtered, starting at that row (capped at 300 songs).
- Ownership as in scrawl: the window's `destroy` handler holds the only
  strong `Rc<Inner>`, closures hold `Weak`.

## Status (2026-10-12, moving development from macOS to Linux)

Verified live with the user's account (web player token + Music User Token
from `.env`): storefront, catalog search, library songs, playlists,
playlist tracks, create playlist, add to playlist, add to library. The GTK
app's Songs/Search/Playlists UI renders (checked with `FIGARO_SNAPSHOT`).

Not yet run anywhere: the engine (CEF, Linux only). Next steps, in order:
1. **Wire `engine/src/page.rs` in** (written, not connected). The web token
   only works from apple.com origins, so the engine must load its page as
   `https://music.apple.com/figaro-engine` instead of 127.0.0.1. In
   `engine/src/main.rs`: `mod server;` → `mod page;`, delete the port/
   `server::start` block, use `EngineApp::new(page::URL.to_owned(), show)`.
   In `host.rs` `on_context_initialized`: call `crate::page::register()`
   first. Delete `server.rs`. (Claude Code's auto-mode classifier blocked
   this edit on macOS. The user had explicitly chosen this approach.)
2. `just engine`, `just debug`, Sign In → does MusicKit authorize and play
   full songs (Widevine via component updater)? See the open questions in
   `docs/APPLE_MUSIC_PROTOCOL.md`.
3. Check that the 18 non-catalog library songs play with `i.` IDs.

`.env` (git-ignored) holds `FIGARO_USER_TOKEN`; the app uses it in place
of a sign-in when present.

## Commands

`cargo run` (with `set -a; . ./.env; set +a` first, to use the `.env` token), `cargo test`, `just engine` (Linux: build + bundle CEF into
`engine/bundle/`), `just debug` (engine window visible), `just probe`.
Render the window to a PNG without screen capture:
`FIGARO_SNAPSHOT=/tmp/out.png [FIGARO_SNAPSHOT_SCRIPT=win.key] cargo run`.
