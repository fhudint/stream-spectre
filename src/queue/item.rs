use std::fmt;
use std::time::SystemTime;

use crate::chat::Sender;
use crate::youtube::VideoDetails;

/// Identifier of a queue entry, unique for the lifetime of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A video request waiting for approval, waiting to play, or playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub id: ItemId,
    pub video: VideoDetails,
    pub sender: Sender,
    pub requested_at: SystemTime,
}
