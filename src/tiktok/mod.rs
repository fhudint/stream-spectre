//! TikTok-specific logic: video IDs, short codes, the links that carry them,
//! and finding the video behind a short link.
//!
//! Nothing here plays a video or touches the queue yet. Like the YouTube side,
//! the parser only ever hands on validated values (`TiktokVideoId`,
//! `ShortCode`); raw chat text is never put in a URL.

mod ids;
mod link;
mod resolver;

pub use ids::{
    InvalidShortCode, InvalidTiktokVideoId, ShortCode, TiktokVideoId, VIDEO_ID_MAX_LEN,
    VIDEO_ID_MIN_LEN,
};
pub use link::{TiktokRef, extract_tiktok_ref};
pub use resolver::{ResolveError, Resolved, ShortLinkResolver};
