use std::path::PathBuf;

/// Errors from locating, reading and validating configuration.
///
/// Messages never include secret values.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no config file found (searched: {}); copy config.example.toml to one of these locations", join_paths(.searched))]
    NotFound { searched: Vec<PathBuf> },

    #[error("cannot read {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid TOML in {}{location}: {message}", .path.display())]
    Parse {
        path: PathBuf,
        /// `":line:column"` when known, otherwise empty.
        location: String,
        message: String,
    },

    #[error("invalid .env file {} at line {line}: {reason}", .path.display())]
    Dotenv {
        path: PathBuf,
        line: usize,
        reason: &'static str,
    },

    #[error("invalid saved settings in {}: {reason}; fix that file, or delete it to go back to config.toml", .path.display())]
    SavedSettings { path: PathBuf, reason: String },

    #[error("invalid config value `{field}`: {reason}")]
    Invalid { field: String, reason: String },

    #[error("invalid secret {name}: {reason}")]
    InvalidSecret { name: &'static str, reason: String },
}

impl ConfigError {
    pub(crate) fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
