//! Chat sources and the types shared by all of them.
//!
//! The `ChatSource` trait and the TikTok adapter arrive in a later step.

pub mod role;
pub mod sender;

pub use role::Role;
pub use sender::Sender;
