//! The JSON events exchanged with the overlay and the dashboard.
//!
//! This file is the contract; `docs/websocket-events.md` describes it for
//! people writing clients.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::chat::{ChatStatus, Role};
use crate::config::{BlacklistConfig, Language, ModerationConfig};
use crate::queue::{QueueItem, QueueSnapshot};

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
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub duration_secs: u64,
    pub thumbnail_url: String,
    pub requested_by: SenderView,
    pub requested_at_ms: u64,
}

impl From<&QueueItem> for ItemView {
    fn from(item: &QueueItem) -> Self {
        Self {
            id: item.id.0,
            video_id: item.video.id.to_string(),
            title: item.video.title.clone(),
            channel: item.video.channel.clone(),
            duration_secs: item.video.duration_secs,
            // Safe to build: the ID is validated to 11 URL-safe characters.
            thumbnail_url: format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", item.video.id),
            requested_by: SenderView {
                name: item.sender.name.clone(),
                role: item.sender.role,
            },
            requested_at_ms: unix_ms(item.requested_at),
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

/// Moderation settings, in the same shape as the `[moderation]` and
/// `[blacklist]` tables of `config.toml`. `moderation.volume` is always the
/// current volume.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SettingsView {
    pub moderation: ModerationConfig,
    pub blacklist: BlacklistConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    Connected,
    WaitingForLive,
    Reconnecting,
    Disconnected,
}

impl From<&ChatStatus> for ChatState {
    fn from(status: &ChatStatus) -> Self {
        match status {
            ChatStatus::Connected { .. } => Self::Connected,
            ChatStatus::WaitingForLive { .. } => Self::WaitingForLive,
            ChatStatus::Reconnecting { .. } => Self::Reconnecting,
            ChatStatus::Disconnected => Self::Disconnected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatStatusView {
    pub state: ChatState,
    pub channel: String,
    /// Ready-to-show message in the configured language.
    pub text: String,
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
    pub chat: Option<ChatStatusView>,
}

impl Hello {
    /// The version sent to the overlay: it only needs to know what is playing
    /// and how loud. Waiting requests (with viewers' names), the blacklist and
    /// the chat status stay out.
    pub fn for_overlay(mut self) -> Self {
        self.queue.pending.clear();
        self.queue.approved.clear();
        self.settings.blacklist = BlacklistConfig::default();
        self.chat = None;
        self
    }
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
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::*;
    use crate::chat::Sender;
    use crate::queue::ItemId;
    use crate::youtube::{LiveStatus, VideoDetails, VideoId};

    fn item(id: u64) -> QueueItem {
        QueueItem {
            id: ItemId(id),
            video: VideoDetails {
                id: VideoId::parse("dQw4w9WgXcQ").unwrap(),
                title: "Song".to_owned(),
                channel: "Chan".to_owned(),
                duration_secs: 213,
                embeddable: true,
                age_restricted: false,
                live: LiveStatus::None,
                private: false,
            },
            sender: Sender::new("u1", "Budi", Role::Subscriber),
            requested_at: UNIX_EPOCH + Duration::from_millis(1_700_000_000_123),
        }
    }

    fn item_json() -> Value {
        json!({
            "id": 7,
            "video_id": "dQw4w9WgXcQ",
            "title": "Song",
            "channel": "Chan",
            "duration_secs": 213,
            "thumbnail_url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
            "requested_by": {"name": "Budi", "role": "subscriber"},
            "requested_at_ms": 1_700_000_000_123u64
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
            state: ChatState::WaitingForLive,
            channel: "streamer".to_owned(),
            text: "Waiting for @streamer to go live".to_owned(),
        });
        assert_eq!(
            to_json(&status),
            json!({"type": "chat_status", "data": {
                "state": "waiting_for_live",
                "channel": "streamer",
                "text": "Waiting for @streamer to go live"
            }})
        );

        let settings = Event::SettingsUpdated(SettingsView {
            moderation: ModerationConfig::default(),
            blacklist: BlacklistConfig::default(),
        });
        let value = to_json(&settings);
        assert_eq!(value["type"], "settings_updated");
        assert_eq!(value["data"]["moderation"]["mode"], "manual");
        assert_eq!(value["data"]["moderation"]["min_role"], "follower");
        assert_eq!(value["data"]["moderation"]["volume"], 50);
        assert_eq!(value["data"]["blacklist"]["video_ids"], json!([]));
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
            },
            volume: 50,
            queue: QueueView {
                pending: vec![],
                approved: vec![],
                now_playing: Some(ItemView::from(&item(7))),
            },
            chat: None,
        };
        let value = to_json(&Event::Hello(Box::new(hello)));
        assert_eq!(value["type"], "hello");
        let data = &value["data"];
        assert_eq!(data["server_version"], "1.2.3");
        assert_eq!(data["language"], "id");
        assert_eq!(data["seq"], 9);
        assert_eq!(data["volume"], 50);
        assert_eq!(data["queue"]["now_playing"]["id"], 7);
        assert_eq!(data["chat"], Value::Null);
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
            },
            volume: 42,
            queue: QueueView {
                pending: vec![ItemView::from(&item(1))],
                approved: vec![ItemView::from(&item(2))],
                now_playing: Some(ItemView::from(&item(3))),
            },
            chat: Some(ChatStatusView {
                state: ChatState::Connected,
                channel: "streamer".to_owned(),
                text: "x".to_owned(),
            }),
        };
        let overlay = hello.for_overlay();
        assert_eq!(overlay.seq, 3);
        assert_eq!(overlay.volume, 42);
        assert_eq!(overlay.queue.now_playing.as_ref().map(|i| i.id), Some(3));
        assert!(overlay.queue.pending.is_empty() && overlay.queue.approved.is_empty());
        assert_eq!(overlay.settings.blacklist, BlacklistConfig::default());
        assert!(overlay.chat.is_none());

        let json = serde_json::to_string(&overlay).unwrap();
        assert!(!json.contains("secretword") && !json.contains("Secret Channel"));
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
        // Extra fields from newer overlays are ignored.
        assert!(parse(r#"{"type":"ended","item_id":7,"extra":true}"#).is_ok());
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
