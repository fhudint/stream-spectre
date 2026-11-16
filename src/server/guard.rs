//! Who may talk to the server.
//!
//! Three independent checks, applied to every request:
//!
//! 1. **Host** (DNS rebinding): when the server listens on a loopback
//!    address, the `Host` header must be a loopback name with our port. A web
//!    page on another domain that tricks the browser into resolving its name
//!    to 127.0.0.1 therefore gets nothing.
//! 2. **Origin** (cross-site requests): if the browser sends an `Origin`, it
//!    must be this very server. Requests without `Origin` (curl, scripts) pass
//!    this check.
//! 3. **Token**: when `DASHBOARD_TOKEN` is set, the API and the WebSockets
//!    need it, as `Authorization: Bearer <token>` or a `token` query
//!    parameter (browsers cannot set headers on WebSockets). The static pages
//!    contain nothing secret and need no token.

use std::net::IpAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;
use url::Url;

use crate::config::Secret;

pub struct Guard {
    token: Option<Secret>,
    /// Accepted `Host` values (lowercase `name:port`); `None` accepts any.
    allowed_hosts: Option<Vec<String>>,
}

impl Guard {
    /// `bind` and `port` are the address the server actually listens on.
    pub fn new(token: Option<&Secret>, bind: IpAddr, port: u16) -> Self {
        let allowed_hosts = bind.is_loopback().then(|| {
            ["127.0.0.1", "localhost", "[::1]"]
                .iter()
                .map(|name| format!("{name}:{port}"))
                .collect()
        });
        Self {
            token: token.cloned(),
            allowed_hosts,
        }
    }

    pub fn host_allowed(&self, host: Option<&str>) -> bool {
        match &self.allowed_hosts {
            None => true,
            Some(allowed) => host.is_some_and(|host| {
                let host = host.to_ascii_lowercase();
                allowed.contains(&host)
            }),
        }
    }

    /// A browser-supplied `Origin` must name this server (same host and port
    /// as the `Host` header). Absent is fine; `null` is not.
    pub fn origin_allowed(origin: Option<&str>, host: Option<&str>) -> bool {
        let Some(origin) = origin else {
            return true;
        };
        let (Some(host), Ok(url)) = (host, Url::parse(origin)) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        let Some(name) = url.host_str() else {
            return false;
        };
        let authority = match url.port() {
            Some(port) => format!("{name}:{port}"),
            None => name.to_owned(),
        };
        authority.eq_ignore_ascii_case(host)
    }

    pub fn token_required(&self) -> bool {
        self.token.is_some()
    }

    /// With no token configured everything passes; otherwise the provided
    /// token must match, compared in constant time.
    pub fn token_allowed(&self, provided: Option<&str>) -> bool {
        match &self.token {
            None => true,
            Some(expected) => provided.is_some_and(|provided| {
                provided
                    .as_bytes()
                    .ct_eq(expected.expose().as_bytes())
                    .into()
            }),
        }
    }
}

/// Paths that carry data or actions, as opposed to the static pages.
pub fn needs_token(path: &str) -> bool {
    path.starts_with("/api/") || path.starts_with("/ws/")
}

/// The token from `Authorization: Bearer ...` or the `token` query parameter.
pub fn provided_token(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    let from_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let (scheme, token) = value.split_once(' ')?;
            scheme
                .eq_ignore_ascii_case("bearer")
                .then(|| token.trim().to_owned())
        });
    from_header.or_else(|| {
        url::form_urlencoded::parse(query?.as_bytes())
            .find(|(key, _)| key == "token")
            .map(|(_, value)| value.into_owned())
    })
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn refuse(status: StatusCode, message: &'static str) -> Response {
    let body = format!("{{\"error\":\"{message}\"}}");
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Bearer"),
        );
    }
    response
}

/// Middleware applying the checks above to every request.
pub async fn gate(State(guard): State<Arc<Guard>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    if !guard.host_allowed(header_str(headers, header::HOST)) {
        return refuse(StatusCode::FORBIDDEN, "host not allowed");
    }
    if !Guard::origin_allowed(
        header_str(headers, header::ORIGIN),
        header_str(headers, header::HOST),
    ) {
        return refuse(StatusCode::FORBIDDEN, "origin not allowed");
    }
    if guard.token_required() && needs_token(request.uri().path()) {
        let provided = provided_token(headers, request.uri().query());
        if !guard.token_allowed(provided.as_deref()) {
            return refuse(StatusCode::UNAUTHORIZED, "missing or wrong token");
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn loopback_guard(token: Option<&str>) -> Guard {
        let secret = token.map(Secret::new);
        Guard::new(secret.as_ref(), "127.0.0.1".parse().unwrap(), 8787)
    }

    #[test]
    fn loopback_servers_only_accept_loopback_host_names_with_their_port() {
        let g = loopback_guard(None);
        for ok in [
            "127.0.0.1:8787",
            "localhost:8787",
            "[::1]:8787",
            "LocalHost:8787",
        ] {
            assert!(g.host_allowed(Some(ok)), "{ok}");
        }
        for bad in [
            "evil.example",
            "evil.example:8787",
            "127.0.0.1",
            "127.0.0.1:9999",
            "localhost.evil.example:8787",
            "127.0.0.1:8787.evil.example",
            "",
        ] {
            assert!(!g.host_allowed(Some(bad)), "{bad}");
        }
        assert!(!g.host_allowed(None));
    }

    #[test]
    fn network_servers_accept_any_host() {
        let g = Guard::new(None, "0.0.0.0".parse().unwrap(), 8787);
        assert!(g.host_allowed(Some("192.168.1.20:8787")));
        assert!(g.host_allowed(Some("my-pc.local:8787")));
        assert!(g.host_allowed(None));
    }

    #[test]
    fn ipv6_loopback_counts_as_loopback() {
        let g = Guard::new(None, "::1".parse().unwrap(), 9000);
        assert!(g.host_allowed(Some("[::1]:9000")));
        assert!(!g.host_allowed(Some("example.com:9000")));
    }

    #[test]
    fn origin_must_be_this_server_when_present() {
        let host = Some("127.0.0.1:8787");
        assert!(Guard::origin_allowed(None, host));
        assert!(Guard::origin_allowed(Some("http://127.0.0.1:8787"), host));
        assert!(Guard::origin_allowed(Some("HTTP://127.0.0.1:8787"), host));
        assert!(Guard::origin_allowed(
            Some("https://example.com"),
            Some("example.com")
        ));
        for bad in [
            "http://evil.example",
            "http://evil.example:8787",
            "http://127.0.0.1:9999",
            "http://127.0.0.1",
            "null",
            "",
            "file://",
            "ftp://127.0.0.1:8787",
            "http://127.0.0.1:8787.evil.example",
            "not a url",
        ] {
            assert!(!Guard::origin_allowed(Some(bad), host), "{bad}");
        }
        assert!(!Guard::origin_allowed(Some("http://127.0.0.1:8787"), None));
    }

    #[test]
    fn no_token_configured_means_open() {
        let g = loopback_guard(None);
        assert!(!g.token_required());
        assert!(g.token_allowed(None));
        assert!(g.token_allowed(Some("anything")));
    }

    #[test]
    fn a_configured_token_must_match_exactly() {
        let g = loopback_guard(Some("correct-horse-battery"));
        assert!(g.token_required());
        assert!(g.token_allowed(Some("correct-horse-battery")));
        for bad in [
            "",
            "correct-horse-batter",
            "correct-horse-batteryy",
            "CORRECT-HORSE-BATTERY",
        ] {
            assert!(!g.token_allowed(Some(bad)), "{bad}");
        }
        assert!(!g.token_allowed(None));
    }

    #[test]
    fn only_the_api_and_websockets_need_a_token() {
        for path in [
            "/api/state",
            "/api/queue/1/approve",
            "/ws/overlay",
            "/ws/dashboard",
        ] {
            assert!(needs_token(path), "{path}");
        }
        for path in [
            "/",
            "/dashboard/",
            "/overlay/",
            "/overlay/app.js",
            "/favicon.ico",
            "/apix",
            "/wsx/a",
        ] {
            assert!(!needs_token(path), "{path}");
        }
    }

    #[test]
    fn the_token_comes_from_the_bearer_header_or_the_query() {
        let mut headers = HeaderMap::new();
        assert_eq!(provided_token(&headers, None), None);
        assert_eq!(provided_token(&headers, Some("a=1")), None);
        assert_eq!(
            provided_token(&headers, Some("a=1&token=abc%21&b=2")).as_deref(),
            Some("abc!")
        );

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer xyz"),
        );
        assert_eq!(provided_token(&headers, None).as_deref(), Some("xyz"));
        // The header wins over the query.
        assert_eq!(
            provided_token(&headers, Some("token=other")).as_deref(),
            Some("xyz")
        );

        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer   spaced "),
        );
        assert_eq!(provided_token(&headers, None).as_deref(), Some("spaced"));

        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(provided_token(&headers, None), None);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer"));
        assert_eq!(provided_token(&headers, None), None);
    }
}
