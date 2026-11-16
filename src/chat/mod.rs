//! Chat sources and the types shared by all of them.
//!
//! `ChatSource` is the contract the rest of the app depends on. TikTok is the
//! only implementation for now (`tiktok`); `connection` holds the
//! platform-independent reconnect logic that other sources can reuse.

pub mod connection;
pub mod role;
pub mod sender;
pub mod source;
pub mod tiktok;

pub use connection::{
    ChatConnector, ChatFeed, ConnectError, ConnectFailure, FeedItem, ReconnectingSource,
};
pub use role::Role;
pub use sender::Sender;
pub use source::{ChatEvent, ChatMessage, ChatSource, ChatStatus};
pub use tiktok::{TiktokSource, tiktok_source};
