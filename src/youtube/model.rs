use super::video_id::VideoId;

/// Whether a video is a live broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveStatus {
    /// An ordinary uploaded video.
    None,
    /// Streaming right now.
    Live,
    /// A scheduled stream or premiere that has not started.
    Upcoming,
}

/// What we need to know about a video to decide whether to play it.
///
/// `title` and `channel` come from YouTube and are untrusted text: they have
/// been stripped of control characters and truncated (see
/// `crate::text::sanitize_text`), but they are not HTML-escaped. Show them
/// only as plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoDetails {
    pub id: VideoId,
    pub title: String,
    pub channel: String,
    pub duration_secs: u64,
    pub embeddable: bool,
    pub age_restricted: bool,
    pub live: LiveStatus,
    /// The video is private and cannot be played by others.
    pub private: bool,
    /// The number of its category (`snippet.categoryId`); 0 when YouTube does
    /// not say.
    pub category_id: u32,
}
