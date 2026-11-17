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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Id,
    En,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    pub language: Language,
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
    /// Minimum role allowed to submit videos.
    pub min_role: Role,
    /// Minimum TikTok gifter level; 0 disables the check.
    pub min_gifter_level: u32,
    /// Minimum TikTok fan club level; 0 disables the check.
    pub min_fan_club_level: u32,
    /// Seconds a user must wait between submissions; 0 disables it.
    pub cooldown_secs: u64,
    pub max_duration_secs: u64,
    pub max_queue_len: usize,
    /// Initial volume, 0-100.
    pub volume: u8,
    /// Upper bound for the volume, 0-100.
    pub max_volume: u8,
}

impl ModerationConfig {
    /// Per-user cooldown as a duration; zero means no cooldown.
    pub fn cooldown(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.cooldown_secs)
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
            min_role: Role::Follower,
            min_gifter_level: 0,
            min_fan_club_level: 0,
            cooldown_secs: 60,
            max_duration_secs: 10 * 60,
            max_queue_len: 20,
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
    /// Words that must not appear in a video title.
    pub title_words: Vec<String>,
}

/// Most chat messages the server will remember for the dashboard.
pub const MAX_CHAT_HISTORY: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    /// How many recent chat messages the server remembers and shows to a
    /// dashboard that has just opened or reloaded; 0 remembers none. At most
    /// `MAX_CHAT_HISTORY`.
    pub chat_history: usize,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self { chat_history: 20 }
    }
}
