use std::future::Future;
use std::time::Duration;

use super::sender::Sender;
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
}

impl ChatStatus {
    /// Message for moderators, in the language of `i18n`.
    pub fn message(&self, i18n: &I18n) -> String {
        match self {
            Self::Connected { channel } => {
                i18n.t_with("status.chat.connected", &[("username", channel)])
            }
            Self::WaitingForLive { channel } => {
                i18n.t_with("status.chat.waiting_for_live", &[("username", channel)])
            }
            Self::Reconnecting { .. } => i18n.t("status.chat.reconnecting").to_owned(),
            Self::Disconnected => i18n.t("status.chat.disconnected").to_owned(),
        }
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
        ];
        for language in [Language::Id, Language::En] {
            let i18n = I18n::load(language).unwrap();
            for status in &statuses {
                let text = status.message(&i18n);
                assert!(!text.is_empty());
                assert!(!text.contains('{'), "unfilled placeholder: {text}");
                assert!(!text.contains("status.chat."), "missing key: {text}");
            }
        }
    }

    #[test]
    fn messages_name_the_channel() {
        let en = I18n::load(Language::En).unwrap();
        assert_eq!(
            ChatStatus::WaitingForLive {
                channel: "streamer".to_owned()
            }
            .message(&en),
            "Waiting for @streamer to go live"
        );
        assert_eq!(
            ChatStatus::Connected {
                channel: "streamer".to_owned()
            }
            .message(&en),
            "Connected to TikTok chat @streamer"
        );
    }
}
