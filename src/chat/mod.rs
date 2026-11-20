//! Chat sources and the types shared by all of them.
//!
//! `ChatSource` is the contract the rest of the app depends on. TikTok
//! (`tiktok`) and YouTube live chat (`youtube`) are the implementations;
//! `connection` holds the platform-independent reconnect logic they share.

pub mod connection;
pub mod developer;
pub mod role;
pub mod sender;
pub mod source;
pub mod tiktok;
pub mod youtube;

pub use connection::{
    ChatConnector, ChatFeed, ConnectError, ConnectFailure, FeedItem, ReconnectingSource,
};
pub use role::Role;
pub use sender::Sender;
pub use source::{ChatEvent, ChatMessage, ChatSource, ChatStatus, GiftEvent};
pub use tiktok::{TiktokSource, tiktok_source};
pub use youtube::{YoutubeSource, youtube_source};
