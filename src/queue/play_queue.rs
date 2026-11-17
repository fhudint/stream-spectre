use std::collections::VecDeque;
use std::time::SystemTime;

use super::gatekeeper::Denial;
use super::item::{ItemId, Media, QueueItem};
use crate::chat::Sender;
use crate::config::{ModerationConfig, ModerationMode};

/// Why an operation on an existing entry failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    #[error("no queue entry with ID {0}")]
    NotFound(ItemId),
    #[error("entry {0} is not waiting for approval")]
    NotPending(ItemId),
    #[error("entry {0} is not the video that is playing")]
    NotPlaying(ItemId),
}

/// Copy of the queue contents, for events and the dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueSnapshot {
    /// Waiting for a moderator's approval, oldest first.
    pub pending: Vec<QueueItem>,
    /// Approved and waiting to play, in play order.
    pub approved: Vec<QueueItem>,
    pub now_playing: Option<QueueItem>,
}

/// The moderation queue and the play queue, plus what is playing now.
///
/// Passive on purpose: it never starts playback by itself. Callers decide
/// when to call `start_next` and publish the resulting events.
#[derive(Debug, Default)]
pub struct PlayQueue {
    next_id: u64,
    pending: VecDeque<QueueItem>,
    approved: VecDeque<QueueItem>,
    now_playing: Option<QueueItem>,
}

impl PlayQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries waiting for approval or to play. The video that is playing
    /// does not count, so the queue limit is about what is still to come.
    pub fn waiting_len(&self) -> usize {
        self.pending.len() + self.approved.len()
    }

    pub fn now_playing(&self) -> Option<&QueueItem> {
        self.now_playing.as_ref()
    }

    /// Adds a request. In manual mode it waits for approval; in auto mode it
    /// joins the play queue directly, unless the video is one that a person
    /// must always look at first (see `Media::can_auto_approve`). Fails with
    /// `Denial::QueueFull` when `max_queue_len` entries are already waiting.
    pub fn submit(
        &mut self,
        sender: Sender,
        media: impl Into<Media>,
        requested_at: SystemTime,
        settings: &ModerationConfig,
    ) -> Result<ItemId, Denial> {
        self.submit_with_message(sender, media, String::new(), requested_at, settings)
    }

    /// `submit` for a request that comes with the viewer's message.
    pub fn submit_with_message(
        &mut self,
        sender: Sender,
        media: impl Into<Media>,
        message: String,
        requested_at: SystemTime,
        settings: &ModerationConfig,
    ) -> Result<ItemId, Denial> {
        if self.waiting_len() >= settings.max_queue_len {
            return Err(Denial::QueueFull);
        }
        self.next_id += 1;
        let id = ItemId(self.next_id);
        let media = media.into();
        let auto = settings.mode == ModerationMode::Auto && media.can_auto_approve();
        let item = QueueItem {
            id,
            media,
            sender,
            requested_at,
            message,
        };
        if auto {
            self.approved.push_back(item);
        } else {
            self.pending.push_back(item);
        }
        Ok(id)
    }

    /// Moves a pending entry to the end of the play queue.
    pub fn approve(&mut self, id: ItemId) -> Result<(), QueueError> {
        if let Some(position) = self.pending.iter().position(|item| item.id == id) {
            if let Some(item) = self.pending.remove(position) {
                self.approved.push_back(item);
            }
            return Ok(());
        }
        if self.approved.iter().any(|item| item.id == id) {
            return Err(QueueError::NotPending(id));
        }
        Err(QueueError::NotFound(id))
    }

    /// Whether `id` is waiting for approval (not yet approved, not playing).
    pub fn is_pending(&self, id: ItemId) -> bool {
        self.pending.iter().any(|item| item.id == id)
    }

    /// Removes an entry that is pending or approved and returns it.
    /// The video that is playing is stopped with `skip` instead.
    pub fn reject(&mut self, id: ItemId) -> Result<QueueItem, QueueError> {
        for list in [&mut self.pending, &mut self.approved] {
            if let Some(position) = list.iter().position(|item| item.id == id) {
                if let Some(item) = list.remove(position) {
                    return Ok(item);
                }
            }
        }
        Err(QueueError::NotFound(id))
    }

    /// Takes the next approved entry and makes it the playing one. Does
    /// nothing while a video is already playing or the play queue is empty.
    pub fn start_next(&mut self) -> Option<&QueueItem> {
        if self.now_playing.is_some() {
            return None;
        }
        self.now_playing = self.approved.pop_front();
        self.now_playing.as_ref()
    }

    /// The overlay reports that `id` has finished. A stale or unknown ID is
    /// an error and leaves the current video alone.
    pub fn finish(&mut self, id: ItemId) -> Result<QueueItem, QueueError> {
        match self.now_playing.take_if(|item| item.id == id) {
            Some(item) => Ok(item),
            None => Err(QueueError::NotPlaying(id)),
        }
    }

    /// Stops the video that is playing, if any, and returns it.
    pub fn skip(&mut self) -> Option<QueueItem> {
        self.now_playing.take()
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            pending: self.pending.iter().cloned().collect(),
            approved: self.approved.iter().cloned().collect(),
            now_playing: self.now_playing.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;
    use crate::chat::Role;
    use crate::youtube::{LiveStatus, VideoDetails, VideoId};

    fn video(n: u8) -> VideoDetails {
        VideoDetails {
            id: VideoId::parse(&format!("video{n:06}")).unwrap(),
            title: format!("title {n}"),
            channel: "channel".to_owned(),
            duration_secs: 100,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
        }
    }

    fn sender(name: &str) -> Sender {
        Sender::new(format!("id-{name}"), name, Role::Follower)
    }

    fn settings(mode: ModerationMode, max_queue_len: usize) -> ModerationConfig {
        ModerationConfig {
            mode,
            max_queue_len,
            ..ModerationConfig::default()
        }
    }

    fn add(q: &mut PlayQueue, n: u8, s: &ModerationConfig) -> ItemId {
        q.submit(sender("a"), video(n), UNIX_EPOCH, s).unwrap()
    }

    fn ids(items: &[QueueItem]) -> Vec<u64> {
        items.iter().map(|i| i.id.0).collect()
    }

    fn tiktok(n: u8) -> Media {
        Media::Tiktok(crate::queue::TiktokDetails {
            id: crate::tiktok::TiktokVideoId::parse(&format!("77001245411455992{n:02}")).unwrap(),
            account: None,
        })
    }

    #[test]
    fn automatic_approval_does_not_apply_to_tiktok_videos() {
        let s = settings(ModerationMode::Auto, 10);
        let mut q = PlayQueue::new();
        let youtube = add(&mut q, 1, &s);
        let tt = q.submit(sender("a"), tiktok(2), UNIX_EPOCH, &s).unwrap();
        let snap = q.snapshot();
        assert_eq!(ids(&snap.approved), vec![youtube.0]);
        assert_eq!(ids(&snap.pending), vec![tt.0]);

        // A moderator's approval puts it behind the others, as for any request.
        q.approve(tt).unwrap();
        assert_eq!(ids(&q.snapshot().approved), vec![youtube.0, tt.0]);
    }

    #[test]
    fn manual_mode_puts_requests_in_the_approval_queue() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        let b = add(&mut q, 2, &s);
        assert_ne!(a, b);
        let snap = q.snapshot();
        assert_eq!(ids(&snap.pending), vec![a.0, b.0]);
        assert!(snap.approved.is_empty());
        assert!(snap.now_playing.is_none());
        assert_eq!(q.waiting_len(), 2);
    }

    #[test]
    fn auto_mode_goes_straight_to_the_play_queue() {
        let s = settings(ModerationMode::Auto, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        let snap = q.snapshot();
        assert!(snap.pending.is_empty());
        assert_eq!(ids(&snap.approved), vec![a.0]);
    }

    #[test]
    fn ids_increase_and_are_never_reused() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        q.reject(a).unwrap();
        let b = add(&mut q, 2, &s);
        assert!(b > a);
    }

    #[test]
    fn the_queue_limit_counts_pending_and_approved_but_not_the_playing_video() {
        let s = settings(ModerationMode::Manual, 3);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        add(&mut q, 2, &s);
        q.approve(a).unwrap();
        add(&mut q, 3, &s);
        assert_eq!(q.waiting_len(), 3);
        assert_eq!(
            q.submit(sender("x"), video(4), UNIX_EPOCH, &s),
            Err(Denial::QueueFull)
        );

        // Starting playback frees a slot because the playing video no longer waits.
        assert!(q.start_next().is_some());
        assert_eq!(q.waiting_len(), 2);
        q.submit(sender("x"), video(4), UNIX_EPOCH, &s).unwrap();
        assert_eq!(
            q.submit(sender("x"), video(5), UNIX_EPOCH, &s),
            Err(Denial::QueueFull)
        );
    }

    #[test]
    fn a_lowered_limit_does_not_evict_but_blocks_new_requests() {
        let mut q = PlayQueue::new();
        let roomy = settings(ModerationMode::Manual, 10);
        for n in 1..=5 {
            add(&mut q, n, &roomy);
        }
        let tight = settings(ModerationMode::Manual, 2);
        assert_eq!(
            q.submit(sender("x"), video(9), UNIX_EPOCH, &tight),
            Err(Denial::QueueFull)
        );
        assert_eq!(q.waiting_len(), 5);
    }

    #[test]
    fn approve_moves_entries_in_approval_order() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        let b = add(&mut q, 2, &s);
        let c = add(&mut q, 3, &s);
        q.approve(c).unwrap();
        q.approve(a).unwrap();
        let snap = q.snapshot();
        assert_eq!(ids(&snap.pending), vec![b.0]);
        assert_eq!(ids(&snap.approved), vec![c.0, a.0]);
    }

    #[test]
    fn approve_reports_unknown_and_already_approved_entries() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        q.approve(a).unwrap();
        assert_eq!(q.approve(a), Err(QueueError::NotPending(a)));
        assert_eq!(
            q.approve(ItemId(999)),
            Err(QueueError::NotFound(ItemId(999)))
        );
    }

    #[test]
    fn reject_removes_from_pending_or_approved() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        let b = add(&mut q, 2, &s);
        q.approve(b).unwrap();

        assert_eq!(q.reject(a).unwrap().id, a);
        assert_eq!(q.reject(b).unwrap().id, b);
        assert_eq!(q.waiting_len(), 0);
        assert_eq!(q.reject(a), Err(QueueError::NotFound(a)));
    }

    #[test]
    fn reject_does_not_touch_the_playing_video() {
        let s = settings(ModerationMode::Auto, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        q.start_next();
        assert_eq!(q.reject(a), Err(QueueError::NotFound(a)));
        assert_eq!(q.now_playing().map(|i| i.id), Some(a));
    }

    #[test]
    fn start_next_plays_the_oldest_approved_and_only_when_idle() {
        let s = settings(ModerationMode::Auto, 10);
        let mut q = PlayQueue::new();
        assert!(q.start_next().is_none());

        let a = add(&mut q, 1, &s);
        let b = add(&mut q, 2, &s);
        assert_eq!(q.start_next().map(|i| i.id), Some(a));
        // Busy: nothing changes.
        assert!(q.start_next().is_none());
        assert_eq!(q.now_playing().map(|i| i.id), Some(a));
        assert_eq!(ids(&q.snapshot().approved), vec![b.0]);
    }

    #[test]
    fn pending_entries_are_never_played() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        add(&mut q, 1, &s);
        assert!(q.start_next().is_none());
    }

    #[test]
    fn finish_requires_the_matching_id() {
        let s = settings(ModerationMode::Auto, 10);
        let mut q = PlayQueue::new();
        let a = add(&mut q, 1, &s);
        let b = add(&mut q, 2, &s);
        q.start_next();

        assert_eq!(q.finish(b), Err(QueueError::NotPlaying(b)));
        assert_eq!(q.now_playing().map(|i| i.id), Some(a));
        assert_eq!(q.finish(a).unwrap().id, a);
        assert!(q.now_playing().is_none());
        assert_eq!(q.finish(a), Err(QueueError::NotPlaying(a)));

        assert_eq!(q.start_next().map(|i| i.id), Some(b));
    }

    #[test]
    fn skip_stops_the_current_video_and_allows_the_next() {
        let s = settings(ModerationMode::Auto, 10);
        let mut q = PlayQueue::new();
        assert!(q.skip().is_none());
        let a = add(&mut q, 1, &s);
        let b = add(&mut q, 2, &s);
        q.start_next();
        assert_eq!(q.skip().map(|i| i.id), Some(a));
        assert!(q.now_playing().is_none());
        assert_eq!(q.start_next().map(|i| i.id), Some(b));
    }

    #[test]
    fn snapshot_is_a_copy() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        add(&mut q, 1, &s);
        let before = q.snapshot();
        add(&mut q, 2, &s);
        assert_eq!(before.pending.len(), 1);
        assert_eq!(q.snapshot().pending.len(), 2);
    }

    #[test]
    fn items_keep_sender_video_and_time() {
        let s = settings(ModerationMode::Manual, 10);
        let mut q = PlayQueue::new();
        let when = UNIX_EPOCH + std::time::Duration::from_secs(42);
        let id = q.submit(sender("budi"), video(7), when, &s).unwrap();
        let item = q.snapshot().pending.remove(0);
        assert_eq!(item.id, id);
        assert_eq!(item.sender.name, "budi");
        assert_eq!(item.media.title(), "title 7");
        assert_eq!(item.requested_at, when);
    }
}
