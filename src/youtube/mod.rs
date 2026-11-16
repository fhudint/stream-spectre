//! YouTube-specific logic: video IDs and links, the Data API client, cached
//! lookups, and the rules a video must pass.

mod client;
mod duration;
mod error;
mod link;
mod model;
mod policy;
mod response;
mod service;
mod video_id;

pub use client::{VideoSource, YoutubeClient};
pub use duration::{format_duration, parse_iso8601_duration};
pub use error::YoutubeError;
pub use link::extract_video_id;
pub use model::{LiveStatus, VideoDetails};
pub use policy::{Rejection, VideoPolicy, check as check_video};
pub use service::{NEGATIVE_TTL, QUOTA_BACKOFF, RATE_LIMIT_BACKOFF, VideoLookup};
pub use video_id::{InvalidVideoId, VIDEO_ID_LEN, VideoId};
