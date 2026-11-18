//! Settings changed in the dashboard, kept across restarts.
//!
//! `config.toml` is written by hand and has comments, so the program never
//! writes to it. Changes made in the dashboard go to a second file next to it
//! (`config.settings.toml` for `config.toml`) that the program owns. When the
//! program starts, a table in that file replaces the same table from
//! `config.toml`.
//!
//! Only whole tables are stored, and only the ones that differ from
//! `config.toml`. A table that goes back to the value in `config.toml` leaves
//! the file, so editing `config.toml` by hand keeps working for every setting
//! that was never changed in the dashboard.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::ConfigError;
use super::model::{
    BlacklistConfig, CommandConfig, Config, DashboardConfig, ModerationConfig, OverlayConfig,
    TiktokConfig, YoutubeChatConfig,
};

/// Appended to the config file's name without its extension.
const FILE_SUFFIX: &str = ".settings.toml";

const HEADER: &str = "\
# Written by stream-spectre when settings are changed in the dashboard.
# A table here replaces the same table in config.toml. To go back to
# config.toml, delete the table (or this whole file) while the program is
# stopped.
";

/// The `[chat]` tables that can be changed in the dashboard.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavedChat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiktok: Option<TiktokConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube: Option<YoutubeChatConfig>,
}

/// The tables that were changed in the dashboard.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavedSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation: Option<ModerationConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blacklist: Option<BlacklistConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<DashboardConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<OverlayConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<SavedChat>,
}

impl SavedSettings {
    /// The tables that differ between `baseline` (what `config.toml` says)
    /// and the settings in effect now.
    pub fn changes(baseline: &Config, current: &Config) -> Self {
        let chat = SavedChat {
            tiktok: (baseline.chat.tiktok != current.chat.tiktok)
                .then(|| current.chat.tiktok.clone()),
            youtube: (baseline.chat.youtube != current.chat.youtube)
                .then(|| current.chat.youtube.clone()),
        };
        Self {
            moderation: (baseline.moderation != current.moderation)
                .then(|| current.moderation.clone()),
            blacklist: (baseline.blacklist != current.blacklist).then(|| current.blacklist.clone()),
            dashboard: (baseline.dashboard != current.dashboard).then(|| current.dashboard.clone()),
            overlay: (baseline.overlay != current.overlay).then_some(current.overlay),
            command: (baseline.command != current.command).then(|| current.command.clone()),
            chat: (chat != SavedChat::default()).then_some(chat),
        }
    }

    /// Names of the tables present, for a log line.
    pub fn tables(&self) -> Vec<&'static str> {
        [
            ("moderation", self.moderation.is_some()),
            ("blacklist", self.blacklist.is_some()),
            ("dashboard", self.dashboard.is_some()),
            ("overlay", self.overlay.is_some()),
            ("command", self.command.is_some()),
            ("chat.tiktok", self.tiktok().is_some()),
            ("chat.youtube", self.youtube().is_some()),
        ]
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect()
    }

    /// Replaces the matching tables of `config`. The result still has to be
    /// validated.
    pub fn apply_to(&self, config: &mut Config) {
        if let Some(moderation) = &self.moderation {
            config.moderation = moderation.clone();
        }
        if let Some(blacklist) = &self.blacklist {
            config.blacklist = blacklist.clone();
        }
        if let Some(dashboard) = &self.dashboard {
            config.dashboard = dashboard.clone();
        }
        if let Some(overlay) = self.overlay {
            config.overlay = overlay;
        }
        if let Some(command) = &self.command {
            config.command = command.clone();
        }
        if let Some(tiktok) = self.tiktok() {
            config.chat.tiktok = tiktok.clone();
        }
        if let Some(youtube) = self.youtube() {
            config.chat.youtube = youtube.clone();
        }
    }

    /// The saved `[chat.tiktok]` table, if there is one.
    pub fn tiktok(&self) -> Option<&TiktokConfig> {
        self.chat.as_ref().and_then(|chat| chat.tiktok.as_ref())
    }

    /// The saved `[chat.youtube]` table, if there is one.
    pub fn youtube(&self) -> Option<&YoutubeChatConfig> {
        self.chat.as_ref().and_then(|chat| chat.youtube.as_ref())
    }
}

/// Where the saved settings for `config_path` live: next to it, named after
/// it, so two config files in one folder do not share one saved file.
pub fn saved_path(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_stem()
        .map_or_else(|| OsString::from("config"), OsString::from);
    name.push(FILE_SUFFIX);
    config_path.with_file_name(name)
}

/// Reads the saved settings. A missing file is not an error.
pub fn read(path: &Path) -> Result<Option<SavedSettings>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    parse(&text, path).map(Some)
}

fn parse(text: &str, path: &Path) -> Result<SavedSettings, ConfigError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    toml::from_str(text).map_err(|err| ConfigError::Parse {
        path: path.to_owned(),
        location: err
            .span()
            .and_then(|span| super::line_col(text, span.start))
            .map(|(line, col)| format!(":{line}:{col}"))
            .unwrap_or_default(),
        message: err.message().to_owned(),
    })
}

/// Writes the file in one step (see `write_atomically`).
pub fn write(path: &Path, saved: &SavedSettings) -> std::io::Result<()> {
    let body = toml::to_string_pretty(saved).map_err(std::io::Error::other)?;
    let mut text = HEADER.to_owned();
    if !body.is_empty() {
        text.push('\n');
        text.push_str(&body);
    }
    write_atomically(path, text.as_bytes())
}

/// Writes `contents` to `path` in one step: the bytes go to a temporary file
/// next to it first, so a crash or a full disk never leaves a half-written
/// file behind.
pub fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);

    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        // Best effort: the temporary file is ours and useless now.
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::chat::Role;
    use crate::config::{CommandEntry, MediashareConfig, MediashareTheme, ModerationMode};

    /// A new empty folder under the system temp folder.
    pub(crate) fn temp_dir(label: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "stream-spectre-test-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn changed_moderation() -> ModerationConfig {
        ModerationConfig {
            mode: ModerationMode::Auto,
            min_role: Role::Moderator,
            cooldown_secs: 5,
            volume: 33,
            ..ModerationConfig::default()
        }
    }

    fn changed_blacklist() -> BlacklistConfig {
        BlacklistConfig {
            video_ids: vec!["dQw4w9WgXcQ".to_owned()],
            channels: vec!["Some \"Channel\"".to_owned()],
            title_words: vec!["spoiler".to_owned(), "naïve é".to_owned()],
        }
    }

    fn changed_commands() -> CommandConfig {
        CommandConfig {
            prefix: "?".to_owned(),
            commands: vec![CommandEntry::youtube("yt"), CommandEntry::youtube("v")],
        }
    }

    #[test]
    fn the_file_sits_next_to_the_config_and_is_named_after_it() {
        assert_eq!(
            saved_path(Path::new("/home/a/config.toml")),
            Path::new("/home/a/config.settings.toml")
        );
        assert_eq!(
            saved_path(Path::new("config.toml")),
            Path::new("config.settings.toml")
        );
        assert_eq!(
            saved_path(Path::new("/x/streamer-b.toml")),
            Path::new("/x/streamer-b.settings.toml")
        );
        // Two configs in one folder must not share a saved file.
        assert_ne!(
            saved_path(Path::new("/x/a.toml")),
            saved_path(Path::new("/x/b.toml"))
        );
    }

    #[test]
    fn only_tables_that_differ_from_the_config_are_kept() {
        let base = Config::default();
        let none = SavedSettings::changes(&base, &base);
        assert_eq!(none, SavedSettings::default());
        assert!(none.tables().is_empty());

        let some = SavedSettings::changes(&base, &{
            let mut config = base.clone();
            config.moderation = changed_moderation();
            config.dashboard = DashboardConfig {
                chat_history: 99,
                ..DashboardConfig::default()
            };
            config
        });
        assert_eq!(some.moderation, Some(changed_moderation()));
        assert_eq!(some.blacklist, None);
        assert_eq!(
            some.dashboard,
            Some(DashboardConfig {
                chat_history: 99,
                ..DashboardConfig::default()
            })
        );
        assert_eq!(some.tables(), vec!["moderation", "dashboard"]);
    }

    #[test]
    fn the_chat_commands_are_saved_as_one_table_and_replace_the_whole_table() {
        let base = Config::default();
        let saved = SavedSettings::changes(&base, &{
            let mut config = base.clone();
            config.command = changed_commands();
            config
        });
        assert_eq!(saved.tables(), vec!["command"]);
        let text = toml::to_string_pretty(&saved).unwrap();
        assert!(text.contains("[command]"), "{text}");
        assert!(text.contains("prefix = \"?\""), "{text}");
        assert!(text.contains("name = \"yt\""), "{text}");
        assert!(text.contains("source = \"youtube\""), "{text}");
        assert!(!text.contains("[moderation]"), "{text}");

        let again: SavedSettings = toml::from_str(&text).unwrap();
        let mut config = base.clone();
        again.apply_to(&mut config);
        assert_eq!(config.command, changed_commands());

        // Back to what config.toml says: nothing to keep.
        let back = SavedSettings::changes(&base, &base);
        assert_eq!(back, SavedSettings::default());
    }

    #[test]
    fn the_tiktok_username_is_saved_as_its_own_chat_table() {
        let mut base = Config::default();
        base.chat.tiktok.username = "from_config".to_owned();
        let changed = TiktokConfig {
            username: "from_dashboard".to_owned(),
            ..TiktokConfig::default()
        };
        let saved = SavedSettings::changes(&base, &{
            let mut config = base.clone();
            config.chat.tiktok = changed.clone();
            config
        });
        assert_eq!(saved.tables(), vec!["chat.tiktok"]);
        assert_eq!(saved.tiktok(), Some(&changed));

        // Changed back to what config.toml says: nothing to keep.
        let back = SavedSettings::changes(&base, &base);
        assert_eq!(back, SavedSettings::default());

        let mut config = base.clone();
        saved.apply_to(&mut config);
        assert_eq!(config.chat.tiktok, changed);
        // The rest of [chat] is not touched.
        assert_eq!(config.chat.youtube, base.chat.youtube);

        let text = toml::to_string_pretty(&saved).unwrap();
        assert!(text.contains("[chat.tiktok]"), "{text}");
        assert!(text.contains("username = \"from_dashboard\""), "{text}");
        assert!(!text.contains("[moderation]"), "{text}");
    }

    #[test]
    fn the_youtube_live_and_the_switches_are_saved_as_chat_tables() {
        let base = Config::default();
        let youtube = YoutubeChatConfig {
            video: "HFSuEKR1b0w".to_owned(),
            enabled: true,
        };
        let tiktok_off = TiktokConfig {
            enabled: false,
            ..TiktokConfig::default()
        };
        let saved = SavedSettings::changes(&base, &{
            let mut config = base.clone();
            config.chat.tiktok = tiktok_off.clone();
            config.chat.youtube = youtube.clone();
            config
        });
        assert_eq!(saved.tables(), vec!["chat.tiktok", "chat.youtube"]);
        let mut config = base.clone();
        saved.apply_to(&mut config);
        assert_eq!(config.chat.tiktok, tiktok_off);
        assert_eq!(config.chat.youtube, youtube);

        // Only what differs is kept: here just the YouTube table.
        let only_youtube = SavedSettings::changes(&base, &{
            let mut config = base.clone();
            config.chat.youtube = youtube.clone();
            config
        });
        assert_eq!(only_youtube.tables(), vec!["chat.youtube"]);
        let text = toml::to_string_pretty(&only_youtube).unwrap();
        assert!(
            text.contains("[chat.youtube]") && !text.contains("[chat.tiktok]"),
            "{text}"
        );
        // What an older version saved (no `enabled`) still reads, as switched on.
        let old: SavedSettings = toml::from_str("[chat.tiktok]\nusername = \"x\"\n").unwrap();
        assert!(old.tiktok().unwrap().enabled);
    }

    #[test]
    fn an_emptied_username_is_a_saved_change_too() {
        let mut base = Config::default();
        base.chat.tiktok.username = "from_config".to_owned();
        let empty = TiktokConfig::default();
        let saved = SavedSettings::changes(&base, &{
            let mut config = base.clone();
            config.chat.tiktok = empty.clone();
            config
        });
        assert_eq!(saved.tiktok(), Some(&empty));
        let mut config = base;
        saved.apply_to(&mut config);
        assert_eq!(config.chat.tiktok.username, "");
    }

    #[test]
    fn saved_tables_replace_the_matching_ones_and_nothing_else() {
        let mut config = Config::default();
        config.dashboard.chat_history = 7;
        let saved = SavedSettings {
            moderation: Some(changed_moderation()),
            blacklist: Some(changed_blacklist()),
            dashboard: None,
            overlay: None,
            command: None,
            chat: None,
        };
        saved.apply_to(&mut config);
        assert_eq!(config.moderation, changed_moderation());
        assert_eq!(config.blacklist, changed_blacklist());
        // Not in the saved file, so config.toml's value stays.
        assert_eq!(config.dashboard.chat_history, 7);
        assert_eq!(config.command, Config::default().command);
        // Other tables are never touched.
        assert_eq!(config.youtube, Config::default().youtube);
    }

    #[test]
    fn what_is_written_is_read_back_unchanged() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("config.settings.toml");
        let saved = SavedSettings {
            moderation: Some(changed_moderation()),
            blacklist: Some(changed_blacklist()),
            dashboard: Some(DashboardConfig {
                chat_history: 0,
                ..DashboardConfig::default()
            }),
            overlay: Some(OverlayConfig {
                mediashare: MediashareConfig {
                    theme: MediashareTheme::Struk,
                },
                ..OverlayConfig::default()
            }),
            command: Some(changed_commands()),
            chat: Some(SavedChat {
                tiktok: Some(TiktokConfig {
                    username: "someone".to_owned(),
                    enabled: false,
                }),
                youtube: Some(YoutubeChatConfig {
                    video: "HFSuEKR1b0w".to_owned(),
                    enabled: true,
                }),
            }),
        };
        write(&path, &saved).unwrap();
        assert_eq!(read(&path).unwrap(), Some(saved));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Written by stream-spectre"));
    }

    #[test]
    fn an_empty_set_is_written_as_a_comment_only_file() {
        let dir = temp_dir("empty");
        let path = dir.join("config.settings.toml");
        write(&path, &SavedSettings::default()).unwrap();
        assert_eq!(read(&path).unwrap(), Some(SavedSettings::default()));
    }

    #[test]
    fn writing_replaces_the_old_file_and_leaves_no_temporary_one() {
        let dir = temp_dir("replace");
        let path = dir.join("config.settings.toml");
        write(
            &path,
            &SavedSettings {
                dashboard: Some(DashboardConfig {
                    chat_history: 1,
                    ..DashboardConfig::default()
                }),
                ..SavedSettings::default()
            },
        )
        .unwrap();
        write(
            &path,
            &SavedSettings {
                dashboard: Some(DashboardConfig {
                    chat_history: 2,
                    ..DashboardConfig::default()
                }),
                ..SavedSettings::default()
            },
        )
        .unwrap();
        assert_eq!(
            read(&path).unwrap().unwrap().dashboard,
            Some(DashboardConfig {
                chat_history: 2,
                ..DashboardConfig::default()
            })
        );
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["config.settings.toml"]);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let dir = temp_dir("failed");
        let path = dir.join("config.settings.toml");
        let old = SavedSettings {
            dashboard: Some(DashboardConfig {
                chat_history: 3,
                ..DashboardConfig::default()
            }),
            ..SavedSettings::default()
        };
        write(&path, &old).unwrap();

        // A folder where the temporary file should go makes `create` fail.
        std::fs::create_dir(dir.join("config.settings.toml.tmp")).unwrap();
        assert!(write(&path, &SavedSettings::default()).is_err());
        assert_eq!(read(&path).unwrap(), Some(old));
    }

    #[test]
    fn a_missing_file_is_nothing_saved_yet() {
        let dir = temp_dir("missing");
        assert_eq!(read(&dir.join("config.settings.toml")).unwrap(), None);
    }

    #[test]
    fn a_broken_file_is_an_error_that_names_the_file_and_the_position() {
        let dir = temp_dir("broken");
        let path = dir.join("config.settings.toml");
        std::fs::write(&path, "[moderation]\ncooldown_secs = \"soon\"\n").unwrap();
        let message = read(&path).unwrap_err().to_string();
        assert!(message.contains("config.settings.toml"), "{message}");
        assert!(message.contains(":2:"), "{message}");
        // The source line itself is never repeated.
        assert!(!message.contains("cooldown_secs"), "{message}");
    }

    #[test]
    fn unknown_tables_and_fields_are_refused() {
        let dir = temp_dir("unknown");
        let path = dir.join("config.settings.toml");
        std::fs::write(&path, "[server]\nport = 1\n").unwrap();
        assert!(read(&path).is_err());
        std::fs::write(&path, "[dashboard]\nhistory = 1\n").unwrap();
        assert!(read(&path).is_err());
    }
}
