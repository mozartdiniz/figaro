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
engine/       figaro-engine: castlabs Electron app (main.js), web/player.html
              (MusicKit JS), bundle.sh (downloads castlabs Electron, pinned
              by SHA-256, into engine/bundle/). Linux only, no Rust
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
  in MusicKit JS, in a hidden castlabs Electron window in a separate process. A crash or
  hang in the engine never takes the UI with it.
- **Developer token:** the web player's token from music.apple.com by
  default (the user has no developer membership; they chose this knowing
  it's against Apple's developer terms and can break). The user's own
  MusicKit `.p8` key is optional, set from the menu.
- **No DRM circumvention.** No key extraction, and no loading Widevine
  outside a licensed host. So no decoding with our own code (e.g.
  Symphonia): streams are encrypted until the CDM hands them to Chromium.
- **castlabs Electron, not CEF.** Stock CEF builds have no AAC decoder, so
  MusicKit fails with "NO DRM"; castlabs Electron has AAC and licensed
  Widevine (as Cider). See `docs/APPLE_MUSIC_PROTOCOL.md`.
- **Queue = visible list.** Playing a row queues the table as sorted and
  filtered, starting at that row (capped at 300 songs).
- Ownership as in scrawl: the window's `destroy` handler holds the only
  strong `Rc<Inner>`, closures hold `Weak`.

## Status (2026-10-13, on Linux / Fedora)

Verified live with the user's account: storefront, catalog search, library
songs, playlists, playlist tracks, create playlist, add to playlist, add to
library, and, through the app with the castlabs Electron engine (2026-10-13),
sign-in in Apple's popup and playback of library songs queued by `i.…` ID.

The engine runs without hardware acceleration: on the dev laptop the GPU
process segfaulted and the sign-in popup stayed blank.

Still to check: the 18 uploaded (non-catalog) songs, search results
(catalog IDs), and playback with the engine window hidden (plain
`just run`).

`.env` (git-ignored) holds `FIGARO_USER_TOKEN`; the app uses it in place
of a sign-in when present.

## Commands

`cargo run` (with `set -a; . ./.env; set +a` first, to use the `.env` token), `cargo test`, `just engine` (Linux: download castlabs
Electron + bundle into `engine/bundle/`), `just debug` (engine window visible), `just probe`.
Render the window to a PNG without screen capture:
`FIGARO_SNAPSHOT=/tmp/out.png [FIGARO_SNAPSHOT_SCRIPT=win.key] cargo run`.
