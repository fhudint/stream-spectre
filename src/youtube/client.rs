use std::future::Future;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use url::Url;

use super::error::YoutubeError;
use super::model::VideoDetails;
use super::response::{PARTS, interpret_response};
use super::search::{MAX_RESULTS, SearchResult, ids_from_api, ids_from_page};
use super::video_id::VideoId;
use crate::config::Secret;

/// Where video details come from. The real implementation is `YoutubeClient`;
/// tests and other front ends can provide their own.
pub trait VideoSource: Send + Sync {
    /// `Ok(None)` means YouTube has no such (visible) video.
    fn fetch(
        &self,
        id: &VideoId,
    ) -> impl Future<Output = Result<Option<VideoDetails>, YoutubeError>> + Send;

    /// Looks for a video by words: YouTube's own search page first, which
    /// costs no quota, and the Data API (100 quota units) only when
    /// `official_allowed` and the page did not give an answer. Sources that
    /// cannot search find nothing.
    fn search(
        &self,
        _query: &str,
        _official_allowed: bool,
    ) -> impl Future<Output = Result<SearchResult, YoutubeError>> + Send {
        async { Ok(SearchResult::default()) }
    }

    /// Uses another API key from now on (`None`: no key). Sources that have
    /// no key ignore this. On an error nothing changes.
    fn set_api_key(&self, _key: Option<&Secret>) -> Result<(), YoutubeError> {
        Ok(())
    }

    /// Tries `key` with one real request (1 quota unit) without switching to
    /// it. `Ok` means YouTube accepted it. Sources that have no key accept
    /// anything.
    fn check_api_key(
        &self,
        _key: &Secret,
    ) -> impl Future<Output = Result<(), YoutubeError>> + Send {
        async { Ok(()) }
    }
}

/// A video that is certain to exist, for trying a key.
const CHECK_VIDEO_ID: &str = "dQw4w9WgXcQ";

const DEFAULT_BASE_URL: &str = "https://www.googleapis.com/";
const VIDEOS_PATH: &str = "youtube/v3/videos";
const SEARCH_PATH: &str = "youtube/v3/search";
const API_KEY_HEADER: &str = "x-goog-api-key";

/// YouTube's own search, asked the way its web page asks it. No key, no quota.
const PAGE_SEARCH_URL: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";
/// Which version of its web page the question says it comes from.
const PAGE_CLIENT_VERSION: &str = "2.20240101.00.00";
/// The answer of the search page is a few hundred kilobytes.
const MAX_PAGE_BYTES: usize = 3 * 1024 * 1024;

/// A `videos.list` response is a few kilobytes; anything near this is wrong.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Client for the YouTube Data API `videos.list` method (1 quota unit per call).
///
/// The API key travels in the `x-goog-api-key` header, never in the URL, so it
/// cannot show up in URLs, error messages or logs.
#[derive(Debug, Clone)]
pub struct YoutubeClient {
    /// `None` while there is no key. Shared by every clone, so a key set on
    /// one is used by all.
    http: Arc<RwLock<Option<reqwest::Client>>>,
    base_url: Url,
    timeout: Duration,
    /// For YouTube's own search page; carries no key.
    page: reqwest::Client,
    page_url: Url,
}

impl YoutubeClient {
    /// A client with `api_key`, or without one (every lookup then fails with
    /// `NoApiKey` until `set_api_key` gives it one).
    pub fn new(api_key: Option<&Secret>, timeout: Duration) -> Result<Self, YoutubeError> {
        let base = Url::parse(DEFAULT_BASE_URL)
            .map_err(|_| YoutubeError::BadResponse("invalid built-in base URL"))?;
        Self::with_base_url(api_key, timeout, base)
    }

    /// Like `new`, against another server (used by tests).
    pub fn with_base_url(
        api_key: Option<&Secret>,
        timeout: Duration,
        base_url: Url,
    ) -> Result<Self, YoutubeError> {
        let http = api_key
            .map(|key| build_http(key, timeout, &base_url))
            .transpose()?;
        let page_url = Url::parse(PAGE_SEARCH_URL)
            .map_err(|_| YoutubeError::BadResponse("invalid built-in search URL"))?;
        let page = build_page_http(timeout, &page_url)?;
        Ok(Self {
            http: Arc::new(RwLock::new(http)),
            base_url,
            timeout,
            page,
            page_url,
        })
    }

    /// Asks another address than YouTube's for the search page (for tests).
    #[cfg(test)]
    fn with_page_url(mut self, page_url: Url) -> Result<Self, YoutubeError> {
        self.page = build_page_http(self.timeout, &page_url)?;
        self.page_url = page_url;
        Ok(self)
    }

    fn current_http(&self) -> Option<reqwest::Client> {
        self.http
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn request_url(&self, id: &VideoId) -> Url {
        let mut url = self.base_url.clone();
        url.set_path(VIDEOS_PATH);
        url.query_pairs_mut()
            .clear()
            .append_pair("part", PARTS)
            .append_pair("id", id.as_str());
        url
    }

    async fn fetch_via(
        &self,
        http: &reqwest::Client,
        id: &VideoId,
    ) -> Result<Option<VideoDetails>, YoutubeError> {
        let mut response = http
            .get(self.request_url(id))
            .send()
            .await
            .map_err(|e| describe(&e))?;
        let status = response.status().as_u16();

        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| describe(&e))? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(YoutubeError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }

        interpret_response(id, status, &body)
    }

    /// The answer to `body` from `http` as bytes, and the status, refusing one
    /// that is too big.
    async fn read(
        response: reqwest::Response,
        limit: usize,
    ) -> Result<(u16, Vec<u8>), YoutubeError> {
        let mut response = response;
        let status = response.status().as_u16();
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| describe(&e))? {
            if body.len() + chunk.len() > limit {
                return Err(YoutubeError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    }

    /// YouTube's own search. `Ok(None)`: the answer is not what is expected.
    async fn search_page(&self, query: &str) -> Result<Option<Vec<VideoId>>, YoutubeError> {
        let question = serde_json::json!({
            "context": {"client": {
                "clientName": "WEB",
                "clientVersion": PAGE_CLIENT_VERSION,
                "hl": "en",
                "gl": "US",
            }},
            "query": query,
        });
        let question = serde_json::to_vec(&question)
            .map_err(|_| YoutubeError::BadResponse("cannot build the search question"))?;
        let response = self
            .page
            .post(self.page_url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(question)
            .send()
            .await
            .map_err(|e| describe(&e))?;
        let (status, body) = Self::read(response, MAX_PAGE_BYTES).await?;
        if status != 200 {
            return Ok(None);
        }
        Ok(ids_from_page(&body))
    }

    /// The Data API's `search.list`: 100 quota units.
    async fn search_api(
        &self,
        http: &reqwest::Client,
        query: &str,
    ) -> Result<Vec<VideoId>, YoutubeError> {
        let mut url = self.base_url.clone();
        url.set_path(SEARCH_PATH);
        url.query_pairs_mut()
            .clear()
            .append_pair("part", "id")
            .append_pair("type", "video")
            .append_pair("videoEmbeddable", "true")
            .append_pair("maxResults", &MAX_RESULTS.to_string())
            .append_pair("q", query);
        let response = http.get(url).send().await.map_err(|e| describe(&e))?;
        let (status, body) = Self::read(response, MAX_BODY_BYTES).await?;
        ids_from_api(status, &body)
    }
}

/// An HTTP client for YouTube's search page: no key, nothing sent along.
fn build_page_http(timeout: Duration, url: &Url) -> Result<reqwest::Client, YoutubeError> {
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .redirect(reqwest::redirect::Policy::none())
        .https_only(url.scheme() == "https");
    if is_loopback_host(url) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|_| YoutubeError::Network("cannot build HTTP client"))
}

/// An HTTP client that sends `api_key` with every request.
fn build_http(
    api_key: &Secret,
    timeout: Duration,
    base_url: &Url,
) -> Result<reqwest::Client, YoutubeError> {
    let mut key =
        HeaderValue::from_str(api_key.expose()).map_err(|_| YoutubeError::InvalidApiKey)?;
    key.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(API_KEY_HEADER, key);

    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!(
            env!("CARGO_PKG_NAME"),
            "/",
            env!("CARGO_PKG_VERSION")
        ))
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .https_only(base_url.scheme() == "https");
    if is_loopback_host(base_url) {
        // Never send local traffic through a configured proxy.
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|_| YoutubeError::Network("cannot build HTTP client"))
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Describes a transport failure without the URL or the error chain.
fn describe(error: &reqwest::Error) -> YoutubeError {
    if error.is_timeout() {
        YoutubeError::Network("timeout")
    } else if error.is_connect() {
        YoutubeError::Network("connection failed")
    } else {
        YoutubeError::Network("request failed")
    }
}

impl VideoSource for YoutubeClient {
    async fn fetch(&self, id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
        let Some(http) = self.current_http() else {
            return Err(YoutubeError::NoApiKey);
        };
        self.fetch_via(&http, id).await
    }

    async fn search(
        &self,
        query: &str,
        official_allowed: bool,
    ) -> Result<SearchResult, YoutubeError> {
        match self.search_page(query).await {
            Ok(Some(ids)) => {
                return Ok(SearchResult {
                    ids,
                    official: false,
                });
            }
            Ok(None) => tracing::warn!("YouTube's search page did not answer as expected"),
            Err(error) => tracing::debug!(%error, "YouTube's search page could not be read"),
        }
        if !official_allowed {
            return Err(YoutubeError::SearchUnavailable);
        }
        let Some(http) = self.current_http() else {
            return Err(YoutubeError::NoApiKey);
        };
        let ids = self.search_api(&http, query).await?;
        Ok(SearchResult {
            ids,
            official: true,
        })
    }

    fn set_api_key(&self, key: Option<&Secret>) -> Result<(), YoutubeError> {
        // Built first: a key that cannot be used leaves the old one in place.
        let http = key
            .map(|key| build_http(key, self.timeout, &self.base_url))
            .transpose()?;
        *self.http.write().unwrap_or_else(PoisonError::into_inner) = http;
        Ok(())
    }

    async fn check_api_key(&self, key: &Secret) -> Result<(), YoutubeError> {
        let http = build_http(key, self.timeout, &self.base_url)?;
        let id = VideoId::parse(CHECK_VIDEO_ID)
            .map_err(|_| YoutubeError::BadResponse("invalid built-in video ID"))?;
        // Whether the video is found hardly matters: the key was accepted.
        self.fetch_via(&http, &id).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const ID: &str = "dQw4w9WgXcQ";
    const KEY: &str = "AIzaSy-very-secret-test-key";

    fn id() -> VideoId {
        VideoId::parse(ID).unwrap()
    }

    fn ok_json() -> String {
        format!(
            r#"{{"items":[{{"id":"{ID}","snippet":{{"title":"T","channelTitle":"C"}},
               "contentDetails":{{"duration":"PT3M"}},
               "status":{{"privacyStatus":"public","embeddable":true}}}}]}}"#
        )
    }

    /// Serves one canned HTTP response and returns the request head it saw.
    async fn serve_once(
        status_line: &'static str,
        body: String,
    ) -> (Url, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
            }
            let response = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            let _ = socket.shutdown().await;
            String::from_utf8_lossy(&head).into_owned()
        });
        (url, handle)
    }

    fn client(base: Url) -> YoutubeClient {
        YoutubeClient::with_base_url(Some(&Secret::new(KEY)), Duration::from_secs(5), base).unwrap()
    }

    #[tokio::test]
    async fn fetches_and_parses_a_video() {
        let (base, server) = serve_once("200 OK", ok_json()).await;
        let details = client(base).fetch(&id()).await.unwrap().unwrap();
        assert_eq!(details.title, "T");
        assert_eq!(details.duration_secs, 180);

        let head = server.await.unwrap();
        let request_line = head.lines().next().unwrap();
        assert!(
            request_line.starts_with(
                "GET /youtube/v3/videos?part=snippet%2CcontentDetails%2Cstatus&id=dQw4w9WgXcQ "
            ),
            "{request_line}"
        );
    }

    #[tokio::test]
    async fn the_api_key_is_sent_as_a_header_and_never_in_the_url() {
        let (base, server) = serve_once("200 OK", ok_json()).await;
        client(base).fetch(&id()).await.unwrap();
        let head = server.await.unwrap();
        let request_line = head.lines().next().unwrap();
        assert!(
            !request_line.contains(KEY),
            "key leaked into URL: {request_line}"
        );
        assert!(!request_line.contains("key="), "{request_line}");
        let header_line = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("x-goog-api-key:"))
            .expect("api key header");
        assert!(header_line.ends_with(KEY));
    }

    fn header_of(head: &str, name: &str) -> Option<String> {
        head.lines()
            .find(|l| l.to_ascii_lowercase().starts_with(&format!("{name}:")))
            .map(|l| l.split_once(':').unwrap().1.trim().to_owned())
    }

    #[tokio::test]
    async fn without_a_key_nothing_is_sent_and_the_error_says_so() {
        // Nothing listens here: a request would fail with a network error.
        let base = Url::parse("http://127.0.0.1:1/").unwrap();
        let client = YoutubeClient::with_base_url(None, Duration::from_secs(1), base).unwrap();
        assert_eq!(client.fetch(&id()).await, Err(YoutubeError::NoApiKey));
        assert!(YoutubeError::NoApiKey.is_config_problem());
        assert_eq!(YoutubeError::NoApiKey.notice_key(), "error.no_api_key");
    }

    #[tokio::test]
    async fn a_key_set_later_is_used_by_every_clone() {
        let (base, server) = serve_once("200 OK", ok_json()).await;
        let client = YoutubeClient::with_base_url(None, Duration::from_secs(5), base).unwrap();
        let clone = client.clone();
        client.set_api_key(Some(&Secret::new(KEY))).unwrap();
        clone.fetch(&id()).await.unwrap();
        let head = server.await.unwrap();
        assert_eq!(header_of(&head, "x-goog-api-key").as_deref(), Some(KEY));
    }

    #[tokio::test]
    async fn a_replaced_key_is_the_one_sent_and_removing_it_stops_requests() {
        let (base, server) = serve_once("200 OK", ok_json()).await;
        let client = client(base);
        client
            .set_api_key(Some(&Secret::new("AIzaSy-the-new-key")))
            .unwrap();
        client.fetch(&id()).await.unwrap();
        let head = server.await.unwrap();
        assert_eq!(
            header_of(&head, "x-goog-api-key").as_deref(),
            Some("AIzaSy-the-new-key")
        );
        assert!(!head.contains(KEY), "the old key is no longer sent");

        client.set_api_key(None).unwrap();
        assert_eq!(client.fetch(&id()).await, Err(YoutubeError::NoApiKey));
    }

    #[tokio::test]
    async fn a_key_that_cannot_be_used_changes_nothing() {
        let (base, server) = serve_once("200 OK", ok_json()).await;
        let client = client(base);
        for bad in ["bad\nkey", "bad\r\nkey", "k\u{e9}y\u{0}"] {
            assert_eq!(
                client.set_api_key(Some(&Secret::new(bad))),
                Err(YoutubeError::InvalidApiKey),
                "{bad:?}"
            );
        }
        client.fetch(&id()).await.unwrap();
        let head = server.await.unwrap();
        assert_eq!(header_of(&head, "x-goog-api-key").as_deref(), Some(KEY));
    }

    #[tokio::test]
    async fn trying_a_key_sends_that_key_and_does_not_switch_to_it() {
        let (base, server) = serve_once("200 OK", ok_json()).await;
        let client = YoutubeClient::with_base_url(None, Duration::from_secs(5), base).unwrap();
        client
            .check_api_key(&Secret::new("AIzaSy-candidate"))
            .await
            .unwrap();
        let head = server.await.unwrap();
        assert_eq!(
            header_of(&head, "x-goog-api-key").as_deref(),
            Some("AIzaSy-candidate")
        );
        // It was only tried: there is still no key in use.
        assert_eq!(client.fetch(&id()).await, Err(YoutubeError::NoApiKey));
    }

    #[tokio::test]
    async fn a_key_is_accepted_when_the_video_is_not_found_and_refused_when_youtube_refuses_it() {
        let (base, _s) = serve_once("200 OK", r#"{"items":[]}"#.to_owned()).await;
        let client1 = YoutubeClient::with_base_url(None, Duration::from_secs(5), base).unwrap();
        assert_eq!(client1.check_api_key(&Secret::new(KEY)).await, Ok(()));

        let body = r#"{"error":{"code":400,"errors":[{"reason":"badRequest"}],"details":[{"reason":"API_KEY_INVALID"}]}}"#.to_owned();
        let (base, _s) = serve_once("400 Bad Request", body).await;
        let client2 = YoutubeClient::with_base_url(None, Duration::from_secs(5), base).unwrap();
        assert_eq!(
            client2.check_api_key(&Secret::new(KEY)).await,
            Err(YoutubeError::InvalidApiKey)
        );

        let body = r#"{"error":{"code":403,"errors":[{"reason":"quotaExceeded"}]}}"#.to_owned();
        let (base, _s) = serve_once("403 Forbidden", body).await;
        let client3 = YoutubeClient::with_base_url(None, Duration::from_secs(5), base).unwrap();
        assert_eq!(
            client3.check_api_key(&Secret::new(KEY)).await,
            Err(YoutubeError::QuotaExceeded)
        );
        // A key that cannot even be put in a header is refused without a request.
        let client4 = YoutubeClient::new(None, Duration::from_secs(1)).unwrap();
        assert_eq!(
            client4.check_api_key(&Secret::new("a\nb")).await,
            Err(YoutubeError::InvalidApiKey)
        );
    }

    #[tokio::test]
    async fn quota_error_is_mapped() {
        let body = r#"{"error":{"code":403,"errors":[{"reason":"quotaExceeded"}]}}"#.to_owned();
        let (base, _server) = serve_once("403 Forbidden", body).await;
        assert_eq!(
            client(base).fetch(&id()).await,
            Err(YoutubeError::QuotaExceeded)
        );
    }

    #[tokio::test]
    async fn unknown_video_is_none() {
        let (base, _server) = serve_once("200 OK", r#"{"items":[]}"#.to_owned()).await;
        assert_eq!(client(base).fetch(&id()).await, Ok(None));
    }

    #[tokio::test]
    async fn oversized_responses_are_refused() {
        let body = "x".repeat(MAX_BODY_BYTES + 1);
        let (base, _server) = serve_once("200 OK", body).await;
        assert_eq!(
            client(base).fetch(&id()).await,
            Err(YoutubeError::ResponseTooLarge)
        );
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let (base, _server) = serve_once("302 Found", String::new()).await;
        let err = client(base).fetch(&id()).await.unwrap_err();
        assert!(
            matches!(err, YoutubeError::Rejected { status: 302, .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn connection_errors_do_not_leak_the_url_or_key() {
        // Bind then drop, so the port is closed.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        drop(listener);

        let err = client(base).fetch(&id()).await.unwrap_err();
        assert_eq!(err, YoutubeError::Network("connection failed"));
        let text = format!("{err} {err:?}");
        assert!(!text.contains(KEY) && !text.contains("127.0.0.1"), "{text}");
    }

    #[tokio::test]
    async fn a_silent_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let _hold = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let client =
            YoutubeClient::with_base_url(Some(&Secret::new(KEY)), Duration::from_millis(200), base)
                .unwrap();
        assert_eq!(
            client.fetch(&id()).await,
            Err(YoutubeError::Network("timeout"))
        );
    }

    #[test]
    fn keys_that_cannot_be_a_header_are_rejected_without_echoing_them() {
        let secret = Secret::new("bad\nkey-value");
        let err = YoutubeClient::new(Some(&secret), Duration::from_secs(1)).unwrap_err();
        assert_eq!(err, YoutubeError::InvalidApiKey);
        assert!(!format!("{err:?}").contains("bad"));
    }

    #[test]
    fn debug_output_of_the_client_has_no_key() {
        let c = YoutubeClient::new(Some(&Secret::new(KEY)), Duration::from_secs(1)).unwrap();
        assert!(!format!("{c:?}").contains(KEY));
    }

    #[test]
    fn loopback_detection() {
        for (url, expected) in [
            ("http://127.0.0.1:1/", true),
            ("http://localhost:1/", true),
            ("http://[::1]:1/", true),
            ("https://www.googleapis.com/", false),
            ("http://10.0.0.5/", false),
        ] {
            assert_eq!(
                is_loopback_host(&Url::parse(url).unwrap()),
                expected,
                "{url}"
            );
        }
    }

    // --- searching by words ---

    const FOUND: &str = "aaaaaaaaaaa";

    fn page_with(id: &str) -> String {
        format!(r#"{{"contents":{{"list":[{{"videoRenderer":{{"videoId":"{id}"}}}}]}}}}"#)
    }

    fn api_with(id: &str) -> String {
        format!(r#"{{"items":[{{"id":{{"kind":"youtube#video","videoId":"{id}"}}}}]}}"#)
    }

    #[tokio::test]
    async fn a_search_asks_the_page_first_with_no_key_and_no_quota() {
        let (page, server) = serve_once("200 OK", page_with(FOUND)).await;
        // Nothing listens on the API address: asking it would fail.
        let api = Url::parse("http://127.0.0.1:1/").unwrap();
        let client = client(api).with_page_url(page).unwrap();
        let found = client.search("dirimu melodi", true).await.unwrap();
        assert_eq!(found.ids, vec![VideoId::parse(FOUND).unwrap()]);
        assert!(!found.official);
        let head = server.await.unwrap();
        assert!(head.starts_with("POST / "), "{head}");
        assert_eq!(
            header_of(&head, "content-type").as_deref(),
            Some("application/json")
        );
        assert!(
            header_of(&head, "x-goog-api-key").is_none(),
            "the page gets no key"
        );
    }

    #[tokio::test]
    async fn when_the_page_fails_the_api_is_used_if_allowed() {
        let (page, _page) = serve_once("200 OK", "{}".to_owned()).await;
        let (api, server) = serve_once("200 OK", api_with(FOUND)).await;
        let client = client(api).with_page_url(page).unwrap();
        let found = client.search("dirimu melodi", true).await.unwrap();
        assert_eq!(found.ids.len(), 1);
        assert!(
            found.official,
            "the caller counts it against the day's limit"
        );
        let head = server.await.unwrap();
        let request_line = head.lines().next().unwrap();
        assert!(
            request_line.starts_with(
                "GET /youtube/v3/search?part=id&type=video&videoEmbeddable=true&maxResults=5&q=dirimu+melodi "
            ),
            "{request_line}"
        );
        assert_eq!(header_of(&head, "x-goog-api-key").as_deref(), Some(KEY));
        assert!(!request_line.contains(KEY));
    }

    #[tokio::test]
    async fn a_page_that_found_nothing_is_an_answer_and_does_not_use_the_api() {
        let (page, _page) = serve_once("200 OK", r#"{"contents":{}}"#.to_owned()).await;
        let api = Url::parse("http://127.0.0.1:1/").unwrap();
        let found = client(api)
            .with_page_url(page)
            .unwrap()
            .search("zzzz qqqq", true)
            .await
            .unwrap();
        assert!(found.ids.is_empty() && !found.official);
    }

    #[tokio::test]
    async fn without_the_page_the_api_is_not_used_beyond_the_limit_or_without_a_key() {
        let down = Url::parse("http://127.0.0.1:1/").unwrap();
        let limited = client(down.clone()).with_page_url(down.clone()).unwrap();
        assert_eq!(
            limited.search("x", false).await,
            Err(YoutubeError::SearchUnavailable)
        );
        let keyless = YoutubeClient::with_base_url(None, Duration::from_secs(1), down.clone())
            .unwrap()
            .with_page_url(down)
            .unwrap();
        assert_eq!(keyless.search("x", true).await, Err(YoutubeError::NoApiKey));
    }

    #[tokio::test]
    async fn the_api_saying_the_quota_is_used_up_is_an_error_to_pass_on() {
        let (page, _page) = serve_once("500 Internal Server Error", "{}".to_owned()).await;
        let (api, _api) = serve_once(
            "403 Forbidden",
            r#"{"error":{"errors":[{"reason":"quotaExceeded"}]}}"#.to_owned(),
        )
        .await;
        let result = client(api)
            .with_page_url(page)
            .unwrap()
            .search("x", true)
            .await;
        assert_eq!(result, Err(YoutubeError::QuotaExceeded));
    }
}
