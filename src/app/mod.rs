//! The application core: ties chat, YouTube, moderation and the queue
//! together and publishes what happens as events for the overlay and the
//! dashboard. The HTTP and WebSocket server is a thin layer on top of this.

mod bus;
mod events;
mod pipeline;

pub use bus::EventBus;
pub use events::{
    ChatLineView, ChatState, ChatStatusView, Envelope, Event, Hello, ItemView, NoticeLevel,
    OverlayMessage, PROTOCOL_VERSION, QueueView, SenderView, SettingsView, SkipReason, unix_ms,
};
pub use pipeline::Pipeline;
