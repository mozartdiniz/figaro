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
Uploaded songs don't. That `catalogId` can belong to another storefront
(where the song was added): in a gb account, library catalog IDs were
missing from the gb catalog, some present only in us. MusicKit then rejects
the whole queue ("One or more items could not be resolved"). So Figaro plays
library songs by their library ID (`i.…`), which MusicKit resolves for the
user's storefront, and catalog songs by their catalog ID.

### Playback: MusicKit JS in a hidden castlabs Electron window

Catalog audio is always encrypted: AAC in MP4 under Widevine. On Linux,
music.apple.com plays only in browsers that ship both Widevine and an AAC
decoder. `engine/` is a castlabs Electron app (`main.js`) with one hidden
window running `engine/web/player.html`, which loads MusicKit JS v3.
castlabs' Electron fork ships Chromium's AAC decoder and gets the Widevine
CDM through castlabs' licensed component updater (Cider works the same
way). Decision 2026-10-12: chosen over loading `libwidevinecdm.so` straight
from Rust, which would break Widevine's license terms, and over a
previews-only player.

First tried CEF (cef-rs, CEF 154). Widevine arrived through the component
updater, but the CEF binary distribution is built without proprietary
codecs: `MediaSource.isTypeSupported('audio/mp4; codecs="mp4a.40.2"')` is
false, every key system rejects AAC, and MusicKit fails with "NO DRM".
Building CEF with `proprietary_codecs` needs ~100 GB and most of a day, so
it was dropped (2026-10-13).

The page is loaded as a data URL with base URL
`https://music.apple.com/figaro-engine`, which gives it that origin; the
web player token is accepted only from apple.com origins. It's a secure
context, which EME needs. Nothing is intercepted: an `https` protocol
handler that passed other requests through broke Apple's sign-in popup.
MusicKit keeps the sign-in in that origin's localStorage, in the engine
profile at `~/.local/share/figaro/electron`.

Hardware acceleration is off. With it, the GPU process segfaulted on the
dev laptop (Fedora 45, GNOME Wayland) and the sign-in popup stayed blank
even though Apple's page had loaded.

See `ENGINE_PROTOCOL.md` for the app ↔ engine messages.

## Confirmed against a live account (2026-10-12)

Playback (2026-10-13, Fedora, castlabs Electron v44.5.1+wvcus, not
VMP-signed): signed in through MusicKit's popup, and a catalog song played
in full (499 s, past the 30 s preview mark). Apple's license server accepts
an unsigned castlabs client. Quality: `music.bitrate` 256, flavor
`28:ctrp256` (AAC 256 kbps, Widevine CTR), files `*.rphq.aac.wa.m3u8` /
`*.rmhq.aac.wa.mp4`. No lossless or Atmos for web players; the engine pins
`bitrate` to HIGH. Then through the app: sign-in, and library
songs queued by library ID play.


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
- **No library song has `dateAdded`**, but the endpoint sorts by it:
  `/v1/me/library/songs?sort=-dateAdded` is newest first (exact order).
  With `include=albums`, each song's library album carries `dateAdded`;
  Figaro shows that date and sorts by the API's order. `next` links drop
  `limit`, `sort` and `include`, so pagination re-adds them.
- Writes (`examples/writetest.rs`) work with the web token: create playlist
  with initial tracks, append `library-songs` tracks (immediately readable),
  and add a catalog song to the library (visible on the next library
  fetch, within seconds). The API has no endpoint for deleting playlists.

## Open questions **[unknown]**

- Does MusicKit v3 `setQueue({songs})` accept library IDs (`i.…`)? It
  resolved them (the "could not be resolved" error went away) but full
  playback by library ID is still unconfirmed, as is the uploaded songs'.
- Does the hidden window keep playing without being mapped? The engine sets
  `backgroundThrottling: false`.
- Right after the first sign-in, a `play` failed with "could not be
  resolved" and the same `play` worked a moment later (storefront not set
  yet?).
- MusicKit user tokens expire after about 6 months. A 401/403 makes the app
  ask for sign-in again. Does `authorize()` stay silent while the engine's
  cookies are still valid?
