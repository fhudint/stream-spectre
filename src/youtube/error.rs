/// Why a YouTube lookup failed.
///
/// Messages never contain the API key or response bodies; only short, fixed
/// descriptions and sanitized reason codes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum YoutubeError {
    #[error("YouTube API quota exhausted")]
    QuotaExceeded,
    #[error("YouTube API rate limit hit")]
    RateLimited,
    #[error("YouTube API key rejected")]
    InvalidApiKey,
    #[error("no YouTube API key is set")]
    NoApiKey,
    #[error("YouTube API access denied ({reason})")]
    AccessDenied { reason: String },
    #[error("YouTube API unavailable (HTTP {status})")]
    Unavailable { status: u16 },
    #[error("YouTube API rejected the request (HTTP {status}, {reason})")]
    Rejected { status: u16, reason: String },
    #[error("network error talking to YouTube ({0})")]
    Network(&'static str),
    #[error("unexpected response from YouTube ({0})")]
    BadResponse(&'static str),
    #[error("YouTube response too large")]
    ResponseTooLarge,
    /// A search by title could not be done: YouTube's own page did not answer
    /// as expected, and the API may not be used (no key, or the day's limit).
    #[error("cannot search for songs now")]
    SearchUnavailable,
}

impl YoutubeError {
    /// Key in the locale catalogs for the message shown to moderators.
    pub fn notice_key(&self) -> &'static str {
        match self {
            Self::QuotaExceeded => "error.quota_exceeded",
            Self::NoApiKey => "error.no_api_key",
            Self::SearchUnavailable => "error.search_unavailable",
            _ => "error.youtube_unavailable",
        }
    }

    /// The streamer has to fix something (key, API enablement); retrying
    /// will not help.
    pub fn is_config_problem(&self) -> bool {
        matches!(
            self,
            Self::InvalidApiKey | Self::NoApiKey | Self::AccessDenied { .. }
        )
    }
}
