# App ↔ engine protocol

`figaro-engine` is a child process of the GTK app. They talk with one JSON
object per line: commands on the engine's stdin, events on its stdout.
Chromium also writes to stdout, so the app ignores any line that isn't an
event object. When stdin closes, the engine quits.

## Commands (app → engine)

| `cmd` | Fields | Effect |
|---|---|---|
| `configure` | `developer_token` | `MusicKit.configure`; replies `authorized` or `needs_auth` |
| `authorize` | | Opens Apple's sign-in popup (a visible window) |
| `unauthorize` | | Signs out of MusicKit |
| `play` | `ids` (array), `start` | `setQueue({songs: ids})`, then play |
| `toggle` / `pause` / `resume` | | |
| `next` / `previous` | | `previous` restarts the song if past 3 s |
| `seek` | `seconds` | |
| `volume` | `value` (0–1) | |
| `show` / `hide` | | Shows or hides the hidden player window (debugging) |
| `quit` | | Closes the browsers and exits |

The engine handles `show`, `hide` and `quit` itself. Every other command
goes to the page as `figaro.dispatch(<json>)`. Commands sent before
MusicKit JS has loaded are queued.

## Events (engine → app)

| `event` | Fields |
|---|---|
| `loaded` | MusicKit JS is ready |
| `authorized` | `user_token` |
| `needs_auth` | |
| `state` | `playing`, `state` (MusicKit playback state number) |
| `now_playing` | `item: {id, title, artist, album, duration, artwork} \| null`, `index` |
| `time` | `position`, `duration` (seconds, at most once per second) |
| `error` | `message` |

Running the engine with `--show` (or the app with `FIGARO_ENGINE_SHOW=1`)
makes the player window visible. Its DevTools help when MusicKit misbehaves.
