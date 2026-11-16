//! TikTok LIVE chat through the `piratetok-live-rs` crate.
//!
//! TikTok has no official chat API. This reads the same anonymous WebSocket
//! feed that the TikTok website uses, so it can break whenever TikTok changes
//! it. Everything that touches the crate lives in this file: to switch to
//! another library or a client of our own, implement `ChatConnector` for it.
//!
//! What is verified and what is not: the mapping below was written from the
//! crate's data definitions, not from live traffic. In particular the
//! follower flag and the level fields should be checked once against a real
//! stream (run with `RUST_LOG=stream_spectre::chat::tiktok=debug` to see the
//! raw values behind each mapped sender).

use piratetok_live_rs::errors::TikTokLiveError;
use piratetok_live_rs::structs::TikTokLiveEvent;
use piratetok_live_rs::structs::proto::{UserIdentity, WebcastChatMessage};
use piratetok_live_rs::{TikTokLive, TikTokLiveStream};

use super::connection::{
    ChatConnector, ChatFeed, ConnectError, ConnectFailure, FeedItem, ReconnectingSource,
};
use super::role::Role;
use super::sender::Sender;
use super::source::{ChatMessage, ChatStatus};
use crate::text::sanitize_text;

/// Longest chat message kept, in characters. TikTok allows far fewer; this
/// only bounds unexpected data before it reaches the command parser.
pub const MAX_COMMENT_CHARS: usize = 500;

/// Chat source for one streamer, reconnecting as needed.
pub type TiktokSource = ReconnectingSource<TiktokConnector>;

/// Creates the TikTok chat source for `username` (without the `@`).
pub fn tiktok_source(username: &str) -> TiktokSource {
    ReconnectingSource::new(TiktokConnector::new(username))
}

pub struct TiktokConnector {
    username: String,
}

impl TiktokConnector {
    pub fn new(username: &str) -> Self {
        Self {
            username: username.trim().trim_start_matches('@').to_owned(),
        }
    }
}

impl ChatConnector for TiktokConnector {
    type Feed = TiktokFeed;

    fn channel(&self) -> &str {
        &self.username
    }

    async fn connect(&mut self) -> Result<TiktokFeed, ConnectError> {
        // The crate also retries dropped connections by itself (a few quick
        // attempts); our supervisor takes over when it gives up.
        match TikTokLive::builder(&self.username).connect().await {
            Ok(stream) => Ok(TiktokFeed {
                stream,
                streamer: self.username.clone(),
            }),
            Err(error) => Err(classify(&error)),
        }
    }
}

pub struct TiktokFeed {
    stream: TikTokLiveStream,
    streamer: String,
}

impl ChatFeed for TiktokFeed {
    async fn next(&mut self) -> FeedItem {
        loop {
            match self.stream.next_event().await {
                Ok(event) => {
                    if let Some(item) = map_event(&event, &self.streamer) {
                        return item;
                    }
                }
                // The only error is "channel closed": the connection is over.
                Err(_) => return FeedItem::Ended,
            }
        }
    }
}

fn classify(error: &TikTokLiveError) -> ConnectError {
    let kind = match error {
        TikTokLiveError::HostNotOnline(_) => ConnectFailure::NotLive,
        TikTokLiveError::TikTokBlocked(_)
        | TikTokLiveError::DeviceBlocked
        | TikTokLiveError::SessionRequired(_)
        | TikTokLiveError::AgeRestricted(_) => ConnectFailure::Blocked,
        TikTokLiveError::Http(_)
        | TikTokLiveError::WebSocket(_)
        | TikTokLiveError::Io(_)
        | TikTokLiveError::Proxy(_) => ConnectFailure::Network,
        _ => ConnectFailure::Other,
    };
    ConnectError {
        kind,
        detail: sanitize_text(&error.to_string()),
    }
}

/// Translates one library event; `None` for everything we do not use.
fn map_event(event: &TikTokLiveEvent, streamer: &str) -> Option<FeedItem> {
    match event {
        TikTokLiveEvent::Chat(message) => map_chat(message, streamer).map(FeedItem::Message),
        TikTokLiveEvent::Reconnecting { delay_secs, .. } => {
            Some(FeedItem::Status(ChatStatus::Reconnecting {
                retry_in: std::time::Duration::from_secs(*delay_secs),
            }))
        }
        TikTokLiveEvent::Disconnected | TikTokLiveEvent::LiveEnded(_) => Some(FeedItem::Ended),
        _ => None,
    }
}

/// Builds a `ChatMessage`, or `None` when the sender cannot be identified
/// (which would let them dodge per-user limits).
fn map_chat(message: &WebcastChatMessage, streamer: &str) -> Option<ChatMessage> {
    let user = message.user.as_ref()?;
    let id = sender_id(user)?;
    let role = role_of(user, streamer);
    let gifter_level = gifter_level(user);
    let fan_club_level = fan_club_level(user);

    tracing::debug!(
        follow_status = user.follow_info.as_ref().map(|f| f.follow_status),
        is_follower_flag = user.is_follower,
        is_admin = user.user_attr.as_ref().map(|a| a.is_admin),
        subscribed = user.is_subscribe,
        gifter_level,
        fan_club_level,
        role = ?role,
        "chat sender mapped"
    );

    let name = if user.nickname.trim().is_empty() {
        format!("@{}", user.unique_id)
    } else {
        user.nickname.clone()
    };
    Some(ChatMessage {
        sender: Sender {
            id,
            name: sanitize_text(&name),
            role,
            gifter_level,
            fan_club_level,
        },
        text: message.comment.chars().take(MAX_COMMENT_CHARS).collect(),
    })
}

/// Stable account ID: the numeric user ID when present, otherwise the
/// `@username`. Display names are never used because they can be changed.
fn sender_id(user: &UserIdentity) -> Option<String> {
    if user.user_id != 0 {
        Some(user.user_id.to_string())
    } else if !user.unique_id.is_empty() {
        Some(user.unique_id.clone())
    } else {
        None
    }
}

/// Highest standing that applies: the streamer and room moderators, then
/// subscribers, then followers.
fn role_of(user: &UserIdentity, streamer: &str) -> Role {
    let is_streamer = !streamer.is_empty() && user.unique_id.eq_ignore_ascii_case(streamer);
    let is_moderator = user
        .user_attr
        .as_ref()
        .is_some_and(|attr| attr.is_admin || attr.is_super_admin);
    if is_streamer || is_moderator {
        return Role::Moderator;
    }

    let is_subscriber =
        user.is_subscribe || user.subscribe_info.as_ref().is_some_and(|s| s.is_subscribe);
    if is_subscriber {
        return Role::Subscriber;
    }

    // follow_status: 0 = no relation, 1 = follows the streamer, 2 = mutual.
    let follows = user
        .follow_info
        .as_ref()
        .is_some_and(|info| info.follow_status > 0);
    if follows {
        Role::Follower
    } else {
        Role::Everyone
    }
}

/// Gifter level: the pay grade, or the older "honor" level when absent.
fn gifter_level(user: &UserIdentity) -> u32 {
    let level = match (&user.pay_grade, &user.user_honor) {
        (Some(grade), _) => grade.level,
        (None, Some(honor)) => i64::from(honor.level),
        (None, None) => 0,
    };
    non_negative(level)
}

/// Fan club level: the membership data, or the summary info when absent.
fn fan_club_level(user: &UserIdentity) -> u32 {
    let from_membership = user
        .fans_club
        .as_ref()
        .and_then(|club| club.data.as_ref())
        .map(|data| i64::from(data.level))
        .filter(|level| *level > 0);
    let level = from_membership
        .or_else(|| user.fans_club_info.as_ref().map(|info| info.fans_level))
        .unwrap_or(0);
    non_negative(level)
}

fn non_negative(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use piratetok_live_rs::structs::proto::{
        FansClubData, FansClubInfo, FansClubMember, FollowInfo, PayGrade, SubscribeInfo, UserAttr,
        UserHonor, WebcastControlMessage,
    };

    use super::*;

    fn user() -> UserIdentity {
        UserIdentity {
            user_id: 42,
            unique_id: "viewer_1".to_owned(),
            nickname: "Viewer One".to_owned(),
            ..UserIdentity::default()
        }
    }

    fn chat(user: UserIdentity, comment: &str) -> WebcastChatMessage {
        WebcastChatMessage {
            user: Some(user),
            comment: comment.to_owned(),
            ..WebcastChatMessage::default()
        }
    }

    fn sender_of(user: UserIdentity) -> Sender {
        map_chat(&chat(user, "!v x"), "streamer").unwrap().sender
    }

    // --- identity ---

    #[test]
    fn sender_id_prefers_the_numeric_id_then_the_username() {
        assert_eq!(sender_id(&user()).as_deref(), Some("42"));

        let mut u = user();
        u.user_id = 0;
        assert_eq!(sender_id(&u).as_deref(), Some("viewer_1"));

        u.unique_id.clear();
        assert_eq!(sender_id(&u), None);
    }

    #[test]
    fn messages_without_an_identifiable_sender_are_dropped() {
        assert!(map_chat(&WebcastChatMessage::default(), "s").is_none());

        let mut u = user();
        u.user_id = 0;
        u.unique_id.clear();
        assert!(map_chat(&chat(u, "!v x"), "s").is_none());
    }

    #[test]
    fn the_id_does_not_depend_on_the_display_name() {
        let mut renamed = user();
        renamed.nickname = "Completely Different".to_owned();
        assert_eq!(sender_of(user()).id, sender_of(renamed).id);
    }

    #[test]
    fn names_are_sanitized_and_fall_back_to_the_username() {
        let mut u = user();
        u.nickname = "  Evil\nName\t<b>x</b> ".to_owned();
        assert_eq!(sender_of(u).name, "Evil Name <b>x</b>");

        let mut u = user();
        u.nickname = "é".repeat(1_000);
        assert_eq!(sender_of(u).name.chars().count(), 200);

        let mut u = user();
        u.nickname = "   ".to_owned();
        assert_eq!(sender_of(u).name, "@viewer_1");
    }

    #[test]
    fn long_comments_are_capped_by_characters() {
        let long = "é".repeat(MAX_COMMENT_CHARS * 4);
        let mapped = map_chat(&chat(user(), &long), "s").unwrap();
        assert_eq!(mapped.text.chars().count(), MAX_COMMENT_CHARS);

        let mapped = map_chat(&chat(user(), "!v dQw4w9WgXcQ"), "s").unwrap();
        assert_eq!(mapped.text, "!v dQw4w9WgXcQ");
    }

    // --- roles ---

    #[test]
    fn a_plain_viewer_has_the_lowest_role() {
        assert_eq!(sender_of(user()).role, Role::Everyone);
    }

    #[test]
    fn followers_are_recognised_by_follow_status() {
        for status in [1, 2] {
            let mut u = user();
            u.follow_info = Some(FollowInfo {
                follow_status: status,
                ..FollowInfo::default()
            });
            assert_eq!(sender_of(u).role, Role::Follower, "status {status}");
        }
        let mut u = user();
        u.follow_info = Some(FollowInfo::default());
        assert_eq!(sender_of(u).role, Role::Everyone);
    }

    #[test]
    fn subscribers_are_recognised_by_either_flag() {
        let mut u = user();
        u.is_subscribe = true;
        assert_eq!(sender_of(u).role, Role::Subscriber);

        let mut u = user();
        u.subscribe_info = Some(SubscribeInfo {
            is_subscribe: true,
            ..SubscribeInfo::default()
        });
        assert_eq!(sender_of(u).role, Role::Subscriber);
    }

    #[test]
    fn room_admins_are_moderators() {
        for (admin, super_admin) in [(true, false), (false, true)] {
            let mut u = user();
            u.user_attr = Some(UserAttr {
                is_admin: admin,
                is_super_admin: super_admin,
                ..UserAttr::default()
            });
            assert_eq!(sender_of(u).role, Role::Moderator);
        }
    }

    #[test]
    fn the_streamer_is_a_moderator_whatever_the_letter_case() {
        let mut u = user();
        u.unique_id = "StReAmEr".to_owned();
        assert_eq!(
            map_chat(&chat(u, "x"), "streamer").unwrap().sender.role,
            Role::Moderator
        );
        // An empty configured name must not match anyone.
        let mut u = user();
        u.unique_id.clear();
        assert_eq!(
            map_chat(&chat(u, "x"), "").unwrap().sender.role,
            Role::Everyone
        );
    }

    #[test]
    fn the_highest_applicable_role_wins() {
        let mut u = user();
        u.is_subscribe = true;
        u.follow_info = Some(FollowInfo {
            follow_status: 1,
            ..FollowInfo::default()
        });
        assert_eq!(sender_of(u.clone()).role, Role::Subscriber);

        u.user_attr = Some(UserAttr {
            is_admin: true,
            ..UserAttr::default()
        });
        assert_eq!(sender_of(u).role, Role::Moderator);
    }

    // --- levels ---

    #[test]
    fn gifter_level_comes_from_the_pay_grade_then_the_honor_level() {
        assert_eq!(sender_of(user()).gifter_level, 0);

        let mut u = user();
        u.user_honor = Some(UserHonor {
            level: 7,
            ..UserHonor::default()
        });
        assert_eq!(sender_of(u.clone()).gifter_level, 7);

        u.pay_grade = Some(PayGrade {
            level: 12,
            ..PayGrade::default()
        });
        assert_eq!(sender_of(u).gifter_level, 12);
    }

    #[test]
    fn fan_club_level_comes_from_membership_then_the_summary() {
        assert_eq!(sender_of(user()).fan_club_level, 0);

        let mut u = user();
        u.fans_club_info = Some(FansClubInfo {
            fans_level: 3,
            ..FansClubInfo::default()
        });
        assert_eq!(sender_of(u.clone()).fan_club_level, 3);

        u.fans_club = Some(FansClubMember {
            data: Some(FansClubData {
                level: 9,
                ..FansClubData::default()
            }),
            ..FansClubMember::default()
        });
        assert_eq!(sender_of(u.clone()).fan_club_level, 9);

        // A membership entry with level 0 falls back to the summary.
        u.fans_club = Some(FansClubMember {
            data: Some(FansClubData::default()),
            ..FansClubMember::default()
        });
        assert_eq!(sender_of(u).fan_club_level, 3);
    }

    #[test]
    fn odd_level_values_are_clamped() {
        assert_eq!(non_negative(-5), 0);
        assert_eq!(non_negative(0), 0);
        assert_eq!(non_negative(33), 33);
        assert_eq!(non_negative(i64::MAX), u32::MAX);

        let mut u = user();
        u.pay_grade = Some(PayGrade {
            level: -1,
            ..PayGrade::default()
        });
        assert_eq!(sender_of(u).gifter_level, 0);
    }

    // --- events ---

    #[test]
    fn chat_events_become_messages() {
        let event = TikTokLiveEvent::Chat(chat(user(), "!v dQw4w9WgXcQ"));
        match map_event(&event, "streamer") {
            Some(FeedItem::Message(message)) => {
                assert_eq!(message.text, "!v dQw4w9WgXcQ");
                assert_eq!(message.sender.id, "42");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn connection_lifecycle_events_are_mapped() {
        let reconnecting = TikTokLiveEvent::Reconnecting {
            attempt: 2,
            max_retries: 5,
            delay_secs: 4,
        };
        assert_eq!(
            map_event(&reconnecting, "s"),
            Some(FeedItem::Status(ChatStatus::Reconnecting {
                retry_in: std::time::Duration::from_secs(4)
            }))
        );
        assert_eq!(
            map_event(&TikTokLiveEvent::Disconnected, "s"),
            Some(FeedItem::Ended)
        );
        let ended = TikTokLiveEvent::LiveEnded(WebcastControlMessage::default());
        assert_eq!(map_event(&ended, "s"), Some(FeedItem::Ended));
    }

    #[test]
    fn events_we_do_not_use_are_ignored() {
        let connected = TikTokLiveEvent::Connected {
            room_id: "1".to_owned(),
        };
        assert_eq!(map_event(&connected, "s"), None);
        let unknown = TikTokLiveEvent::Unknown {
            method: "WebcastSomethingNew".to_owned(),
            payload: vec![1, 2, 3],
        };
        assert_eq!(map_event(&unknown, "s"), None);
    }

    // --- connection errors ---

    #[test]
    fn connection_errors_are_classified() {
        let kind = |error: TikTokLiveError| classify(&error).kind;
        assert_eq!(
            kind(TikTokLiveError::HostNotOnline("x".to_owned())),
            ConnectFailure::NotLive
        );
        assert_eq!(
            kind(TikTokLiveError::TikTokBlocked("x".to_owned())),
            ConnectFailure::Blocked
        );
        assert_eq!(
            kind(TikTokLiveError::DeviceBlocked),
            ConnectFailure::Blocked
        );
        assert_eq!(
            kind(TikTokLiveError::Io(std::io::Error::other("x"))),
            ConnectFailure::Network
        );
        assert_eq!(
            kind(TikTokLiveError::Proxy("x".to_owned())),
            ConnectFailure::Network
        );
        assert_eq!(
            kind(TikTokLiveError::UserNotFound("x".to_owned())),
            ConnectFailure::Other
        );
    }

    #[test]
    fn error_details_are_bounded_plain_text() {
        let error = TikTokLiveError::TikTokBlocked(format!("line1\nline2 {}", "x".repeat(1_000)));
        let detail = classify(&error).detail;
        assert!(!detail.contains('\n'));
        assert!(detail.chars().count() <= 200);
    }

    #[test]
    fn the_connector_normalises_the_username() {
        assert_eq!(TiktokConnector::new(" @someone ").channel(), "someone");
        assert_eq!(TiktokConnector::new("plain").channel(), "plain");
    }
}
