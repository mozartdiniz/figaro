//! Client for the `figaro-engine` process (hidden CEF + MusicKit JS).
//!
//! The engine is a child process speaking JSON lines: commands on its stdin,
//! events on its stdout. Its stdout also carries Chromium noise, so only
//! lines that parse as an [`Event`] count.

use std::path::PathBuf;
use std::process::Stdio;

use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// MusicKit JS loaded in the page.
    Loaded,
    Authorized {
        user_token: String,
    },
    NeedsAuth,
    State {
        playing: bool,
    },
    NowPlaying {
        item: Option<NowPlaying>,
    },
    Time {
        position: f64,
        duration: f64,
    },
    Error {
        message: String,
    },
    /// Not sent by the engine: the process ended.
    #[serde(skip)]
    Exited,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NowPlaying {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub duration: f64,
    pub artwork: Option<String>,
}

#[derive(Clone)]
pub struct Engine {
    tx: mpsc::UnboundedSender<String>,
}

impl Engine {
    /// Starts the engine. Must be called inside the tokio runtime.
    pub fn spawn(events: async_channel::Sender<Event>) -> anyhow::Result<Engine> {
        let path = locate().ok_or_else(|| {
            anyhow::anyhow!("figaro-engine not found (build it with `just engine`, or set FIGARO_ENGINE)")
        })?;
        log::info!("starting engine {}", path.display());
        let mut cmd = tokio::process::Command::new(&path);
        if std::env::var_os("FIGARO_ENGINE_SHOW").is_some() {
            cmd.arg("--show");
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("can't start {}: {e}", path.display()))?;

        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if stdin.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.starts_with('{') {
                    continue;
                }
                match serde_json::from_str::<Event>(&line) {
                    Ok(ev) => {
                        let _ = events.send(ev).await;
                    }
                    Err(_) => log::debug!("engine: {line}"),
                }
            }
            let status = child.wait().await;
            log::info!("engine exited: {status:?}");
            let _ = events.send(Event::Exited).await;
        });
        Ok(Engine { tx })
    }

    fn send(&self, value: serde_json::Value) {
        let _ = self.tx.send(value.to_string());
    }

    pub fn configure(&self, developer_token: &str) {
        self.send(json!({ "cmd": "configure", "developer_token": developer_token }));
    }

    pub fn authorize(&self) {
        self.send(json!({ "cmd": "authorize" }));
    }

    pub fn unauthorize(&self) {
        self.send(json!({ "cmd": "unauthorize" }));
    }

    /// Queues `ids` (catalog or library song IDs) and starts at `start`.
    pub fn play(&self, ids: &[&str], start: usize) {
        self.send(json!({ "cmd": "play", "ids": ids, "start": start }));
    }

    pub fn toggle(&self) {
        self.send(json!({ "cmd": "toggle" }));
    }

    pub fn next(&self) {
        self.send(json!({ "cmd": "next" }));
    }

    pub fn previous(&self) {
        self.send(json!({ "cmd": "previous" }));
    }

    pub fn seek(&self, seconds: f64) {
        self.send(json!({ "cmd": "seek", "seconds": seconds }));
    }

    pub fn set_volume(&self, value: f64) {
        self.send(json!({ "cmd": "volume", "value": value }));
    }

    pub fn quit(&self) {
        self.send(json!({ "cmd": "quit" }));
    }
}

/// `$FIGARO_ENGINE`, then next to our executable (installed layout:
/// `<libexec>/figaro-engine/figaro-engine`), then the dev bundle.
fn locate() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("FIGARO_ENGINE") {
        return Some(p.into());
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [
        exe_dir.join("figaro-engine").join("figaro-engine"),
        exe_dir.join("../libexec/figaro-engine/figaro-engine"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine/bundle/figaro-engine"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_engine_events() {
        let ev: Event = serde_json::from_str(r#"{"event":"authorized","user_token":"abc"}"#).unwrap();
        assert!(matches!(ev, Event::Authorized { user_token } if user_token == "abc"));
        let ev: Event = serde_json::from_str(
            r#"{"event":"now_playing","item":{"id":"1","title":"T","artist":"A","album":"B","duration":200.5,"artwork":null},"index":3}"#,
        )
        .unwrap();
        assert!(matches!(ev, Event::NowPlaying { item: Some(ref n) } if n.title == "T"));
        let ev: Event = serde_json::from_str(r#"{"event":"now_playing","item":null,"index":-1}"#).unwrap();
        assert!(matches!(ev, Event::NowPlaying { item: None }));
        assert!(serde_json::from_str::<Event>(r#"{"event":"loaded"}"#).is_ok());
        assert!(serde_json::from_str::<Event>(r#"{"level":"x"}"#).is_err());
    }
}
