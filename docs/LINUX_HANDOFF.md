# Linux handoff — instructions for Claude Code

Development moved from macOS to Linux on 2026-10-12. This file is for
Claude Code on the Linux machine. Work through the steps **in order**. After
each one, report the result to the user before going on. Where a step says
"ask", ask the user and don't guess.

Read `CLAUDE.md`, `docs/APPLE_MUSIC_PROTOCOL.md` and `docs/ENGINE_PROTOCOL.md`
first.

## Decisions already made by the user — don't re-litigate

- **Developer token:** the user has no Apple Developer membership. They
  explicitly chose to use the web player's token, fetched from
  music.apple.com (`applemusic/src/webtoken.rs`), knowing it goes against
  Apple's developer terms and can break. The web token only works from
  `apple.com` origins, so the Rust client sends
  `Origin: https://music.apple.com`, and the engine must serve its page as
  `https://music.apple.com/figaro-engine` (step 4). This is the user's
  authorised design.
- **Playback:** MusicKit JS in a hidden CEF window with Chromium's own
  Widevine (licensed path).
- **Hard limit, unchanged:** no DRM circumvention. No extracting Widevine or
  FairPlay keys, and no loading `libwidevinecdm.so` outside a licensed
  host. If playback can't work within that, say so and stop.

## What already works (verified on macOS against the user's account)

Storefront, catalog search, library songs (678), playlists (27), playlist
tracks, create playlist, add to playlist, add to library. The GTK UI
renders. The engine has never run anywhere.

## 1. System packages

Detect the distro (`/etc/os-release`), then install the equivalents of:

- **Debian/Ubuntu/Pop!_OS:** `build-essential pkg-config libgtk-4-dev
  libadwaita-1-dev libsecret-1-dev gnome-keyring cmake ninja-build just`
  (plus `rustup` if Rust is missing)
- **Fedora:** `gtk4-devel libadwaita-devel libsecret-devel cmake
  ninja-build just`
- **Arch:** `gtk4 libadwaita libsecret cmake ninja just`

Show the user the install command and let them run it (it needs sudo).
Check that `pkg-config --modversion gtk4 libadwaita-1` reports
GTK ≥ 4.14 and libadwaita ≥ 1.5. If they're older, ask before lowering the
`gnome_46` / `v1_5` features in `Cargo.toml`.

## 2. The app builds and the tests pass

```sh
cargo build && cargo test
```

Fix any Linux-only build errors (the code was only compiled on macOS).

## 3. The account still works

`.env` (git-ignored) holds `FIGARO_USER_TOKEN`. It isn't in the repo. If
it's missing, ask the user to copy it over, or to get a new one: sign in at
music.apple.com in a browser and run
`MusicKit.getInstance().musicUserToken` in the console. Never print the
token, and never ask the user to paste it into the chat.

```sh
just probe "mozart"
```

The `justfile` has `set dotenv-load`. Expect the storefront, search
results, about 678 library songs and 27 playlists. Note the "no catalog
ID" song IDs it prints (`i.…`); step 7 needs them. Then run `cargo run`:
the window should show the library through the `.env` token.

## 4. Wire the engine's page handler in

`engine/src/page.rs` is written but not connected. The engine still serves
`player.html` from `http://127.0.0.1:47231`, which the web token rejects.
The edit (it was blocked by Claude Code's auto-mode classifier on macOS):

- `engine/src/main.rs`:
  - `mod server;` → `mod page;`
  - delete the `let port = …` and `let url = match server::start(port) {…};`
    block, and the `FIGARO_ENGINE_PORT` mention in the docs
  - `host::EngineApp::new(url, show)` → `host::EngineApp::new(page::URL.to_owned(), show)`
- `engine/src/host.rs`: first line of `on_context_initialized`:
  `crate::page::register();`
- delete `engine/src/server.rs`
- `docs/APPLE_MUSIC_PROTOCOL.md` / `docs/ENGINE_PROTOCOL.md`: replace the
  127.0.0.1 description with the `music.apple.com/figaro-engine` one

If the classifier blocks it here too, give the user the edit above to make
by hand. Don't work around the block.

## 5. Build the engine

```sh
just engine      # downloads CEF (~300 MB) the first time; output in engine/bundle/
```

- Make sure `engine/bundle/figaro-engine` exists, with `libcef.so` next to it.
- `engine/build.rs` sets an `$ORIGIN` rpath. If libcef still isn't found,
  check with `ldd engine/bundle/figaro-engine`.
- If `cef::build_util::linux::build_bundle` misbehaves, see
  https://github.com/tauri-apps/cef-rs (README → Linux) for the manual
  layout.

## 6. Engine on its own, then sign-in

Test the engine without the app first. JSON lines over stdin:

```sh
cd engine/bundle && ./figaro-engine --show
# then type:  {"cmd":"configure","developer_token":"<from web-token cache>"}
```

Or simply run `just debug`, which starts the app with the engine window
visible. In either case check:

1. The player window loads `https://music.apple.com/figaro-engine`, and
   the event `{"event":"loaded"}` arrives (MusicKit JS loaded).
2. `configure` answers `needs_auth` or `authorized`, not an `error`.
   An origin or 401 error means step 4 didn't take effect.
3. **Sign In** in the app opens Apple's popup. Sign in with the user's
   Apple ID (the user types it). Afterwards `authorized` arrives, the token
   is saved to the keyring, and the library loads. After that, `.env`
   isn't needed.

Possible problems: CEF sandbox errors (`no_sandbox` is already set),
Wayland vs X11 (try `--ozone-platform=x11` via
`on_before_command_line_processing`), a popup that doesn't appear (check
`on_popup_browser_view_created`).

## 7. Playback — the real unknown

Double-click a song in the app.

- **Full song plays:** Widevine works. Record it in
  `docs/APPLE_MUSIC_PROTOCOL.md` (open questions → confirmed).
- **Only 30 s / key system error:** Widevine is missing. Check
  `chrome://components` in the `--show` window: is "Widevine Content
  Decryption Module" present or updating? It's downloaded into the profile
  (`~/.local/share/figaro/engine`) by the component updater and may take a
  minute on first run. Investigate how CEF 154 enables it on Linux, within
  the hard limit above. Report back before trying anything structural.
- Then the `i.…` songs from step 3: do they play? If not, try
  `setQueue({song: id})` in `engine/web/player.html` for library IDs.
- Also check: does playback continue with the window hidden? (The engine
  window is hidden without `--show`.)

## 8. Finish

- Update `CLAUDE.md` → "Status" and the open questions in
  `docs/APPLE_MUSIC_PROTOCOL.md` with what was confirmed.
- Commit (ask first). End the message with
  `Co-Authored-By: Claude <noreply@anthropic.com>`.

## Leftovers the user may want next (ask)

- A desktop file + icon, and `just install` (like scrawl)
- MPRIS, so media keys and GNOME's media controls work
- A "Sign In with Token…" dialog as a fallback to the engine sign-in
