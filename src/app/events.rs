//! The JSON events exchanged with the overlay and the dashboard.
//!
//! This file is the contract; `docs/websocket-events.md` describes it for
//! people writing clients.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::history::HistoryEntry;
use crate::chat::{ChatStatus, Role};
use crate::config::{
    BlacklistConfig, ChatSourceKind, CommandConfig, CommandSource, DashboardConfig, Language,
    MediashareTheme, ModerationConfig, OverlayConfig, TiktokConfig, YoutubeChatConfig,
};
use crate::queue::{Media, QueueItem, QueueSnapshot};

/// Bumped when events change in a way old clients cannot handle.
pub const PROTOCOL_VERSION: u8 = 1;

/// Milliseconds since the Unix epoch, or 0 for times before it.
pub fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SenderView {
    pub name: String,
    pub role: Role,
}

/// A queue entry as clients see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemView {
    pub id: u64,
    /// Where the video is from: `"youtube"` or `"tiktok"`.
    pub source: CommandSource,
    /// The ID in the source's own form: 11 characters for YouTube, digits for
    /// TikTok. Safe to put in a URL.
    pub video_id: String,
    /// For TikTok the account (`@name`), or the ID when it is not known.
    pub title: String,
    /// `"TikTok"` for a TikTok video.
    pub channel: String,
    /// 0 when the length is not known, which is always so for TikTok.
    pub duration_secs: u64,
    /// Empty for TikTok, which has no thumbnail to show.
    pub thumbnail_url: String,
    pub requested_by: SenderView,
    pub requested_at_ms: u64,
    /// What the viewer wrote after the link; empty when nothing.
    pub message: String,
}

impl From<&QueueItem> for ItemView {
    fn from(item: &QueueItem) -> Self {
        Self {
            id: item.id.0,
            source: item.media.source(),
            video_id: item.media.video_id(),
            title: item.media.title(),
            channel: item.media.channel(),
            duration_secs: item.media.duration_secs().unwrap_or(0),
            thumbnail_url: match &item.media {
                // Safe to build: the ID is validated to 11 URL-safe characters.
                Media::Youtube(video) => {
                    format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", video.id)
                }
                Media::Tiktok(_) => String::new(),
            },
            requested_by: SenderView {
                name: item.sender.name.clone(),
                role: item.sender.role,
            },
            requested_at_ms: unix_ms(item.requested_at),
            message: item.message.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueView {
    /// Waiting for a moderator, oldest first.
    pub pending: Vec<ItemView>,
    /// Approved, in play order.
    pub approved: Vec<ItemView>,
    pub now_playing: Option<ItemView>,
}

impl From<&QueueSnapshot> for QueueView {
    fn from(snapshot: &QueueSnapshot) -> Self {
        Self {
            pending: snapshot.pending.iter().map(ItemView::from).collect(),
            approved: snapshot.approved.iter().map(ItemView::from).collect(),
            now_playing: snapshot.now_playing.as_ref().map(ItemView::from),
        }
    }
}

/// One chat message as the dashboard sees it. `name` and `text` are written
/// by other people and have been cleaned up (see `sanitize_text`), but they
/// are still not safe as HTML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatLineView {
    pub name: String,
    pub role: Role,
    pub text: String,
    /// When the message arrived, in milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// The message started with the video command (valid or not).
    pub command: bool,
    /// How many messages before this one were left out of the stream because
    /// chat was too fast; 0 almost always.
    pub skipped: u32,
}

/// Settings, in the same shape as the `[moderation]`, `[blacklist]`,
/// `[dashboard]` and `[command]` tables of `config.toml`. `moderation.volume` is always the
/// current volume.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsView {
    pub moderation: ModerationConfig,
    pub blacklist: BlacklistConfig,
    pub dashboard: DashboardConfig,
    pub overlay: OverlayConfig,
    pub command: CommandConfig,
    pub chat: ChatSettingsView,
}

/// The `[chat]` settings the dashboard can change.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ChatSettingsView {
    pub tiktok: TiktokConfig,
    pub youtube: YoutubeChatConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Connected,
    WaitingForLive,
    Reconnecting,
    Disconnected,
    /// No username or video is set, so nothing is being read.
    NotConfigured,
    /// This source is switched off on the Account page.
    Disabled,
    /// A moderator disconnected the chat; it stays so until they connect it
    /// again (or the program restarts).
    Paused,
}

impl From<&ChatStatus> for ChatState {
    fn from(status: &ChatStatus) -> Self {
        match status {
            ChatStatus::Connected { .. } => Self::Connected,
            ChatStatus::WaitingForLive { .. } => Self::WaitingForLive,
            ChatStatus::Reconnecting { .. } => Self::Reconnecting,
            ChatStatus::Disconnected => Self::Disconnected,
            ChatStatus::NotSet => Self::NotConfigured,
            ChatStatus::Paused => Self::Paused,
            ChatStatus::Disabled => Self::Disabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatStatusView {
    /// Which chat this is about. Older versions had only TikTok and sent no
    /// source; a client that finds none means TikTok.
    pub source: ChatSourceKind,
    pub state: ChatState,
    pub channel: String,
    /// Ready-to-show message in the configured language.
    pub text: String,
}

/// What the moderators have decided since the program started. Counts only
/// requests that passed the rules and entered the queue, so at any moment
/// `received = approved + rejected + waiting` (waiting being the requests still
/// awaiting approval).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StatsView {
    /// When counting started, in milliseconds since the Unix epoch.
    pub since_ms: u64,
    /// Requests that entered the queue.
    pub received: u64,
    /// Requests approved by a moderator or automatically, and not rejected
    /// afterwards. Includes the video playing and the ones already played.
    pub approved: u64,
    /// Requests rejected by a moderator, before or after approval.
    pub rejected: u64,
}

/// Sent to each client right after it connects.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hello {
    pub server_version: String,
    pub language: Language,
    /// Sequence number of the last event this snapshot includes. Clients
    /// ignore later-arriving events whose `seq` is not greater.
    pub seq: u64,
    pub settings: SettingsView,
    pub volume: u8,
    pub queue: QueueView,
    /// The status of the TikTok chat, as before there were two. New clients
    /// read `chats`.
    pub chat: Option<ChatStatusView>,
    /// The status of each chat that has reported one: TikTok first, then
    /// YouTube.
    pub chats: Vec<ChatStatusView>,
    /// The most recent chat messages, oldest first, at most
    /// `settings.dashboard.chat_history` of them.
    pub chat_log: Vec<ChatLineView>,
    /// Every request kept, oldest first (see [`history_entry`](Event::HistoryEntry)).
    pub history: Vec<HistoryEntry>,
    pub stats: StatsView,
    /// The video that is playing was paused by a moderator.
    pub playback_paused: bool,
}

impl Hello {
    /// The version sent to the mediashare overlay: it only needs to know what is playing
    /// and how loud. Waiting requests (with viewers' names), the blacklist and
    /// the chat status stay out.
    pub fn for_mediashare(mut self) -> Self {
        self.queue.pending.clear();
        self.queue.approved.clear();
        self.settings.blacklist = BlacklistConfig::default();
        self.chat = None;
        self.chats.clear();
        self.chat_log.clear();
        self.history.clear();
        self.stats = StatsView::default();
        self
    }
}

/// The overlays there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayKind {
    Mediashare,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// A moderator skipped it.
    Moderator,
    /// The overlay could not play it.
    PlayerError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
    Error,
}

/// Something that happened. Serialized as `{"type": ..., "data": {...}}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    /// Boxed because it is much larger than the other events.
    Hello(Box<Hello>),
    /// The queue changed; carries the complete queue.
    QueueUpdated(QueueView),
    /// Start playing this video now.
    Play {
        item: ItemView,
        volume: u8,
    },
    /// Stop the current video; `item_id` says which one.
    Skip {
        item_id: u64,
        reason: SkipReason,
    },
    /// Nothing is playing any more; clear the screen.
    Stop {},
    Volume {
        volume: u8,
    },
    SettingsUpdated(SettingsView),
    /// The look of an overlay changed (that overlay and the dashboard).
    OverlayTheme {
        overlay: OverlayKind,
        theme: MediashareTheme,
    },
    /// A message for the moderators, in the configured language.
    Notice {
        level: NoticeLevel,
        /// Locale catalog key of the message, for programmatic use.
        code: String,
        text: String,
        /// Display name of the viewer concerned, if any.
        sender: Option<String>,
    },
    ChatStatus(ChatStatusView),
    /// A chat message (dashboard).
    ChatMessage(ChatLineView),
    /// The counters changed (dashboard).
    StatsUpdated(StatsView),
    /// A request was logged, or what became of it changed (dashboard). A
    /// client replaces the entry with the same `id`, or adds it.
    HistoryEntry(HistoryEntry),
    /// A moderator paused or resumed the video that is playing.
    PlaybackPaused {
        item_id: u64,
        paused: bool,
    },
    /// How far the video that is playing has got, and how long it is
    /// (dashboard). The length of a TikTok video is the one its player told
    /// the overlay.
    Playback {
        item_id: u64,
        position_secs: u64,
        duration_secs: u64,
    },
}

/// An event with its place in the stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope {
    pub v: u8,
    /// 1, 2, 3, ... in publishing order.
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub ts: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// What the overlay tells the server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OverlayMessage {
    /// The page is loaded and listening.
    PlayerReady,
    /// The video finished playing.
    Ended { item_id: u64 },
    /// The YouTube player reported an error for this video.
    PlayerError { item_id: u64, code: String },
    /// How far the video has got, about once a second while it plays. For a
    /// TikTok video, whose length only its player knows, it also says how long
    /// the video is.
    Progress {
        item_id: u64,
        position_secs: u64,
        #[serde(default)]
        duration_secs: Option<u64>,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::app::history::{History, RequestStatus, Subject};
    use crate::chat::Sender;
    use crate::queue::{ItemId, TiktokDetails};
    use crate::tiktok::TiktokVideoId;
    use crate::youtube::{LiveStatus, VideoDetails, VideoId};

    fn item(id: u64) -> QueueItem {
        QueueItem {
            id: ItemId(id),
            media: Media::Youtube(VideoDetails {
                id: VideoId::parse("dQw4w9WgXcQ").unwrap(),
                title: "Song".to_owned(),
                channel: "Chan".to_owned(),
                duration_secs: 213,
                embeddable: true,
                age_restricted: false,
                live: LiveStatus::None,
                private: false,
            }),
            sender: Sender::new("u1", "Budi", Role::Subscriber),
            requested_at: UNIX_EPOCH + Duration::from_millis(1_700_000_000_123),
            message: "halo ka".to_owned(),
        }
    }

    fn item_json() -> Value {
        json!({
            "id": 7,
            "source": "youtube",
            "video_id": "dQw4w9WgXcQ",
            "title": "Song",
            "channel": "Chan",
            "duration_secs": 213,
            "thumbnail_url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
            "requested_by": {"name": "Budi", "role": "subscriber"},
            "requested_at_ms": 1_700_000_000_123u64,
            "message": "halo ka"
        })
    }

    fn to_json(event: &Event) -> Value {
        serde_json::to_value(event).unwrap()
    }

    #[test]
    fn unix_ms_converts_and_clamps() {
        assert_eq!(unix_ms(UNIX_EPOCH), 0);
        assert_eq!(unix_ms(UNIX_EPOCH + Duration::from_millis(1_500)), 1_500);
        assert_eq!(unix_ms(UNIX_EPOCH - Duration::from_secs(10)), 0);
    }

    #[test]
    fn item_view_has_the_documented_shape() {
        let view = ItemView::from(&item(7));
        assert_eq!(serde_json::to_value(view).unwrap(), item_json());
    }

    #[test]
    fn a_tiktok_item_has_a_source_no_length_and_no_thumbnail() {
        let mut tiktok = item(8);
        tiktok.media = Media::Tiktok(TiktokDetails {
            id: TiktokVideoId::parse("7700124541145599252").unwrap(),
            account: Some("cherlynn052".to_owned()),
        });
        let view = serde_json::to_value(ItemView::from(&tiktok)).unwrap();
        assert_eq!(view["source"], "tiktok");
        assert_eq!(view["video_id"], "7700124541145599252");
        assert_eq!(view["title"], "@cherlynn052");
        assert_eq!(view["channel"], "TikTok");
        assert_eq!(view["duration_secs"], 0);
        assert_eq!(view["thumbnail_url"], "");

        tiktok.media = Media::Tiktok(TiktokDetails {
            id: TiktokVideoId::parse("7700124541145599252").unwrap(),
            account: None,
        });
        let view = serde_json::to_value(ItemView::from(&tiktok)).unwrap();
        assert_eq!(view["title"], "7700124541145599252");
    }

    #[test]
    fn play_skip_stop_and_volume_events_have_the_documented_shape() {
        let play = Event::Play {
            item: ItemView::from(&item(7)),
            volume: 40,
        };
        assert_eq!(
            to_json(&play),
            json!({"type": "play", "data": {"item": item_json(), "volume": 40}})
        );
        assert_eq!(
            to_json(&Event::Skip {
                item_id: 7,
                reason: SkipReason::PlayerError
            }),
            json!({"type": "skip", "data": {"item_id": 7, "reason": "player_error"}})
        );
        assert_eq!(
            to_json(&Event::Skip {
                item_id: 7,
                reason: SkipReason::Moderator
            }),
            json!({"type": "skip", "data": {"item_id": 7, "reason": "moderator"}})
        );
        assert_eq!(
            to_json(&Event::Stop {}),
            json!({"type": "stop", "data": {}})
        );
        assert_eq!(
            to_json(&Event::Volume { volume: 55 }),
            json!({"type": "volume", "data": {"volume": 55}})
        );
    }

    #[test]
    fn queue_and_notice_events_have_the_documented_shape() {
        let snapshot = QueueSnapshot {
            pending: vec![item(7)],
            approved: vec![],
            now_playing: None,
        };
        assert_eq!(
            to_json(&Event::QueueUpdated(QueueView::from(&snapshot))),
            json!({"type": "queue_updated", "data": {
                "pending": [item_json()], "approved": [], "now_playing": null
            }})
        );

        let notice = Event::Notice {
            level: NoticeLevel::Warning,
            code: "reject.too_long".to_owned(),
            text: "Video is longer than the 10:00 limit".to_owned(),
            sender: Some("Budi".to_owned()),
        };
        assert_eq!(
            to_json(&notice),
            json!({"type": "notice", "data": {
                "level": "warning",
                "code": "reject.too_long",
                "text": "Video is longer than the 10:00 limit",
                "sender": "Budi"
            }})
        );
    }

    #[test]
    fn chat_status_and_settings_events_have_the_documented_shape() {
        let status = Event::ChatStatus(ChatStatusView {
            source: ChatSourceKind::Tiktok,
            state: ChatState::WaitingForLive,
            channel: "streamer".to_owned(),
            text: "Waiting for @streamer to go live on TikTok".to_owned(),
        });
        assert_eq!(
            to_json(&status),
            json!({"type": "chat_status", "data": {
                "source": "tiktok",
                "state": "waiting_for_live",
                "channel": "streamer",
                "text": "Waiting for @streamer to go live on TikTok"
            }})
        );

        let settings = Event::SettingsUpdated(SettingsView {
            moderation: ModerationConfig::default(),
            blacklist: BlacklistConfig::default(),
            dashboard: DashboardConfig::default(),
            overlay: OverlayConfig::default(),
            command: CommandConfig::default(),
            chat: ChatSettingsView::default(),
        });
        let value = to_json(&settings);
        assert_eq!(value["type"], "settings_updated");
        assert_eq!(value["data"]["moderation"]["mode"], "manual");
        assert_eq!(value["data"]["moderation"]["min_role"], "follower");
        assert_eq!(value["data"]["moderation"]["volume"], 50);
        assert_eq!(value["data"]["blacklist"]["video_ids"], json!([]));
        assert_eq!(value["data"]["dashboard"]["chat_history"], 20);
        assert_eq!(
            value["data"]["chat"],
            json!({
                "tiktok": {"username": "", "enabled": true},
                "youtube": {"video": "", "enabled": true},
            })
        );
    }

    #[test]
    fn a_chat_that_is_not_configured_has_its_own_state() {
        let view = ChatStatusView {
            source: ChatSourceKind::Youtube,
            state: ChatState::from(&ChatStatus::NotSet),
            channel: String::new(),
            text: "x".to_owned(),
        };
        assert_eq!(
            to_json(&Event::ChatStatus(view)),
            json!({"type": "chat_status", "data": {
                "source": "youtube", "state": "not_configured", "channel": "", "text": "x"
            }})
        );
    }

    #[test]
    fn chat_message_events_have_the_documented_shape() {
        let line = ChatLineView {
            name: "Budi".to_owned(),
            role: Role::Follower,
            text: "!v dQw4w9WgXcQ".to_owned(),
            at_ms: 1_700_000_000_123,
            command: true,
            skipped: 3,
        };
        assert_eq!(
            to_json(&Event::ChatMessage(line)),
            json!({"type": "chat_message", "data": {
                "name": "Budi",
                "role": "follower",
                "text": "!v dQw4w9WgXcQ",
                "at_ms": 1_700_000_000_123u64,
                "command": true,
                "skipped": 3
            }})
        );
    }

    #[test]
    fn the_envelope_flattens_type_and_data_next_to_the_metadata() {
        let envelope = Envelope {
            v: PROTOCOL_VERSION,
            seq: 12,
            ts: 1_700_000_000_000,
            event: Event::Volume { volume: 30 },
        };
        assert_eq!(
            serde_json::to_value(&envelope).unwrap(),
            json!({"v": 1, "seq": 12, "ts": 1_700_000_000_000u64,
                   "type": "volume", "data": {"volume": 30}})
        );
    }

    #[test]
    fn hello_carries_everything_a_new_client_needs() {
        let hello = Hello {
            server_version: "1.2.3".to_owned(),
            language: Language::Id,
            seq: 9,
            settings: SettingsView {
                moderation: ModerationConfig::default(),
                blacklist: BlacklistConfig::default(),
                dashboard: DashboardConfig::default(),
                overlay: OverlayConfig::default(),
                command: CommandConfig::default(),
                chat: ChatSettingsView::default(),
            },
            volume: 50,
            queue: QueueView {
                pending: vec![],
                approved: vec![],
                now_playing: Some(ItemView::from(&item(7))),
            },
            chat: None,
            chats: vec![],
            chat_log: vec![ChatLineView {
                name: "Sari".to_owned(),
                role: Role::Everyone,
                text: "hi".to_owned(),
                at_ms: 1,
                command: false,
                skipped: 0,
            }],
            history: vec![],
            stats: StatsView {
                since_ms: 1000,
                received: 5,
                approved: 3,
                rejected: 1,
            },
            playback_paused: true,
        };
        let value = to_json(&Event::Hello(Box::new(hello)));
        assert_eq!(value["type"], "hello");
        let data = &value["data"];
        assert_eq!(
            data["stats"],
            json!({"since_ms": 1000, "received": 5, "approved": 3, "rejected": 1})
        );
        assert_eq!(data["playback_paused"], true);
        assert_eq!(data["server_version"], "1.2.3");
        assert_eq!(data["language"], "id");
        assert_eq!(data["seq"], 9);
        assert_eq!(data["volume"], 50);
        assert_eq!(data["queue"]["now_playing"]["id"], 7);
        assert_eq!(data["chat"], Value::Null);
        assert_eq!(data["chat_log"][0]["name"], "Sari");
        assert_eq!(data["settings"]["dashboard"]["chat_history"], 20);
    }

    #[test]
    fn the_overlay_hello_keeps_only_what_is_playing_and_the_volume() {
        let hello = Hello {
            server_version: "1".to_owned(),
            language: Language::En,
            seq: 3,
            settings: SettingsView {
                moderation: ModerationConfig::default(),
                blacklist: BlacklistConfig {
                    video_ids: vec!["dQw4w9WgXcQ".to_owned()],
                    channels: vec!["Secret Channel".to_owned()],
                    title_words: vec!["secretword".to_owned()],
                },
                dashboard: DashboardConfig::default(),
                overlay: OverlayConfig::default(),
                command: CommandConfig::default(),
                chat: ChatSettingsView::default(),
            },
            volume: 42,
            queue: QueueView {
                pending: vec![ItemView::from(&item(1))],
                approved: vec![ItemView::from(&item(2))],
                now_playing: Some(ItemView::from(&item(3))),
            },
            chat: Some(ChatStatusView {
                source: ChatSourceKind::Tiktok,
                state: ChatState::Connected,
                channel: "streamer".to_owned(),
                text: "x".to_owned(),
            }),
            chats: vec![ChatStatusView {
                source: ChatSourceKind::Youtube,
                state: ChatState::Connected,
                channel: "video".to_owned(),
                text: "y".to_owned(),
            }],
            chat_log: vec![ChatLineView {
                name: "Secret Viewer".to_owned(),
                role: Role::Follower,
                text: "secret comment".to_owned(),
                at_ms: 1,
                command: false,
                skipped: 0,
            }],
            history: vec![History::default().add(
                Subject::bare(CommandSource::Youtube, "dQw4w9WgXcQ"),
                &Sender::new("id", "Secret Viewer", Role::Follower),
                None,
                RequestStatus::Refused,
                String::new(),
            )],
            stats: StatsView {
                since_ms: 1000,
                received: 5,
                approved: 3,
                rejected: 1,
            },
            playback_paused: true,
        };
        let overlay = hello.for_mediashare();
        assert_eq!(overlay.seq, 3);
        assert_eq!(overlay.volume, 42);
        assert_eq!(overlay.queue.now_playing.as_ref().map(|i| i.id), Some(3));
        assert!(overlay.queue.pending.is_empty() && overlay.queue.approved.is_empty());
        assert_eq!(overlay.settings.blacklist, BlacklistConfig::default());
        assert!(overlay.chat.is_none() && overlay.chats.is_empty());
        assert!(overlay.history.is_empty());
        assert!(overlay.chat_log.is_empty());
        assert_eq!(overlay.stats, StatsView::default());
        assert!(
            overlay.playback_paused,
            "the overlay must know to stay paused"
        );

        let json = serde_json::to_string(&overlay).unwrap();
        assert!(!json.contains("secretword") && !json.contains("Secret Channel"));
        assert!(!json.contains("Secret Viewer") && !json.contains("secret comment"));
    }

    #[test]
    fn overlay_messages_parse() {
        let parse = |text: &str| serde_json::from_str::<OverlayMessage>(text);
        assert_eq!(
            parse(r#"{"type":"player_ready"}"#).unwrap(),
            OverlayMessage::PlayerReady
        );
        assert_eq!(
            parse(r#"{"type":"ended","item_id":7}"#).unwrap(),
            OverlayMessage::Ended { item_id: 7 }
        );
        assert_eq!(
            parse(r#"{"type":"player_error","item_id":7,"code":"150"}"#).unwrap(),
            OverlayMessage::PlayerError {
                item_id: 7,
                code: "150".to_owned()
            }
        );
        assert_eq!(
            parse(r#"{"type":"progress","item_id":7,"position_secs":23}"#).unwrap(),
            OverlayMessage::Progress {
                item_id: 7,
                position_secs: 23,
                duration_secs: None
            }
        );
        assert_eq!(
            parse(r#"{"type":"progress","item_id":7,"position_secs":3,"duration_secs":15}"#)
                .unwrap(),
            OverlayMessage::Progress {
                item_id: 7,
                position_secs: 3,
                duration_secs: Some(15)
            }
        );
        // Extra fields from newer overlays are ignored.
        assert!(parse(r#"{"type":"ended","item_id":7,"extra":true}"#).is_ok());
    }

    #[test]
    fn the_new_dashboard_events_have_their_documented_shape() {
        assert_eq!(
            to_json(&Event::StatsUpdated(StatsView {
                since_ms: 5,
                received: 4,
                approved: 3,
                rejected: 1
            })),
            json!({"type": "stats_updated", "data": {
                "since_ms": 5, "received": 4, "approved": 3, "rejected": 1
            }})
        );
        assert_eq!(
            to_json(&Event::PlaybackPaused {
                item_id: 7,
                paused: true
            }),
            json!({"type": "playback_paused", "data": {"item_id": 7, "paused": true}})
        );
        assert_eq!(
            to_json(&Event::Playback {
                item_id: 7,
                position_secs: 23,
                duration_secs: 213
            }),
            json!({"type": "playback", "data": {"item_id": 7, "position_secs": 23, "duration_secs": 213}})
        );
    }

    #[test]
    fn malformed_overlay_messages_are_rejected_not_guessed() {
        let parse = |text: &str| serde_json::from_str::<OverlayMessage>(text);
        for bad in [
            "",
            "null",
            "[]",
            "{}",
            r#"{"type":"explode"}"#,
            r#"{"type":"ended"}"#,
            r#"{"type":"ended","item_id":"7"}"#,
            r#"{"type":"ended","item_id":-1}"#,
            r#"{"type":"player_error","item_id":7}"#,
            r#"{"type":"player_error","item_id":7,"code":5}"#,
            r#"{"type":"progress","item_id":7}"#,
            r#"{"type":"progress","item_id":7,"position_secs":-1}"#,
            r#"{"type":"progress","item_id":7,"position_secs":2.5}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn chat_status_converts_to_a_state() {
        let cases = [
            (
                ChatStatus::Connected {
                    channel: "c".to_owned(),
                },
                ChatState::Connected,
            ),
            (
                ChatStatus::WaitingForLive {
                    channel: "c".to_owned(),
                },
                ChatState::WaitingForLive,
            ),
            (
                ChatStatus::Reconnecting {
                    retry_in: Duration::from_secs(1),
                },
                ChatState::Reconnecting,
            ),
            (ChatStatus::Disconnected, ChatState::Disconnected),
        ];
        for (status, state) in cases {
            assert_eq!(ChatState::from(&status), state);
        }
    }
}
