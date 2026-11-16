//! Secrets saved from the dashboard, in a file only their owner can read.
//!
//! The file sits next to `config.toml` and is named after it
//! (`config.secrets.toml`). It is separate from the saved settings
//! (`config.settings.toml`) on purpose: that file is meant to be looked at,
//! this one is not. On Linux and macOS it is created readable and writable by
//! its owner alone (mode 0600). Windows has no such mode; there the folder
//! of the user's profile is what protects it.
//!
//! Nothing in here ever prints a secret: errors carry the file name and, for a
//! parse error, a position, never the text.

use std::ffi::OsString;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::ConfigError;
use super::secrets::Secret;

/// Appended to the config file's name without its extension.
const FILE_SUFFIX: &str = ".secrets.toml";

const HEADER: &str = "\
# Written by stream-spectre when a secret is entered on the dashboard.
# Keep this file private: it holds your YouTube API key in plain text.
# It is git-ignored. To go back to the key in the environment (or .env),
# delete the line or this whole file while the program is stopped.
";

/// What the file holds.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SavedSecrets {
    pub youtube_api_key: Option<Secret>,
}

impl std::fmt::Debug for SavedSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedSecrets")
            .field("youtube_api_key", &self.youtube_api_key)
            .finish()
    }
}

/// The file as TOML. Kept apart so `Secret` itself never needs `Serialize`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    youtube_api_key: Option<String>,
}

/// Where the secrets for `config_path` live: next to it, named after it.
pub fn secrets_path(config_path: &Path) -> PathBuf {
    let mut name = config_path
        .file_stem()
        .map_or_else(|| OsString::from("config"), OsString::from);
    name.push(FILE_SUFFIX);
    config_path.with_file_name(name)
}

/// Reads the saved secrets. A missing file is not an error. A blank value
/// counts as no value.
pub fn read(path: &Path) -> Result<Option<SavedSecrets>, ConfigError> {
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
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let raw: Raw = toml::from_str(text).map_err(|err| ConfigError::Parse {
        path: path.to_owned(),
        location: err
            .span()
            .and_then(|span| super::line_col(text, span.start))
            .map(|(line, col)| format!(":{line}:{col}"))
            .unwrap_or_default(),
        // The usual message may repeat the offending value.
        message: "not a valid secrets file (its content is not shown)".to_owned(),
    })?;
    Ok(Some(SavedSecrets {
        youtube_api_key: raw
            .youtube_api_key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .map(Secret::new),
    }))
}

/// Writes the file in one step, readable by its owner alone: the text goes to
/// a temporary file that is created private, then replaces the real one, so a
/// crash never leaves a half-written file and the secret is never in a file
/// others can read, even for a moment.
pub fn write(path: &Path, secrets: &SavedSecrets) -> std::io::Result<()> {
    let raw = Raw {
        youtube_api_key: secrets
            .youtube_api_key
            .as_ref()
            .map(|key| key.expose().to_owned()),
    };
    let body = toml::to_string_pretty(&raw).map_err(std::io::Error::other)?;

    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    // An old leftover could have looser permissions; start from nothing.
    let _ = std::fs::remove_file(&temporary);

    let result = (|| {
        let mut file = create_private(&temporary)?;
        file.write_all(HEADER.as_bytes())?;
        if !body.is_empty() {
            file.write_all(b"\n")?;
            file.write_all(body.as_bytes())?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// True when other users of this computer can read or change the file. Only
/// meaningful on Linux and macOS; elsewhere it is always false.
#[cfg(unix)]
pub fn is_too_open(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o077 != 0)
}

#[cfg(not(unix))]
pub fn is_too_open(_path: &Path) -> bool {
    false
}

/// A new empty folder for tests elsewhere in the crate.
#[cfg(test)]
pub(crate) fn tests_dir(label: &str) -> PathBuf {
    crate::config::saved::tests::temp_dir(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::saved::tests::temp_dir;

    const KEY: &str = "AIzaSy-test-key-value-0001";

    fn saved(key: &str) -> SavedSecrets {
        SavedSecrets {
            youtube_api_key: Some(Secret::new(key)),
        }
    }

    #[test]
    fn the_file_sits_next_to_the_config_and_is_named_after_it() {
        assert_eq!(
            secrets_path(Path::new("/home/a/config.toml")),
            Path::new("/home/a/config.secrets.toml")
        );
        assert_ne!(
            secrets_path(Path::new("/x/a.toml")),
            secrets_path(Path::new("/x/b.toml"))
        );
        // Not the file with the settings.
        assert_ne!(
            secrets_path(Path::new("config.toml")),
            crate::config::saved_path(Path::new("config.toml"))
        );
    }

    #[test]
    fn what_is_written_is_read_back_and_a_missing_file_is_nothing() {
        let dir = temp_dir("secrets-roundtrip");
        let path = dir.join("config.secrets.toml");
        assert_eq!(read(&path).unwrap(), None);
        write(&path, &saved(KEY)).unwrap();
        assert_eq!(read(&path).unwrap(), Some(saved(KEY)));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Written by stream-spectre"));
    }

    #[test]
    fn an_empty_set_is_a_comment_only_file_and_a_blank_key_is_no_key() {
        let dir = temp_dir("secrets-empty");
        let path = dir.join("config.secrets.toml");
        write(&path, &SavedSecrets::default()).unwrap();
        assert_eq!(read(&path).unwrap(), Some(SavedSecrets::default()));
        std::fs::write(&path, "youtube_api_key = \"   \"\n").unwrap();
        assert_eq!(read(&path).unwrap(), Some(SavedSecrets::default()));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_from_the_start_and_stays_so_when_replaced() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("secrets-mode");
        let path = dir.join("config.secrets.toml");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        write(&path, &saved(KEY)).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(!is_too_open(&path));

        // An old, loose temporary file does not make the new one loose.
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::write(&temporary, "old").unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o666)).unwrap();
        write(&path, &saved("AIzaSy-another-key-0002")).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert!(!std::path::Path::new(&temporary).exists());
        assert_eq!(read(&path).unwrap(), Some(saved("AIzaSy-another-key-0002")));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_read_is_noticed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_dir("secrets-open");
        let path = dir.join("config.secrets.toml");
        write(&path, &saved(KEY)).unwrap();
        for loose in [0o644, 0o640, 0o604, 0o666] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(loose)).unwrap();
            assert!(is_too_open(&path), "{loose:o}");
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(!is_too_open(&path));
        assert!(!is_too_open(&dir.join("missing.toml")));
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temporary_one() {
        let dir = temp_dir("secrets-failed");
        let path = dir.join("config.secrets.toml");
        write(&path, &saved(KEY)).unwrap();
        // A folder where the temporary file goes makes the write fail.
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::create_dir(&temporary).unwrap();
        // `remove_file` cannot remove a folder, so `create_new` then fails.
        assert!(write(&path, &saved("AIzaSy-another-key-0002")).is_err());
        assert_eq!(read(&path).unwrap(), Some(saved(KEY)));
    }

    #[test]
    fn a_broken_file_names_the_position_but_never_shows_its_content() {
        let dir = temp_dir("secrets-broken");
        let path = dir.join("config.secrets.toml");
        for bad in [
            "youtube_api_key = 12345678901234567890\n",
            "youtube_api_key = \"AIzaSy-leaky-key\" garbage\n",
            "other = \"AIzaSy-leaky-key\"\n",
        ] {
            std::fs::write(&path, bad).unwrap();
            let message = read(&path).unwrap_err().to_string();
            assert!(message.contains("config.secrets.toml"), "{message}");
            assert!(!message.contains("AIzaSy-leaky-key"), "{message}");
            assert!(!message.contains("12345678901234567890"), "{message}");
        }
    }

    #[test]
    fn debug_output_hides_the_key() {
        let text = format!("{:?}", saved(KEY));
        assert!(!text.contains(KEY), "{text}");
        assert!(text.contains("redacted"), "{text}");
    }
}
