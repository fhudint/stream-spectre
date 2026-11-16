use std::fmt;

use super::error::ConfigError;

pub const ENV_YOUTUBE_API_KEY: &str = "YOUTUBE_API_KEY";
pub const ENV_DASHBOARD_TOKEN: &str = "DASHBOARD_TOKEN";
pub const MIN_DASHBOARD_TOKEN_LEN: usize = 16;

/// A secret string. `Debug` is redacted and there is no `Display`, so it
/// cannot end up in logs by accident; use `expose` where the value is needed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Secrets read from environment variables or a `.env` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secrets {
    pub youtube_api_key: Secret,
    /// Token protecting the dashboard when it is exposed beyond loopback.
    pub dashboard_token: Option<Secret>,
}

impl Secrets {
    /// Builds secrets from a key lookup (environment first, then `.env`).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let youtube_api_key =
            non_empty(lookup(ENV_YOUTUBE_API_KEY)).ok_or(ConfigError::MissingSecret {
                name: ENV_YOUTUBE_API_KEY,
            })?;

        let dashboard_token = non_empty(lookup(ENV_DASHBOARD_TOKEN));
        if dashboard_token
            .as_ref()
            .is_some_and(|t| t.chars().count() < MIN_DASHBOARD_TOKEN_LEN)
        {
            return Err(ConfigError::InvalidSecret {
                name: ENV_DASHBOARD_TOKEN,
                reason: format!("must be at least {MIN_DASHBOARD_TOKEN_LEN} characters"),
            });
        }

        Ok(Self {
            youtube_api_key: Secret::new(youtube_api_key),
            dashboard_token: dashboard_token.map(Secret::new),
        })
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "AIzaSy-test-key-value";
    const TOKEN: &str = "0123456789abcdef-token";

    fn lookup(
        key: Option<&'static str>,
        token: Option<&'static str>,
    ) -> impl Fn(&str) -> Option<String> {
        move |name| match name {
            ENV_YOUTUBE_API_KEY => key.map(str::to_owned),
            ENV_DASHBOARD_TOKEN => token.map(str::to_owned),
            _ => None,
        }
    }

    #[test]
    fn reads_both_secrets_and_trims() {
        let s =
            Secrets::from_lookup(lookup(Some("  AIzaSy-test-key-value\n"), Some(TOKEN))).unwrap();
        assert_eq!(s.youtube_api_key.expose(), KEY);
        assert_eq!(s.dashboard_token.unwrap().expose(), TOKEN);
    }

    #[test]
    fn dashboard_token_is_optional() {
        let s = Secrets::from_lookup(lookup(Some(KEY), None)).unwrap();
        assert!(s.dashboard_token.is_none());
        let s = Secrets::from_lookup(lookup(Some(KEY), Some("   "))).unwrap();
        assert!(s.dashboard_token.is_none());
    }

    #[test]
    fn missing_or_blank_api_key_is_an_error() {
        for key in [None, Some(""), Some("   ")] {
            let err = Secrets::from_lookup(lookup(key, None)).unwrap_err();
            assert!(
                matches!(err, ConfigError::MissingSecret { name } if name == ENV_YOUTUBE_API_KEY)
            );
        }
    }

    #[test]
    fn short_dashboard_token_is_rejected() {
        let err = Secrets::from_lookup(lookup(Some(KEY), Some("short"))).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSecret { .. }));
        assert!(!err.to_string().contains("short"));
    }

    #[test]
    fn debug_output_is_redacted() {
        let s = Secrets::from_lookup(lookup(Some(KEY), Some(TOKEN))).unwrap();
        let dbg = format!("{s:?}");
        assert!(!dbg.contains(KEY));
        assert!(!dbg.contains(TOKEN));
        assert!(dbg.contains("<redacted>"));
    }
}
