use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{broadcast, watch};

use super::bus::EventBus;
use super::events::ChatSettingsView;
use super::events::{
    ChatLineView, ChatState, ChatStatusView, Envelope, Event, Hello, ItemView, NoticeLevel,
    QueueView, SettingsView, SkipReason, StatsView, unix_ms,
};
use super::history::{History, HistoryEntry, RequestStatus, Subject};
use crate::chat::{ChatEvent, ChatMessage, ChatSource, ChatStatus, Sender};
use crate::command::{CommandParser, InvalidReason, Parsed};
use crate::config::{
    BlacklistConfig, CommandConfig, CommandSource, Config, ConfigError, DashboardConfig,
    ModerationConfig, Secret, TiktokConfig,
};
use crate::i18n::I18n;
use crate::queue::{
    Blacklist, Denial, Gatekeeper, ItemId, Media, PlayQueue, QueueError, TiktokDetails,
};
use crate::text::sanitize_text;
use crate::tiktok::{ShortLinkResolver, TiktokRef};
use crate::youtube::{Rejection, VideoId, VideoLookup, VideoSource, YoutubeError};

/// Events kept for slow clients before they are told they lagged.
const BUS_CAPACITY: usize = 256;

/// Longest player error code accepted from the overlay.
const MAX_ERROR_CODE_LEN: usize = 16;

/// Chat messages let onto the event stream per `CHAT_RATE_WINDOW`. The rest
/// are still handled as commands but not shown, so a flood of chat cannot
/// crowd out the events that matter.
const CHAT_RATE_LIMIT: u32 = 20;
const CHAT_RATE_WINDOW: Duration = Duration::from_secs(1);

/// Counts chat messages per window and the ones turned away.
#[derive(Default)]
struct ChatRate {
    window_start: Option<Instant>,
    in_window: u32,
    skipped: u32,
}

impl ChatRate {
    /// `Some(n)` when the message may be shown, where `n` is how many were
    /// turned away since the last one that was; `None` when it may not.
    fn admit(&mut self, now: Instant) -> Option<u32> {
        let expired = self
            .window_start
            .is_none_or(|start| now.saturating_duration_since(start) >= CHAT_RATE_WINDOW);
        if expired {
            self.window_start = Some(now);
            self.in_window = 0;
        }
        if self.in_window >= CHAT_RATE_LIMIT {
            self.skipped = self.skipped.saturating_add(1);
            return None;
        }
        self.in_window += 1;
        Some(std::mem::take(&mut self.skipped))
    }
}

struct Inner {
    queue: PlayQueue,
    gatekeeper: Gatekeeper,
    blacklist: BlacklistConfig,
    dashboard: DashboardConfig,
    /// The chat words, and the parser built from them.
    command: CommandConfig,
    parser: Arc<CommandParser>,
    chat: Option<ChatStatusView>,
    chat_log: VecDeque<ChatLineView>,
    /// Every request and what became of it.
    history: History,
    chat_rate: ChatRate,
    tiktok: TiktokConfig,
    /// The chat was disconnected from the account menu. Only for this run: it
    /// is neither saved nor part of the settings.
    chat_paused: bool,
    /// The video that is playing was paused by a moderator. Cleared whenever
    /// another video starts or the screen empties.
    playback_paused: bool,
    stats: StatsView,
    /// The last playback position published, to skip repeats.
    progress: Option<(ItemId, u64)>,
}

/// What came of a request to pause or resume the playing video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseOutcome {
    Done,
    NothingPlaying,
    /// The playing video is one whose player cannot be paused from outside
    /// (TikTok).
    Unsupported,
}

/// Connects chat, YouTube lookups, moderation and the play queue, and
/// publishes everything that happens as events.
///
/// All state changes and the events describing them happen under one lock, so
/// events always arrive in the order the changes were made. The lock is never
/// held while waiting for YouTube.
pub struct Pipeline<S> {
    lookup: VideoLookup<S>,
    /// Finds the video behind a TikTok short link.
    short_links: ShortLinkResolver,
    i18n: I18n,
    bus: EventBus,
    /// What the chat connection should be doing: the TikTok username to read
    /// (normalized; empty for none) and whether the moderators switched it
    /// off. Changed together with `Inner::tiktok` and `Inner::chat_paused`,
    /// under their lock.
    chat_target: watch::Sender<ChatTarget>,
    inner: Mutex<Inner>,
}

impl<S: VideoSource> Pipeline<S> {
    pub fn new(config: &Config, i18n: I18n, video_source: S) -> Result<Self, ConfigError> {
        config.moderation.validate()?;
        config.dashboard.validate()?;
        config.command.validate()?;
        config.chat.tiktok.validate()?;
        let tiktok = normalized(&config.chat.tiktok);
        let blacklist = Blacklist::from_config(&config.blacklist)?;
        let lookup = VideoLookup::new(
            video_source,
            Duration::from_secs(config.youtube.cache_ttl_secs),
            config.youtube.cache_max_entries as u64,
        );
        let short_links = ShortLinkResolver::new()
            .map_err(|error| ConfigError::invalid("tiktok.short_links", error.to_string()))?;
        Ok(Self {
            lookup,
            short_links,
            i18n,
            bus: EventBus::new(BUS_CAPACITY),
            chat_target: watch::channel(ChatTarget {
                username: tiktok.username.clone(),
                paused: false,
            })
            .0,
            inner: Mutex::new(Inner {
                queue: PlayQueue::new(),
                gatekeeper: Gatekeeper::new(config.moderation.clone(), blacklist),
                blacklist: config.blacklist.clone(),
                dashboard: config.dashboard.clone(),
                command: config.command.clone(),
                parser: Arc::new(CommandParser::new(&config.command)),
                chat: None,
                chat_log: VecDeque::new(),
                history: History::default(),
                chat_rate: ChatRate::default(),
                tiktok,
                chat_paused: false,
                playback_paused: false,
                stats: StatsView {
                    since_ms: unix_ms(SystemTime::now()),
                    ..StatsView::default()
                },
                progress: None,
            }),
        })
    }

    /// Uses another resolver for TikTok short links (for tests, which must
    /// not reach the real service).
    #[cfg(test)]
    pub(crate) fn with_short_links(mut self, short_links: ShortLinkResolver) -> Self {
        self.short_links = short_links;
        self
    }

    /// Reads chat until the source ends. Drop the future to stop earlier.
    ///
    /// Messages are handled one at a time, so a slow YouTube lookup delays the
    /// ones behind it (by at most the request timeout) but never reorders them.
    pub async fn run_chat<C: ChatSource>(&self, source: C) {
        self.run_chat_until(source, std::future::pending()).await;
    }

    /// `run_chat` that also stops, between two messages, when `stop` is ready.
    /// A message that is being handled is always handled to the end. Returns
    /// true when it stopped because of `stop`, false when the source ended.
    async fn run_chat_until<C: ChatSource>(
        &self,
        mut source: C,
        stop: impl std::future::Future<Output = ()>,
    ) -> bool {
        let mut stop = std::pin::pin!(stop);
        loop {
            let event = tokio::select! {
                biased;
                () = &mut stop => return true,
                event = source.next_event() => event,
            };
            match event {
                Some(ChatEvent::Message(message)) => self.handle_chat(message).await,
                Some(ChatEvent::Status(status)) => self.chat_status(&status),
                None => return false,
            }
        }
    }

    /// Reads the chat of whichever TikTok username is set, and follows it when
    /// it changes: the old connection is dropped (after the message in hand)
    /// and a new source is made with `make_source`. While no username is set,
    /// nothing is read and the dashboard is told so. Never returns, except
    /// when the pipeline itself is dropped; drop the future to stop.
    pub async fn run_chat_sources<C, F>(&self, make_source: F)
    where
        C: ChatSource,
        F: Fn(&str) -> C,
    {
        let mut targets = self.chat_target.subscribe();
        loop {
            let target = targets.borrow_and_update().clone();
            self.chat_status(&target.waiting_status());
            if target.should_read() {
                let changed = async {
                    let _ = targets.changed().await;
                };
                if self
                    .run_chat_until(make_source(&target.username), changed)
                    .await
                {
                    continue;
                }
            }
            // Nothing to read, or the source ended for good: wait for a change.
            if targets.changed().await.is_err() {
                return;
            }
        }
    }

    pub async fn handle_chat(&self, message: ChatMessage) {
        self.handle_chat_at(message, Instant::now()).await;
    }

    /// `handle_chat` with the clock passed in, for tests.
    async fn handle_chat_at(&self, message: ChatMessage, now: Instant) {
        let ChatMessage { sender, text } = message;

        let parser = Arc::clone(&self.lock().parser);
        let parsed = parser.parse(&text);
        self.show_chat(&sender, &text, !matches!(parsed, Parsed::NotACommand), now);

        match parsed {
            Parsed::NotACommand => {}
            Parsed::Invalid { source, reason } => {
                self.tell_invalid(&parser, &sender, source, reason);
            }
            Parsed::Youtube(video_id) => self.request_youtube(sender, video_id, now).await,
            Parsed::Tiktok(target) => self.request_tiktok(sender, target, now).await,
        }
    }

    /// Tells the moderators that a command could not be used, when the sender
    /// could have submitted something. A link of the other source is
    /// answered with the command that takes it.
    fn tell_invalid(
        &self,
        parser: &CommandParser,
        sender: &Sender,
        source: CommandSource,
        reason: InvalidReason,
    ) {
        if self.lock().gatekeeper.check_sender(sender).is_err() {
            return;
        }
        let own = match source {
            CommandSource::Youtube => "reject.invalid_link",
            CommandSource::Tiktok => "reject.invalid_tiktok_link",
        };
        let (key, text) = match reason {
            InvalidReason::WrongSource(other) => match parser.command_for(other) {
                Some(command) => {
                    let key = match other {
                        CommandSource::Youtube => "reject.use_youtube_command",
                        CommandSource::Tiktok => "reject.use_tiktok_command",
                    };
                    (key, self.i18n.t_with(key, &[("command", &command)]))
                }
                None => (own, self.i18n.t(own).to_owned()),
            },
            InvalidReason::MissingArgument | InvalidReason::InvalidLink => {
                (own, self.i18n.t(own).to_owned())
            }
        };
        self.notice(NoticeLevel::Warning, key, text, Some(sender));
    }

    async fn request_youtube(&self, sender: Sender, video_id: VideoId, now: Instant) {
        // Stage 1: everything that needs no YouTube call.
        let admitted = {
            let mut inner = self.lock();
            let waiting = inner.queue.waiting_len();
            inner.gatekeeper.admit(&sender, &video_id, waiting, now)
        };
        let bare = Subject::bare(CommandSource::Youtube, video_id.as_str());
        if let Err(denial) = admitted {
            self.deny(&sender, bare, denial);
            return;
        }

        let details = match self.lookup.lookup(&video_id).await {
            Ok(Some(details)) => details,
            Ok(None) => {
                self.deny(&sender, bare, Denial::Video(Rejection::NotFound));
                return;
            }
            Err(error) => {
                // Not the sender's fault: give the cooldown back.
                self.lock().gatekeeper.refund(&sender);
                if error.is_config_problem() {
                    tracing::error!(%error, "YouTube lookups are failing; check the API key");
                } else {
                    tracing::warn!(%error, "YouTube lookup failed");
                }
                let level = if error.is_config_problem() {
                    NoticeLevel::Error
                } else {
                    NoticeLevel::Warning
                };
                let text = self.i18n.t(error.notice_key()).to_owned();
                self.notice(level, error.notice_key(), text.clone(), Some(&sender));
                self.log_refused(&sender, bare, text);
                return;
            }
        };

        // Stage 2: rules that need the video's details, then queue it.
        self.enqueue(&sender, details.into());
    }

    async fn request_tiktok(&self, sender: Sender, target: TiktokRef, now: Instant) {
        // Stage 1: everything that needs no request to TikTok. A short code
        // stands in for the video here; it cannot be on the blacklist, which
        // is checked again once the video is known.
        let key = match &target {
            TiktokRef::Video { id, .. } => id.to_string(),
            TiktokRef::Short(code) => code.to_string(),
        };
        let admitted = {
            let mut inner = self.lock();
            let waiting = inner.queue.waiting_len();
            inner.gatekeeper.admit(&sender, &key, waiting, now)
        };
        let bare = Subject::bare(CommandSource::Tiktok, key);
        if let Err(denial) = admitted {
            self.deny(&sender, bare, denial);
            return;
        }

        let details = match target {
            TiktokRef::Video { id, account } => TiktokDetails { id, account },
            TiktokRef::Short(code) => match self.short_links.resolve(&code).await {
                Ok(Some(found)) => TiktokDetails {
                    id: found.id,
                    account: Some(found.account),
                },
                Ok(None) => {
                    self.deny(&sender, bare, Denial::Video(Rejection::NotFound));
                    return;
                }
                Err(error) => {
                    // Not the sender's fault: give the cooldown back.
                    self.lock().gatekeeper.refund(&sender);
                    tracing::warn!(%error, "TikTok short link lookup failed");
                    let text = self.i18n.t("error.tiktok_unavailable").to_owned();
                    self.notice(
                        NoticeLevel::Warning,
                        "error.tiktok_unavailable",
                        text.clone(),
                        Some(&sender),
                    );
                    self.log_refused(&sender, bare, text);
                    return;
                }
            },
        };

        self.enqueue(&sender, Media::Tiktok(details));
    }

    /// Stage 2 for either source: the rules that need the video, then the
    /// queue. Starts the video when nothing is playing and it is approved. A
    /// request that is turned away is told to the moderators and logged.
    fn enqueue(&self, sender: &Sender, media: Media) {
        let subject = Subject::from(&media);
        if let Err(denial) = self.try_enqueue(sender, media, &subject) {
            self.deny(sender, subject, denial);
        }
    }

    fn try_enqueue(&self, sender: &Sender, media: Media, subject: &Subject) -> Result<(), Denial> {
        let mut inner = self.lock();
        match &media {
            Media::Youtube(details) => inner.gatekeeper.vet(details)?,
            Media::Tiktok(details) => inner.gatekeeper.vet_tiktok(details)?,
        }
        let settings = inner.gatekeeper.settings().clone();
        match inner
            .queue
            .submit(sender.clone(), media, SystemTime::now(), &settings)
        {
            Ok(id) => {
                inner.stats.received += 1;
                let status = if inner.queue.is_pending(id) {
                    RequestStatus::Waiting
                } else {
                    inner.stats.approved += 1;
                    RequestStatus::Queued
                };
                let entry =
                    inner
                        .history
                        .add(subject.clone(), sender, Some(id), status, String::new());
                self.bus.publish(Event::HistoryEntry(entry));
                self.advance(&mut inner);
                self.publish_queue(&inner);
                self.publish_stats(&inner);
                Ok(())
            }
            Err(denial) => {
                inner.gatekeeper.refund(sender);
                Err(denial)
            }
        }
    }

    /// Shows a chat message on the dashboard, within the rate limit, and
    /// remembers it for dashboards that open later.
    fn show_chat(&self, sender: &Sender, text: &str, command: bool, now: Instant) {
        let text = sanitize_text(text);
        if text.is_empty() {
            return;
        }
        let mut inner = self.lock();
        let Some(skipped) = inner.chat_rate.admit(now) else {
            return;
        };
        let line = ChatLineView {
            name: sanitize_text(&sender.name),
            role: sender.role,
            text,
            at_ms: unix_ms(SystemTime::now()),
            command,
            skipped,
        };
        if inner.dashboard.chat_history > 0 {
            inner.chat_log.push_back(line.clone());
            let keep = inner.dashboard.chat_history;
            trim_front(&mut inner.chat_log, keep);
        }
        self.bus.publish(Event::ChatMessage(line));
    }

    /// Records and publishes a change in the chat connection.
    pub fn chat_status(&self, status: &ChatStatus) {
        let mut inner = self.lock();
        let view = ChatStatusView {
            state: ChatState::from(status),
            channel: inner.tiktok.username.clone(),
            text: status.message(&self.i18n),
        };
        inner.chat = Some(view.clone());
        self.bus.publish(Event::ChatStatus(view));
    }

    // --- actions from the dashboard ---

    /// Moves a pending request to the play queue, and starts it if idle.
    pub fn approve(&self, id: ItemId) -> Result<(), QueueError> {
        let mut inner = self.lock();
        inner.queue.approve(id)?;
        inner.stats.approved += 1;
        self.set_status(&mut inner, id, RequestStatus::Queued);
        self.advance(&mut inner);
        self.publish_queue(&inner);
        self.publish_stats(&inner);
        Ok(())
    }

    /// Removes a pending or approved request.
    pub fn reject(&self, id: ItemId) -> Result<(), QueueError> {
        let mut inner = self.lock();
        let was_pending = inner.queue.is_pending(id);
        inner.queue.reject(id)?;
        if !was_pending {
            // It had been approved and counted as such.
            inner.stats.approved = inner.stats.approved.saturating_sub(1);
        }
        inner.stats.rejected += 1;
        self.set_status(&mut inner, id, RequestStatus::Rejected);
        self.publish_queue(&inner);
        self.publish_stats(&inner);
        Ok(())
    }

    /// Skips the video that is playing. Returns false when nothing was.
    pub fn skip(&self) -> bool {
        let mut inner = self.lock();
        let Some(skipped) = inner.queue.skip() else {
            return false;
        };
        self.bus.publish(Event::Skip {
            item_id: skipped.id.0,
            reason: SkipReason::Moderator,
        });
        self.set_status(&mut inner, skipped.id, RequestStatus::Skipped);
        self.play_next_or_stop(&mut inner);
        self.publish_queue(&inner);
        true
    }

    /// Sets the volume, limited to the configured maximum. Returns the volume
    /// that is now in effect.
    pub fn set_volume(&self, requested: u8) -> u8 {
        let mut inner = self.lock();
        let mut settings = inner.gatekeeper.settings().clone();
        let volume = settings.clamp_volume(requested);
        if volume != settings.volume {
            settings.volume = volume;
            inner.gatekeeper.set_settings(settings);
            self.bus.publish(Event::Volume { volume });
        }
        volume
    }

    /// Replaces the moderation settings (including the current volume).
    /// Invalid settings are refused and nothing changes. Requests already
    /// waiting stay where they are, even if the new limits would exclude them.
    pub fn update_settings(&self, new: ModerationConfig) -> Result<(), ConfigError> {
        new.validate()?;
        let mut inner = self.lock();
        let old_volume = inner.gatekeeper.settings().volume;
        let new_volume = new.volume;
        inner.gatekeeper.set_settings(new);
        self.bus
            .publish(Event::SettingsUpdated(Self::settings_view(&inner)));
        if new_volume != old_volume {
            self.bus.publish(Event::Volume { volume: new_volume });
        }
        Ok(())
    }

    /// Replaces the blacklist. Invalid entries are refused and nothing changes.
    pub fn update_blacklist(&self, new: BlacklistConfig) -> Result<(), ConfigError> {
        let blacklist = Blacklist::from_config(&new)?;
        let mut inner = self.lock();
        inner.gatekeeper.set_blacklist(blacklist);
        inner.blacklist = new;
        self.bus
            .publish(Event::SettingsUpdated(Self::settings_view(&inner)));
        Ok(())
    }

    /// Replaces the dashboard settings. Invalid values are refused and
    /// nothing changes. Remembered chat beyond the new limit is forgotten.
    pub fn update_dashboard(&self, new: DashboardConfig) -> Result<(), ConfigError> {
        new.validate()?;
        let mut inner = self.lock();
        trim_front(&mut inner.chat_log, new.chat_history);
        inner.dashboard = new;
        self.bus
            .publish(Event::SettingsUpdated(Self::settings_view(&inner)));
        Ok(())
    }

    /// Replaces the chat commands (the prefix and the words). Invalid ones
    /// are refused and nothing changes; messages already read are not looked
    /// at again.
    pub fn update_commands(&self, new: CommandConfig) -> Result<(), ConfigError> {
        new.validate()?;
        let mut inner = self.lock();
        if inner.command == new {
            return Ok(());
        }
        inner.parser = Arc::new(CommandParser::new(&new));
        inner.command = new;
        self.bus
            .publish(Event::SettingsUpdated(Self::settings_view(&inner)));
        Ok(())
    }

    /// Changes the TikTok username whose chat is read; empty stops reading.
    /// An invalid name is refused and nothing changes. Saying the same name
    /// again changes nothing either.
    pub fn update_tiktok(&self, new: TiktokConfig) -> Result<(), ConfigError> {
        new.validate()?;
        let new = normalized(&new);
        let mut inner = self.lock();
        if inner.tiktok == new {
            return Ok(());
        }
        inner.tiktok = new.clone();
        let paused = inner.chat_paused;
        self.chat_target.send_replace(ChatTarget {
            username: new.username,
            paused,
        });
        self.bus
            .publish(Event::SettingsUpdated(Self::settings_view(&inner)));
        Ok(())
    }

    /// Pauses (`paused`) or resumes the video that is playing. Changes
    /// nothing when no video is playing, and when it is a TikTok video, whose
    /// player cannot be paused from outside. Saying what is already so changes
    /// nothing and publishes nothing. A video that starts later always begins
    /// playing.
    pub fn set_playback_paused(&self, paused: bool) -> PauseOutcome {
        let mut inner = self.lock();
        let Some(item) = inner.queue.now_playing() else {
            return PauseOutcome::NothingPlaying;
        };
        if paused && item.media.source() != CommandSource::Youtube {
            return PauseOutcome::Unsupported;
        }
        let item_id = item.id.0;
        if inner.playback_paused != paused {
            inner.playback_paused = paused;
            self.bus.publish(Event::PlaybackPaused { item_id, paused });
        }
        PauseOutcome::Done
    }

    /// Disconnects the chat (`paused`) or connects it again. Only for this
    /// run: after a restart the chat connects by itself. Saying what is
    /// already so changes nothing. The chat status is published at once, so
    /// whoever asked sees the result in the answer.
    pub fn set_chat_paused(&self, paused: bool) {
        let mut inner = self.lock();
        if inner.chat_paused == paused {
            return;
        }
        inner.chat_paused = paused;
        let target = ChatTarget {
            username: inner.tiktok.username.clone(),
            paused,
        };
        drop(inner);
        self.chat_status(&target.waiting_status());
        self.chat_target.send_replace(target);
    }

    // --- reports from the overlay ---

    /// The overlay finished playing `id`. A stale ID is an error and changes
    /// nothing.
    pub fn player_ended(&self, id: ItemId) -> Result<(), QueueError> {
        let mut inner = self.lock();
        inner.queue.finish(id)?;
        self.set_status(&mut inner, id, RequestStatus::Played);
        self.play_next_or_stop(&mut inner);
        self.publish_queue(&inner);
        Ok(())
    }

    /// The overlay reports how far `id` has got. Reports for a video that is
    /// not playing are errors and change nothing; repeats of the last position
    /// are dropped. The position is limited to the video's length.
    pub fn player_progress(&self, id: ItemId, position_secs: u64) -> Result<(), QueueError> {
        let mut inner = self.lock();
        let duration = match inner.queue.now_playing() {
            Some(item) if item.id == id => item.media.duration_secs(),
            _ => return Err(QueueError::NotPlaying(id)),
        };
        // A video of unknown length has no position to show.
        let Some(duration) = duration else {
            return Ok(());
        };
        let position_secs = position_secs.min(duration);
        if inner.progress == Some((id, position_secs)) {
            return Ok(());
        }
        inner.progress = Some((id, position_secs));
        self.bus.publish(Event::Playback {
            item_id: id.0,
            position_secs,
        });
        Ok(())
    }

    /// The overlay could not play `id`. Skips it and tells the moderators.
    /// `code` comes from the browser and is reduced to a short safe token.
    pub fn player_error(&self, id: ItemId, code: &str) -> Result<(), QueueError> {
        let mut inner = self.lock();
        let title = match inner.queue.now_playing() {
            Some(item) if item.id == id => item.media.title(),
            _ => return Err(QueueError::NotPlaying(id)),
        };
        inner.queue.finish(id)?;
        self.set_status(&mut inner, id, RequestStatus::Failed);

        self.bus.publish(Event::Skip {
            item_id: id.0,
            reason: SkipReason::PlayerError,
        });
        let code = sanitize_code(code);
        self.notice(
            NoticeLevel::Warning,
            "error.player_failed",
            self.i18n
                .t_with("error.player_failed", &[("title", &title), ("code", &code)]),
            None,
        );
        self.play_next_or_stop(&mut inner);
        self.publish_queue(&inner);
        Ok(())
    }

    // --- the request history ---

    /// Takes over the history read from disk. Meant to be called once, before
    /// the first request.
    pub fn restore_history(&self, saved: Vec<HistoryEntry>) {
        self.lock().history = History::restore(saved);
    }

    /// Every request kept, oldest first.
    pub fn history(&self) -> Vec<HistoryEntry> {
        self.lock().history.entries()
    }

    // --- for new clients ---

    /// Subscribe first, then call `hello`, and ignore events with
    /// `seq <= hello.seq`: nothing is missed and nothing is applied twice.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.bus.subscribe()
    }

    /// The video source, for tests that need to look at it.
    #[cfg(test)]
    pub(crate) fn lookup_source(&self) -> &S {
        self.lookup.source()
    }

    /// Switches to another YouTube API key (`None`: no key) and forgets what
    /// was learned with the old one: cached answers, and a pause after a quota
    /// or rate limit. A key that cannot be used changes nothing.
    pub fn set_youtube_key(&self, key: Option<&Secret>) -> Result<(), YoutubeError> {
        self.lookup.source().set_api_key(key)?;
        self.lookup.reset();
        Ok(())
    }

    /// Tries a YouTube API key with one real request, without switching to it.
    pub async fn check_youtube_key(&self, key: &Secret) -> Result<(), YoutubeError> {
        self.lookup.source().check_api_key(key).await
    }

    /// The settings in effect now (moderation including the current volume,
    /// blacklist, dashboard).
    pub fn settings(&self) -> SettingsView {
        Self::settings_view(&self.lock())
    }

    /// Tells the moderators that settings could not be written to disk, so
    /// they know a restart would lose them.
    pub fn settings_not_saved(&self) {
        self.notice(
            NoticeLevel::Warning,
            "error.settings_not_saved",
            self.i18n.t("error.settings_not_saved").to_owned(),
            None,
        );
    }

    /// The complete current state, for a client that just connected.
    pub fn hello(&self) -> Hello {
        let inner = self.lock();
        Hello {
            server_version: env!("CARGO_PKG_VERSION").to_owned(),
            language: self.i18n.language(),
            seq: self.bus.last_seq(),
            settings: Self::settings_view(&inner),
            volume: inner.gatekeeper.settings().volume,
            queue: QueueView::from(&inner.queue.snapshot()),
            chat: inner.chat.clone(),
            chat_log: inner.chat_log.iter().cloned().collect(),
            history: inner.history.entries(),
            stats: inner.stats,
            playback_paused: inner.playback_paused,
        }
    }

    // --- internals ---

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn settings_view(inner: &Inner) -> SettingsView {
        SettingsView {
            moderation: inner.gatekeeper.settings().clone(),
            blacklist: inner.blacklist.clone(),
            dashboard: inner.dashboard.clone(),
            command: inner.command.clone(),
            chat: ChatSettingsView {
                tiktok: inner.tiktok.clone(),
            },
        }
    }

    fn publish_stats(&self, inner: &Inner) {
        self.bus.publish(Event::StatsUpdated(inner.stats));
    }

    fn publish_queue(&self, inner: &Inner) {
        self.bus.publish(Event::QueueUpdated(QueueView::from(
            &inner.queue.snapshot(),
        )));
    }

    /// Starts the next approved video if nothing is playing. Returns whether
    /// something is playing afterwards.
    fn advance(&self, inner: &mut Inner) -> bool {
        if inner.queue.now_playing().is_some() {
            return true;
        }
        let volume = inner.gatekeeper.settings().volume;
        let started = inner
            .queue
            .start_next()
            .map(|item| (item.id, ItemView::from(item)));
        let Some((id, item)) = started else {
            return false;
        };
        self.bus.publish(Event::Play { item, volume });
        self.set_status(inner, id, RequestStatus::Playing);
        true
    }

    /// After a video ends: play the next one, or tell clients the screen is
    /// free.
    fn play_next_or_stop(&self, inner: &mut Inner) {
        // Whatever happens next, it does not begin paused.
        inner.playback_paused = false;
        if !self.advance(inner) {
            self.bus.publish(Event::Stop {});
        }
    }

    fn deny(&self, sender: &Sender, subject: Subject, denial: Denial) {
        let text = denial.message(&self.i18n);
        self.log_refused(sender, subject, text.clone());
        if denial.is_silent() {
            tracing::debug!(sender = %sender.name, ?denial, "request ignored");
            return;
        }
        self.notice(NoticeLevel::Warning, denial.i18n_key(), text, Some(sender));
    }

    /// Logs a request that was turned away and tells the dashboards.
    fn log_refused(&self, sender: &Sender, subject: Subject, reason: String) {
        let mut inner = self.lock();
        let entry = inner
            .history
            .add(subject, sender, None, RequestStatus::Refused, reason);
        self.bus.publish(Event::HistoryEntry(entry));
    }

    /// Records what became of a queue entry and tells the dashboards.
    fn set_status(&self, inner: &mut Inner, id: ItemId, status: RequestStatus) {
        if let Some(entry) = inner.history.set_status(id, status) {
            self.bus.publish(Event::HistoryEntry(entry));
        }
    }

    fn notice(&self, level: NoticeLevel, code: &str, text: String, sender: Option<&Sender>) {
        self.bus.publish(Event::Notice {
            level,
            code: code.to_owned(),
            text,
            sender: sender.map(|s| s.name.clone()),
        });
    }
}

/// What the chat connection should be doing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChatTarget {
    username: String,
    paused: bool,
}

impl ChatTarget {
    fn should_read(&self) -> bool {
        !self.username.is_empty() && !self.paused
    }

    /// What to tell the moderators until a connection reports for itself.
    fn waiting_status(&self) -> ChatStatus {
        if self.username.is_empty() {
            ChatStatus::NoUsername
        } else if self.paused {
            ChatStatus::Paused
        } else {
            ChatStatus::Reconnecting {
                retry_in: Duration::ZERO,
            }
        }
    }
}

/// The TikTok settings with the username as it is used: no spaces, no `@`.
fn normalized(config: &TiktokConfig) -> TiktokConfig {
    TiktokConfig {
        username: config.username().to_owned(),
    }
}

/// Drops the oldest entries until at most `keep` remain.
fn trim_front<T>(list: &mut VecDeque<T>, keep: usize) {
    while list.len() > keep {
        list.pop_front();
    }
}

/// Reduces a browser-supplied error code to at most 16 characters of
/// `A-Z a-z 0-9 _ - .`, or "unknown".
fn sanitize_code(code: &str) -> String {
    let cleaned: String = code
        .chars()
        .take(MAX_ERROR_CODE_LEN)
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::Value;

    use super::*;
    use crate::chat::Role;
    use crate::config::{DashboardConfig, Language, ModerationMode};
    use crate::youtube::{LiveStatus, VideoDetails, VideoId, YoutubeError};

    // --- fakes ---

    /// Answers every ID with a normal video unless told otherwise.
    #[derive(Default)]
    struct FakeSource {
        overrides: Mutex<HashMap<String, Option<VideoDetails>>>,
        failure: Mutex<Option<YoutubeError>>,
        calls: AtomicUsize,
    }

    impl FakeSource {
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    fn video(id: &str) -> VideoDetails {
        VideoDetails {
            id: VideoId::parse(id).unwrap(),
            title: format!("Title of {id}"),
            channel: "Some Channel".to_owned(),
            duration_secs: 200,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
        }
    }

    impl VideoSource for FakeSource {
        async fn fetch(&self, id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.failure.lock().unwrap().clone() {
                return Err(error);
            }
            let overridden = self.overrides.lock().unwrap().get(id.as_str()).cloned();
            Ok(overridden.unwrap_or_else(|| Some(video(id.as_str()))))
        }
    }

    const ID_A: &str = "aaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbb";
    const ID_C: &str = "ccccccccccc";

    fn config(mutate: impl FnOnce(&mut ModerationConfig)) -> Config {
        let mut config = Config::default();
        config.chat.tiktok.username = "streamer".to_owned();
        config.general.language = Language::En;
        // Tests that need a cooldown say so explicitly.
        config.moderation.cooldown_secs = 0;
        mutate(&mut config.moderation);
        config
    }

    fn pipeline(config: &Config) -> Pipeline<FakeSource> {
        Pipeline::new(
            config,
            I18n::load(Language::En).unwrap(),
            FakeSource::default(),
        )
        .unwrap()
    }

    fn default_pipeline() -> Pipeline<FakeSource> {
        pipeline(&config(|_| {}))
    }

    fn sender(n: u32, role: Role) -> Sender {
        Sender::new(format!("user-{n}"), format!("Viewer {n}"), role)
    }

    fn chat(sender: Sender, text: &str) -> ChatMessage {
        ChatMessage {
            sender,
            text: text.to_owned(),
        }
    }

    async fn say(p: &Pipeline<FakeSource>, sender: Sender, text: &str) {
        p.handle_chat_at(chat(sender, text), Instant::now()).await;
    }

    /// Everything published so far, including the chat lines.
    fn drain_all(rx: &mut broadcast::Receiver<Arc<Envelope>>) -> Vec<Arc<Envelope>> {
        let mut out = Vec::new();
        while let Ok(envelope) = rx.try_recv() {
            out.push(envelope);
        }
        out
    }

    /// Everything published so far except the chat lines, the counters and
    /// the history, which every request produces and which have tests of
    /// their own.
    fn drain(rx: &mut broadcast::Receiver<Arc<Envelope>>) -> Vec<Arc<Envelope>> {
        let mut events = drain_all(rx);
        events.retain(|e| {
            !matches!(
                e.event,
                Event::ChatMessage(_) | Event::StatsUpdated(_) | Event::HistoryEntry(_)
            )
        });
        events
    }

    fn kinds(events: &[Arc<Envelope>]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                let value = serde_json::to_value(&e.event).unwrap();
                value["type"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    fn data(envelope: &Envelope) -> Value {
        serde_json::to_value(&envelope.event).unwrap()["data"].clone()
    }

    fn pending_ids(p: &Pipeline<FakeSource>) -> Vec<u64> {
        p.hello().queue.pending.iter().map(|i| i.id).collect()
    }

    // --- chat monitor ---

    fn lines(events: &[Arc<Envelope>]) -> Vec<Value> {
        events
            .iter()
            .filter(|e| matches!(e.event, Event::ChatMessage(_)))
            .map(|e| data(e))
            .collect()
    }

    fn with_history(history: usize) -> Pipeline<FakeSource> {
        let mut c = config(|_| {});
        c.dashboard.chat_history = history;
        pipeline(&c)
    }

    #[tokio::test]
    async fn every_message_is_shown_and_commands_are_marked() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Everyone), "hello everyone").await;
        say(&p, sender(2, Role::Follower), &format!("!v {ID_A}")).await;
        say(&p, sender(3, Role::Moderator), "!v not-a-link").await;

        let shown = lines(&drain_all(&mut rx));
        assert_eq!(shown.len(), 3);
        assert_eq!(shown[0]["name"], "Viewer 1");
        assert_eq!(shown[0]["role"], "everyone");
        assert_eq!(shown[0]["text"], "hello everyone");
        assert_eq!(shown[0]["command"], false);
        assert_eq!(shown[0]["skipped"], 0);
        assert_eq!(shown[1]["command"], true);
        assert_eq!(shown[1]["role"], "follower");
        assert_eq!(
            shown[2]["command"], true,
            "an unusable argument is still a command"
        );
        assert_eq!(shown[2]["role"], "moderator");
    }

    #[tokio::test]
    async fn names_and_text_are_cleaned_and_empty_messages_are_not_shown() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let rude = Sender::new("u", "Bu\ndi\u{7}", Role::Follower);
        say(&p, rude.clone(), "hi\r\nthere\t!").await;
        say(&p, rude.clone(), " \n\t ").await;
        say(&p, rude, &"x".repeat(500)).await;

        let shown = lines(&drain_all(&mut rx));
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0]["name"], "Bu di");
        assert_eq!(shown[0]["text"], "hi  there !");
        assert_eq!(shown[1]["text"].as_str().unwrap().chars().count(), 200);
        assert_eq!(p.hello().chat_log.len(), 2);
    }

    #[tokio::test]
    async fn a_new_dashboard_gets_the_last_messages_oldest_first() {
        let p = default_pipeline();
        let start = Instant::now();
        for n in 1..=25 {
            // 10 per second: well inside the rate limit.
            let at = start + Duration::from_millis(100 * n);
            p.handle_chat_at(chat(sender(1, Role::Follower), &format!("message {n}")), at)
                .await;
        }
        let log = p.hello().chat_log;
        assert_eq!(log.len(), 20, "the default history is 20");
        assert_eq!(log[0].text, "message 6");
        assert_eq!(log[19].text, "message 25");
    }

    #[tokio::test]
    async fn the_hello_and_the_stream_agree_about_chat() {
        let p = default_pipeline();
        say(&p, sender(1, Role::Follower), "before").await;
        let mut rx = p.subscribe();
        let hello = p.hello();
        say(&p, sender(1, Role::Follower), "after").await;
        let events = drain_all(&mut rx);

        assert_eq!(hello.chat_log.len(), 1);
        assert_eq!(hello.chat_log[0].text, "before");
        assert!(events[0].seq > hello.seq);
        assert_eq!(lines(&events)[0]["text"], "after");
    }

    #[tokio::test]
    async fn history_zero_remembers_nothing_but_still_shows_live_chat() {
        let p = with_history(0);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), "hello").await;
        assert_eq!(lines(&drain_all(&mut rx)).len(), 1);
        assert!(p.hello().chat_log.is_empty());
    }

    #[tokio::test]
    async fn the_history_size_can_change_while_running() {
        let p = default_pipeline();
        for n in 1..=10 {
            say(&p, sender(1, Role::Follower), &format!("m{n}")).await;
        }
        let mut rx = p.subscribe();

        p.update_dashboard(DashboardConfig { chat_history: 3 })
            .unwrap();
        let log = p.hello().chat_log;
        assert_eq!(
            log.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["m8", "m9", "m10"],
            "the newest messages are kept"
        );
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["settings_updated"]);
        assert_eq!(data(&events[0])["dashboard"]["chat_history"], 3);
        assert_eq!(p.hello().settings.dashboard.chat_history, 3);

        say(&p, sender(1, Role::Follower), "m11").await;
        assert_eq!(p.hello().chat_log.len(), 3);

        // Growing again does not bring old messages back.
        p.update_dashboard(DashboardConfig { chat_history: 50 })
            .unwrap();
        assert_eq!(p.hello().chat_log.len(), 3);
    }

    #[tokio::test]
    async fn a_history_size_that_is_too_large_is_refused_and_nothing_changes() {
        let p = default_pipeline();
        say(&p, sender(1, Role::Follower), "hello").await;
        let mut rx = p.subscribe();
        let too_big = DashboardConfig {
            chat_history: crate::config::MAX_CHAT_HISTORY + 1,
        };
        assert!(p.update_dashboard(too_big).is_err());
        assert_eq!(p.hello().settings.dashboard.chat_history, 20);
        assert_eq!(p.hello().chat_log.len(), 1);
        assert!(drain_all(&mut rx).is_empty());

        let mut c = config(|_| {});
        c.dashboard.chat_history = crate::config::MAX_CHAT_HISTORY + 1;
        assert!(
            Pipeline::new(&c, I18n::load(Language::En).unwrap(), FakeSource::default()).is_err()
        );
    }

    #[tokio::test]
    async fn a_flood_is_limited_and_the_next_message_says_how_many_were_left_out() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let start = Instant::now();
        for n in 0..25 {
            p.handle_chat_at(
                chat(sender(1, Role::Follower), &format!("flood {n}")),
                start,
            )
            .await;
        }
        let shown = lines(&drain_all(&mut rx));
        assert_eq!(shown.len(), 20, "at most 20 per second");
        assert!(shown.iter().all(|line| line["skipped"] == 0));
        assert_eq!(p.hello().chat_log.len(), 20);

        // Half a second later the window is still full.
        p.handle_chat_at(
            chat(sender(1, Role::Follower), "too soon"),
            start + Duration::from_millis(500),
        )
        .await;
        assert!(lines(&drain_all(&mut rx)).is_empty());

        // A second later it opens again, and the line carries the count: the
        // five from the flood and "too soon".
        p.handle_chat_at(
            chat(sender(1, Role::Follower), "calm again"),
            start + Duration::from_millis(1_000),
        )
        .await;
        let shown = lines(&drain_all(&mut rx));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0]["text"], "calm again");
        assert_eq!(shown[0]["skipped"], 6);

        p.handle_chat_at(
            chat(sender(1, Role::Follower), "and on"),
            start + Duration::from_millis(1_100),
        )
        .await;
        assert_eq!(
            lines(&drain_all(&mut rx))[0]["skipped"],
            0,
            "the count resets"
        );
    }

    #[tokio::test]
    async fn commands_still_work_when_the_display_is_limited() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let start = Instant::now();
        for n in 0..20 {
            p.handle_chat_at(
                chat(sender(1, Role::Follower), &format!("flood {n}")),
                start,
            )
            .await;
        }
        // The 21st message is not shown, yet it is a valid request.
        p.handle_chat_at(
            chat(sender(2, Role::Follower), &format!("!v {ID_A}")),
            start,
        )
        .await;
        let events = drain_all(&mut rx);
        assert_eq!(lines(&events).len(), 20);
        assert_eq!(pending_ids(&p).len(), 1);
        assert!(kinds(&events).contains(&"queue_updated".to_owned()));
    }

    // --- chat handling ---

    #[tokio::test]
    async fn ordinary_chat_does_nothing() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), "hello everyone").await;
        say(&p, sender(1, Role::Follower), "!vid something").await;
        assert!(drain(&mut rx).is_empty());
        assert_eq!(p.lookup.source().calls(), 0);
    }

    #[tokio::test]
    async fn a_valid_request_waits_for_approval_in_manual_mode() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;

        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["queue_updated"]);
        let pending = &data(&events[0])["pending"];
        assert_eq!(pending[0]["video_id"], ID_A);
        assert_eq!(pending[0]["requested_by"]["name"], "Viewer 1");
        assert_eq!(p.hello().queue.now_playing, None);
        assert_eq!(p.lookup.source().calls(), 1);
    }

    #[tokio::test]
    async fn auto_mode_starts_playing_at_once() {
        let p = pipeline(&config(|m| m.mode = ModerationMode::Auto));
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;

        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["play", "queue_updated"]);
        assert_eq!(data(&events[0])["item"]["video_id"], ID_A);
        assert_eq!(data(&events[0])["volume"], 50);
        assert_eq!(p.hello().queue.now_playing.unwrap().video_id, ID_A);
    }

    #[tokio::test]
    async fn links_work_like_ids() {
        let p = default_pipeline();
        say(
            &p,
            sender(1, Role::Follower),
            &format!("!v https://youtu.be/{ID_A}?t=5"),
        )
        .await;
        assert_eq!(p.hello().queue.pending[0].video_id, ID_A);
    }

    #[tokio::test]
    async fn viewers_below_the_minimum_role_are_ignored_without_spending_quota() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Everyone), &format!("!v {ID_A}")).await;
        assert!(drain(&mut rx).is_empty());
        assert_eq!(p.lookup.source().calls(), 0);
    }

    #[tokio::test]
    async fn level_requirements_apply_and_are_silent() {
        let p = pipeline(&config(|m| m.min_gifter_level = 5));
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert!(drain(&mut rx).is_empty());

        let mut rich = sender(2, Role::Follower);
        rich.gifter_level = 5;
        say(&p, rich, &format!("!v {ID_B}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
    }

    #[tokio::test]
    async fn an_unusable_argument_warns_the_moderators_only_for_eligible_senders() {
        let p = default_pipeline();
        let mut rx = p.subscribe();

        say(
            &p,
            sender(1, Role::Follower),
            "!v definitely-not-a-video-id",
        )
        .await;
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["notice"]);
        let notice = data(&events[0]);
        assert_eq!(notice["level"], "warning");
        assert_eq!(notice["code"], "reject.invalid_link");
        assert_eq!(notice["text"], "Invalid YouTube link or ID");
        assert_eq!(notice["sender"], "Viewer 1");

        say(
            &p,
            sender(2, Role::Everyone),
            "!v definitely-not-a-video-id",
        )
        .await;
        assert!(drain(&mut rx).is_empty());
        assert_eq!(p.lookup.source().calls(), 0);
    }

    #[tokio::test]
    async fn the_cooldown_blocks_quietly_and_expires() {
        let p = pipeline(&config(|m| m.cooldown_secs = 60));
        let mut rx = p.subscribe();
        let user = sender(1, Role::Follower);
        let t0 = Instant::now();

        p.handle_chat_at(chat(user.clone(), &format!("!v {ID_A}")), t0)
            .await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);

        p.handle_chat_at(
            chat(user.clone(), &format!("!v {ID_B}")),
            t0 + Duration::from_secs(10),
        )
        .await;
        assert!(drain(&mut rx).is_empty());
        assert_eq!(p.lookup.source().calls(), 1);

        p.handle_chat_at(
            chat(user, &format!("!v {ID_B}")),
            t0 + Duration::from_secs(61),
        )
        .await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
        assert_eq!(pending_ids(&p).len(), 2);
    }

    #[tokio::test]
    async fn a_rejected_video_still_uses_up_the_cooldown() {
        let p = pipeline(&config(|m| m.cooldown_secs = 60));
        p.lookup.source().overrides.lock().unwrap().insert(
            ID_A.to_owned(),
            Some(VideoDetails {
                embeddable: false,
                ..video(ID_A)
            }),
        );
        let user = sender(1, Role::Follower);
        let t0 = Instant::now();
        let mut rx = p.subscribe();

        p.handle_chat_at(chat(user.clone(), &format!("!v {ID_A}")), t0)
            .await;
        p.handle_chat_at(
            chat(user, &format!("!v {ID_B}")),
            t0 + Duration::from_secs(5),
        )
        .await;
        // One notice for the first, nothing for the second.
        assert_eq!(kinds(&drain(&mut rx)), vec!["notice"]);
        assert_eq!(p.lookup.source().calls(), 1);
    }

    #[tokio::test]
    async fn video_rules_produce_notices_and_nothing_is_queued() {
        let p = pipeline(&config(|m| m.max_duration_secs = 120));
        {
            let mut overrides = p.lookup.source().overrides.lock().unwrap();
            overrides.insert(
                ID_A.to_owned(),
                Some(VideoDetails {
                    embeddable: false,
                    ..video(ID_A)
                }),
            );
            overrides.insert(
                ID_B.to_owned(),
                Some(VideoDetails {
                    age_restricted: true,
                    ..video(ID_B)
                }),
            );
            overrides.insert(ID_C.to_owned(), None);
        }
        let mut rx = p.subscribe();

        for (n, id) in [ID_A, ID_B, ID_C, "ddddddddddd"].into_iter().enumerate() {
            let n = u32::try_from(n).unwrap();
            say(&p, sender(n, Role::Follower), &format!("!v {id}")).await;
        }
        let events = drain(&mut rx);
        let texts: Vec<String> = events
            .iter()
            .map(|e| data(e)["text"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            texts,
            vec![
                "This video cannot be embedded",
                "Video is age-restricted",
                "Video not found",
                // 200 s video against a 120 s limit
                "Video is longer than the 2:00 limit",
            ]
        );
        assert!(p.hello().queue.pending.is_empty());
    }

    #[tokio::test]
    async fn blacklists_apply_before_and_after_the_lookup() {
        let mut cfg = config(|_| {});
        cfg.blacklist.video_ids = vec![ID_A.to_owned()];
        cfg.blacklist.channels = vec!["Bad Channel".to_owned()];
        let p = pipeline(&cfg);
        p.lookup.source().overrides.lock().unwrap().insert(
            ID_B.to_owned(),
            Some(VideoDetails {
                channel: "bad channel".to_owned(),
                ..video(ID_B)
            }),
        );
        let mut rx = p.subscribe();

        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        // Blocked by ID before any API call.
        assert_eq!(p.lookup.source().calls(), 0);
        say(&p, sender(2, Role::Follower), &format!("!v {ID_B}")).await;
        assert_eq!(p.lookup.source().calls(), 1);

        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["notice", "notice"]);
        assert_eq!(data(&events[0])["code"], "reject.blacklisted_video");
        assert_eq!(data(&events[1])["code"], "reject.blacklisted_channel");
    }

    #[tokio::test]
    async fn a_full_queue_refuses_before_spending_quota() {
        let p = pipeline(&config(|m| m.max_queue_len = 2));
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        say(&p, sender(2, Role::Follower), &format!("!v {ID_B}")).await;
        let mut rx = p.subscribe();
        say(&p, sender(3, Role::Follower), &format!("!v {ID_C}")).await;

        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["notice"]);
        assert_eq!(data(&events[0])["text"], "The queue is full");
        assert_eq!(p.lookup.source().calls(), 2);
    }

    #[tokio::test]
    async fn youtube_failures_notify_and_give_the_cooldown_back() {
        let p = pipeline(&config(|m| m.cooldown_secs = 60));
        *p.lookup.source().failure.lock().unwrap() = Some(YoutubeError::Network("timeout"));
        let user = sender(1, Role::Follower);
        let t0 = Instant::now();
        let mut rx = p.subscribe();

        p.handle_chat_at(chat(user.clone(), &format!("!v {ID_A}")), t0)
            .await;
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["notice"]);
        assert_eq!(data(&events[0])["code"], "error.youtube_unavailable");
        assert_eq!(data(&events[0])["level"], "warning");

        // The failure was not the viewer's fault, so they may retry at once.
        *p.lookup.source().failure.lock().unwrap() = None;
        p.handle_chat_at(
            chat(user, &format!("!v {ID_A}")),
            t0 + Duration::from_secs(1),
        )
        .await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
    }

    #[tokio::test]
    async fn an_exhausted_quota_is_reported_and_pauses_api_calls() {
        let p = default_pipeline();
        *p.lookup.source().failure.lock().unwrap() = Some(YoutubeError::QuotaExceeded);
        let mut rx = p.subscribe();

        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let events = drain(&mut rx);
        assert_eq!(data(&events[0])["code"], "error.quota_exceeded");
        assert_eq!(
            data(&events[0])["text"],
            "YouTube API quota exhausted, try again later"
        );
        assert_eq!(p.lookup.source().calls(), 1);

        // Even after the source recovers, the lookup layer keeps the API
        // paused for a while, so the next request is refused without a call.
        *p.lookup.source().failure.lock().unwrap() = None;
        say(&p, sender(2, Role::Follower), &format!("!v {ID_B}")).await;
        assert_eq!(data(&drain(&mut rx)[0])["code"], "error.quota_exceeded");
        assert_eq!(p.lookup.source().calls(), 1);
    }

    #[tokio::test]
    async fn a_rejected_api_key_is_reported_as_an_error() {
        let p = default_pipeline();
        *p.lookup.source().failure.lock().unwrap() = Some(YoutubeError::InvalidApiKey);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let events = drain(&mut rx);
        assert_eq!(data(&events[0])["level"], "error");
        assert_eq!(data(&events[0])["code"], "error.youtube_unavailable");
    }

    // --- dashboard actions ---

    async fn two_pending(p: &Pipeline<FakeSource>) -> (ItemId, ItemId) {
        say(p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        say(p, sender(2, Role::Follower), &format!("!v {ID_B}")).await;
        let ids = pending_ids(p);
        (ItemId(ids[0]), ItemId(ids[1]))
    }

    #[tokio::test]
    async fn approving_starts_playback_when_idle_and_queues_otherwise() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        let mut rx = p.subscribe();

        p.approve(a).unwrap();
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["play", "queue_updated"]);
        assert_eq!(data(&events[0])["item"]["id"], a.0);

        p.approve(b).unwrap();
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["queue_updated"]);
        let view = p.hello().queue;
        assert_eq!(view.now_playing.unwrap().id, a.0);
        assert_eq!(view.approved[0].id, b.0);
        assert!(view.pending.is_empty());
    }

    #[tokio::test]
    async fn rejecting_removes_without_other_events() {
        let p = default_pipeline();
        let (a, _b) = two_pending(&p).await;
        let mut rx = p.subscribe();
        p.reject(a).unwrap();
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
        assert_eq!(pending_ids(&p).len(), 1);
        assert_eq!(p.reject(a), Err(QueueError::NotFound(a)));
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn unknown_or_repeated_approvals_are_errors_without_events() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        p.approve(a).unwrap(); // starts playing
        p.approve(b).unwrap(); // waits in the play queue
        let mut rx = p.subscribe();
        assert_eq!(p.approve(b), Err(QueueError::NotPending(b)));
        // The playing video is no longer part of the queue.
        assert_eq!(p.approve(a), Err(QueueError::NotFound(a)));
        assert_eq!(
            p.approve(ItemId(999)),
            Err(QueueError::NotFound(ItemId(999)))
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn skip_moves_on_to_the_next_video_or_stops() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        p.approve(a).unwrap();
        p.approve(b).unwrap();
        let mut rx = p.subscribe();

        assert!(p.skip());
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["skip", "play", "queue_updated"]);
        assert_eq!(data(&events[0])["item_id"], a.0);
        assert_eq!(data(&events[0])["reason"], "moderator");
        assert_eq!(data(&events[1])["item"]["id"], b.0);

        assert!(p.skip());
        assert_eq!(
            kinds(&drain(&mut rx)),
            vec!["skip", "stop", "queue_updated"]
        );

        assert!(!p.skip());
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn the_history_follows_each_request_to_its_end() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        let statuses = |p: &Pipeline<FakeSource>| -> Vec<RequestStatus> {
            p.history().iter().map(|entry| entry.status).collect()
        };
        assert_eq!(statuses(&p), [RequestStatus::Waiting; 2]);

        p.approve(a).unwrap();
        p.reject(b).unwrap();
        assert_eq!(
            statuses(&p),
            [RequestStatus::Playing, RequestStatus::Rejected]
        );
        p.player_ended(a).unwrap();
        assert_eq!(statuses(&p)[0], RequestStatus::Played);

        // A request turned away is logged with the reason, and published.
        let mut rx = p.subscribe();
        p.update_blacklist(BlacklistConfig {
            video_ids: vec![ID_C.to_owned()],
            ..BlacklistConfig::default()
        })
        .unwrap();
        say(&p, sender(3, Role::Moderator), &format!("!v {ID_C}")).await;
        let last = p.history().pop().unwrap();
        assert_eq!(last.status, RequestStatus::Refused);
        assert_eq!(last.video_id, ID_C);
        assert_eq!(last.reason, "This video is blacklisted");
        let events = drain_all(&mut rx);
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.event, Event::HistoryEntry(entry) if entry.id == last.id))
        );
    }

    #[tokio::test]
    async fn the_overlay_ending_a_video_advances_the_queue() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        p.approve(a).unwrap();
        p.approve(b).unwrap();
        let mut rx = p.subscribe();

        p.player_ended(a).unwrap();
        assert_eq!(kinds(&drain(&mut rx)), vec!["play", "queue_updated"]);
        p.player_ended(b).unwrap();
        assert_eq!(kinds(&drain(&mut rx)), vec!["stop", "queue_updated"]);
        assert_eq!(p.hello().queue.now_playing, None);
    }

    #[tokio::test]
    async fn stale_end_reports_change_nothing() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        p.approve(a).unwrap();
        let mut rx = p.subscribe();
        assert_eq!(p.player_ended(b), Err(QueueError::NotPlaying(b)));
        assert_eq!(
            p.player_ended(ItemId(77)),
            Err(QueueError::NotPlaying(ItemId(77)))
        );
        assert!(drain(&mut rx).is_empty());
        assert_eq!(p.hello().queue.now_playing.unwrap().id, a.0);
    }

    #[tokio::test]
    async fn a_player_error_skips_the_video_and_tells_the_moderators() {
        let p = default_pipeline();
        let (a, b) = two_pending(&p).await;
        p.approve(a).unwrap();
        p.approve(b).unwrap();
        let mut rx = p.subscribe();

        p.player_error(a, "150").unwrap();
        let events = drain(&mut rx);
        assert_eq!(
            kinds(&events),
            vec!["skip", "notice", "play", "queue_updated"]
        );
        assert_eq!(data(&events[0])["reason"], "player_error");
        assert_eq!(
            data(&events[1])["text"],
            format!("Video “Title of {ID_A}” failed to play (code 150)")
        );
        assert_eq!(data(&events[1])["level"], "warning");
        assert_eq!(data(&events[2])["item"]["id"], b.0);
    }

    #[tokio::test]
    async fn player_error_codes_from_the_browser_are_sanitized() {
        let p = pipeline(&config(|m| m.mode = ModerationMode::Auto));
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let id = ItemId(p.hello().queue.now_playing.unwrap().id);
        let mut rx = p.subscribe();

        p.player_error(id, "<script>alert(1)</script>").unwrap();
        let events = drain(&mut rx);
        let text = data(&events[1])["text"].as_str().unwrap().to_owned();
        assert!(!text.contains('<') && !text.contains('>'), "{text}");
        assert!(text.ends_with("(code scriptalert1)"), "{text}");
    }

    #[test]
    fn code_sanitizing_examples() {
        assert_eq!(sanitize_code("150"), "150");
        assert_eq!(sanitize_code("YT_ERR-5.1"), "YT_ERR-5.1");
        assert_eq!(sanitize_code(""), "unknown");
        assert_eq!(sanitize_code("!!!"), "unknown");
        assert_eq!(sanitize_code(&"a".repeat(100)).len(), MAX_ERROR_CODE_LEN);
    }

    #[tokio::test]
    async fn an_error_for_a_video_that_is_not_playing_is_refused() {
        let p = default_pipeline();
        let (a, _b) = two_pending(&p).await;
        let mut rx = p.subscribe();
        assert_eq!(p.player_error(a, "2"), Err(QueueError::NotPlaying(a)));
        assert!(drain(&mut rx).is_empty());
    }

    // --- volume and settings ---

    #[test]
    fn volume_is_limited_and_only_announced_when_it_changes() {
        let p = pipeline(&config(|m| {
            m.volume = 50;
            m.max_volume = 80;
        }));
        let mut rx = p.subscribe();

        assert_eq!(p.set_volume(100), 80);
        assert_eq!(p.set_volume(80), 80);
        assert_eq!(p.set_volume(20), 20);
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["volume", "volume"]);
        assert_eq!(data(&events[0])["volume"], 80);
        assert_eq!(data(&events[1])["volume"], 20);
        assert_eq!(p.hello().volume, 20);
        assert_eq!(p.hello().settings.moderation.volume, 20);
    }

    #[tokio::test]
    async fn playback_uses_the_current_volume() {
        let p = pipeline(&config(|m| m.mode = ModerationMode::Auto));
        p.set_volume(33);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(data(&drain(&mut rx)[0])["volume"], 33);
    }

    #[test]
    fn valid_settings_apply_and_are_announced() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let mut new = p.hello().settings.moderation;
        new.min_role = Role::Subscriber;
        new.max_queue_len = 5;
        new.volume = 70;
        new.max_volume = 90;
        p.update_settings(new).unwrap();

        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["settings_updated", "volume"]);
        assert_eq!(data(&events[0])["moderation"]["min_role"], "subscriber");
        assert_eq!(data(&events[0])["moderation"]["max_queue_len"], 5);
        assert_eq!(data(&events[1])["volume"], 70);
        assert_eq!(p.hello().volume, 70);
    }

    #[test]
    fn invalid_settings_are_refused_and_change_nothing() {
        let p = default_pipeline();
        let before = p.hello().settings;
        let mut rx = p.subscribe();

        let mut bad = before.moderation.clone();
        bad.volume = 100;
        bad.max_volume = 50;
        assert!(p.update_settings(bad).is_err());
        let mut bad = before.moderation.clone();
        bad.max_queue_len = 0;
        assert!(p.update_settings(bad).is_err());

        assert!(drain(&mut rx).is_empty());
        assert_eq!(p.hello().settings, before);
    }

    #[tokio::test]
    async fn new_settings_take_effect_for_the_next_request() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Everyone), &format!("!v {ID_A}")).await;
        assert!(drain(&mut rx).is_empty());

        let mut open = p.hello().settings.moderation;
        open.min_role = Role::Everyone;
        p.update_settings(open).unwrap();
        drain(&mut rx);
        say(&p, sender(1, Role::Everyone), &format!("!v {ID_A}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
    }

    fn commands(prefix: &str, names: &[&str]) -> CommandConfig {
        CommandConfig {
            prefix: prefix.to_owned(),
            commands: names
                .iter()
                .map(|name| crate::config::CommandEntry::youtube(*name))
                .collect(),
        }
    }

    #[tokio::test]
    async fn new_chat_commands_apply_to_the_next_message_and_are_announced() {
        let p = pipeline(&config(|m| m.mode = ModerationMode::Auto));
        let mut rx = p.subscribe();
        p.update_commands(commands("?", &["yt", "vt"])).unwrap();
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["settings_updated"]);
        assert_eq!(data(&events[0])["command"]["prefix"], "?");
        assert_eq!(data(&events[0])["command"]["commands"][1]["name"], "vt");
        assert_eq!(
            data(&events[0])["command"]["commands"][0]["source"],
            "youtube"
        );

        // The old word is ordinary chat now, the new ones are commands.
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert!(drain(&mut rx).is_empty());
        // Automatic approval: the first request starts playing at once.
        say(&p, sender(1, Role::Follower), &format!("?vt {ID_A}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["play", "queue_updated"]);
        say(&p, sender(2, Role::Follower), &format!("?YT {ID_B}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
    }

    #[test]
    fn bad_chat_commands_are_refused_and_change_nothing() {
        let p = default_pipeline();
        let before = p.hello().settings;
        let mut rx = p.subscribe();
        for bad in [
            commands("!", &[]),
            commands("!", &["v", "V"]),
            commands("!", &["two words"]),
            commands("", &["v"]),
            commands("a b", &["v"]),
        ] {
            assert!(p.update_commands(bad.clone()).is_err(), "{bad:?}");
        }
        assert!(drain_all(&mut rx).is_empty());
        assert_eq!(p.hello().settings, before);
    }

    #[test]
    fn saying_the_same_chat_commands_again_announces_nothing() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        p.update_commands(p.hello().settings.command).unwrap();
        assert!(drain_all(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn blacklist_updates_apply_and_bad_ones_are_refused() {
        let p = default_pipeline();
        let mut rx = p.subscribe();

        let bad = BlacklistConfig {
            video_ids: vec!["not-an-id".to_owned()],
            ..BlacklistConfig::default()
        };
        assert!(p.update_blacklist(bad).is_err());
        assert!(drain(&mut rx).is_empty());

        let good = BlacklistConfig {
            video_ids: vec![ID_A.to_owned()],
            ..BlacklistConfig::default()
        };
        p.update_blacklist(good.clone()).unwrap();
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["settings_updated"]);
        assert_eq!(data(&events[0])["blacklist"]["video_ids"][0], ID_A);
        assert_eq!(p.hello().settings.blacklist, good);

        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(data(&drain(&mut rx)[0])["code"], "reject.blacklisted_video");
    }

    // --- chat status, hello, ordering ---

    #[test]
    fn chat_status_is_published_and_remembered_for_new_clients() {
        let p = default_pipeline();
        assert!(p.hello().chat.is_none());
        let mut rx = p.subscribe();

        p.chat_status(&ChatStatus::WaitingForLive {
            channel: "streamer".to_owned(),
        });
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["chat_status"]);
        assert_eq!(data(&events[0])["state"], "waiting_for_live");
        assert_eq!(data(&events[0])["text"], "Waiting for @streamer to go live");

        let chat = p.hello().chat.unwrap();
        assert_eq!(chat.channel, "streamer");
    }

    #[tokio::test]
    async fn hello_describes_the_whole_state_and_matches_the_event_stream() {
        let p = default_pipeline();
        let before = p.hello();
        assert_eq!(before.seq, 0);
        assert_eq!(before.language, Language::En);
        assert_eq!(before.server_version, env!("CARGO_PKG_VERSION"));

        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let events = drain_all(&mut rx);
        let after = p.hello();
        assert_eq!(after.seq, events.last().unwrap().seq);
        assert_eq!(after.queue.pending.len(), 1);
    }

    #[tokio::test]
    async fn sequence_numbers_increase_across_everything() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let (a, _b) = two_pending(&p).await;
        p.approve(a).unwrap();
        p.set_volume(10);
        p.skip();
        let events = drain_all(&mut rx);
        assert!(events.len() >= 6);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(event.seq, i as u64 + 1);
        }
    }

    struct ScriptedSource(VecDeque<ChatEvent>);

    impl ChatSource for ScriptedSource {
        async fn next_event(&mut self) -> Option<ChatEvent> {
            self.0.pop_front()
        }
    }

    #[tokio::test]
    async fn run_chat_feeds_messages_and_status_until_the_source_ends() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let source = ScriptedSource(
            vec![
                ChatEvent::Status(ChatStatus::Connected {
                    channel: "streamer".to_owned(),
                }),
                ChatEvent::Message(chat(sender(1, Role::Follower), "just chatting")),
                ChatEvent::Message(chat(sender(1, Role::Follower), &format!("!v {ID_A}"))),
                ChatEvent::Status(ChatStatus::Disconnected),
            ]
            .into(),
        );
        p.run_chat(source).await;
        assert_eq!(
            kinds(&drain(&mut rx)),
            vec!["chat_status", "queue_updated", "chat_status"]
        );
    }

    #[test]
    fn construction_rejects_bad_settings() {
        let mut cfg = config(|_| {});
        cfg.moderation.max_queue_len = 0;
        let result = Pipeline::new(
            &cfg,
            I18n::load(Language::En).unwrap(),
            FakeSource::default(),
        );
        assert!(result.is_err());

        let mut cfg = config(|_| {});
        cfg.blacklist.title_words = vec!["!!!".to_owned()];
        let result = Pipeline::new(
            &cfg,
            I18n::load(Language::En).unwrap(),
            FakeSource::default(),
        );
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn chat_lines_carry_the_time_they_arrived() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        let before = unix_ms(SystemTime::now());
        say(&p, sender(1, Role::Follower), "hello").await;
        let after = unix_ms(SystemTime::now());
        let events = drain_all(&mut rx);
        let at = data(&events[0])["at_ms"].as_u64().unwrap();
        assert!(
            (before..=after).contains(&at),
            "{before} <= {at} <= {after}"
        );
        assert_eq!(p.hello().chat_log[0].at_ms, at);
    }

    // --- counters and playback progress ---

    fn stats(p: &Pipeline<FakeSource>) -> StatsView {
        p.hello().stats
    }

    /// The counters always add up: every request that entered the queue is
    /// approved, rejected or still waiting for approval.
    fn assert_adds_up(p: &Pipeline<FakeSource>) {
        let s = stats(p);
        let waiting = p.hello().queue.pending.len() as u64;
        assert_eq!(s.received, s.approved + s.rejected + waiting, "{s:?}");
    }

    #[tokio::test]
    async fn the_counters_follow_every_decision_and_always_add_up() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        for (n, id) in [(1, ID_A), (2, ID_B), (3, ID_C)] {
            say(&p, sender(n, Role::Follower), &format!("!v {id}")).await;
            assert_adds_up(&p);
        }
        assert_eq!((stats(&p).received, stats(&p).approved), (3, 0));
        let ids = pending_ids(&p);

        p.approve(ItemId(ids[0])).unwrap();
        p.approve(ItemId(ids[1])).unwrap();
        assert_eq!((stats(&p).approved, stats(&p).rejected), (2, 0));
        assert_adds_up(&p);

        // Rejecting a pending request and an already approved one.
        p.reject(ItemId(ids[2])).unwrap();
        assert_eq!((stats(&p).approved, stats(&p).rejected), (2, 1));
        assert_adds_up(&p);
        p.reject(ItemId(ids[1])).unwrap();
        assert_eq!((stats(&p).approved, stats(&p).rejected), (1, 2));
        assert_adds_up(&p);

        // Skipping and finishing a video do not touch the counters.
        let before = stats(&p);
        assert!(p.skip());
        assert_eq!(stats(&p), before);

        // The last event carries the same numbers as the snapshot.
        let last = drain_all(&mut rx)
            .into_iter()
            .rev()
            .find_map(|e| match &e.event {
                Event::StatsUpdated(view) => Some(*view),
                _ => None,
            })
            .unwrap();
        assert_eq!(last, stats(&p));
        assert!(stats(&p).since_ms > 0);
    }

    #[tokio::test]
    async fn auto_mode_counts_a_request_as_approved_at_once() {
        let p = pipeline(&config(|m| m.mode = ModerationMode::Auto));
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!((stats(&p).received, stats(&p).approved), (1, 1));
        assert_adds_up(&p);
    }

    #[tokio::test]
    async fn requests_that_never_enter_the_queue_are_not_counted() {
        let p = pipeline(&config(|m| m.max_queue_len = 1));
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        // Queue full, below the minimum role, invalid link, plain chat.
        say(&p, sender(2, Role::Follower), &format!("!v {ID_B}")).await;
        say(&p, sender(3, Role::Everyone), &format!("!v {ID_C}")).await;
        say(&p, sender(4, Role::Follower), "!v not-an-id").await;
        say(&p, sender(5, Role::Follower), "hello").await;
        assert_eq!(stats(&p).received, 1);
        assert_adds_up(&p);
    }

    #[tokio::test]
    async fn a_failed_action_changes_no_counter_and_publishes_nothing() {
        let p = default_pipeline();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let before = stats(&p);
        let mut rx = p.subscribe();
        assert!(p.approve(ItemId(999)).is_err());
        assert!(p.reject(ItemId(999)).is_err());
        assert_eq!(stats(&p), before);
        assert!(drain_all(&mut rx).is_empty());
    }

    async fn playing(p: &Pipeline<FakeSource>) -> ItemId {
        say(p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let id = ItemId(pending_ids(p)[0]);
        p.approve(id).unwrap();
        id
    }

    #[tokio::test]
    async fn progress_is_published_for_the_video_that_is_playing() {
        let p = default_pipeline();
        let id = playing(&p).await;
        let mut rx = p.subscribe();

        p.player_progress(id, 12).unwrap();
        let events = drain_all(&mut rx);
        assert_eq!(kinds(&events), vec!["playback"]);
        assert_eq!(
            data(&events[0]),
            serde_json::json!({"item_id": id.0, "position_secs": 12})
        );

        // The same second again is not repeated; the next one is published.
        p.player_progress(id, 12).unwrap();
        assert!(drain_all(&mut rx).is_empty());
        p.player_progress(id, 13).unwrap();
        assert_eq!(kinds(&drain_all(&mut rx)), vec!["playback"]);
    }

    #[tokio::test]
    async fn progress_never_passes_the_length_of_the_video() {
        let p = default_pipeline();
        let id = playing(&p).await;
        let mut rx = p.subscribe();
        // The fake video is 200 seconds long.
        p.player_progress(id, u64::MAX).unwrap();
        let events = drain_all(&mut rx);
        assert_eq!(data(&events[0])["position_secs"], 200);
    }

    #[tokio::test]
    async fn progress_for_a_video_that_is_not_playing_is_refused() {
        let p = default_pipeline();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let waiting = ItemId(pending_ids(&p)[0]);
        let mut rx = p.subscribe();
        assert_eq!(
            p.player_progress(waiting, 1),
            Err(QueueError::NotPlaying(waiting))
        );
        assert_eq!(
            p.player_progress(ItemId(999), 1),
            Err(QueueError::NotPlaying(ItemId(999)))
        );
        assert!(drain_all(&mut rx).is_empty());

        // After it was skipped, a late report is refused too.
        p.approve(waiting).unwrap();
        assert!(p.skip());
        assert!(p.player_progress(waiting, 5).is_err());
    }

    // --- pausing the video that is playing ---

    #[tokio::test]
    async fn the_playing_video_can_be_paused_and_resumed() {
        let p = default_pipeline();
        let id = playing(&p).await;
        let mut rx = p.subscribe();
        assert!(!p.hello().playback_paused);

        assert_eq!(p.set_playback_paused(true), PauseOutcome::Done);
        assert!(p.hello().playback_paused);
        let events = drain_all(&mut rx);
        assert_eq!(kinds(&events), vec!["playback_paused"]);
        assert_eq!(
            data(&events[0]),
            serde_json::json!({"item_id": id.0, "paused": true})
        );

        // Saying what is already so is fine and silent.
        assert_eq!(p.set_playback_paused(true), PauseOutcome::Done);
        assert!(drain_all(&mut rx).is_empty());

        assert_eq!(p.set_playback_paused(false), PauseOutcome::Done);
        assert!(!p.hello().playback_paused);
        let events = drain_all(&mut rx);
        assert_eq!(data(&events[0])["paused"], false);
    }

    #[tokio::test]
    async fn with_nothing_playing_there_is_nothing_to_pause() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        assert_eq!(p.set_playback_paused(true), PauseOutcome::NothingPlaying);
        assert!(!p.hello().playback_paused);
        assert_eq!(p.set_playback_paused(false), PauseOutcome::NothingPlaying);
        assert!(drain_all(&mut rx).is_empty());

        // A request that is only waiting is not playing either.
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(p.set_playback_paused(true), PauseOutcome::NothingPlaying);
    }

    #[tokio::test]
    async fn the_next_video_never_starts_paused() {
        for how in ["skip", "ended", "error", "last one ends"] {
            let p = default_pipeline();
            say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
            say(&p, sender(2, Role::Follower), &format!("!v {ID_B}")).await;
            let (a, b) = (ItemId(pending_ids(&p)[0]), ItemId(pending_ids(&p)[1]));
            p.approve(a).unwrap();
            if how != "last one ends" {
                p.approve(b).unwrap();
            }
            p.set_playback_paused(true);
            assert!(p.hello().playback_paused, "{how}");

            match how {
                "skip" => assert!(p.skip()),
                "ended" | "last one ends" => p.player_ended(a).unwrap(),
                _ => p.player_error(a, "150").unwrap(),
            }
            assert!(!p.hello().playback_paused, "{how}: cleared");
            // And pausing again works for whatever plays now.
            if how != "last one ends" {
                assert_eq!(p.hello().queue.now_playing.map(|i| i.id), Some(b.0));
                assert_eq!(p.set_playback_paused(true), PauseOutcome::Done, "{how}");
            }
        }
    }

    #[tokio::test]
    async fn skipping_a_paused_video_with_nothing_after_it_leaves_no_pause_behind() {
        let p = default_pipeline();
        playing(&p).await;
        p.set_playback_paused(true);
        assert!(p.skip());
        assert!(!p.hello().playback_paused);
        assert_eq!(
            p.set_playback_paused(true),
            PauseOutcome::NothingPlaying,
            "nothing is playing now"
        );
    }

    // --- the YouTube API key, changed while running ---

    #[tokio::test]
    async fn changing_the_key_forgets_what_was_looked_up_with_the_old_one() {
        let p = default_pipeline();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        say(&p, sender(2, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(
            p.lookup.source().calls(),
            1,
            "the second one came from the cache"
        );

        p.set_youtube_key(Some(&Secret::new("AIzaSy-new-key")))
            .unwrap();
        say(&p, sender(3, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(p.lookup.source().calls(), 2, "asked again with the new key");

        p.set_youtube_key(None).unwrap();
        say(&p, sender(4, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(p.lookup.source().calls(), 3, "no memory of the old answers");
    }

    #[tokio::test]
    async fn a_missing_key_is_reported_to_the_moderators_and_gives_the_cooldown_back() {
        let p = pipeline(&config(|m| m.cooldown_secs = 60));
        *p.lookup.source().failure.lock().unwrap() = Some(YoutubeError::NoApiKey);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["notice"]);
        assert_eq!(data(&events[0])["code"], "error.no_api_key");
        assert_eq!(data(&events[0])["level"], "error");
        assert!(
            data(&events[0])["text"]
                .as_str()
                .unwrap()
                .contains("API key")
        );
        assert!(pending_ids(&p).is_empty());

        // Not the viewer's fault: they may try again at once.
        *p.lookup.source().failure.lock().unwrap() = None;
        say(&p, sender(1, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(pending_ids(&p).len(), 1);
    }

    // --- the TikTok username, changed while running ---

    fn tiktok(name: &str) -> TiktokConfig {
        TiktokConfig {
            username: name.to_owned(),
        }
    }

    fn username_in_settings(p: &Pipeline<FakeSource>) -> String {
        p.hello().settings.chat.tiktok.username
    }

    #[test]
    fn the_username_is_changed_checked_and_announced() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        assert_eq!(username_in_settings(&p), "streamer");

        p.update_tiktok(tiktok("  @Some.One_1 ")).unwrap();
        assert_eq!(username_in_settings(&p), "Some.One_1");
        let events = drain_all(&mut rx);
        assert_eq!(kinds(&events), vec!["settings_updated"]);
        assert_eq!(
            data(&events[0])["chat"],
            serde_json::json!({"tiktok": {"username": "Some.One_1"}})
        );

        // The same name again says nothing.
        p.update_tiktok(tiktok("@Some.One_1")).unwrap();
        assert!(drain_all(&mut rx).is_empty());

        // A bad name is refused and nothing changes.
        for bad in ["a/b", "a b", "@", "x?y", &"x".repeat(65)] {
            assert!(p.update_tiktok(tiktok(bad)).is_err(), "{bad}");
        }
        assert_eq!(username_in_settings(&p), "Some.One_1");
        assert!(drain_all(&mut rx).is_empty());

        // Empty is allowed: it stops reading chat.
        p.update_tiktok(tiktok("")).unwrap();
        assert_eq!(username_in_settings(&p), "");
    }

    #[test]
    fn a_pipeline_can_start_without_a_username() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = String::new();
        let p = pipeline(&cfg);
        assert_eq!(username_in_settings(&p), "");
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = "bad name".to_owned();
        assert!(
            Pipeline::new(
                &cfg,
                I18n::load(Language::En).unwrap(),
                FakeSource::default()
            )
            .is_err()
        );
    }

    #[test]
    fn chat_status_names_the_current_channel() {
        let p = default_pipeline();
        let mut rx = p.subscribe();
        p.chat_status(&ChatStatus::Connected {
            channel: "ignored".to_owned(),
        });
        p.update_tiktok(tiktok("second")).unwrap();
        p.chat_status(&ChatStatus::Disconnected);
        let views: Vec<_> = drain_all(&mut rx)
            .into_iter()
            .filter_map(|e| match &e.event {
                Event::ChatStatus(view) => Some(view.channel.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(views, vec!["streamer", "second"]);
    }

    /// A chat source that says what it is given, then stays quiet like a
    /// connection waiting for the next message. Tells when it is dropped.
    struct QuietSource {
        events: VecDeque<ChatEvent>,
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }

    impl ChatSource for QuietSource {
        async fn next_event(&mut self) -> Option<ChatEvent> {
            match self.events.pop_front() {
                Some(event) => Some(event),
                None => std::future::pending().await,
            }
        }
    }

    impl Drop for QuietSource {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Set when a `QuietSource` is dropped.
    type DroppedFlag = Arc<std::sync::atomic::AtomicBool>;

    /// Sources made for each username, and the flags that say they are gone.
    #[derive(Default, Clone)]
    struct Sources {
        made: Arc<Mutex<Vec<(String, DroppedFlag)>>>,
    }

    impl Sources {
        /// The factory for `run_chat_sources`: each source sends one video
        /// request that names the username it was made for.
        fn factory(&self) -> impl Fn(&str) -> QuietSource + use<> {
            let made = Arc::clone(&self.made);
            move |name: &str| {
                let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
                made.lock()
                    .unwrap()
                    .push((name.to_owned(), Arc::clone(&dropped)));
                let id = if name == "first" { ID_A } else { ID_B };
                QuietSource {
                    events: vec![ChatEvent::Message(chat(
                        sender(if name == "first" { 1 } else { 2 }, Role::Follower),
                        &format!("!v {id}"),
                    ))]
                    .into(),
                    dropped,
                }
            }
        }

        fn names(&self) -> Vec<String> {
            self.made
                .lock()
                .unwrap()
                .iter()
                .map(|(n, _)| n.clone())
                .collect()
        }

        fn is_dropped(&self, index: usize) -> bool {
            self.made.lock().unwrap()[index]
                .1
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    use std::sync::Mutex;

    /// Waits (at most five seconds) until `condition` holds.
    async fn eventually(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "the condition never became true");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn waiting_videos(p: &Pipeline<FakeSource>) -> Vec<String> {
        p.hello()
            .queue
            .pending
            .iter()
            .map(|i| i.video_id.clone())
            .collect()
    }

    #[tokio::test]
    async fn the_chat_follows_the_username_when_it_changes() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = "first".to_owned();
        let p = Arc::new(pipeline(&cfg));
        let sources = Sources::default();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let make = sources.factory();
            async move { p.run_chat_sources(make).await }
        });

        eventually(|| waiting_videos(&p) == vec![ID_A.to_owned()]).await;
        assert_eq!(sources.names(), vec!["first"]);
        assert!(!sources.is_dropped(0));

        let mut rx = p.subscribe();
        p.update_tiktok(tiktok("second")).unwrap();
        eventually(|| waiting_videos(&p).len() == 2).await;
        assert_eq!(sources.names(), vec!["first", "second"]);
        assert!(sources.is_dropped(0), "the old connection is dropped");
        assert!(!sources.is_dropped(1));
        assert_eq!(waiting_videos(&p), vec![ID_A.to_owned(), ID_B.to_owned()]);

        // The old channel is not shown while the new one connects.
        let statuses: Vec<_> = drain_all(&mut rx)
            .into_iter()
            .filter_map(|e| match &e.event {
                Event::ChatStatus(view) => Some((view.channel.clone(), view.state)),
                _ => None,
            })
            .collect();
        assert_eq!(
            statuses,
            vec![("second".to_owned(), ChatState::Reconnecting)]
        );
        task.abort();
    }

    #[tokio::test]
    async fn without_a_username_nothing_is_read_until_one_is_set() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = String::new();
        let p = Arc::new(pipeline(&cfg));
        let sources = Sources::default();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let make = sources.factory();
            async move { p.run_chat_sources(make).await }
        });

        eventually(|| p.hello().chat.is_some()).await;
        let chat = p.hello().chat.unwrap();
        assert_eq!(chat.state, ChatState::NotConfigured);
        assert_eq!(chat.channel, "");
        assert!(chat.text.contains("username"), "{}", chat.text);
        assert!(sources.names().is_empty());

        p.update_tiktok(tiktok("first")).unwrap();
        eventually(|| waiting_videos(&p) == vec![ID_A.to_owned()]).await;
        assert_eq!(sources.names(), vec!["first"]);
        task.abort();
    }

    #[tokio::test]
    async fn emptying_the_username_stops_reading_and_says_so() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = "first".to_owned();
        let p = Arc::new(pipeline(&cfg));
        let sources = Sources::default();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let make = sources.factory();
            async move { p.run_chat_sources(make).await }
        });
        eventually(|| waiting_videos(&p).len() == 1).await;

        p.update_tiktok(tiktok("")).unwrap();
        eventually(|| sources.is_dropped(0)).await;
        eventually(|| {
            p.hello()
                .chat
                .is_some_and(|c| c.state == ChatState::NotConfigured)
        })
        .await;
        assert_eq!(
            sources.names(),
            vec!["first"],
            "no new source for an empty name"
        );
        task.abort();
    }

    fn chat_state(p: &Pipeline<FakeSource>) -> Option<ChatState> {
        p.hello().chat.map(|c| c.state)
    }

    #[tokio::test]
    async fn disconnecting_the_chat_drops_the_source_and_connecting_makes_a_new_one() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = "first".to_owned();
        let p = Arc::new(pipeline(&cfg));
        let sources = Sources::default();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let make = sources.factory();
            async move { p.run_chat_sources(make).await }
        });
        eventually(|| waiting_videos(&p).len() == 1).await;

        let mut rx = p.subscribe();
        p.set_chat_paused(true);
        // The answer is there at once, for whoever asked.
        let chat = p.hello().chat.unwrap();
        assert_eq!(chat.state, ChatState::Paused);
        assert_eq!(chat.channel, "first", "the username is kept");
        eventually(|| sources.is_dropped(0)).await;
        assert_eq!(sources.names(), vec!["first"], "no new source while paused");
        let announced: Vec<_> = drain_all(&mut rx)
            .into_iter()
            .filter_map(|e| match &e.event {
                Event::ChatStatus(view) => Some(view.state),
                _ => None,
            })
            .collect();
        assert!(announced.contains(&ChatState::Paused), "{announced:?}");

        // Saying it again changes nothing.
        let before = p.hello().seq;
        p.set_chat_paused(true);
        assert_eq!(p.hello().seq, before);

        p.set_chat_paused(false);
        assert_eq!(chat_state(&p), Some(ChatState::Reconnecting));
        eventually(|| sources.names() == vec!["first", "first"]).await;
        eventually(|| waiting_videos(&p).len() == 2).await;
        assert!(!sources.is_dropped(1));
        task.abort();
    }

    #[tokio::test]
    async fn a_username_changed_while_the_chat_is_disconnected_is_not_read_until_it_is_connected() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = "first".to_owned();
        let p = Arc::new(pipeline(&cfg));
        let sources = Sources::default();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let make = sources.factory();
            async move { p.run_chat_sources(make).await }
        });
        eventually(|| sources.names().len() == 1).await;

        p.set_chat_paused(true);
        p.update_tiktok(tiktok("second")).unwrap();
        eventually(|| p.hello().chat.is_some_and(|c| c.channel == "second")).await;
        assert_eq!(chat_state(&p), Some(ChatState::Paused));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            sources.names(),
            vec!["first"],
            "nothing is read while disconnected"
        );

        p.set_chat_paused(false);
        eventually(|| sources.names() == vec!["first", "second"]).await;
        task.abort();
    }

    #[tokio::test]
    async fn connecting_without_a_username_still_says_there_is_none() {
        let mut cfg = config(|_| {});
        cfg.chat.tiktok.username = String::new();
        let p = Arc::new(pipeline(&cfg));
        let sources = Sources::default();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let make = sources.factory();
            async move { p.run_chat_sources(make).await }
        });
        eventually(|| chat_state(&p) == Some(ChatState::NotConfigured)).await;
        p.set_chat_paused(true);
        assert_eq!(
            chat_state(&p),
            Some(ChatState::NotConfigured),
            "nothing to disconnect"
        );
        p.set_chat_paused(false);
        assert_eq!(chat_state(&p), Some(ChatState::NotConfigured));
        assert!(sources.names().is_empty());
        task.abort();
    }

    #[test]
    fn a_new_run_starts_connected() {
        let p = default_pipeline();
        assert!(!p.lock().chat_paused);
        assert!(!p.chat_target.borrow().paused);
        assert_eq!(p.chat_target.borrow().username, "streamer");
    }

    #[tokio::test]
    async fn a_source_that_ended_waits_for_a_new_username() {
        let p = Arc::new(default_pipeline());
        let made = Arc::new(Mutex::new(Vec::<String>::new()));
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            let made = Arc::clone(&made);
            async move {
                p.run_chat_sources(move |name: &str| {
                    made.lock().unwrap().push(name.to_owned());
                    ScriptedSource(VecDeque::new()) // ends at once
                })
                .await;
            }
        });
        eventually(|| made.lock().unwrap().len() == 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            *made.lock().unwrap(),
            vec!["streamer"],
            "not started over by itself"
        );

        p.update_tiktok(tiktok("other")).unwrap();
        eventually(|| made.lock().unwrap().len() == 2).await;
        assert_eq!(*made.lock().unwrap(), vec!["streamer", "other"]);
        task.abort();
    }

    #[tokio::test]
    async fn run_chat_until_handles_what_it_has_read_then_stops_for_the_signal() {
        let p = Arc::new(default_pipeline());
        let source = QuietSource {
            events: vec![ChatEvent::Message(chat(
                sender(1, Role::Follower),
                &format!("!v {ID_A}"),
            ))]
            .into(),
            dropped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn({
            let p = Arc::clone(&p);
            async move {
                p.run_chat_until(source, async {
                    let _ = stopped.await;
                })
                .await
            }
        });
        eventually(|| waiting_videos(&p).len() == 1).await;
        assert!(!task.is_finished(), "the source is only quiet, not ended");
        stop.send(()).unwrap();
        assert!(task.await.unwrap(), "stopped because it was asked to");

        let ended = p
            .run_chat_until(ScriptedSource(VecDeque::new()), std::future::pending())
            .await;
        assert!(!ended, "a source that ends is not a stop request");
    }

    // --- TikTok requests ---

    const TT_ID: &str = "7700124541145599252";
    const TT_SHORT_ID: &str = "7219950637247728923";
    const TT_LOCATION: &str =
        "https://www.tiktok.com/@feriumar758/video/7219950637247728923?_r=1&_t=ZS-9BTRBCxhj9O";

    /// A local stand-in for TikTok's short link host: answers every request
    /// with `response` and counts them.
    async fn short_link_host(response: String) -> (ShortLinkResolver, Arc<AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = url::Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&hits);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        let resolver = ShortLinkResolver::with_base_url(Duration::from_secs(5), base).unwrap();
        (resolver, hits)
    }

    fn redirect_to(location: &str) -> String {
        format!(
            "HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    fn status_only(status: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// `!v` for YouTube and `!vt` for TikTok.
    fn both_sources(mutate: impl FnOnce(&mut ModerationConfig)) -> Config {
        let mut config = config(mutate);
        config.command.commands = vec![
            crate::config::CommandEntry::youtube("v"),
            crate::config::CommandEntry::tiktok("vt"),
        ];
        config
    }

    fn notice_of(events: &[Arc<Envelope>]) -> Value {
        assert_eq!(kinds(events), vec!["notice"], "{events:?}");
        data(&events[0])
    }

    #[tokio::test]
    async fn a_tiktok_id_waits_for_approval_and_is_shown_without_details() {
        let p = pipeline(&both_sources(|_| {}));
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);

        let item = p.hello().queue.pending.remove(0);
        assert_eq!(item.source, CommandSource::Tiktok);
        assert_eq!(item.video_id, TT_ID);
        // No account was told, so the ID stands in for the title.
        assert_eq!(item.title, TT_ID);
        assert_eq!(item.channel, "TikTok");
        assert_eq!(item.duration_secs, 0);
        assert_eq!(item.thumbnail_url, "");
        assert_eq!(item.requested_by.name, "Viewer 1");
        assert_eq!(p.lookup_source().calls(), 0, "YouTube is not asked");
    }

    #[tokio::test]
    async fn a_web_link_tells_the_account() {
        let p = pipeline(&both_sources(|_| {}));
        say(
            &p,
            sender(1, Role::Follower),
            &format!("!vt https://www.tiktok.com/@cherlynn052/video/{TT_ID}?is_from_webapp=1"),
        )
        .await;
        let item = p.hello().queue.pending.remove(0);
        assert_eq!(item.video_id, TT_ID);
        assert_eq!(item.title, "@cherlynn052");
    }

    #[tokio::test]
    async fn automatic_approval_never_applies_to_tiktok() {
        let p = pipeline(&both_sources(|m| m.mode = ModerationMode::Auto));
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        // Waiting for a person: nothing plays, nothing is counted as approved.
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
        assert_eq!(p.hello().queue.pending.len(), 1);
        assert!(p.hello().queue.now_playing.is_none());
        assert_eq!(p.hello().stats.approved, 0);
        assert_eq!(p.hello().stats.received, 1);

        // A YouTube request in the same mode still goes straight through.
        say(&p, sender(2, Role::Follower), &format!("!v {ID_A}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["play", "queue_updated"]);

        // Approving the TikTok one puts it behind, and it plays in its turn.
        let id = p.hello().queue.pending[0].id;
        p.approve(ItemId(id)).unwrap();
        assert_eq!(p.hello().stats.approved, 2);
        let playing = p.hello().queue.now_playing.unwrap().id;
        p.player_ended(ItemId(playing)).unwrap();
        let events = drain(&mut rx);
        let play = events
            .iter()
            .find(|e| matches!(e.event, Event::Play { .. }))
            .unwrap();
        assert_eq!(data(play)["item"]["source"], "tiktok");
        assert_eq!(data(play)["item"]["video_id"], TT_ID);
    }

    #[tokio::test]
    async fn a_short_code_is_looked_up_once_and_remembered() {
        let (resolver, hits) = short_link_host(redirect_to(TT_LOCATION)).await;
        let p = pipeline(&both_sources(|_| {})).with_short_links(resolver);
        say(&p, sender(1, Role::Follower), "!vt ZSpGELffb").await;
        let item = p.hello().queue.pending.remove(0);
        assert_eq!(item.video_id, TT_SHORT_ID);
        assert_eq!(item.title, "@feriumar758");
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // The same code from someone else does not ask again.
        say(
            &p,
            sender(2, Role::Follower),
            "!vt https://vt.tiktok.com/ZSpGELffb/",
        )
        .await;
        assert_eq!(p.hello().queue.pending.len(), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_short_code_that_leads_nowhere_is_turned_down_and_costs_the_cooldown() {
        let (resolver, _) = short_link_host(redirect_to("https://www.tiktok.com/?_r=1")).await;
        let p = pipeline(&both_sources(|m| m.cooldown_secs = 60)).with_short_links(resolver);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), "!vt Nothing123").await;
        let notice = notice_of(&drain(&mut rx));
        assert_eq!(notice["code"], "reject.not_found");
        assert_eq!(notice["sender"], "Viewer 1");
        assert!(p.hello().queue.pending.is_empty());

        // The cooldown was spent (a cooldown denial is silent).
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        assert!(drain(&mut rx).is_empty());
        assert!(p.hello().queue.pending.is_empty());
    }

    #[tokio::test]
    async fn when_tiktok_cannot_be_reached_the_sender_keeps_the_cooldown() {
        let (resolver, _) = short_link_host(status_only("503 Service Unavailable")).await;
        let p = pipeline(&both_sources(|m| m.cooldown_secs = 60)).with_short_links(resolver);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), "!vt ZSpGELffb").await;
        let notice = notice_of(&drain(&mut rx));
        assert_eq!(notice["code"], "error.tiktok_unavailable");
        assert_eq!(notice["text"], "Cannot reach TikTok to open the short link");

        // Not their fault, so they may try again at once, with an ID.
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
    }

    #[tokio::test]
    async fn an_id_needs_no_request_to_tiktok() {
        let (resolver, hits) = short_link_host(redirect_to(TT_LOCATION)).await;
        let p = pipeline(&both_sources(|_| {})).with_short_links(resolver);
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        say(
            &p,
            sender(2, Role::Follower),
            &format!("!vt https://www.tiktok.com/@a/video/{TT_ID}"),
        )
        .await;
        assert_eq!(p.hello().queue.pending.len(), 2);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn the_same_gates_apply_before_anything_is_asked_of_tiktok() {
        let (resolver, hits) = short_link_host(redirect_to(TT_LOCATION)).await;
        let p =
            pipeline(&both_sources(|m| m.min_role = Role::Moderator)).with_short_links(resolver);
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), "!vt ZSpGELffb").await;
        // Not allowed to submit: silently ignored, and nothing was asked.
        assert!(drain(&mut rx).is_empty());
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "nothing was asked of TikTok"
        );

        let p = pipeline(&both_sources(|m| m.max_queue_len = 1));
        let mut rx = p.subscribe();
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        say(
            &p,
            sender(2, Role::Follower),
            &format!("!vt {}", "1".repeat(19)),
        )
        .await;
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), vec!["queue_updated", "notice"]);
        assert_eq!(data(&events[1])["code"], "reject.queue_full");
    }

    #[tokio::test]
    async fn the_blacklist_covers_tiktok_ids_and_accounts() {
        let mut config = both_sources(|_| {});
        config.blacklist.video_ids = vec![TT_ID.to_owned()];
        config.blacklist.channels = vec!["Bad.Account_1".to_owned()];
        let p = pipeline(&config);
        let mut rx = p.subscribe();

        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        assert_eq!(
            notice_of(&drain(&mut rx))["code"],
            "reject.blacklisted_video"
        );

        say(
            &p,
            sender(2, Role::Follower),
            &format!(
                "!vt https://www.tiktok.com/@bad.account_1/video/{}",
                "2".repeat(19)
            ),
        )
        .await;
        assert_eq!(
            notice_of(&drain(&mut rx))["code"],
            "reject.blacklisted_channel"
        );

        // The same list still catches a YouTube ID, and lets other TikToks by.
        say(
            &p,
            sender(3, Role::Follower),
            &format!("!vt https://www.tiktok.com/@fine/video/{}", "3".repeat(19)),
        )
        .await;
        assert_eq!(kinds(&drain(&mut rx)), vec!["queue_updated"]);
    }

    #[tokio::test]
    async fn a_link_of_the_other_source_is_answered_with_the_right_command() {
        let p = pipeline(&both_sources(|_| {}));
        let mut rx = p.subscribe();

        say(
            &p,
            sender(1, Role::Follower),
            "!v https://vt.tiktok.com/ZSpGELffb/",
        )
        .await;
        let notice = notice_of(&drain(&mut rx));
        assert_eq!(notice["code"], "reject.use_tiktok_command");
        assert_eq!(notice["text"], "That is a TikTok link: use !vt for TikTok");

        say(
            &p,
            sender(1, Role::Follower),
            "!vt https://youtu.be/dQw4w9WgXcQ",
        )
        .await;
        let notice = notice_of(&drain(&mut rx));
        assert_eq!(notice["code"], "reject.use_youtube_command");
        assert_eq!(notice["text"], "That is a YouTube link: use !v for YouTube");

        // Nothing was queued and nothing was asked of YouTube.
        assert!(p.hello().queue.pending.is_empty());
        assert_eq!(p.lookup_source().calls(), 0);
    }

    #[tokio::test]
    async fn with_no_command_for_the_other_source_it_is_just_an_invalid_link() {
        // Only `!v` exists: a TikTok link has no command to be sent to.
        let p = default_pipeline();
        let mut rx = p.subscribe();
        say(
            &p,
            sender(1, Role::Follower),
            "!v https://vt.tiktok.com/ZSpGELffb/",
        )
        .await;
        assert_eq!(notice_of(&drain(&mut rx))["code"], "reject.invalid_link");
    }

    #[tokio::test]
    async fn an_unusable_tiktok_argument_is_told_in_tiktok_words() {
        let p = pipeline(&both_sources(|_| {}));
        let mut rx = p.subscribe();
        for text in ["!vt", "!vt nope!", "!vt https://evil.com/x", "!vt 12"] {
            say(&p, sender(1, Role::Follower), text).await;
            let notice = notice_of(&drain(&mut rx));
            assert_eq!(notice["code"], "reject.invalid_tiktok_link", "{text}");
            assert_eq!(
                notice["text"], "Invalid TikTok link, video ID or short link code",
                "{text}"
            );
        }
        // Not for someone who could not have submitted anything anyway.
        say(&p, sender(2, Role::Everyone), "!vt nope!").await;
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn a_tiktok_video_has_no_progress_and_cannot_be_paused() {
        let p = pipeline(&both_sources(|_| {}));
        say(&p, sender(1, Role::Follower), &format!("!vt {TT_ID}")).await;
        let id = ItemId(p.hello().queue.pending[0].id);
        p.approve(id).unwrap();
        assert_eq!(
            p.hello().queue.now_playing.unwrap().source,
            CommandSource::Tiktok
        );

        let mut rx = p.subscribe();
        // The overlay has no position to report; a stray one is quietly kept out.
        p.player_progress(id, 5).unwrap();
        assert_eq!(
            p.player_progress(ItemId(id.0 + 1), 5),
            Err(QueueError::NotPlaying(ItemId(id.0 + 1)))
        );
        assert_eq!(p.set_playback_paused(true), PauseOutcome::Unsupported);
        // Resuming something that is not paused is harmless.
        assert_eq!(p.set_playback_paused(false), PauseOutcome::Done);
        assert!(drain_all(&mut rx).is_empty());
        assert!(!p.hello().playback_paused);
    }

    #[tokio::test]
    async fn a_tiktok_video_that_fails_to_play_is_skipped_by_its_title() {
        let p = pipeline(&both_sources(|_| {}));
        say(
            &p,
            sender(1, Role::Follower),
            &format!("!vt https://www.tiktok.com/@cherlynn052/video/{TT_ID}"),
        )
        .await;
        let id = ItemId(p.hello().queue.pending[0].id);
        p.approve(id).unwrap();
        let mut rx = p.subscribe();
        p.player_error(id, "load").unwrap();
        let events = drain(&mut rx);
        let notice = events
            .iter()
            .find(|e| matches!(e.event, Event::Notice { .. }))
            .unwrap();
        assert!(
            data(notice)["text"]
                .as_str()
                .unwrap()
                .contains("@cherlynn052")
        );
    }
}
