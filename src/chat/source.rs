use std::future::Future;
use std::time::Duration;

use super::sender::Sender;
use crate::config::ChatSourceKind;
use crate::i18n::I18n;

/// An ordinary chat message from a viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub sender: Sender,
    /// What the sender typed. Untrusted: it only ever goes to the command
    /// parser, never into a URL or HTML.
    pub text: String,
}

/// State of the connection to the chat, for the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatStatus {
    Connected {
        channel: String,
    },
    /// The streamer is not live yet; connecting will be retried.
    WaitingForLive {
        channel: String,
    },
    Reconnecting {
        retry_in: Duration,
    },
    Disconnected,
    /// Nothing to read yet: no TikTok username or no YouTube video is set.
    NotSet,
    /// A moderator disconnected the chat from the account menu.
    Paused,
    /// This source is switched off on the Account page.
    Disabled,
}

impl ChatStatus {
    /// Message for moderators, in the language of `i18n`, about the chat of
    /// `source`.
    pub fn message(&self, i18n: &I18n, source: ChatSourceKind) -> String {
        let (state, channel) = match self {
            Self::Connected { channel } => ("connected", channel.as_str()),
            Self::WaitingForLive { channel } => ("waiting_for_live", channel.as_str()),
            Self::Reconnecting { .. } => ("reconnecting", ""),
            Self::Disconnected => ("disconnected", ""),
            Self::NotSet => ("not_set", ""),
            Self::Paused => ("paused", ""),
            Self::Disabled => ("disabled", ""),
        };
        let name = match source {
            ChatSourceKind::Tiktok => "tiktok",
            ChatSourceKind::Youtube => "youtube",
        };
        i18n.t_with(
            &format!("status.chat.{name}.{state}"),
            &[("channel", channel)],
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    Message(ChatMessage),
    Status(ChatStatus),
}

/// A source of chat messages: TikTok today, other platforms later.
///
/// Implementations deal with their own connection problems (reconnecting,
/// waiting for the streamer) and report them as `ChatEvent::Status`.
pub trait ChatSource: Send {
    /// Waits for the next event. Returns `None` only when the source has
    /// stopped for good; sources that reconnect never do.
    fn next_event(&mut self) -> impl Future<Output = Option<ChatEvent>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;

    #[test]
    fn every_status_has_a_filled_in_message_in_both_languages() {
        let statuses = [
            ChatStatus::Connected {
                channel: "someone".to_owned(),
            },
            ChatStatus::WaitingForLive {
                channel: "someone".to_owned(),
            },
            ChatStatus::Reconnecting {
                retry_in: Duration::from_secs(4),
            },
            ChatStatus::Disconnected,
            ChatStatus::NotSet,
            ChatStatus::Paused,
            ChatStatus::Disabled,
        ];
        for language in [Language::Id, Language::En] {
            let i18n = I18n::load(language).unwrap();
            for source in [ChatSourceKind::Tiktok, ChatSourceKind::Youtube] {
                for status in &statuses {
                    let text = status.message(&i18n, source);
                    assert!(!text.is_empty());
                    assert!(!text.contains('{'), "unfilled placeholder: {text}");
                    assert!(!text.contains("status.chat."), "missing key: {text}");
                }
            }
        }
    }

    #[test]
    fn messages_name_the_channel_and_the_source() {
        let en = I18n::load(Language::En).unwrap();
        let waiting = ChatStatus::WaitingForLive {
            channel: "streamer".to_owned(),
        };
        assert_eq!(
            waiting.message(&en, ChatSourceKind::Tiktok),
            "Waiting for @streamer to go live on TikTok"
        );
        assert_eq!(
            waiting.message(&en, ChatSourceKind::Youtube),
            "Waiting for the YouTube video streamer to go live"
        );
        let connected = ChatStatus::Connected {
            channel: "streamer".to_owned(),
        };
        assert_eq!(
            connected.message(&en, ChatSourceKind::Tiktok),
            "Connected to TikTok chat @streamer"
        );
        assert_eq!(
            ChatStatus::Disabled.message(&en, ChatSourceKind::Youtube),
            "YouTube chat is switched off. Switch it on in the Account page to read it."
        );
    }
}
