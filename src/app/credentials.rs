//! The secrets that can be changed from the dashboard, and what the dashboard
//! may learn about them.
//!
//! Today that is the YouTube API key. It is kept in a file only its owner can
//! read (see `config::secrets_file`), checked with one real YouTube request
//! before it is saved, and put to use at once, without a restart.
//!
//! The key is write-only: nothing here, and nothing built on it, returns the
//! key itself. `AccountView` says whether a key is set, where it came from and
//! its last four characters, and that is all the dashboard ever receives.

use std::path::PathBuf;

use serde::Serialize;
use tokio::sync::Mutex;

use super::Pipeline;
use crate::config::{
    KeySource, SavedSecrets, Secret, secrets_file_is_too_open, write_saved_secrets,
};
use crate::youtube::{VideoSource, YoutubeError};

/// Longest key accepted. Google's keys are 39 characters.
const MAX_KEY_LEN: usize = 200;

/// A key shorter than this never shows any of its characters.
const MIN_LEN_TO_SHOW_ENDING: usize = 12;

/// What the dashboard may know about the YouTube API key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyStatus {
    pub set: bool,
    /// `none`, `environment` or `saved`.
    pub source: &'static str,
    /// The last four characters, to tell keys apart; `None` for no key or a
    /// key too short to show any part of.
    pub last4: Option<String>,
}

/// Whether a dashboard token protects the dashboard. The token itself is
/// never shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TokenStatus {
    pub set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountView {
    pub youtube_key: KeyStatus,
    pub dashboard_token: TokenStatus,
}

/// How a key that was saved had checked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCheck {
    /// YouTube answered a request made with it.
    Valid,
    /// YouTube accepted the key but its daily quota is used up: the key is
    /// real, and lookups work again when the quota resets.
    QuotaExhausted,
}

/// Why a key was not saved.
#[derive(Debug, thiserror::Error)]
pub enum CredentialsError {
    #[error("the key is empty")]
    Empty,
    #[error("the key has characters or a length no YouTube API key has")]
    BadFormat,
    #[error("YouTube refused the key")]
    Rejected,
    #[error("could not check the key with YouTube ({0})")]
    CannotCheck(YoutubeError),
    #[error("could not save the key to disk: {0}")]
    NotSaved(std::io::Error),
}

impl CredentialsError {
    /// A short stable name for programs (the dashboard picks its text by it).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Empty => "key_empty",
            Self::BadFormat => "key_bad_format",
            Self::Rejected => "key_rejected",
            Self::CannotCheck(_) => "cannot_check",
            Self::NotSaved(_) => "not_saved",
        }
    }
}

struct State {
    saved: Option<Secret>,
}

pub struct Credentials {
    path: PathBuf,
    /// The key from the environment or `.env`: what is used when no key is
    /// saved.
    env_key: Option<Secret>,
    token_in_use: bool,
    /// One change at a time, held across the YouTube check and the write.
    state: Mutex<State>,
}

impl Credentials {
    /// `saved` is the key found in the secrets file at startup, if any.
    pub fn new(
        path: PathBuf,
        env_key: Option<Secret>,
        saved: Option<Secret>,
        token_in_use: bool,
    ) -> Self {
        if saved.is_some() && secrets_file_is_too_open(&path) {
            tracing::warn!("the secrets file can be read by other users of this computer");
        }
        Self {
            path,
            env_key,
            token_in_use,
            state: Mutex::new(State { saved }),
        }
    }

    /// What the dashboard may know right now.
    pub async fn status(&self) -> AccountView {
        let state = self.state.lock().await;
        self.view(&state)
    }

    fn view(&self, state: &State) -> AccountView {
        let (key, source) = match (&state.saved, &self.env_key) {
            (Some(saved), _) => (Some(saved), KeySource::Saved),
            (None, Some(env)) => (Some(env), KeySource::Environment),
            (None, None) => (None, KeySource::None),
        };
        AccountView {
            youtube_key: KeyStatus {
                set: key.is_some(),
                source: match source {
                    KeySource::None => "none",
                    KeySource::Environment => "environment",
                    KeySource::Saved => "saved",
                },
                last4: key.and_then(|k| ending(k.expose())),
            },
            dashboard_token: TokenStatus {
                set: self.token_in_use,
            },
        }
    }

    /// Checks `key` with YouTube, saves it, and starts using it.
    ///
    /// Nothing changes unless all three work: a key YouTube refuses, or a file
    /// that cannot be written, leaves the key in use as it was.
    pub async fn set_youtube_key<S: VideoSource + 'static>(
        &self,
        pipeline: &Pipeline<S>,
        key: &str,
    ) -> Result<(KeyCheck, AccountView), CredentialsError> {
        let secret = Secret::new(validated(key)?);

        let checked = match pipeline.check_youtube_key(&secret).await {
            Ok(()) => KeyCheck::Valid,
            Err(YoutubeError::QuotaExceeded | YoutubeError::RateLimited) => {
                KeyCheck::QuotaExhausted
            }
            Err(YoutubeError::InvalidApiKey | YoutubeError::AccessDenied { .. }) => {
                return Err(CredentialsError::Rejected);
            }
            Err(other) => return Err(CredentialsError::CannotCheck(other)),
        };

        let mut state = self.state.lock().await;
        self.save(Some(secret.clone())).await?;
        // The same key was just built into a client by the check, so this
        // does not fail; if it somehow did, the key is still saved.
        if let Err(error) = pipeline.set_youtube_key(Some(&secret)) {
            tracing::error!(%error, "could not start using the saved YouTube key");
        }
        state.saved = Some(secret);
        tracing::info!("YouTube API key replaced from the dashboard");
        Ok((checked, self.view(&state)))
    }

    /// Forgets the saved key and goes back to the one in the environment, if
    /// there is one.
    pub async fn remove_saved_youtube_key<S: VideoSource + 'static>(
        &self,
        pipeline: &Pipeline<S>,
    ) -> Result<AccountView, CredentialsError> {
        let mut state = self.state.lock().await;
        self.save(None).await?;
        if let Err(error) = pipeline.set_youtube_key(self.env_key.as_ref()) {
            tracing::error!(%error, "could not go back to the YouTube key from the environment");
        }
        state.saved = None;
        tracing::info!("saved YouTube API key removed from the dashboard");
        Ok(self.view(&state))
    }

    async fn save(&self, key: Option<Secret>) -> Result<(), CredentialsError> {
        let path = self.path.clone();
        let secrets = SavedSecrets {
            youtube_api_key: key,
        };
        tokio::task::spawn_blocking(move || write_saved_secrets(&path, &secrets))
            .await
            .unwrap_or_else(|error| Err(std::io::Error::other(error)))
            .map_err(CredentialsError::NotSaved)
    }
}

/// The key as it will be used: no surrounding whitespace, and only printable
/// ASCII without spaces, as every YouTube API key is. Refusing anything else
/// here also keeps a pasted line break or a stray character out of an HTTP
/// header.
fn validated(key: &str) -> Result<&str, CredentialsError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(CredentialsError::Empty);
    }
    if key.len() > MAX_KEY_LEN || !key.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(CredentialsError::BadFormat);
    }
    Ok(key)
}

fn ending(key: &str) -> Option<String> {
    let chars: Vec<char> = key.chars().collect();
    (chars.len() >= MIN_LEN_TO_SHOW_ENDING).then(|| chars[chars.len() - 4..].iter().collect())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::config::{Config, Language, read_saved_secrets};
    use crate::i18n::I18n;
    use crate::youtube::{VideoDetails, VideoId};

    /// A video source whose answer to "is this key good?" the test decides,
    /// and that remembers which keys it was told to use.
    #[derive(Default)]
    struct Keys {
        verdict: StdMutex<Option<YoutubeError>>,
        checked: StdMutex<Vec<String>>,
        used: StdMutex<Vec<Option<String>>>,
    }

    impl VideoSource for Keys {
        async fn fetch(&self, _id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
            Ok(None)
        }

        fn set_api_key(&self, key: Option<&Secret>) -> Result<(), YoutubeError> {
            self.used
                .lock()
                .unwrap()
                .push(key.map(|k| k.expose().to_owned()));
            Ok(())
        }

        fn check_api_key(
            &self,
            key: &Secret,
        ) -> impl Future<Output = Result<(), YoutubeError>> + Send {
            self.checked.lock().unwrap().push(key.expose().to_owned());
            let verdict = self.verdict.lock().unwrap().clone();
            async move { verdict.map_or(Ok(()), Err) }
        }
    }

    fn pipeline() -> Pipeline<Keys> {
        let mut config = Config::default();
        config.chat.tiktok.username = "streamer".to_owned();
        Pipeline::new(&config, I18n::load(Language::En).unwrap(), Keys::default()).unwrap()
    }

    fn credentials(label: &str, env: Option<&str>, saved: Option<&str>) -> Credentials {
        let dir = crate::config::test_temp_dir(label);
        Credentials::new(
            dir.join("config.secrets.toml"),
            env.map(Secret::new),
            saved.map(Secret::new),
            true,
        )
    }

    fn on_disk(c: &Credentials) -> Option<String> {
        read_saved_secrets(&c.path)
            .unwrap()
            .and_then(|s| s.youtube_api_key)
            .map(|k| k.expose().to_owned())
    }

    const GOOD: &str = "TESTKEY-1234567890abcdefghijklmnopqrstu";

    #[tokio::test]
    async fn the_status_says_where_the_key_is_from_and_shows_only_its_ending() {
        let none = credentials("cred-status-none", None, None).status().await;
        assert_eq!(
            none.youtube_key,
            KeyStatus {
                set: false,
                source: "none",
                last4: None
            }
        );
        assert!(none.dashboard_token.set);

        let env = credentials("cred-status-env", Some(GOOD), None)
            .status()
            .await;
        assert_eq!(env.youtube_key.source, "environment");
        assert_eq!(env.youtube_key.last4.as_deref(), Some("rstu"));

        let saved = credentials("cred-status-saved", Some("env-key-value-123"), Some(GOOD))
            .status()
            .await;
        assert_eq!(saved.youtube_key.source, "saved", "the saved key wins");
        assert!(saved.youtube_key.set);

        // A short key shows nothing of itself.
        let short = credentials("cred-status-short", Some("shortkey"), None)
            .status()
            .await;
        assert!(short.youtube_key.set);
        assert_eq!(short.youtube_key.last4, None);

        // Whatever is shown, the key itself never is.
        for view in [&none, &env, &saved, &short] {
            let json = serde_json::to_string(view).unwrap();
            assert!(!json.contains(GOOD) && !json.contains("shortkey"), "{json}");
        }
    }

    #[tokio::test]
    async fn a_good_key_is_checked_saved_and_put_to_use() {
        let p = pipeline();
        let c = credentials("cred-set", None, None);
        let (checked, view) = c.set_youtube_key(&p, &format!("  {GOOD}\n")).await.unwrap();
        assert_eq!(checked, KeyCheck::Valid);
        assert_eq!(view.youtube_key.source, "saved");
        assert_eq!(view.youtube_key.last4.as_deref(), Some("rstu"));
        assert_eq!(on_disk(&c).as_deref(), Some(GOOD));
        assert_eq!(*p.lookup_source().checked.lock().unwrap(), vec![GOOD]);
        assert_eq!(
            *p.lookup_source().used.lock().unwrap(),
            vec![Some(GOOD.to_owned())]
        );
        assert_eq!(c.status().await, view);
    }

    #[tokio::test]
    async fn a_key_youtube_refuses_is_neither_saved_nor_used() {
        let p = pipeline();
        let c = credentials("cred-refused", Some("env-key-value-123"), None);
        for refusal in [
            YoutubeError::InvalidApiKey,
            YoutubeError::AccessDenied {
                reason: "forbidden".to_owned(),
            },
        ] {
            *p.lookup_source().verdict.lock().unwrap() = Some(refusal);
            let error = c.set_youtube_key(&p, GOOD).await.unwrap_err();
            assert_eq!(error.code(), "key_rejected");
        }
        assert_eq!(on_disk(&c), None);
        assert!(p.lookup_source().used.lock().unwrap().is_empty());
        assert_eq!(c.status().await.youtube_key.source, "environment");
    }

    #[tokio::test]
    async fn a_key_that_could_not_be_checked_is_not_saved_either() {
        let p = pipeline();
        let c = credentials("cred-offline", None, None);
        *p.lookup_source().verdict.lock().unwrap() = Some(YoutubeError::Network("timeout"));
        let error = c.set_youtube_key(&p, GOOD).await.unwrap_err();
        assert_eq!(error.code(), "cannot_check");
        assert_eq!(on_disk(&c), None);
        assert!(!c.status().await.youtube_key.set);
    }

    #[tokio::test]
    async fn a_key_that_hit_the_quota_limit_is_real_and_is_saved() {
        let p = pipeline();
        for limit in [YoutubeError::QuotaExceeded, YoutubeError::RateLimited] {
            let c = credentials("cred-quota", None, None);
            *p.lookup_source().verdict.lock().unwrap() = Some(limit);
            let (checked, view) = c.set_youtube_key(&p, GOOD).await.unwrap();
            assert_eq!(checked, KeyCheck::QuotaExhausted);
            assert!(view.youtube_key.set);
            assert_eq!(on_disk(&c).as_deref(), Some(GOOD));
        }
    }

    #[tokio::test]
    async fn a_key_in_the_wrong_shape_is_refused_before_any_request() {
        let p = pipeline();
        let c = credentials("cred-shape", None, None);
        for (bad, code) in [
            ("", "key_empty"),
            ("   \n", "key_empty"),
            ("two words", "key_bad_format"),
            ("with\nnewline", "key_bad_format"),
            ("tab\there", "key_bad_format"),
            ("caf\u{e9}-key", "key_bad_format"),
            ("\u{0}", "key_bad_format"),
            (&"x".repeat(201), "key_bad_format"),
        ] {
            let error = c.set_youtube_key(&p, bad).await.unwrap_err();
            assert_eq!(error.code(), code, "{bad:?}");
        }
        assert!(p.lookup_source().checked.lock().unwrap().is_empty());
        assert_eq!(on_disk(&c), None);
        // The longest allowed key is fine.
        assert!(c.set_youtube_key(&p, &"x".repeat(200)).await.is_ok());
    }

    #[tokio::test]
    async fn when_the_file_cannot_be_written_the_old_key_stays_in_use() {
        let p = pipeline();
        let c = credentials("cred-unwritable", Some("env-key-value-123"), None);
        // A folder where the temporary file goes makes every write fail.
        let mut temporary = c.path.as_os_str().to_owned();
        temporary.push(".tmp");
        std::fs::create_dir(&temporary).unwrap();

        let error = c.set_youtube_key(&p, GOOD).await.unwrap_err();
        assert_eq!(error.code(), "not_saved");
        assert!(p.lookup_source().used.lock().unwrap().is_empty());
        assert_eq!(c.status().await.youtube_key.source, "environment");
    }

    #[tokio::test]
    async fn removing_the_saved_key_goes_back_to_the_environment_or_to_none() {
        let p = pipeline();
        let c = credentials("cred-remove", Some("env-key-value-123"), Some(GOOD));
        let view = c.remove_saved_youtube_key(&p).await.unwrap();
        assert_eq!(view.youtube_key.source, "environment");
        assert_eq!(
            *p.lookup_source().used.lock().unwrap(),
            vec![Some("env-key-value-123".to_owned())]
        );
        assert_eq!(on_disk(&c), None);

        let c = credentials("cred-remove-none", None, Some(GOOD));
        let view = c.remove_saved_youtube_key(&p).await.unwrap();
        assert!(!view.youtube_key.set);
        assert_eq!(p.lookup_source().used.lock().unwrap().last(), Some(&None));
    }

    #[tokio::test]
    async fn the_key_never_appears_in_an_error_message() {
        let p = pipeline();
        let c = credentials("cred-leak", None, None);
        *p.lookup_source().verdict.lock().unwrap() = Some(YoutubeError::InvalidApiKey);
        let error = c.set_youtube_key(&p, GOOD).await.unwrap_err();
        assert!(!format!("{error} {error:?}").contains(GOOD));
    }
}
