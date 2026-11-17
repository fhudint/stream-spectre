use std::fmt;
use std::time::SystemTime;

use crate::chat::Sender;
use crate::config::CommandSource;
use crate::tiktok::TiktokVideoId;
use crate::youtube::VideoDetails;

/// Identifier of a queue entry, unique for the lifetime of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What is known about a TikTok video. Very little: TikTok gives no title,
/// no length and no thumbnail without an account on its developer platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiktokDetails {
    pub id: TiktokVideoId,
    /// The account that posted it, when the link told. Only letters, digits,
    /// `_` and `.`.
    pub account: Option<String>,
}

/// The video behind a queue entry, from either source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Media {
    Youtube(VideoDetails),
    Tiktok(TiktokDetails),
}

impl From<VideoDetails> for Media {
    fn from(video: VideoDetails) -> Self {
        Self::Youtube(video)
    }
}

impl Media {
    pub fn source(&self) -> CommandSource {
        match self {
            Self::Youtube(_) => CommandSource::Youtube,
            Self::Tiktok(_) => CommandSource::Tiktok,
        }
    }

    /// The ID in the source's own form; safe to put in a URL.
    pub fn video_id(&self) -> String {
        match self {
            Self::Youtube(video) => video.id.to_string(),
            Self::Tiktok(video) => video.id.to_string(),
        }
    }

    /// What to show as the title. For TikTok that is the account, or the ID
    /// when the link did not say.
    pub fn title(&self) -> String {
        match self {
            Self::Youtube(video) => video.title.clone(),
            Self::Tiktok(video) => match &video.account {
                Some(account) => format!("@{account}"),
                None => video.id.to_string(),
            },
        }
    }

    pub fn channel(&self) -> String {
        match self {
            Self::Youtube(video) => video.channel.clone(),
            Self::Tiktok(_) => "TikTok".to_owned(),
        }
    }

    /// The length, when it is known.
    pub fn duration_secs(&self) -> Option<u64> {
        match self {
            Self::Youtube(video) => Some(video.duration_secs),
            Self::Tiktok(_) => None,
        }
    }

    /// Whether automatic approval may let this through. A TikTok video has
    /// had no length, age or embedding check, so a person always looks first.
    pub fn can_auto_approve(&self) -> bool {
        matches!(self, Self::Youtube(_))
    }
}

/// A video request waiting for approval, waiting to play, or playing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub id: ItemId,
    pub media: Media,
    pub sender: Sender,
    pub requested_at: SystemTime,
    /// What the viewer wrote after the link: plain text, possibly empty.
    pub message: String,
}
