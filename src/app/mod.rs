//! The application core: ties chat, YouTube, moderation and the queue
//! together and publishes what happens as events for the overlay and the
//! dashboard. The HTTP and WebSocket server is a thin layer on top of this.

mod autosave;
mod bus;
mod credentials;
mod events;
mod history;
mod history_store;
mod pipeline;

pub use autosave::{SETTLE, autosave};
pub use bus::EventBus;
pub use credentials::{
    AccountView, Credentials, CredentialsError, KeyCheck, KeyStatus, TokenStatus,
};
pub use events::{
    ChatLineView, ChatSettingsView, ChatState, ChatStatusView, Envelope, Event, Hello, ItemView,
    NoticeLevel, OverlayKind, OverlayMessage, PROTOCOL_VERSION, QueueView, SenderView,
    SettingsView, SkipReason, StatsView, unix_ms,
};
pub use history::{HISTORY_LIMIT, HistoryEntry, RequestStatus};
pub use history_store::{autosave_history, history_path, load_history};
pub use pipeline::{PauseOutcome, Pipeline};
