# Apple Music — Protocol Research

Reference for Figaro, the GTK client for an Apple Music subscription. Same
method as amadeuz's `ICLOUD_NOTES_PROTOCOL.md`: look at what Apple's own app
does, then pick the transport we can legitimately use from Linux.

Each fact is tagged by where it came from:

- **[RE]** — observed in Apple's shipped binaries (macOS 26.5, October 2026)
- **[public]** — from Apple's public MusicKit / Apple Music API documentation
- **[unknown]** — open question, must be verified against a live account

---

## How Music.app does it **[RE]**

Music.app doesn't talk to the network itself, like Notes.app. It links
Apple's private media frameworks: `AMPLibrary`, `iTunesCloud`,
`AppleMediaServices`, `MusicKitInternal`, `MediaPlaybackCore`, plus the
public `MusicKit`. Endpoint URLs come from Apple's server-sent "bag"
(`bag://enhanced-audio/hls-key-server-url`).

Playback is HLS with **FairPlay** keys tied to the device:
`NativeFairPlaySAPSetupAsync`, `FairPlayCheckAccountPresent`,
`OfflineHLSKeyManager`, `hlsKeyServerURL` / `hlsKeyCertificateURL`, and the
`Apple FairPlay Certification Authority` certificate chain embedded in the
binary.

### Why we don't clone the native transport

Same reason as `cloudd` for Notes: the account tokens (AMS/GrandSlam) and the
FairPlay SAP context are tied to an Apple device. Reproducing them means
emulating an Apple device *and* getting around DRM. We don't go there.

## Chosen transport

### Library, search, playlists: Apple Music API **[public]**

Plain JSON over HTTPS at `https://api.music.apple.com`, called from Rust
(`applemusic/`). There's no web view on this path.

| Header | Value |
|---|---|
| `Authorization` | `Bearer <developer token>` |
| `Music-User-Token` | user token, required for `/v1/me/*` |

- **Developer token**, from one of two sources:
  - *Own key* (optional): ES256 JWT, header `{alg, kid: <Key ID>}`, claims
    `{iss: <Team ID>, iat, exp}`, `exp − iat ≤ 15777000 s`, signed locally
    from a MusicKit `.p8` key. Needs a paid Apple Developer membership.
  - *Web player token* (default): music.apple.com's main script
    (`/assets/index~<hash>.js`) embeds three ES256 tokens. The web player's
    is `kid: "WebPlayKid"`, `iss: "AMPWebPlay"`, lasting 70 days, with
    `root_https_origin: ["apple.com"]`. Requests go to
    `amp-api.music.apple.com` with `Origin: https://music.apple.com`.
    Decision 2026-10-12: the user's developer membership had expired and
    they chose this, knowing it goes against Apple's developer terms and
    that Apple can rotate or restrict it. Cached in
    `~/.cache/figaro/web-token.json` and fetched again before expiry, or
    once on a 401.
- **Music User Token**: comes only from MusicKit's `authorize()` (Apple's
  sign-in popup), so it needs the playback engine. Stored in the keyring.

| Feature | Request |
|---|---|
| Storefront | `GET /v1/me/storefront` |
| Catalog search | `GET /v1/catalog/{sf}/search?types=songs&term=…&limit≤25&offset=` |
| Library songs | `GET /v1/me/library/songs?limit≤100`, follow `next` |
| Library playlists | `GET /v1/me/library/playlists?limit≤100` |
| Playlist tracks | `GET /v1/me/library/playlists/{id}/tracks` (404 = empty) |
| Add to library | `POST /v1/me/library?ids[songs]=1,2` → 202, applied later |
| Create playlist | `POST /v1/me/library/playlists` `{attributes:{name,description}, relationships:{tracks:{data:[{id,type}]}}}` |
| Add to playlist | `POST /v1/me/library/playlists/{id}/tracks` `{data:[{id,type}]}` |

Track references use `type: "songs"` (catalog ID) or `"library-songs"`
(`i.` ID). Only playlists with `canEdit: true` accept tracks.

Library songs carry `playParams.catalogId` when they're catalog songs.
Uploaded songs don't. Figaro plays the catalog ID when one exists and falls
back to the library ID otherwise.

### Playback: MusicKit JS in a hidden CEF window

Catalog audio is always encrypted. On Linux, music.apple.com plays only in
browsers that ship Google's Widevine CDM. `engine/` is a CEF (Chromium)
process with one hidden window running `engine/web/player.html`, which
loads MusicKit JS v3. CEF gets Widevine through Chromium's component
updater, which is the licensed path (Cider works the same way).
Decision 2026-10-12: chosen over loading `libwidevinecdm.so` straight from
Rust, which would break Widevine's license terms, and over a
previews-only player.

The page is served from `http://127.0.0.1:47231/`. Loopback counts as a
secure context, which EME needs. The port is fixed because MusicKit keeps the
sign-in in that origin's localStorage, in the CEF profile at
`~/.local/share/figaro/engine`.

See `ENGINE_PROTOCOL.md` for the app ↔ engine messages.

## Confirmed against a live account (2026-10-12)

Probe (`applemusic/examples/probe.rs`) with the web player token and a Music
User Token copied from music.apple.com's console
(`MusicKit.getInstance().musicUserToken`):

- The web token works from outside a browser on `amp-api.music.apple.com`
  with `Origin`/`Referer` set to `https://music.apple.com`.
- Storefront, catalog search, library songs (677, paginated by 100),
  playlists (27) and playlist tracks all work. `canEdit` is false for
  Apple-made playlists in the library ("Favourite Songs").
- 659 of 677 library songs have `playParams.catalogId`. The other 18 are
  presumably uploads.
- **No library song has `dateAdded`.** There's no "Date Added" sort from
  this endpoint.
- Writes (`examples/writetest.rs`) work with the web token: create playlist
  with initial tracks, append `library-songs` tracks (immediately readable),
  and add a catalog song to the library (visible on the next library
  fetch, within seconds). The API has no endpoint for deleting playlists.

## Open questions **[unknown]**

- Does the CEF build from `cef-dll-sys` (Spotify CDN, CEF 154) fetch
  Widevine on Linux through the component updater, with only a persistent
  `root_cache_path`? If not, playback falls back to 30-second previews, or
  `mediaPlaybackError` reports a key-system error.
- Does MusicKit v3 `setQueue({songs})` accept library IDs (`i.…`) for
  uploaded songs, or do they need `setQueue({song})` one at a time or a
  `library-songs` URL?
- Does the hidden Views window on Wayland/X11 keep playing without being
  mapped, or does Chromium throttle it? If it does, show it minimized or
  switch to windowless rendering.
- MusicKit user tokens expire after about 6 months. A 401/403 makes the app
  ask for sign-in again. Does `authorize()` stay silent while the CEF
  cookies are still valid?
