use std::time::{Duration, Instant};

use moka::Expiry;
use moka::future::Cache;
use reqwest::header::LOCATION;
use url::Url;

use super::ids::{ShortCode, TiktokVideoId};
use super::link::video_from_url;

/// Where short links live. The code is appended to this; nothing else of what
/// a viewer typed ever reaches the address.
const SHORT_LINK_BASE: &str = "https://vt.tiktok.com/";

/// How long to wait for TikTok's answer.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A short link leads to the same video for good, so a found video is kept
/// long. A code that leads nowhere is forgotten soon, in case it was a typo
/// of a code that works.
pub const FOUND_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const MISSING_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_ENTRIES: u64 = 1000;

/// A `Location` header longer than this is not a TikTok address.
const MAX_LOCATION_LEN: usize = 2048;

/// The video a short link leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub id: TiktokVideoId,
    /// The account that posted it, as it is in the address. Only letters,
    /// digits, `_` and `.`, so it is safe to show.
    pub account: String,
}

/// Why a short link could not be looked up (not: it leads nowhere, which is
/// `Ok(None)`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("TikTok did not answer ({0})")]
    Unavailable(&'static str),
}

struct TtlPolicy;

impl Expiry<ShortCode, Option<Resolved>> for TtlPolicy {
    fn expire_after_create(
        &self,
        _key: &ShortCode,
        value: &Option<Resolved>,
        _created_at: Instant,
    ) -> Option<Duration> {
        Some(if value.is_some() {
            FOUND_TTL
        } else {
            MISSING_TTL
        })
    }
}

/// Finds the video behind a TikTok short link, with one request to TikTok's
/// own host.
///
/// - The address is built here from a validated `ShortCode`; viewers cannot
///   point it elsewhere.
/// - Redirects are never followed. The `Location` header is read and checked
///   (https, TikTok's own host, a video path), so TikTok cannot send us on to
///   any other server either.
/// - The body is never read.
/// - Answers are cached, and concurrent lookups of one code share one request.
///   Failures are not cached.
pub struct ShortLinkResolver {
    http: reqwest::Client,
    base: Url,
    cache: Cache<ShortCode, Option<Resolved>>,
}

impl ShortLinkResolver {
    pub fn new() -> Result<Self, ResolveError> {
        match Url::parse(SHORT_LINK_BASE) {
            Ok(base) => Self::with_base_url(REQUEST_TIMEOUT, base),
            Err(_) => Err(ResolveError::Unavailable("bad address")),
        }
    }

    /// Like `new`, with another address to ask (for tests).
    pub fn with_base_url(timeout: Duration, base: Url) -> Result<Self, ResolveError> {
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(base.scheme() == "https");
        if is_loopback(&base) {
            // Never send local traffic through a configured proxy.
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|_| ResolveError::Unavailable("cannot build HTTP client"))?;
        Ok(Self {
            http,
            base,
            cache: Cache::builder()
                .max_capacity(MAX_ENTRIES)
                .expire_after(TtlPolicy)
                .build(),
        })
    }

    /// `Ok(None)`: the code leads to no video.
    pub async fn resolve(&self, code: &ShortCode) -> Result<Option<Resolved>, ResolveError> {
        match self
            .cache
            .try_get_with(code.clone(), self.fetch(code))
            .await
        {
            Ok(value) => Ok(value),
            Err(shared) => Err(ResolveError::clone(&shared)),
        }
    }

    async fn fetch(&self, code: &ShortCode) -> Result<Option<Resolved>, ResolveError> {
        let url = self
            .base
            .join(&format!("{code}/"))
            .map_err(|_| ResolveError::Unavailable("bad address"))?;
        let response = self.http.get(url).send().await.map_err(|error| {
            ResolveError::Unavailable(if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connection failed"
            } else {
                "request failed"
            })
        })?;

        let status = response.status();
        if status.is_redirection() {
            let target = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .filter(|value| value.len() <= MAX_LOCATION_LEN)
                .and_then(|value| self.base.join(value).ok());
            return Ok(target
                .as_ref()
                .and_then(video_from_url)
                .map(|(account, id)| Resolved { id, account }));
        }
        if status.as_u16() == 429 {
            return Err(ResolveError::Unavailable("rate limited"));
        }
        if status.is_server_error() {
            return Err(ResolveError::Unavailable("server error"));
        }
        // Anything else (found without a redirect, not found, refused): this
        // code does not lead to a video.
        Ok(None)
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const CODE: &str = "ZSpGELffb";
    const ID: &str = "7219950637247728923";
    const GOOD: &str =
        "https://www.tiktok.com/@feriumar758/video/7219950637247728923?_r=1&_t=ZS-9BTRBCxhj9O";

    fn code() -> ShortCode {
        ShortCode::parse(CODE).unwrap()
    }

    fn redirect(status: &str, location: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    fn plain(status: &str) -> String {
        format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// Answers each connection with the next canned response, in order, and
    /// keeps every request head it saw. After the last one it stops listening.
    async fn serve(responses: Vec<String>) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let handle = tokio::spawn(async move {
            let mut heads = Vec::new();
            for response in responses {
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
                heads.push(String::from_utf8_lossy(&head).into_owned());
                socket.write_all(response.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            }
            heads
        });
        (url, handle)
    }

    fn resolver(base: Url) -> ShortLinkResolver {
        ShortLinkResolver::with_base_url(Duration::from_secs(5), base).unwrap()
    }

    async fn resolve_once(response: String) -> Result<Option<Resolved>, ResolveError> {
        let (base, server) = serve(vec![response]).await;
        let result = resolver(base).resolve(&code()).await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn a_redirect_to_a_video_gives_its_id_and_account() {
        for status in [
            "301 Moved Permanently",
            "302 Found",
            "303 See Other",
            "307 Temporary Redirect",
            "308 Permanent Redirect",
        ] {
            let found = resolve_once(redirect(status, GOOD)).await.unwrap().unwrap();
            assert_eq!(found.id.as_str(), ID, "{status}");
            assert_eq!(found.account, "feriumar758", "{status}");
        }
    }

    #[tokio::test]
    async fn the_request_goes_to_the_code_and_nothing_else() {
        let (base, server) = serve(vec![redirect("301 Moved Permanently", GOOD)]).await;
        resolver(base).resolve(&code()).await.unwrap();
        let heads = server.await.unwrap();
        let line = heads[0].lines().next().unwrap();
        assert_eq!(line, "GET /ZSpGELffb/ HTTP/1.1");
        let lower = heads[0].to_ascii_lowercase();
        assert!(lower.contains("user-agent: stream-spectre/"), "{lower}");
        assert!(!lower.contains("cookie"), "{lower}");
        assert!(!lower.contains("authorization"), "{lower}");
    }

    #[tokio::test]
    async fn a_redirect_anywhere_else_is_not_a_video_and_is_not_followed() {
        for location in [
            "https://evil.com/@a/video/7219950637247728923",
            "https://www.tiktok.com.evil.com/@a/video/7219950637247728923",
            "http://www.tiktok.com/@a/video/7219950637247728923",
            "https://www.tiktok.com/?_r=1", // what a made-up code gets
            "https://www.tiktok.com/@a/video/12",
            "https://www.tiktok.com/@a/video/7219950637247728923/extra",
            "https://vt.tiktok.com/Other123/",
            "ftp://www.tiktok.com/@a/video/7219950637247728923",
            "javascript:alert(1)",
            "",
        ] {
            assert_eq!(
                resolve_once(redirect("302 Found", location)).await,
                Ok(None),
                "{location:?}"
            );
        }
        assert_eq!(
            resolve_once(redirect(
                "302 Found",
                &format!(
                    "https://www.tiktok.com/@a/video/7219950637247728923?{}",
                    "x".repeat(3000)
                )
            ))
            .await,
            Ok(None)
        );
        // No Location at all.
        assert_eq!(resolve_once(plain("302 Found")).await, Ok(None));
    }

    #[tokio::test]
    async fn the_other_answers_mean_no_video_or_a_failure_to_ask() {
        for status in [
            "200 OK",
            "204 No Content",
            "400 Bad Request",
            "403 Forbidden",
            "404 Not Found",
            "410 Gone",
        ] {
            assert_eq!(resolve_once(plain(status)).await, Ok(None), "{status}");
        }
        assert_eq!(
            resolve_once(plain("429 Too Many Requests")).await,
            Err(ResolveError::Unavailable("rate limited"))
        );
        for status in [
            "500 Internal Server Error",
            "502 Bad Gateway",
            "503 Service Unavailable",
        ] {
            assert_eq!(
                resolve_once(plain(status)).await,
                Err(ResolveError::Unavailable("server error")),
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn nobody_listening_and_a_slow_server_are_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        drop(listener);
        assert_eq!(
            resolver(base).resolve(&code()).await,
            Err(ResolveError::Unavailable("connection failed"))
        );

        // Accepts and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let _held = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(socket);
        });
        let slow = ShortLinkResolver::with_base_url(Duration::from_millis(100), base).unwrap();
        assert_eq!(
            slow.resolve(&code()).await,
            Err(ResolveError::Unavailable("timeout"))
        );
    }

    #[tokio::test]
    async fn a_found_video_is_asked_for_once() {
        // The server answers one connection and stops: a second request would
        // fail with "connection failed", so success proves the cache.
        let (base, server) = serve(vec![redirect("301 Moved Permanently", GOOD)]).await;
        let resolver = resolver(base);
        let first = resolver.resolve(&code()).await.unwrap().unwrap();
        server.await.unwrap();
        let again = resolver.resolve(&code()).await.unwrap().unwrap();
        assert_eq!(first, again);
        // Another code is another question.
        assert!(
            resolver
                .resolve(&ShortCode::parse("Other1234").unwrap())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_code_that_leads_nowhere_is_remembered_but_a_failure_is_not() {
        let (base, server) =
            serve(vec![redirect("302 Found", "https://www.tiktok.com/?_r=1")]).await;
        let resolver = resolver(base);
        assert_eq!(resolver.resolve(&code()).await, Ok(None));
        server.await.unwrap();
        assert_eq!(resolver.resolve(&code()).await, Ok(None), "from the cache");

        // A failure is asked again: the second answer is a success.
        let (base, server) = serve(vec![
            plain("503 Service Unavailable"),
            redirect("301 Moved Permanently", GOOD),
        ])
        .await;
        let resolver = self::resolver(base);
        assert!(resolver.resolve(&code()).await.is_err());
        assert_eq!(
            resolver
                .resolve(&code())
                .await
                .unwrap()
                .unwrap()
                .id
                .as_str(),
            ID
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn lookups_of_one_code_at_the_same_time_share_one_request() {
        let (base, server) = serve(vec![redirect("301 Moved Permanently", GOOD)]).await;
        let resolver = resolver(base);
        let code = code();
        let (a, b, c) = tokio::join!(
            resolver.resolve(&code),
            resolver.resolve(&code),
            resolver.resolve(&code)
        );
        for result in [a, b, c] {
            assert_eq!(result.unwrap().unwrap().id.as_str(), ID);
        }
        assert_eq!(server.await.unwrap().len(), 1);
    }

    #[test]
    fn the_real_address_is_tiktoks_short_link_host_over_https() {
        let url = Url::parse(SHORT_LINK_BASE).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("vt.tiktok.com"));
        assert!(ShortLinkResolver::new().is_ok());
    }
}
