use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::broadcast;

use super::bus::EventBus;
use super::events::{
    ChatState, ChatStatusView, Envelope, Event, Hello, ItemView, NoticeLevel, QueueView,
    SettingsView, SkipReason,
};
use crate::chat::{ChatEvent, ChatMessage, ChatSource, ChatStatus, Sender};
use crate::command::{CommandParser, Parsed};
use crate::config::{BlacklistConfig, Config, ConfigError, ModerationConfig};
use crate::i18n::I18n;
use crate::queue::{Blacklist, Denial, Gatekeeper, ItemId, PlayQueue, QueueError};
use crate::youtube::{Rejection, VideoLookup, VideoSource};

/// Events kept for slow clients before they are told they lagged.
const BUS_CAPACITY: usize = 256;

/// Longest player error code accepted from the overlay.
const MAX_ERROR_CODE_LEN: usize = 16;

struct Inner {
    queue: PlayQueue,
    gatekeeper: Gatekeeper,
    blacklist: BlacklistConfig,
    chat: Option<ChatStatusView>,
}

/// Connects chat, YouTube lookups, moderation and the play queue, and
/// publishes everything that happens as events.
///
/// All state changes and the events describing them happen under one lock, so
/// events always arrive in the order the changes were made. The lock is never
/// held while waiting for YouTube.
pub struct Pipeline<S> {
    parser: CommandParser,
    lookup: VideoLookup<S>,
    i18n: I18n,
    channel: String,
    bus: EventBus,
    inner: Mutex<Inner>,
}

impl<S: VideoSource> Pipeline<S> {
    pub fn new(config: &Config, i18n: I18n, video_source: S) -> Result<Self, ConfigError> {
        config.moderation.validate()?;
        let blacklist = Blacklist::from_config(&config.blacklist)?;
        let lookup = VideoLookup::new(
            video_source,
            Duration::from_secs(config.youtube.cache_ttl_secs),
            config.youtube.cache_max_entries as u64,
        );
        Ok(Self {
            parser: CommandParser::new(&config.command.prefix, &config.command.name),
            lookup,
            i18n,
            channel: config.chat.tiktok.username().to_owned(),
            bus: EventBus::new(BUS_CAPACITY),
            inner: Mutex::new(Inner {
                queue: PlayQueue::new(),
                gatekeeper: Gatekeeper::new(config.moderation.clone(), blacklist),
                blacklist: config.blacklist.clone(),
                chat: None,
            }),
        })
    }

    /// Reads chat until the source ends. Drop the future to stop earlier.
    ///
    /// Messages are handled one at a time, so a slow YouTube lookup delays the
    /// ones behind it (by at most the request timeout) but never reorders them.
    pub async fn run_chat<C: ChatSource>(&self, mut source: C) {
        while let Some(event) = source.next_event().await {
            match event {
                ChatEvent::Message(message) => self.handle_chat(message).await,
                ChatEvent::Status(status) => self.chat_status(&status),
            }
        }
    }

    pub async fn handle_chat(&self, message: ChatMessage) {
        self.handle_chat_at(message, Instant::now()).await;
    }

    /// `handle_chat` with the clock passed in, for tests.
    async fn handle_chat_at(&self, message: ChatMessage, now: Instant) {
        let ChatMessage { sender, text } = message;

        let video_id = match self.parser.parse(&text) {
            Parsed::NotACommand => return,
            Parsed::Invalid(_) => {
                // Only worth telling the moderators when the sender could
                // have submitted something.
                let eligible = self.lock().gatekeeper.check_sender(&sender).is_ok();
                if eligible {
                    self.notice(
                        NoticeLevel::Warning,
                        "reject.invalid_link",
                        self.i18n.t("reject.invalid_link").to_owned(),
                        Some(&sender),
                    );
                }
                return;
            }
            Parsed::Video(id) => id,
        };

        // Stage 1: everything that needs no YouTube call.
        let admitted = {
            let mut inner = self.lock();
            let waiting = inner.queue.waiting_len();
            inner.gatekeeper.admit(&sender, &video_id, waiting, now)
        };
        if let Err(denial) = admitted {
            self.deny(&sender, denial);
            return;
        }

        let details = match self.lookup.lookup(&video_id).await {
            Ok(Some(details)) => details,
            Ok(None) => {
                self.deny(&sender, Denial::Video(Rejection::NotFound));
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
                self.notice(
                    level,
                    error.notice_key(),
                    self.i18n.t(error.notice_key()).to_owned(),
                    Some(&sender),
                );
                return;
            }
        };

        // Stage 2: rules that need the video's details, then queue it.
        let outcome = {
            let mut inner = self.lock();
            match inner.gatekeeper.vet(&details) {
                Err(denial) => Err(denial),
                Ok(()) => {
                    let settings = inner.gatekeeper.settings().clone();
                    match inner
                        .queue
                        .submit(sender.clone(), details, SystemTime::now(), &settings)
                    {
                        Ok(_) => {
                            self.advance(&mut inner);
                            self.publish_queue(&inner);
                            Ok(())
                        }
                        Err(denial) => {
                            inner.gatekeeper.refund(&sender);
                            Err(denial)
                        }
                    }
                }
            }
        };
        if let Err(denial) = outcome {
            self.deny(&sender, denial);
        }
    }

    /// Records and publishes a change in the chat connection.
    pub fn chat_status(&self, status: &ChatStatus) {
        let view = ChatStatusView {
            state: ChatState::from(status),
            channel: self.channel.clone(),
            text: status.message(&self.i18n),
        };
        let mut inner = self.lock();
        inner.chat = Some(view.clone());
        self.bus.publish(Event::ChatStatus(view));
    }

    // --- actions from the dashboard ---

    /// Moves a pending request to the play queue, and starts it if idle.
    pub fn approve(&self, id: ItemId) -> Result<(), QueueError> {
        let mut inner = self.lock();
        inner.queue.approve(id)?;
        self.advance(&mut inner);
        self.publish_queue(&inner);
        Ok(())
    }

    /// Removes a pending or approved request.
    pub fn reject(&self, id: ItemId) -> Result<(), QueueError> {
        let mut inner = self.lock();
        inner.queue.reject(id)?;
        self.publish_queue(&inner);
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

    // --- reports from the overlay ---

    /// The overlay finished playing `id`. A stale ID is an error and changes
    /// nothing.
    pub fn player_ended(&self, id: ItemId) -> Result<(), QueueError> {
        let mut inner = self.lock();
        inner.queue.finish(id)?;
        self.play_next_or_stop(&mut inner);
        self.publish_queue(&inner);
        Ok(())
    }

    /// The overlay could not play `id`. Skips it and tells the moderators.
    /// `code` comes from the browser and is reduced to a short safe token.
    pub fn player_error(&self, id: ItemId, code: &str) -> Result<(), QueueError> {
        let mut inner = self.lock();
        let title = match inner.queue.now_playing() {
            Some(item) if item.id == id => item.video.title.clone(),
            _ => return Err(QueueError::NotPlaying(id)),
        };
        inner.queue.finish(id)?;

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

    // --- for new clients ---

    /// Subscribe first, then call `hello`, and ignore events with
    /// `seq <= hello.seq`: nothing is missed and nothing is applied twice.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.bus.subscribe()
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
        }
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
        match inner.queue.start_next() {
            Some(item) => {
                self.bus.publish(Event::Play {
                    item: ItemView::from(item),
                    volume,
                });
                true
            }
            None => false,
        }
    }

    /// After a video ends: play the next one, or tell clients the screen is
    /// free.
    fn play_next_or_stop(&self, inner: &mut Inner) {
        if !self.advance(inner) {
            self.bus.publish(Event::Stop {});
        }
    }

    fn deny(&self, sender: &Sender, denial: Denial) {
        if denial.is_silent() {
            tracing::debug!(sender = %sender.name, ?denial, "request ignored");
            return;
        }
        self.notice(
            NoticeLevel::Warning,
            denial.i18n_key(),
            denial.message(&self.i18n),
            Some(sender),
        );
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
    use crate::config::{Language, ModerationMode};
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

    fn drain(rx: &mut broadcast::Receiver<Arc<Envelope>>) -> Vec<Arc<Envelope>> {
        let mut out = Vec::new();
        while let Ok(envelope) = rx.try_recv() {
            out.push(envelope);
        }
        out
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
        let events = drain(&mut rx);
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
        let events = drain(&mut rx);
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
}
