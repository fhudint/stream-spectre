//! Keeps settings changed in the dashboard on disk.
//!
//! Runs beside the pipeline and listens to the same events as the pages. When
//! settings or the volume change, it waits a moment (a volume slider produces
//! many changes in a row), then writes the tables that differ from
//! `config.toml` to the saved settings file. See `config::saved`.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::{Receiver, error::RecvError};
use tokio::sync::watch;

use super::{Envelope, Event, Pipeline};
use crate::config::{Config, SavedSettings, write_saved};
use crate::youtube::VideoSource;

/// How long changes may keep arriving before the file is written.
pub const SETTLE: Duration = Duration::from_millis(500);

/// Writes settings changes to `path` until `stopped` turns true, then writes
/// once more if something is still unsaved.
///
/// `baseline` is what `config.toml` alone says. A failed write is logged and
/// the moderators get a notice; the next change tries again.
///
/// It subscribes to events and notes the starting state when this function is
/// called, not when the returned future is first polled, so a change made
/// right after spawning is never missed.
pub fn autosave<S: VideoSource + 'static>(
    pipeline: Arc<Pipeline<S>>,
    baseline: Config,
    path: PathBuf,
    settle: Duration,
    mut stopped: watch::Receiver<bool>,
) -> impl Future<Output = ()> + Send + 'static {
    let mut events = pipeline.subscribe();
    // What the file holds now: what was applied when the program started.
    let mut written = changes(&pipeline, &baseline);

    async move {
        loop {
            let mut stop = tokio::select! {
                biased;
                () = wait_for_stop(&mut stopped) => true,
                more = next_settings_event(&mut events) => !more,
            };
            if !stop {
                tokio::select! {
                    () = tokio::time::sleep(settle) => {}
                    () = wait_for_stop(&mut stopped) => stop = true,
                }
                // Everything that arrived meanwhile is covered by reading the
                // current state below.
                while events.try_recv().is_ok() {}
            }

            let current = changes(&pipeline, &baseline);
            if current != written {
                let to_write = current.clone();
                let target = path.clone();
                let result = tokio::task::spawn_blocking(move || write_saved(&target, &to_write))
                    .await
                    .unwrap_or_else(|err| Err(std::io::Error::other(err)));
                match result {
                    Ok(()) => {
                        tracing::debug!(path = %path.display(), "settings saved");
                        written = current;
                    }
                    Err(err) => {
                        tracing::warn!(path = %path.display(), "cannot save settings: {err}");
                        pipeline.settings_not_saved();
                    }
                }
            }

            if stop {
                return;
            }
        }
    }
}

/// The tables that differ from `config.toml` right now.
fn changes<S: VideoSource>(pipeline: &Pipeline<S>, baseline: &Config) -> SavedSettings {
    let settings = pipeline.settings();
    let mut current = baseline.clone();
    current.moderation = settings.moderation;
    current.blacklist = settings.blacklist;
    current.dashboard = settings.dashboard;
    current.overlay = settings.overlay;
    current.command = settings.command;
    current.chat.tiktok = settings.chat.tiktok;
    current.chat.youtube = settings.chat.youtube;
    SavedSettings::changes(baseline, &current)
}

/// Waits for an event that may have changed the settings. Returns false when
/// the event stream has ended. A client that fell behind may have missed such
/// an event, so that counts as one too.
async fn next_settings_event(events: &mut Receiver<Arc<Envelope>>) -> bool {
    loop {
        match events.recv().await {
            Ok(envelope) => {
                if matches!(
                    envelope.event,
                    Event::SettingsUpdated(_) | Event::Volume { .. }
                ) {
                    return true;
                }
            }
            Err(RecvError::Lagged(_)) => return true,
            Err(RecvError::Closed) => return false,
        }
    }
}

/// Completes when `stopped` turns true or its sender is gone.
pub(super) async fn wait_for_stop(stopped: &mut watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    use super::*;
    use crate::chat::Role;
    use crate::config::{
        BlacklistConfig, DashboardConfig, Language, ModerationConfig, ModerationMode, read_saved,
        saved_path,
    };
    use crate::i18n::I18n;
    use crate::youtube::{VideoDetails, VideoId, YoutubeError};

    struct NoVideos;

    impl VideoSource for NoVideos {
        async fn fetch(&self, _id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
            Ok(None)
        }
    }

    fn baseline() -> Config {
        let mut config = Config::default();
        config.chat.tiktok.username = "someone".to_owned();
        config
    }

    fn pipeline(config: &Config) -> Arc<Pipeline<NoVideos>> {
        Arc::new(Pipeline::new(config, I18n::load(Language::En).unwrap(), NoVideos).unwrap())
    }

    fn temp_file(label: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "stream-spectre-autosave-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        saved_path(&dir.join("config.toml"))
    }

    struct Running {
        pipeline: Arc<Pipeline<NoVideos>>,
        stop: watch::Sender<bool>,
        task: tokio::task::JoinHandle<()>,
    }

    /// Starts the task for a pipeline built from `effective` (what the
    /// program started with) while `baseline` is what `config.toml` says.
    fn start_with(effective: &Config, baseline: &Config, path: &Path, settle: Duration) -> Running {
        let pipeline = pipeline(effective);
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(autosave(
            Arc::clone(&pipeline),
            baseline.clone(),
            path.to_owned(),
            settle,
            stopped,
        ));
        Running {
            pipeline,
            stop,
            task,
        }
    }

    fn start(baseline: &Config, path: &Path, settle: Duration) -> Running {
        start_with(baseline, baseline, path, settle)
    }

    impl Running {
        /// Stops the task, which writes anything still unsaved first.
        async fn finish(self) {
            self.stop.send(true).unwrap();
            self.task.await.unwrap();
        }
    }

    fn moderation(f: impl FnOnce(&mut ModerationConfig)) -> ModerationConfig {
        let mut m = ModerationConfig::default();
        f(&mut m);
        m
    }

    #[tokio::test]
    async fn nothing_is_written_when_nothing_changed() {
        let path = temp_file("untouched");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        tokio::time::sleep(Duration::from_millis(50)).await;
        run.finish().await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn changed_tables_are_written_and_untouched_ones_are_not() {
        let path = temp_file("changed");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        run.pipeline
            .update_settings(moderation(|m| {
                m.mode = ModerationMode::Auto;
                m.min_role = Role::Moderator;
            }))
            .unwrap();
        run.finish().await;

        let saved = read_saved(&path).unwrap().unwrap();
        assert_eq!(saved.tables(), vec!["moderation"]);
        assert_eq!(saved.moderation.unwrap().min_role, Role::Moderator);
    }

    #[tokio::test]
    async fn the_file_is_written_while_the_program_keeps_running() {
        let path = temp_file("running");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        run.pipeline
            .update_blacklist(BlacklistConfig {
                title_words: vec!["spoiler".to_owned()],
                ..BlacklistConfig::default()
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let saved = read_saved(&path).unwrap().unwrap();
        assert_eq!(
            saved.blacklist.unwrap().title_words,
            vec!["spoiler".to_owned()]
        );
        run.finish().await;
    }

    #[tokio::test]
    async fn a_volume_change_is_saved_with_the_moderation_table() {
        let path = temp_file("volume");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        assert_eq!(run.pipeline.set_volume(21), 21);
        run.finish().await;
        assert_eq!(
            read_saved(&path)
                .unwrap()
                .unwrap()
                .moderation
                .unwrap()
                .volume,
            21
        );
    }

    #[tokio::test]
    async fn a_volume_change_is_written_while_the_program_keeps_running() {
        let path = temp_file("volume-running");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        assert_eq!(run.pipeline.set_volume(17), 17);

        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Read before stopping: only the running task can have written it.
        let saved = read_saved(&path).unwrap().unwrap();
        assert_eq!(saved.moderation.unwrap().volume, 17);
        run.finish().await;
    }

    #[tokio::test]
    async fn a_changed_username_is_saved_as_the_chat_table() {
        let path = temp_file("username");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        run.pipeline
            .update_tiktok(crate::config::TiktokConfig {
                username: "@someone_new".to_owned(),
                ..Default::default()
            })
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let saved = read_saved(&path).unwrap().unwrap();
        assert_eq!(saved.tables(), vec!["chat.tiktok"]);
        assert_eq!(saved.tiktok().unwrap().username, "someone_new");
        run.finish().await;
    }

    #[tokio::test]
    async fn changing_a_setting_back_leaves_nothing_to_override_config_toml() {
        let path = temp_file("back");
        let run = start(&baseline(), &path, Duration::from_millis(5));
        run.pipeline
            .update_dashboard(DashboardConfig {
                chat_history: 99,
                ..DashboardConfig::default()
            })
            .unwrap();
        run.pipeline
            .update_dashboard(DashboardConfig::default())
            .unwrap();
        run.finish().await;
        // Whether or not an intermediate write happened, the end state holds
        // no tables.
        if path.exists() {
            assert_eq!(
                read_saved(&path).unwrap().unwrap(),
                SavedSettings::default()
            );
        }
    }

    #[tokio::test]
    async fn settings_that_were_saved_before_a_restart_are_not_rewritten() {
        let path = temp_file("restart");
        // The program started with this already applied from the saved file.
        let mut applied = baseline();
        applied.dashboard.chat_history = 5;
        let first = SavedSettings::changes(&baseline(), &applied);
        write_saved(&path, &first).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        let run = start_with(&applied, &baseline(), &path, Duration::from_millis(5));
        let _ = run.pipeline.hello();
        run.finish().await;
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn a_failed_write_tells_the_moderators_and_the_next_change_tries_again() {
        let path = temp_file("failing");
        // A folder where the temporary file goes makes every write fail.
        let mut blocker = path.as_os_str().to_owned();
        blocker.push(".tmp");
        std::fs::create_dir(&blocker).unwrap();

        let baseline = baseline();
        let pipeline = pipeline(&baseline);
        let mut events = pipeline.subscribe();
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(autosave(
            Arc::clone(&pipeline),
            baseline,
            path.clone(),
            Duration::from_millis(5),
            stopped,
        ));

        pipeline
            .update_dashboard(DashboardConfig {
                chat_history: 1,
                ..DashboardConfig::default()
            })
            .unwrap();
        let notice = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = events.recv().await.unwrap();
                if let Event::Notice { code, .. } = &envelope.event {
                    return code.clone();
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(notice, "error.settings_not_saved");

        // The problem goes away; the next change is written, and it includes
        // the change that failed before.
        std::fs::remove_dir(&blocker).unwrap();
        pipeline
            .update_settings(moderation(|m| m.cooldown_secs = 1))
            .unwrap();
        stop.send(true).unwrap();
        task.await.unwrap();
        let saved = read_saved(&path).unwrap().unwrap();
        assert_eq!(saved.tables(), vec!["moderation", "dashboard"]);
    }
}
