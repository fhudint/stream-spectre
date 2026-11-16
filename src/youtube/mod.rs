//! YouTube-specific logic: video IDs and links.
//!
//! The Data API client and result cache are added in a later step.

mod link;
mod video_id;

pub use link::extract_video_id;
pub use video_id::{InvalidVideoId, VIDEO_ID_LEN, VideoId};
