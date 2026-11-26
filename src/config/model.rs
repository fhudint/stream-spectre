use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};

use crate::chat::Role;
use crate::youtube::{VideoId, extract_video_id};

/// Contents of `config.toml`. Secrets are not part of it; see `Secrets`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: GeneralConfig,
    pub server: ServerConfig,
    pub chat: ChatConfig,
    pub command: CommandConfig,
    pub youtube: YoutubeConfig,
    pub moderation: ModerationConfig,
    pub blacklist: BlacklistConfig,
    pub dashboard: DashboardConfig,
    pub overlay: OverlayConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Id,
    En,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    pub language: Language,
    /// Look on GitHub for a newer version when the program starts and then once
    /// a day, and say so in the dashboard. Nothing is installed by itself.
    pub check_updates: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            language: Language::default(),
            check_updates: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Address the dashboard and overlay bind to. Loopback by default; any
    /// other address requires `DASHBOARD_TOKEN`.
    pub bind: IpAddr,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 8787,
        }
    }
}

/// The places chat is read from. Both can be read at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatSourceKind {
    Tiktok,
    Youtube,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatConfig {
    /// Not used any more: both sources are read, each unless it is switched
    /// off or has nothing to read. Still accepted so that config files that
    /// have it keep loading.
    pub source: Option<ChatSourceKind>,
    pub tiktok: TiktokConfig,
    pub youtube: YoutubeChatConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TiktokConfig {
    /// Username of the streamer whose LIVE chat is read, with or without `@`.
    /// Empty means no chat is read yet; the program starts anyway and waits
    /// for a username to be set from the dashboard.
    pub username: String,
    /// Switched off, the chat is not read even with a username set.
    pub enabled: bool,
}

impl Default for TiktokConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            enabled: true,
        }
    }
}

impl TiktokConfig {
    /// Username with surrounding whitespace and a leading `@` removed.
    pub fn username(&self) -> &str {
        self.username.trim().trim_start_matches('@')
    }
}

/// The YouTube live whose chat is read.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct YoutubeChatConfig {
    /// The live video: its link or its 11-character ID. Empty means no chat
    /// is read; a live gets a new video each time, so this is meant to be set
    /// from the dashboard.
    pub video: String,
    /// Switched off, the chat is not read even with a video set.
    pub enabled: bool,
}

impl Default for YoutubeChatConfig {
    fn default() -> Self {
        Self {
            video: String::new(),
            enabled: true,
        }
    }
}

impl YoutubeChatConfig {
    /// The video ID, when `video` is a link or ID that holds one.
    pub fn video_id(&self) -> Option<VideoId> {
        extract_video_id(self.video.trim())
    }
}

/// Where a video asked for with a command comes from. A command only accepts
/// links and IDs of its own source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandSource {
    Youtube,
    Tiktok,
    /// A song: a YouTube video played by the song overlay, with the rules for
    /// songs. Only YouTube links and IDs.
    Song,
}

/// One chat word and what it asks for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEntry {
    /// The word after the prefix, without it (`v` for `!v`).
    pub name: String,
    pub source: CommandSource,
}

impl CommandEntry {
    pub fn youtube(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: CommandSource::Youtube,
        }
    }

    pub fn tiktok(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: CommandSource::Tiktok,
        }
    }

    pub fn song(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: CommandSource::Song,
        }
    }
}

/// `[command]`: one prefix shared by every command, and the commands.
///
/// `config.toml` may also say `name = "v"` instead of `commands`, which is the
/// same as one YouTube command with that name. It is read, never written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "RawCommandConfig")]
pub struct CommandConfig {
    pub prefix: String,
    pub commands: Vec<CommandEntry>,
}

impl Default for CommandConfig {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_PREFIX.to_owned(),
            commands: vec![CommandEntry::youtube(DEFAULT_COMMAND_NAME)],
        }
    }
}

const DEFAULT_PREFIX: &str = "!";
const DEFAULT_COMMAND_NAME: &str = "v";

/// `[command]` as it is written in a file.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawCommandConfig {
    prefix: Option<String>,
    name: Option<String>,
    commands: Option<Vec<CommandEntry>>,
}

impl TryFrom<RawCommandConfig> for CommandConfig {
    type Error = String;

    fn try_from(raw: RawCommandConfig) -> Result<Self, Self::Error> {
        let defaults = Self::default();
        let commands = match (raw.name, raw.commands) {
            (Some(_), Some(_)) => {
                return Err("set either `name` or `commands`, not both".to_owned());
            }
            (Some(name), None) => vec![CommandEntry::youtube(name)],
            (None, Some(commands)) => commands,
            (None, None) => defaults.commands,
        };
        Ok(Self {
            prefix: raw.prefix.unwrap_or(defaults.prefix),
            commands,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct YoutubeConfig {
    pub cache_ttl_secs: u64,
    pub cache_max_entries: usize,
    pub request_timeout_secs: u64,
}

impl Default for YoutubeConfig {
    fn default() -> Self {
        Self {
            cache_ttl_secs: 6 * 60 * 60,
            cache_max_entries: 1000,
            request_timeout_secs: 10,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModerationMode {
    /// Every request waits for approval in the dashboard.
    #[default]
    Manual,
    /// Requests that pass validation are approved automatically.
    Auto,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModerationConfig {
    pub mode: ModerationMode,
    /// Minimum role allowed to submit videos from the TikTok chat. (`min_role`
    /// was its name when there was only TikTok.)
    #[serde(alias = "min_role")]
    pub tiktok_min_role: Role,
    /// The same for the YouTube chat.
    pub youtube_min_role: YoutubeRole,
    /// A Super Chat or Super Sticker sender gets in whatever their role is.
    pub youtube_superchat_bypass: bool,
    /// Minimum TikTok gifter level; 0 disables the check.
    pub min_gifter_level: u32,
    /// Minimum TikTok fan club level; 0 disables the check.
    pub min_fan_club_level: u32,
    /// Seconds a user must wait between submissions; 0 disables it. Applies to
    /// both chats unless one has its own below.
    pub cooldown_secs: u64,
    /// The cooldown of the YouTube chat, when it is not `cooldown_secs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub youtube_cooldown_secs: Option<u64>,
    /// The cooldown of the TikTok chat, when it is not `cooldown_secs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiktok_cooldown_secs: Option<u64>,
    pub max_duration_secs: u64,
    pub max_queue_len: usize,
    /// Most requests of the YouTube chat that may be waiting or queued at
    /// once, so that one chat cannot fill the queue; 0 is no limit.
    pub youtube_max_waiting: usize,
    /// The same for the TikTok chat.
    pub tiktok_max_waiting: usize,
    /// Requests from this YouTube role and above are approved at once, even
    /// when `mode` is manual; none when not set. (There is no such setting for
    /// TikTok: a TikTok video has had no length, age or embedding check, so a
    /// person always looks first.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub youtube_auto_approve: Option<YoutubeRole>,
    /// Live streams and scheduled premieres may be requested.
    pub allow_live: bool,
    /// Videos requested with a `youtube.com/shorts/` link may be.
    pub allow_shorts: bool,
    /// Only videos of these categories may be requested; all of them when
    /// empty.
    pub allowed_categories: Vec<VideoCategory>,
    /// The longest song that may be requested, in seconds.
    pub song_max_duration_secs: u64,
    /// Only songs of these categories may be requested; all of them when
    /// empty. Many songs are filed under Entertainment, not only Music.
    pub song_allowed_categories: Vec<VideoCategory>,
    /// Initial volume, 0-100.
    pub volume: u8,
    /// Upper bound for the volume, 0-100.
    pub max_volume: u8,
}

/// The roles of a YouTube chat, which has no followers: everyone is a viewer,
/// some pay as members of the channel, some moderate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum YoutubeRole {
    /// (`everyone` and `follower` were the names before this one.)
    #[default]
    #[serde(alias = "everyone", alias = "follower")]
    Viewer,
    #[serde(alias = "subscriber")]
    Member,
    Moderator,
}

impl YoutubeRole {
    /// The same standing on the scale every chat is mapped to.
    pub fn role(self) -> Role {
        match self {
            Self::Viewer => Role::Follower,
            Self::Member => Role::Subscriber,
            Self::Moderator => Role::Moderator,
        }
    }
}

/// The categories YouTube sorts videos into (`snippet.categoryId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCategory {
    Film,
    Autos,
    Music,
    Pets,
    Sports,
    Travel,
    Gaming,
    People,
    Comedy,
    Entertainment,
    News,
    Howto,
    Education,
    Science,
    Nonprofits,
}

impl VideoCategory {
    pub const ALL: [Self; 15] = [
        Self::Film,
        Self::Autos,
        Self::Music,
        Self::Pets,
        Self::Sports,
        Self::Travel,
        Self::Gaming,
        Self::People,
        Self::Comedy,
        Self::Entertainment,
        Self::News,
        Self::Howto,
        Self::Education,
        Self::Science,
        Self::Nonprofits,
    ];

    /// The number YouTube gives the category.
    pub fn id(self) -> u32 {
        match self {
            Self::Film => 1,
            Self::Autos => 2,
            Self::Music => 10,
            Self::Pets => 15,
            Self::Sports => 17,
            Self::Travel => 19,
            Self::Gaming => 20,
            Self::People => 22,
            Self::Comedy => 23,
            Self::Entertainment => 24,
            Self::News => 25,
            Self::Howto => 26,
            Self::Education => 27,
            Self::Science => 28,
            Self::Nonprofits => 29,
        }
    }
}

impl ModerationConfig {
    /// Per-user cooldown of the chat `source`, as a duration; zero means no
    /// cooldown.
    pub fn cooldown_for(&self, source: ChatSourceKind) -> std::time::Duration {
        let own = match source {
            ChatSourceKind::Youtube => self.youtube_cooldown_secs,
            ChatSourceKind::Tiktok => self.tiktok_cooldown_secs,
        };
        std::time::Duration::from_secs(own.unwrap_or(self.cooldown_secs))
    }

    /// Most requests of `source` that may be waiting or queued; 0 is no limit.
    pub fn max_waiting_for(&self, source: ChatSourceKind) -> usize {
        match source {
            ChatSourceKind::Youtube => self.youtube_max_waiting,
            ChatSourceKind::Tiktok => self.tiktok_max_waiting,
        }
    }

    /// Whether a sender of `source` with `role` is approved without waiting:
    /// the whole chat is in `auto` mode, or their role is one that is.
    pub fn approves_at_once(&self, source: ChatSourceKind, role: Role) -> bool {
        if self.mode == ModerationMode::Auto {
            return true;
        }
        match source {
            ChatSourceKind::Youtube => self
                .youtube_auto_approve
                .is_some_and(|min| role >= min.role()),
            ChatSourceKind::Tiktok => false,
        }
    }

    /// Limits a requested volume to `max_volume`.
    pub fn clamp_volume(&self, requested: u8) -> u8 {
        requested.min(self.max_volume)
    }
}

impl Default for ModerationConfig {
    fn default() -> Self {
        Self {
            mode: ModerationMode::Manual,
            tiktok_min_role: Role::Follower,
            youtube_min_role: YoutubeRole::Viewer,
            youtube_superchat_bypass: false,
            min_gifter_level: 0,
            min_fan_club_level: 0,
            cooldown_secs: 60,
            youtube_cooldown_secs: None,
            tiktok_cooldown_secs: None,
            max_duration_secs: 10 * 60,
            max_queue_len: 20,
            youtube_max_waiting: 0,
            tiktok_max_waiting: 0,
            youtube_auto_approve: None,
            allow_live: false,
            allow_shorts: true,
            allowed_categories: Vec::new(),
            song_max_duration_secs: 7 * 60,
            song_allowed_categories: vec![VideoCategory::Music, VideoCategory::Entertainment],
            volume: 50,
            max_volume: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlacklistConfig {
    /// YouTube video IDs.
    pub video_ids: Vec<String>,
    /// YouTube channel names.
    pub channels: Vec<String>,
    /// Words that must not appear in a video title or in a viewer's message.
    pub title_words: Vec<String>,
    /// Viewers who may not request anything: a name as it is shown, or the ID
    /// of the account (`youtube:UC...`).
    pub senders: Vec<String>,
}

/// Most chat messages the server will remember for the dashboard.
pub const MAX_CHAT_HISTORY: usize = 200;
/// The longest name shown next to the avatar, in characters.
pub const MAX_DISPLAY_NAME_CHARS: usize = 40;
/// The longest avatar picture, as the `data:` address the dashboard sends. The
/// dashboard shrinks a photo to a small square, so this is far more than needed.
pub const MAX_AVATAR_LEN: usize = 48 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    /// How many recent chat messages the server remembers and shows to a
    /// dashboard that has just opened or reloaded; 0 remembers none. At most
    /// `MAX_CHAT_HISTORY`.
    pub chat_history: usize,
    /// The name shown next to the avatar in the dashboard; empty shows
    /// "Account". It is not read from any chat.
    pub display_name: String,
    /// The avatar picture as a `data:image/(jpeg|png|webp);base64,` address;
    /// empty shows the first letter of the name. Only the dashboard gets it.
    pub avatar: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            chat_history: 20,
            display_name: String::new(),
            avatar: String::new(),
        }
    }
}

/// How the mediashare overlay looks. Each theme is a set of styles in its page
/// (`static/overlay/mediashare/overlay.css`); the names there must match these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediashareTheme {
    #[default]
    Obsidian,
    #[serde(alias = "kertas")]
    Paper,
    Brutal,
    Terminal,
    #[serde(alias = "permen")]
    Candy,
    Pixel,
    #[serde(alias = "komik")]
    Comic,
    #[serde(alias = "garis")]
    Outline,
    #[serde(alias = "struk")]
    Receipt,
    #[serde(alias = "pesan")]
    Messages,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediashareConfig {
    pub theme: MediashareTheme,
    /// Let the author of the program show a short notice on this overlay with
    /// `@dev text` in the chat (see `chat::developer`).
    pub developer_notice: bool,
}

impl Default for MediashareConfig {
    fn default() -> Self {
        Self {
            theme: MediashareTheme::default(),
            developer_notice: true,
        }
    }
}

/// How the chat overlay with a background looks (`/overlay/chat/panel/`). Each
/// look is a block of styles in `static/overlay/chat/chat.css`; the names there
/// must match these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatPanelTheme {
    #[default]
    Obsidian,
    #[serde(alias = "kertas")]
    Paper,
    Brutal,
    Terminal,
    #[serde(alias = "permen")]
    Candy,
    Pixel,
    #[serde(alias = "struk")]
    Receipt,
    #[serde(alias = "pesan")]
    Messages,
    Ticker,
}

/// How the transparent chat overlay looks (`/overlay/chat/float/`): the lines
/// float over the scene, with no panel behind them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatFloatTheme {
    #[default]
    #[serde(alias = "garis")]
    Outline,
    #[serde(alias = "komik")]
    Comic,
    #[serde(alias = "catatan")]
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatPanelConfig {
    pub theme: ChatPanelTheme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ChatFloatConfig {
    pub theme: ChatFloatTheme,
}

/// The overlays, one table each (`[overlay.mediashare]`, `[overlay.chat_panel]`,
/// `[overlay.chat_float]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct OverlayConfig {
    pub mediashare: MediashareConfig,
    pub chat_panel: ChatPanelConfig,
    pub chat_float: ChatFloatConfig,
}
