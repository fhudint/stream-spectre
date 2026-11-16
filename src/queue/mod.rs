//! The moderation queue: who may submit, what is accepted, and the order in
//! which approved videos play.
//!
//! Everything here is plain data and rules with no I/O, and time is passed
//! in by the caller, so it is deterministic to test. The pipeline that wires
//! chat, YouTube lookups and this queue together lives elsewhere.

mod blacklist;
mod cooldown;
mod gatekeeper;
mod item;
mod play_queue;

pub use blacklist::{Blacklist, BlacklistHit};
pub use cooldown::Cooldowns;
pub use gatekeeper::{Denial, Gatekeeper};
pub use item::{ItemId, QueueItem};
pub use play_queue::{PlayQueue, QueueError, QueueSnapshot};
