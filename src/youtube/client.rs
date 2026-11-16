use std::future::Future;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};
use url::Url;

use super::error::YoutubeError;
use super::model::VideoDetails;
use super::response::{PARTS, interpret_response};
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
}

const DEFAULT_BASE_URL: &str = "https://www.googleapis.com/";
const VIDEOS_PATH: &str = "youtube/v3/videos";
const API_KEY_HEADER: &str = "x-goog-api-key";

/// A `videos.list` response is a few kilobytes; anything near this is wrong.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Client for the YouTube Data API `videos.list` method (1 quota unit per call).
///
/// The API key travels in the `x-goog-api-key` header, never in the URL, so it
/// cannot show up in URLs, error messages or logs.
#[derive(Debug, Clone)]
pub struct YoutubeClient {
    http: reqwest::Client,
    base_url: Url,
}

impl YoutubeClient {
    pub fn new(api_key: &Secret, timeout: Duration) -> Result<Self, YoutubeError> {
        let base = Url::parse(DEFAULT_BASE_URL)
            .map_err(|_| YoutubeError::BadResponse("invalid built-in base URL"))?;
        Self::with_base_url(api_key, timeout, base)
    }

    /// Like `new`, against another server (used by tests).
    pub fn with_base_url(
        api_key: &Secret,
        timeout: Duration,
        base_url: Url,
    ) -> Result<Self, YoutubeError> {
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
        if is_loopback_host(&base_url) {
            // Never send local traffic through a configured proxy.
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|_| YoutubeError::Network("cannot build HTTP client"))?;

        Ok(Self { http, base_url })
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
        let mut response = self
            .http
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
        YoutubeClient::with_base_url(&Secret::new(KEY), Duration::from_secs(5), base).unwrap()
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
            YoutubeClient::with_base_url(&Secret::new(KEY), Duration::from_millis(200), base)
                .unwrap();
        assert_eq!(
            client.fetch(&id()).await,
            Err(YoutubeError::Network("timeout"))
        );
    }

    #[test]
    fn keys_that_cannot_be_a_header_are_rejected_without_echoing_them() {
        let secret = Secret::new("bad\nkey-value");
        let err = YoutubeClient::new(&secret, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err, YoutubeError::InvalidApiKey);
        assert!(!format!("{err:?}").contains("bad"));
    }

    #[test]
    fn debug_output_of_the_client_has_no_key() {
        let c = YoutubeClient::new(&Secret::new(KEY), Duration::from_secs(1)).unwrap();
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
}
