use std::path::{Path, PathBuf};

use super::error::ConfigError;

pub const CONFIG_FILE_NAME: &str = "config.toml";
pub const DOTENV_FILE_NAME: &str = ".env";
pub const APP_DIR_NAME: &str = "stream-spectre";
/// Environment variable that overrides the config file location.
pub const ENV_CONFIG_PATH: &str = "STREAM_SPECTRE_CONFIG";

/// Ordered list of config file locations to try.
///
/// An explicit path (`--config`) or `STREAM_SPECTRE_CONFIG` is used on its own
/// with no fallback, so a typo is reported instead of silently ignored.
/// Otherwise: `./config.toml`, then the per-user config directory.
#[cfg(test)]
pub fn config_candidates(
    explicit: Option<PathBuf>,
    env_value: Option<String>,
    user_config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    config_candidates_with(
        PathBuf::from(CONFIG_FILE_NAME),
        explicit,
        env_value,
        user_config_dir,
    )
}

/// [`config_candidates`], with the place of the "current directory" file given.
pub fn config_candidates_with(
    local: PathBuf,
    explicit: Option<PathBuf>,
    env_value: Option<String>,
    user_config_dir: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(path) = explicit {
        return vec![path];
    }
    if let Some(value) = env_value.filter(|v| !v.trim().is_empty()) {
        return vec![PathBuf::from(value)];
    }
    let mut list = vec![local];
    if let Some(dir) = user_config_dir {
        list.push(dir.join(APP_DIR_NAME).join(CONFIG_FILE_NAME));
    }
    list
}

/// Locations of the `.env` file, in priority order.
pub fn dotenv_candidates(user_config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut list = vec![PathBuf::from(DOTENV_FILE_NAME)];
    if let Some(dir) = user_config_dir {
        list.push(dir.join(APP_DIR_NAME).join(DOTENV_FILE_NAME));
    }
    list
}

/// First candidate for which `exists` is true.
pub fn locate(
    candidates: &[PathBuf],
    exists: impl Fn(&Path) -> bool,
) -> Result<PathBuf, ConfigError> {
    candidates
        .iter()
        .find(|p| exists(p))
        .cloned()
        .ok_or_else(|| ConfigError::NotFound {
            searched: candidates.to_vec(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_path_wins_and_has_no_fallback() {
        let c = config_candidates(
            Some(PathBuf::from("/x/a.toml")),
            Some("/y/b.toml".into()),
            Some(Path::new("/home/u/.config")),
        );
        assert_eq!(c, vec![PathBuf::from("/x/a.toml")]);
    }

    #[test]
    fn env_value_beats_defaults_but_blank_is_ignored() {
        let c = config_candidates(None, Some("/y/b.toml".into()), None);
        assert_eq!(c, vec![PathBuf::from("/y/b.toml")]);

        let c = config_candidates(None, Some("  ".into()), None);
        assert_eq!(c, vec![PathBuf::from("config.toml")]);
    }

    #[test]
    fn defaults_are_cwd_then_user_dir() {
        let c = config_candidates(None, None, Some(Path::new("/home/u/.config")));
        assert_eq!(
            c,
            vec![
                PathBuf::from("config.toml"),
                PathBuf::from("/home/u/.config/stream-spectre/config.toml"),
            ]
        );
    }

    #[test]
    fn dotenv_candidates_mirror_config_order() {
        let c = dotenv_candidates(Some(Path::new("/home/u/.config")));
        assert_eq!(
            c,
            vec![
                PathBuf::from(".env"),
                PathBuf::from("/home/u/.config/stream-spectre/.env"),
            ]
        );
        assert_eq!(dotenv_candidates(None), vec![PathBuf::from(".env")]);
    }

    #[test]
    fn locate_picks_first_existing_or_reports_all_searched() {
        let list = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        let found = locate(&list, |p| p == Path::new("b") || p == Path::new("c")).unwrap();
        assert_eq!(found, PathBuf::from("b"));

        let err = locate(&list, |_| false).unwrap_err();
        match err {
            ConfigError::NotFound { searched } => assert_eq!(searched, list),
            other => panic!("unexpected error: {other}"),
        }
    }
}
