//! Configuration: `config.toml` for settings, environment variables or a
//! `.env` file for secrets.

mod dotenv;
mod error;
mod model;
mod paths;
mod saved;
mod secrets;
mod secrets_file;
mod validate;

use std::path::{Path, PathBuf};

pub use error::ConfigError;
pub use model::{
    BlacklistConfig, ChatConfig, ChatSourceKind, CommandConfig, CommandEntry, CommandSource,
    Config, DashboardConfig, GeneralConfig, Language, MAX_CHAT_HISTORY, MediashareConfig,
    MediashareTheme, ModerationConfig, ModerationMode, OverlayConfig, ServerConfig, TiktokConfig,
    YoutubeChatConfig, YoutubeConfig,
};
pub use paths::{CONFIG_FILE_NAME, ENV_CONFIG_PATH};
/// A new empty folder under the system temp folder, for tests.
#[cfg(test)]
pub(crate) use saved::tests::temp_dir as test_temp_dir;
pub use saved::{
    SavedSettings, read as read_saved, saved_path, write as write_saved, write_atomically,
};
pub use secrets::{
    ENV_DASHBOARD_TOKEN, ENV_YOUTUBE_API_KEY, KeySource, MIN_DASHBOARD_TOKEN_LEN, Secret, Secrets,
};
pub use secrets_file::{
    SavedSecrets, is_too_open as secrets_file_is_too_open, read as read_saved_secrets,
    secrets_path, write as write_saved_secrets,
};

use crate::platform;

/// Everything the rest of the app needs from configuration.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub secrets: Secrets,
    /// The config file that was read.
    pub path: PathBuf,
    /// What `config.toml` alone says, before saved settings were applied.
    /// Used to tell which settings were changed in the dashboard.
    pub baseline: Config,
    /// Where settings changed in the dashboard are saved.
    pub saved_path: PathBuf,
    /// Tables of `config.toml` that saved settings replaced at startup.
    pub saved_tables: Vec<&'static str>,
    /// Where secrets entered on the dashboard are saved.
    pub secrets_path: PathBuf,
    /// Where the API key in `secrets` came from.
    pub api_key_source: KeySource,
    /// The API key from the environment or `.env`, whether or not a saved
    /// key replaced it: what is used again when the saved key is removed.
    pub env_api_key: Option<Secret>,
}

#[derive(Debug, Default)]
pub struct LoadOptions {
    /// Path given with `--config`.
    pub explicit_path: Option<PathBuf>,
}

/// Locates, reads and validates the configuration and secrets.
pub fn load(options: LoadOptions) -> Result<Loaded, ConfigError> {
    let user_dir = platform::config_dir();

    let candidates = paths::config_candidates(
        options.explicit_path,
        std::env::var(ENV_CONFIG_PATH).ok(),
        user_dir.as_deref(),
    );
    let path = paths::locate(&candidates, Path::is_file)?;

    let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
        path: path.clone(),
        source,
    })?;
    let baseline = parse(&text, &path)?;
    baseline.validate()?;

    let saved_path = saved::saved_path(&path);
    let (config, saved_tables) = with_saved_settings(&baseline, &saved_path)?;

    let (dotenv_vars, _) = dotenv::load_first(&paths::dotenv_candidates(user_dir.as_deref()))?;
    let secrets = Secrets::from_lookup(|key| {
        std::env::var(key)
            .ok()
            .or_else(|| dotenv_vars.get(key).cloned())
    })?;
    let env_api_key = secrets.youtube_api_key.clone();
    let secrets_path = secrets_file::secrets_path(&path);
    let (secrets, api_key_source) = with_saved_secrets(secrets, &secrets_path)?;
    config.validate_with_secrets(&secrets)?;

    Ok(Loaded {
        config,
        secrets,
        path,
        baseline,
        saved_path,
        saved_tables,
        secrets_path,
        api_key_source,
        env_api_key,
    })
}

/// `baseline` with the tables from the saved settings file put on top, if the
/// file exists, plus the names of the tables replaced. The result is
/// validated, so a bad saved file stops the program before it starts.
fn with_saved_settings(
    baseline: &Config,
    saved_path: &Path,
) -> Result<(Config, Vec<&'static str>), ConfigError> {
    let mut config = baseline.clone();
    let Some(saved) = saved::read(saved_path)? else {
        return Ok((config, Vec::new()));
    };
    saved.apply_to(&mut config);
    config
        .validate()
        .map_err(|err| ConfigError::SavedSettings {
            path: saved_path.to_owned(),
            reason: err.to_string(),
        })?;
    Ok((config, saved.tables()))
}

/// `secrets` with the YouTube API key from the secrets file put on top, if
/// the file has one: a key entered on the dashboard wins over the environment.
/// Also says where the key now in use came from.
fn with_saved_secrets(
    mut secrets: Secrets,
    secrets_path: &Path,
) -> Result<(Secrets, KeySource), ConfigError> {
    let saved = secrets_file::read(secrets_path)?;
    if saved.is_some() && secrets_file::is_too_open(secrets_path) {
        tracing::warn!(
            path = %secrets_path.display(),
            "other users of this computer can read the secrets file; make it private (chmod 600)"
        );
    }
    if let Some(key) = saved.and_then(|saved| saved.youtube_api_key) {
        secrets.youtube_api_key = Some(key);
        return Ok((secrets, KeySource::Saved));
    }
    let source = if secrets.youtube_api_key.is_some() {
        KeySource::Environment
    } else {
        KeySource::None
    };
    Ok((secrets, source))
}

/// Parses TOML text. The error carries the problem and its position but never
/// the offending source line, which keeps stray secrets out of logs.
pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    toml::from_str(text).map_err(|err| ConfigError::Parse {
        path: path.to_owned(),
        location: err
            .span()
            .and_then(|span| line_col(text, span.start))
            .map(|(line, col)| format!(":{line}:{col}"))
            .unwrap_or_default(),
        message: err.message().to_owned(),
    })
}

/// 1-based line and column of a byte offset.
fn line_col(text: &str, offset: usize) -> Option<(usize, usize)> {
    let before = text.get(..offset)?;
    let line = before.matches('\n').count() + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Role;

    fn parse_str(text: &str) -> Result<Config, ConfigError> {
        parse(text, Path::new("config.toml"))
    }

    #[test]
    fn both_chats_are_on_by_default_and_the_old_source_setting_still_loads() {
        let config = parse_str(&format!(
            "[chat]\nsource = \"tiktok\"\n{USER}\n[chat.youtube]\nvideo = \"HFSuEKR1b0w\"\n"
        ))
        .unwrap();
        assert!(config.chat.tiktok.enabled && config.chat.youtube.enabled);
        assert_eq!(config.chat.youtube.video, "HFSuEKR1b0w");
        let off = parse_str("[chat.tiktok]\nenabled = false\n").unwrap();
        assert!(!off.chat.tiktok.enabled && off.chat.youtube.enabled);
        assert!(parse_str("[chat.youtube]\nvideo = \"x\"\nnope = 1\n").is_err());
    }

    /// The one setting a config must have to be valid.
    const USER: &str = "[chat.tiktok]\nusername = \"someone\"\n";

    fn secrets_from(env_key: Option<&str>) -> Secrets {
        Secrets::from_lookup(|name| match name {
            ENV_YOUTUBE_API_KEY => env_key.map(str::to_owned),
            _ => None,
        })
        .unwrap()
    }

    #[test]
    fn a_saved_key_wins_over_the_environment_and_the_source_is_reported() {
        let dir = secrets_file::tests_dir("layer-secrets");
        let path = dir.join("config.secrets.toml");

        // Nothing saved: the environment decides.
        let (s, source) = with_saved_secrets(secrets_from(Some("env-key")), &path).unwrap();
        assert_eq!(s.youtube_api_key.unwrap().expose(), "env-key");
        assert_eq!(source, KeySource::Environment);
        let (s, source) = with_saved_secrets(secrets_from(None), &path).unwrap();
        assert!(s.youtube_api_key.is_none());
        assert_eq!(source, KeySource::None);

        // A saved key replaces the environment's, or fills the gap.
        write_saved_secrets(
            &path,
            &SavedSecrets {
                youtube_api_key: Some(Secret::new("saved-key")),
            },
        )
        .unwrap();
        for env in [Some("env-key"), None] {
            let (s, source) = with_saved_secrets(secrets_from(env), &path).unwrap();
            assert_eq!(s.youtube_api_key.unwrap().expose(), "saved-key");
            assert_eq!(source, KeySource::Saved);
        }

        // A file without a key changes nothing.
        write_saved_secrets(&path, &SavedSecrets::default()).unwrap();
        let (s, source) = with_saved_secrets(secrets_from(Some("env-key")), &path).unwrap();
        assert_eq!(s.youtube_api_key.unwrap().expose(), "env-key");
        assert_eq!(source, KeySource::Environment);
    }

    #[test]
    fn a_damaged_secrets_file_stops_startup_without_showing_its_content() {
        let dir = secrets_file::tests_dir("layer-secrets-bad");
        let path = dir.join("config.secrets.toml");
        std::fs::write(&path, "youtube_api_key = AIzaSy-leaky\n").unwrap();
        let err = with_saved_secrets(secrets_from(Some("env-key")), &path).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("config.secrets.toml"), "{text}");
        assert!(!text.contains("AIzaSy-leaky"), "{text}");
    }

    #[test]
    fn without_a_saved_file_the_config_is_used_as_it_is() {
        let dir = saved::tests::temp_dir("layer-none");
        let baseline = parse_str(&format!("{USER}[moderation]\ncooldown_secs = 90")).unwrap();
        let (config, tables) =
            with_saved_settings(&baseline, &dir.join("config.settings.toml")).unwrap();
        assert_eq!(config, baseline);
        assert!(tables.is_empty());
    }

    #[test]
    fn saved_tables_win_over_config_toml_and_the_rest_is_kept() {
        let dir = saved::tests::temp_dir("layer-some");
        let path = dir.join("config.settings.toml");
        std::fs::write(
            &path,
            "[moderation]\ncooldown_secs = 5\n\n[blacklist]\nchannels = [\"X\"]\n",
        )
        .unwrap();
        let baseline = parse_str(&format!(
            "{USER}[moderation]\ncooldown_secs = 90\nmax_queue_len = 3\n[dashboard]\nchat_history = 7"
        ))
        .unwrap();
        let (config, tables) = with_saved_settings(&baseline, &path).unwrap();
        assert_eq!(tables, vec!["moderation", "blacklist"]);
        assert_eq!(config.moderation.cooldown_secs, 5);
        // A saved table replaces the whole table, not single fields.
        assert_eq!(config.moderation.max_queue_len, 20);
        assert_eq!(config.blacklist.channels, vec!["X".to_owned()]);
        assert_eq!(config.dashboard.chat_history, 7);
    }

    #[test]
    fn invalid_saved_values_stop_startup_and_name_the_saved_file() {
        let dir = saved::tests::temp_dir("layer-bad");
        let path = dir.join("config.settings.toml");
        let baseline = parse_str(USER).unwrap();
        for (bad, field) in [
            ("[moderation]\nvolume = 90\nmax_volume = 50\n", "volume"),
            ("[dashboard]\nchat_history = 5000\n", "chat_history"),
            ("[blacklist]\nvideo_ids = [\"not an id\"]\n", "video_ids"),
        ] {
            std::fs::write(&path, bad).unwrap();
            let err = with_saved_settings(&baseline, &path).unwrap_err();
            assert!(
                matches!(err, ConfigError::SavedSettings { .. }),
                "{bad}: {err}"
            );
            let message = err.to_string();
            assert!(message.contains("config.settings.toml"), "{message}");
            assert!(message.contains(field), "{message}");
        }
    }

    #[test]
    fn example_config_parses_and_validates() {
        let config = parse_str(include_str!("../../config.example.toml")).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn empty_file_yields_defaults() {
        assert_eq!(parse_str("").unwrap(), Config::default());
    }

    #[test]
    fn defaults_match_documented_values() {
        let c = Config::default();
        assert_eq!(c.server.bind.to_string(), "127.0.0.1");
        assert_eq!(c.moderation.mode, ModerationMode::Manual);
        assert_eq!(c.moderation.min_role, Role::Follower);
        assert_eq!(c.command.prefix, "!");
        assert_eq!(c.command.commands, vec![CommandEntry::youtube("v")]);
        assert_eq!(c.general.language, Language::Id);
        assert_eq!(c.dashboard.chat_history, 20);
    }

    #[test]
    fn command_table_takes_a_list_or_the_single_name_of_older_files() {
        let c = parse_str(
            "[command]\nprefix = \"#\"\n\
             commands = [{ name = \"yt\", source = \"youtube\" }, { name = \"v\", source = \"youtube\" }]",
        )
        .unwrap();
        assert_eq!(c.command.prefix, "#");
        assert_eq!(
            c.command.commands,
            vec![CommandEntry::youtube("yt"), CommandEntry::youtube("v")]
        );

        // Older files: one name, same as a YouTube command with that name.
        let c = parse_str("[command]\nname = \"play\"").unwrap();
        assert_eq!(c.command.prefix, "!");
        assert_eq!(c.command.commands, vec![CommandEntry::youtube("play")]);

        // Only a prefix: the default command stays.
        let c = parse_str("[command]\nprefix = \"?\"").unwrap();
        assert_eq!(c.command.commands, vec![CommandEntry::youtube("v")]);
    }

    #[test]
    fn command_table_mistakes_are_refused() {
        for bad in [
            // Both ways at once is ambiguous.
            "[command]\nname = \"v\"\ncommands = [{ name = \"yt\", source = \"youtube\" }]",
            // A source the program does not have.
            "[command]\ncommands = [{ name = \"vt\", source = \"twitch\" }]",
            // A command needs both fields, and nothing else.
            "[command]\ncommands = [{ name = \"vt\" }]",
            "[command]\ncommands = [{ name = \"vt\", source = \"youtube\", extra = 1 }]",
            "[command]\nsurprise = 1",
        ] {
            assert!(parse_str(bad).is_err(), "{bad}");
        }
        // A list with a repeated name is well formed but does not validate.
        let twice = parse_str(
            "[command]\ncommands = [{ name = \"v\", source = \"youtube\" }, { name = \"V\", source = \"youtube\" }]",
        )
        .unwrap();
        assert!(twice.validate().is_err());
    }

    #[test]
    fn dashboard_table_is_read_and_checked() {
        let c = parse_str("[dashboard]\nchat_history = 50").unwrap();
        assert_eq!(c.dashboard.chat_history, 50);
        assert!(parse_str("[dashboard]\nchat_history = -1").is_err());
        assert!(parse_str("[dashboard]\nhistory = 5").is_err());
    }

    #[test]
    fn partial_file_overrides_only_given_keys() {
        let c = parse_str(
            r#"
            [moderation]
            min_role = "subscriber"
            min_gifter_level = 3
            min_fan_club_level = 2
            mode = "auto"

            [general]
            language = "en"
            "#,
        )
        .unwrap();
        assert_eq!(c.moderation.min_role, Role::Subscriber);
        assert_eq!(c.moderation.min_gifter_level, 3);
        assert_eq!(c.moderation.min_fan_club_level, 2);
        assert_eq!(c.moderation.mode, ModerationMode::Auto);
        assert_eq!(c.general.language, Language::En);
        assert_eq!(c.moderation.cooldown_secs, 60);
    }

    #[test]
    fn unknown_keys_and_bad_values_are_rejected() {
        assert!(parse_str("typo = 1").is_err());
        assert!(parse_str("[server]\nprt = 1").is_err());
        assert!(parse_str("[server]\nport = 70000").is_err());
        assert!(parse_str("[moderation]\nmin_role = \"vip\"").is_err());
        assert!(parse_str("[chat]\nsource = \"twitch\"").is_err());
        assert!(parse_str("[general]\nlanguage = \"fr\"").is_err());
    }

    #[test]
    fn parse_error_reports_position() {
        let err = parse_str("[server]\nport = 1\nbogus = 2\n").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("config.toml:3:1"), "{text}");
    }

    #[test]
    fn parse_error_does_not_echo_secret_values() {
        let err = parse_str("youtube_api_key = \"AIzaSy-super-secret\"\n").unwrap_err();
        let text = err.to_string();
        assert!(!text.contains("AIzaSy-super-secret"), "{text}");
        assert!(text.contains("youtube_api_key"), "{text}");
    }

    #[test]
    fn utf8_bom_is_ignored() {
        let c = parse_str("\u{feff}[server]\nport = 9000\n").unwrap();
        assert_eq!(c.server.port, 9000);
    }

    #[test]
    fn line_col_handles_edges() {
        assert_eq!(line_col("abc", 0), Some((1, 1)));
        assert_eq!(line_col("ab\ncd", 4), Some((2, 2)));
        assert_eq!(line_col("é\nx", 3), Some((2, 1)));
        assert_eq!(line_col("abc", 99), None);
        // offset inside a multi-byte character
        assert_eq!(line_col("é", 1), None);
    }
}
