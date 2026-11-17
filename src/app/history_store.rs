//! Keeps the request history on disk between runs.
//!
//! The file is a JSON list of entries, the same shape as `hello.history`.
//! Viewers' display names are in it, so it sits next to `config.toml` and
//! nowhere else.

use std::ffi::OsString;
use std::future::Future;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::sync::watch;

use super::autosave::wait_for_stop;
use super::{Envelope, Event, HistoryEntry, Pipeline};
use crate::config::write_atomically;
use crate::youtube::VideoSource;

/// Where the history for `config_path` lives: next to it, named after it, so
/// two config files in one folder do not share one history.
pub fn history_path(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_stem()
        .map_or_else(|| OsString::from("config"), OsString::from);
    name.push(".history.json");
    config_path.with_file_name(name)
}

/// Reads the history. A missing file means an empty history. A file that
/// cannot be read is moved aside (to `<name>.bad`) rather than overwritten by
/// the next save.
pub fn load_history(path: &Path) -> Vec<HistoryEntry> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(path = %path.display(), "cannot read the request history: {err}");
            return Vec::new();
        }
    };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        let mut aside = path.as_os_str().to_owned();
        aside.push(".bad");
        tracing::warn!(
            path = %path.display(),
            "the request history is damaged and was moved to {}: {err}",
            Path::new(&aside).display()
        );
        let _ = std::fs::rename(path, aside);
        Vec::new()
    })
}

/// Writes the history to `path` after each change, once things have been
/// quiet for `settle`, until `stopped` turns true; then once more if
/// something is still unsaved. A failed write is logged and tried again at
/// the next change.
///
/// Like `autosave`, it subscribes when called, so nothing that happens right
/// after spawning is missed.
pub fn autosave_history<S: VideoSource + 'static>(
    pipeline: Arc<Pipeline<S>>,
    path: PathBuf,
    settle: Duration,
    mut stopped: watch::Receiver<bool>,
) -> impl Future<Output = ()> + Send + 'static {
    let mut events = pipeline.subscribe();
    // What the file holds, as far as this run knows.
    let mut written = pipeline.history();

    async move {
        loop {
            let mut stop = tokio::select! {
                biased;
                () = wait_for_stop(&mut stopped) => true,
                more = next_history_event(&mut events) => !more,
            };
            if !stop {
                tokio::select! {
                    () = tokio::time::sleep(settle) => {}
                    () = wait_for_stop(&mut stopped) => stop = true,
                }
                while events.try_recv().is_ok() {}
            }

            let current = pipeline.history();
            if current != written {
                let to_write = current.clone();
                let target = path.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let json = serde_json::to_vec(&to_write).map_err(std::io::Error::other)?;
                    write_atomically(&target, &json)
                })
                .await
                .unwrap_or_else(|err| Err(std::io::Error::other(err)));
                match result {
                    Ok(()) => written = current,
                    Err(err) => {
                        tracing::warn!(path = %path.display(), "cannot save the request history: {err}");
                    }
                }
            }

            if stop {
                return;
            }
        }
    }
}

/// Waits for an event that may have changed the history. Returns false when
/// the stream has ended. A client that fell behind may have missed one, so
/// that counts too.
async fn next_history_event(events: &mut Receiver<Arc<Envelope>>) -> bool {
    loop {
        match events.recv().await {
            Ok(envelope) => {
                if matches!(envelope.event, Event::HistoryEntry(_)) {
                    return true;
                }
            }
            Err(RecvError::Lagged(_)) => return true,
            Err(RecvError::Closed) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::RequestStatus;
    use crate::chat::{ChatMessage, Role, Sender};
    use crate::config::{Config, Language};
    use crate::i18n::I18n;
    use crate::youtube::{VideoDetails, VideoId, YoutubeError};

    struct NoVideos;

    impl VideoSource for NoVideos {
        async fn fetch(&self, _id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
            Ok(None)
        }
    }

    fn temp_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stream-spectre-history-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        history_path(&dir.join("config.toml"))
    }

    #[test]
    fn the_history_file_is_named_after_the_config() {
        assert_eq!(
            history_path(Path::new("/x/config.toml")),
            Path::new("/x/config.history.json")
        );
    }

    #[test]
    fn a_damaged_file_is_set_aside_not_overwritten() {
        let path = temp_path("damaged");
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_history(&path).is_empty());
        assert!(!path.exists());
        assert!(path.with_extension("json.bad").exists());
        assert!(load_history(&path).is_empty()); // missing is fine too
    }

    #[tokio::test]
    async fn a_request_is_saved_and_read_back() {
        let path = temp_path("saver");
        let mut config = Config::default();
        config.chat.tiktok.username = "someone".to_owned();
        let pipeline =
            Arc::new(Pipeline::new(&config, I18n::load(Language::En).unwrap(), NoVideos).unwrap());
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(autosave_history(
            Arc::clone(&pipeline),
            path.clone(),
            Duration::from_millis(10),
            stopped,
        ));

        let sender = Sender::new("id", "Viewer", Role::Moderator);
        pipeline
            .handle_chat(ChatMessage {
                sender,
                text: "!v dQw4w9WgXcQ".to_owned(),
            })
            .await;
        stop.send(true).unwrap();
        task.await.unwrap();

        let saved = load_history(&path);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].status, RequestStatus::Refused);
        assert_eq!(saved[0].video_id, "dQw4w9WgXcQ");
        assert_eq!(saved[0].requested_by, "Viewer");
    }
}
