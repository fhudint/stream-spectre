//! A log of every request viewers made and what became of it.
//!
//! Kept in memory, newest last, up to `HISTORY_LIMIT` entries. The pipeline
//! publishes each change as an event; `history_store` keeps the list on disk
//! between runs.

use std::collections::VecDeque;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::events::unix_ms;
use crate::chat::Sender;
use crate::config::CommandSource;
use crate::queue::{ItemId, Media};
use crate::text::{sanitize_message, sanitize_text};

/// Entries kept. shortcut: a fixed size; make it a setting if people want
/// longer memory.
pub const HISTORY_LIMIT: usize = 500;

/// What became of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestStatus {
    /// Waiting for a moderator.
    Waiting,
    /// Approved, waiting for its turn.
    Queued,
    Playing,
    Played,
    /// A moderator skipped it while it played.
    Skipped,
    /// A moderator removed it before it played.
    Rejected,
    /// The player could not play it.
    Failed,
    /// Turned away by the rules or a failed lookup; it never entered the queue.
    Refused,
    /// Was still waiting or playing when the program stopped.
    Dropped,
}

impl RequestStatus {
    /// Statuses that only make sense while the program runs.
    fn is_open(self) -> bool {
        matches!(self, Self::Waiting | Self::Queued | Self::Playing)
    }
}

/// One request, as the dashboard and the history file hold it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// 1, 2, 3, ... in the order requests came in; kept across runs.
    pub id: u64,
    pub source: CommandSource,
    /// In the source's own form. A TikTok short code when the request was
    /// turned away before the link could be followed.
    pub video_id: String,
    /// For TikTok the account (`@name`), or the ID. Empty when the request
    /// was turned away before the video was looked up.
    pub title: String,
    /// The YouTube channel or the TikTok account (without `@`); empty when
    /// not known.
    pub channel: String,
    pub requested_by: String,
    /// When the request came in, in milliseconds since the Unix epoch.
    pub requested_at_ms: u64,
    pub status: RequestStatus,
    /// Why a refused request was turned away, in the configured language.
    #[serde(default)]
    pub reason: String,
    /// What the viewer wrote after the link; empty when nothing.
    #[serde(default)]
    pub message: String,
    /// The queue entry this belongs to, while the program that made it runs.
    #[serde(skip)]
    item: Option<ItemId>,
}

/// What a request was for, as far as it is known.
#[derive(Debug, Clone)]
pub struct Subject {
    source: CommandSource,
    video_id: String,
    title: String,
    channel: String,
    message: String,
}

impl Subject {
    /// A request only known by its ID, or by a short link's code.
    pub fn bare(source: CommandSource, video_id: impl Into<String>) -> Self {
        Self {
            source,
            video_id: video_id.into(),
            title: String::new(),
            channel: String::new(),
            message: String::new(),
        }
    }

    /// The same, with what the viewer wrote after the link.
    pub fn with_message(mut self, message: &str) -> Self {
        self.message = message.to_owned();
        self
    }
}

impl From<&Media> for Subject {
    fn from(media: &Media) -> Self {
        Self {
            source: media.source(),
            video_id: media.video_id(),
            title: media.title(),
            channel: match media {
                Media::Youtube(_) | Media::Song(_) => media.channel(),
                Media::Tiktok(video) => video.account.clone().unwrap_or_default(),
            },
            message: String::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    entries: VecDeque<HistoryEntry>,
    last_id: u64,
}

impl History {
    /// The list read from disk. Requests that were still open when the last
    /// run ended can never finish, so they become `Dropped`.
    pub fn restore(saved: Vec<HistoryEntry>) -> Self {
        let mut entries: VecDeque<HistoryEntry> = saved.into();
        for entry in &mut entries {
            if entry.status.is_open() {
                entry.status = RequestStatus::Dropped;
            }
        }
        let mut history = Self {
            last_id: entries.iter().map(|entry| entry.id).max().unwrap_or(0),
            entries,
        };
        history.trim();
        history
    }

    /// Oldest first.
    pub fn entries(&self) -> Vec<HistoryEntry> {
        self.entries.iter().cloned().collect()
    }

    /// Notes a new request and returns it as logged.
    pub fn add(
        &mut self,
        subject: Subject,
        sender: &Sender,
        item: Option<ItemId>,
        status: RequestStatus,
        reason: String,
    ) -> HistoryEntry {
        self.last_id = self.last_id.saturating_add(1);
        let entry = HistoryEntry {
            id: self.last_id,
            source: subject.source,
            video_id: subject.video_id,
            title: sanitize_text(&subject.title),
            channel: sanitize_text(&subject.channel),
            requested_by: sanitize_text(&sender.name),
            requested_at_ms: unix_ms(SystemTime::now()),
            status,
            reason: sanitize_text(&reason),
            message: sanitize_message(&subject.message),
            item,
        };
        self.entries.push_back(entry.clone());
        self.trim();
        entry
    }

    /// Records what became of a queue entry. `None` when it is not in the
    /// log (it was trimmed) or the status is already so.
    pub fn set_status(&mut self, item: ItemId, status: RequestStatus) -> Option<HistoryEntry> {
        let entry = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.item == Some(item))?;
        if entry.status == status {
            return None;
        }
        entry.status = status;
        Some(entry.clone())
    }

    fn trim(&mut self) {
        while self.entries.len() > HISTORY_LIMIT {
            self.entries.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Role;

    fn viewer() -> Sender {
        Sender::new("id-1".to_owned(), "Viewer".to_owned(), Role::Follower)
    }

    fn add(history: &mut History, item: Option<u64>, status: RequestStatus) -> HistoryEntry {
        history.add(
            Subject::bare(CommandSource::Youtube, "aaaaaaaaaaa"),
            &viewer(),
            item.map(ItemId),
            status,
            String::new(),
        )
    }

    #[test]
    fn a_request_is_followed_through_its_statuses() {
        let mut history = History::default();
        let entry = add(&mut history, Some(7), RequestStatus::Waiting);
        assert_eq!(entry.id, 1);
        let played = history
            .set_status(ItemId(7), RequestStatus::Played)
            .unwrap();
        assert_eq!((played.id, played.status), (1, RequestStatus::Played));
        // Nothing to publish when nothing changes, or when it is unknown.
        assert!(
            history
                .set_status(ItemId(7), RequestStatus::Played)
                .is_none()
        );
        assert!(
            history
                .set_status(ItemId(8), RequestStatus::Played)
                .is_none()
        );
    }

    #[test]
    fn what_was_open_at_the_end_of_a_run_is_dropped_and_numbering_goes_on() {
        let mut history = History::default();
        add(&mut history, Some(1), RequestStatus::Waiting);
        add(&mut history, None, RequestStatus::Refused);
        let json = serde_json::to_string(&history.entries()).unwrap();

        let mut restored = History::restore(serde_json::from_str(&json).unwrap());
        let statuses: Vec<_> = restored.entries().iter().map(|e| e.status).collect();
        assert_eq!(statuses, [RequestStatus::Dropped, RequestStatus::Refused]);
        // An old queue number must not touch the restored entries.
        assert!(
            restored
                .set_status(ItemId(1), RequestStatus::Played)
                .is_none()
        );
        assert_eq!(add(&mut restored, None, RequestStatus::Refused).id, 3);
    }

    #[test]
    fn the_oldest_entries_are_forgotten() {
        let mut history = History::default();
        for _ in 0..HISTORY_LIMIT + 3 {
            add(&mut history, None, RequestStatus::Refused);
        }
        let entries = history.entries();
        assert_eq!(entries.len(), HISTORY_LIMIT);
        assert_eq!(entries[0].id, 4);
    }
}
