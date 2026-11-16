//! Configuration: `config.toml` for settings, environment variables or a
//! `.env` file for secrets.

mod dotenv;
mod error;
mod model;
mod paths;
mod secrets;
mod validate;

use std::path::{Path, PathBuf};

pub use error::ConfigError;
pub use model::{
    BlacklistConfig, ChatConfig, ChatSourceKind, CommandConfig, Config, DashboardConfig,
    GeneralConfig, Language, MAX_CHAT_HISTORY, ModerationConfig, ModerationMode, ServerConfig,
    TiktokConfig, YoutubeConfig,
};
pub use paths::{CONFIG_FILE_NAME, ENV_CONFIG_PATH};
pub use secrets::{
    ENV_DASHBOARD_TOKEN, ENV_YOUTUBE_API_KEY, MIN_DASHBOARD_TOKEN_LEN, Secret, Secrets,
};

use crate::platform;

/// Everything the rest of the app needs from configuration.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub secrets: Secrets,
    /// The config file that was read.
    pub path: PathBuf,
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
    let config = parse(&text, &path)?;
    config.validate()?;

    let (dotenv_vars, _) = dotenv::load_first(&paths::dotenv_candidates(user_dir.as_deref()))?;
    let secrets = Secrets::from_lookup(|key| {
        std::env::var(key)
            .ok()
            .or_else(|| dotenv_vars.get(key).cloned())
    })?;
    config.validate_with_secrets(&secrets)?;

    Ok(Loaded {
        config,
        secrets,
        path,
    })
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
        assert_eq!(c.command.name, "v");
        assert_eq!(c.general.language, Language::Id);
        assert_eq!(c.dashboard.chat_history, 20);
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
