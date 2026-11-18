//! Finding out that a newer version has been released.
//!
//! The program asks GitHub for the list of releases of this project, once when
//! it starts and then once a day (unless `check_updates = false`), and again
//! whenever the dashboard's button is pressed. It only tells: installing the new
//! version is up to the user, through the installer or the package manager.
//!
//! The request is the same one a browser makes for the releases page: nothing
//! about the user or the stream is sent, only the program's name and version as
//! the `User-Agent` GitHub asks for.

use std::cmp::Ordering;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::sync::watch;

use crate::{APP_NAME, APP_VERSION};

/// The list of releases, newest first (drafts are not in it).
const RELEASES_URL: &str =
    "https://api.github.com/repos/fhudint/stream-spectre/releases?per_page=20";

/// A release page must be one of this project's: the link is shown to the user
/// and opened, and it comes from the network.
const RELEASE_PAGE_PREFIX: &str = "https://github.com/fhudint/stream-spectre/";

/// How long to wait for GitHub.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of the answer that is read.
const MAX_ANSWER_BYTES: usize = 1024 * 1024;

/// How often the background check runs, and how long after the start it first
/// runs (so it does not compete with everything else that starts).
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const FIRST_AFTER: Duration = Duration::from_secs(15);

/// A newer version, and the page to get it from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub url: String,
}

/// What the program knows about updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub current: &'static str,
    /// The background check is on (`check_updates`).
    pub enabled: bool,
    /// A check has been made since the program started.
    pub checked: bool,
    pub latest: Option<Update>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("cannot ask GitHub: {0}")]
    Request(#[from] reqwest::Error),
    #[error("GitHub answered with status {0}")]
    Status(u16),
    #[error("the answer is too big or not what was expected")]
    BadAnswer,
}

#[derive(Debug, Default)]
struct Known {
    checked: bool,
    latest: Option<Update>,
}

pub struct Updates {
    client: reqwest::Client,
    url: String,
    enabled: bool,
    known: Mutex<Known>,
}

impl Updates {
    /// `enabled`: whether to check by itself (`check_updates`).
    pub fn new(enabled: bool) -> Result<Self, reqwest::Error> {
        Self::with_url(enabled, RELEASES_URL)
    }

    pub fn with_url(enabled: bool, url: &str) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .user_agent(format!("{APP_NAME}/{APP_VERSION}"))
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            url: url.to_owned(),
            enabled,
            known: Mutex::new(Known::default()),
        })
    }

    pub fn status(&self) -> Status {
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        Status {
            current: APP_VERSION,
            enabled: self.enabled,
            checked: known.checked,
            latest: known.latest.clone(),
        }
    }

    /// Asks GitHub now, and remembers the answer.
    pub async fn check(&self) -> Result<Status, UpdateError> {
        let response = self
            .client
            .get(&self.url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(UpdateError::Status(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ANSWER_BYTES as u64)
        {
            return Err(UpdateError::BadAnswer);
        }
        let body = response.bytes().await?;
        if body.len() > MAX_ANSWER_BYTES {
            return Err(UpdateError::BadAnswer);
        }
        let latest = newer_than(APP_VERSION, &body).ok_or(UpdateError::BadAnswer)?;
        {
            let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
            known.checked = true;
            known.latest = latest;
        }
        Ok(self.status())
    }

    /// Checks when the program starts and then every day, until `stop`. Does
    /// nothing at all when `check_updates` is off. A check that fails (no
    /// connection, GitHub busy) is only logged: the next one tries again.
    pub async fn run(&self, mut stop: watch::Receiver<bool>) {
        if !self.enabled {
            return;
        }
        let mut wait = FIRST_AFTER;
        loop {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = stop.wait_for(|stop| *stop) => return,
            }
            wait = EVERY;
            match self.check().await {
                Ok(Status {
                    latest: Some(update),
                    ..
                }) => tracing::info!(
                    version = %update.version,
                    url = %update.url,
                    "a newer version is available"
                ),
                Ok(_) => tracing::debug!("this is the latest version"),
                Err(error) => tracing::debug!(%error, "could not check for a newer version"),
            }
        }
    }
}

/// A version as the releases write it: `0.1.0`, `v0.1.0`, `0.1.0-rc1`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    numbers: [u64; 3],
    /// What follows the first `-`: `rc1`. A version with one comes before the
    /// same numbers without.
    pre: Option<String>,
}

impl Version {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim().strip_prefix('v').unwrap_or(text.trim());
        // A build suffix (`+abc`) does not count.
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut numbers = [0u64; 3];
        let mut parts = core.split('.');
        for slot in &mut numbers {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *slot = part.parse().ok()?;
        }
        parts.next().is_none().then_some(Self { numbers, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers
            .cmp(&other.numbers)
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_pre(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `rc2` before `rc10`: each dot-separated part is compared by its letters, then
/// by the number that ends it.
fn compare_pre(a: &str, b: &str) -> Ordering {
    fn split(part: &str) -> (&str, u64) {
        let letters = part.trim_end_matches(|c: char| c.is_ascii_digit());
        (letters, part[letters.len()..].parse().unwrap_or(0))
    }
    let (mut a, mut b) = (a.split('.'), b.split('.'));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = split(x).cmp(&split(y));
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The newest release that is newer than `current`, from GitHub's list.
/// `None` when the list is not one; `Some(None)` when there is nothing newer.
/// A pre-release is only offered to someone who runs one: a person on a
/// stable version is not sent to a release candidate.
fn newer_than(current: &str, body: &[u8]) -> Option<Option<Update>> {
    let current = Version::parse(current)?;
    let list: Vec<Value> = serde_json::from_slice(body).ok()?;
    let best = list
        .into_iter()
        .filter_map(|item| serde_json::from_value::<Release>(item).ok())
        .filter(|release| !release.draft)
        .filter(|release| release.html_url.starts_with(RELEASE_PAGE_PREFIX))
        .filter_map(|release| Some((Version::parse(&release.tag_name)?, release)))
        .filter(|(version, release)| {
            current.pre.is_some() || (version.pre.is_none() && !release.prerelease)
        })
        .filter(|(version, _)| *version > current)
        .max_by(|(a, _), (b, _)| a.cmp(b));
    Some(best.map(|(_, release)| Update {
        version: release.tag_name.trim_start_matches('v').to_owned(),
        url: release.html_url,
    }))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_are_read_with_or_without_a_v_and_a_suffix() {
        assert_eq!(v("0.1.0").numbers, [0, 1, 0]);
        assert_eq!(v("v1.20.3").numbers, [1, 20, 3]);
        assert_eq!(v("0.1.0-rc1").pre.as_deref(), Some("rc1"));
        assert_eq!(v("1.0.0+build5"), v("1.0.0"));
        for bad in [
            "", "v", "1", "1.2", "1.2.3.4", "a.b.c", "1.2.x", "1.2.3-", "-1.2.3", "1..3",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn versions_are_ordered_and_a_pre_release_comes_before_its_release() {
        let ordered = [
            "0.1.0-rc1",
            "0.1.0-rc2",
            "0.1.0-rc10",
            "0.1.0",
            "0.1.1",
            "0.2.0",
            "1.0.0-alpha",
            "1.0.0-beta",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
            assert!(v(pair[1]) > v(pair[0]));
        }
        assert_eq!(v("1.0.0").cmp(&v("v1.0.0")), Ordering::Equal);
        assert!(v("0.10.0") > v("0.9.9"), "numbers, not text");
    }

    fn list(items: &[(&str, bool, bool)]) -> Vec<u8> {
        let items: Vec<Value> = items
            .iter()
            .map(|(tag, prerelease, draft)| {
                json!({
                    "tag_name": tag,
                    "html_url": format!("{RELEASE_PAGE_PREFIX}releases/tag/{tag}"),
                    "prerelease": prerelease,
                    "draft": draft,
                })
            })
            .collect();
        serde_json::to_vec(&items).unwrap()
    }

    #[test]
    fn a_release_candidate_is_told_of_a_newer_candidate_and_of_the_release() {
        let body = list(&[("v0.1.0-rc2", true, false), ("v0.1.0-rc1", true, false)]);
        let update = newer_than("0.1.0-rc1", &body).unwrap().unwrap();
        assert_eq!(update.version, "0.1.0-rc2");
        assert!(update.url.ends_with("/releases/tag/v0.1.0-rc2"));

        let body = list(&[("v0.1.0", false, false), ("v0.1.0-rc2", true, false)]);
        assert_eq!(
            newer_than("0.1.0-rc1", &body).unwrap().unwrap().version,
            "0.1.0"
        );
    }

    #[test]
    fn nothing_is_offered_when_there_is_nothing_newer() {
        let body = list(&[("v0.1.0-rc1", true, false)]);
        assert_eq!(newer_than("0.1.0-rc1", &body), Some(None), "the same");
        assert_eq!(newer_than("0.2.0", &body), Some(None), "older");
        assert_eq!(
            newer_than("0.1.0", &body),
            Some(None),
            "a candidate is before its release"
        );
        assert_eq!(newer_than("0.1.0", b"[]"), Some(None));
    }

    #[test]
    fn a_stable_version_is_not_sent_to_a_pre_release_or_a_draft() {
        let body = list(&[
            ("v0.3.0", false, true),
            ("v0.2.0-rc1", true, false),
            ("v0.2.0", true, false),
            ("v0.1.5", false, false),
        ]);
        let update = newer_than("0.1.0", &body).unwrap().unwrap();
        assert_eq!(update.version, "0.1.5", "not the draft, not the candidates");
        // Someone on a candidate does see candidates, but never a draft.
        let update = newer_than("0.1.0-rc1", &body).unwrap().unwrap();
        assert_eq!(update.version, "0.2.0");
    }

    #[test]
    fn the_newest_wins_whatever_the_order_of_the_list() {
        let body = list(&[
            ("v0.1.1", false, false),
            ("v0.3.0", false, false),
            ("v0.2.0", false, false),
        ]);
        assert_eq!(
            newer_than("0.1.0", &body).unwrap().unwrap().version,
            "0.3.0"
        );
    }

    #[test]
    fn a_link_that_is_not_this_projects_is_ignored_and_a_bad_answer_is_none() {
        let body = serde_json::to_vec(&json!([
            {"tag_name": "v9.9.9", "html_url": "https://evil.example/fhudint/stream-spectre/releases/tag/v9.9.9", "prerelease": false, "draft": false},
            {"tag_name": "v9.9.8", "html_url": "https://github.com/fhudint/stream-spectre-evil/releases", "prerelease": false, "draft": false},
            {"tag_name": "v9.9.7", "prerelease": false},
            {"tag_name": "not a version", "html_url": "https://github.com/fhudint/stream-spectre/releases/tag/x"},
            "junk",
            7,
        ]))
        .unwrap();
        assert_eq!(newer_than("0.1.0", &body), Some(None));
        for bad in [
            &b"not json"[..],
            b"{}",
            b"",
            b"null",
            b"{\"message\":\"rate limited\"}",
        ] {
            assert_eq!(newer_than("0.1.0", bad), None, "{bad:?}");
        }
        assert_eq!(newer_than("garbage", b"[]"), None);
    }

    /// Answers one request with `status` and `body`, and says what was asked.
    async fn serve_once(status: u16, body: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/releases", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 4096];
            let read = socket.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let head = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
            request
        });
        (url, task)
    }

    #[tokio::test]
    async fn a_check_asks_politely_and_remembers_what_it_found() {
        let body = list(&[("v99.0.0", false, false)]);
        let (url, request) = serve_once(200, body).await;
        let updates = Updates::with_url(true, &url).unwrap();
        assert_eq!(
            updates.status(),
            Status {
                current: APP_VERSION,
                enabled: true,
                checked: false,
                latest: None
            }
        );
        let status = updates.check().await.unwrap();
        assert!(status.checked);
        assert_eq!(status.latest.as_ref().unwrap().version, "99.0.0");
        assert_eq!(updates.status(), status, "what was found is kept");

        let request = request.await.unwrap().to_ascii_lowercase();
        assert!(
            request.contains(&format!("user-agent: {APP_NAME}/{APP_VERSION}")),
            "{request}"
        );
        assert!(request.contains("accept: application/vnd.github+json"));
        assert!(!request.contains("authorization") && !request.contains("cookie"));
    }

    #[tokio::test]
    async fn a_check_that_fails_changes_nothing_and_says_why() {
        let (url, _) = serve_once(403, b"{\"message\":\"rate limit\"}".to_vec()).await;
        let updates = Updates::with_url(true, &url).unwrap();
        assert!(matches!(
            updates.check().await,
            Err(UpdateError::Status(403))
        ));
        assert!(!updates.status().checked, "not checked: it failed");

        let (url, _) = serve_once(200, b"{\"not\":\"a list\"}".to_vec()).await;
        let updates = Updates::with_url(true, &url).unwrap();
        assert!(matches!(updates.check().await, Err(UpdateError::BadAnswer)));

        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", closed.local_addr().unwrap());
        drop(closed);
        let updates = Updates::with_url(true, &url).unwrap();
        assert!(matches!(
            updates.check().await,
            Err(UpdateError::Request(_))
        ));
    }

    #[tokio::test]
    async fn the_background_check_does_nothing_when_it_is_switched_off() {
        let (url, request) = serve_once(200, list(&[("v99.0.0", false, false)])).await;
        let off = Updates::with_url(false, &url).unwrap();
        let (_stop, stopped) = watch::channel(false);
        tokio::time::timeout(Duration::from_secs(1), off.run(stopped))
            .await
            .expect("it returns at once");
        assert!(!off.status().checked);
        request.abort();
    }

    #[tokio::test]
    async fn the_background_check_ends_when_the_program_stops() {
        let (url, request) = serve_once(200, b"[]".to_vec()).await;
        let on = Updates::with_url(true, &url).unwrap();
        let (stop, stopped) = watch::channel(false);
        let task = async { on.run(stopped).await };
        let stopper = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            stop.send(true).unwrap();
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(task, stopper)
        })
        .await
        .expect("it ends before its first check");
        assert!(
            !on.status().checked,
            "it was stopped before the first check"
        );
        request.abort();
    }
}
