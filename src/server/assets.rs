//! The overlay and dashboard pages, embedded in the binary.
//!
//! In release builds the files from `static/` are compiled in, so the program
//! is a single executable. In debug builds they are read from disk, which
//! lets you edit a page and just reload it.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "static/"]
struct Assets;

/// What the pages may load. Both talk only to this server; the overlay also
/// needs the YouTube player. Tightened or loosened as the pages require.
const OVERLAY_CSP: &str = "default-src 'self'; \
    script-src 'self' https://www.youtube.com https://s.ytimg.com; \
    frame-src https://www.youtube.com https://www.youtube-nocookie.com; \
    img-src 'self' https://i.ytimg.com data:; \
    connect-src 'self' ws: wss:; \
    frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

const DASHBOARD_CSP: &str = "default-src 'self'; \
    img-src 'self' https://i.ytimg.com data:; \
    connect-src 'self' ws: wss:; \
    frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

#[derive(Clone, Copy)]
enum Section {
    Overlay,
    Dashboard,
}

impl Section {
    fn dir(self) -> &'static str {
        match self {
            Self::Overlay => "overlay",
            Self::Dashboard => "dashboard",
        }
    }

    fn csp(self) -> &'static str {
        match self {
            Self::Overlay => OVERLAY_CSP,
            Self::Dashboard => DASHBOARD_CSP,
        }
    }
}

pub async fn overlay_index() -> Response {
    respond(Section::Overlay, "")
}

pub async fn overlay_file(Path(path): Path<String>) -> Response {
    respond(Section::Overlay, &path)
}

pub async fn dashboard_index() -> Response {
    respond(Section::Dashboard, "")
}

pub async fn dashboard_file(Path(path): Path<String>) -> Response {
    respond(Section::Dashboard, &path)
}

pub async fn to_dashboard() -> Redirect {
    Redirect::temporary("/dashboard/")
}

pub async fn to_overlay() -> Redirect {
    Redirect::temporary("/overlay/")
}

/// Safe relative path inside a section: no empty, `.` or `..` segments and no
/// backslashes. An empty path means the section's index page.
fn clean(path: &str) -> Option<String> {
    if path.is_empty() {
        return Some("index.html".to_owned());
    }
    let ok = path
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\\'));
    ok.then(|| path.to_owned())
}

fn respond(section: Section, path: &str) -> Response {
    let Some(relative) = clean(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(file) = Assets::get(&format!("{}/{relative}", section.dir())) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, file.metadata.mimetype().to_owned()),
            (header::CACHE_CONTROL, "no-cache".to_owned()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (header::REFERRER_POLICY, "no-referrer".to_owned()),
            (header::CONTENT_SECURITY_POLICY, section.csp().to_owned()),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_accepts_ordinary_relative_paths() {
        assert_eq!(clean("").as_deref(), Some("index.html"));
        assert_eq!(clean("app.js").as_deref(), Some("app.js"));
        assert_eq!(clean("css/main.css").as_deref(), Some("css/main.css"));
    }

    #[test]
    fn clean_rejects_traversal_and_odd_segments() {
        for bad in [
            "..",
            "../Cargo.toml",
            "a/../b",
            "a/./b",
            "/etc/passwd",
            "a//b",
            "a/",
            "..\\secret",
            "a\\b",
            ".",
        ] {
            assert_eq!(clean(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_embedded_index_pages_exist() {
        assert!(Assets::get("overlay/index.html").is_some());
        assert!(Assets::get("dashboard/index.html").is_some());
        assert!(Assets::get("overlay/nope.js").is_none());
    }

    #[test]
    fn page_responses_carry_security_headers() {
        let response = respond(Section::Dashboard, "");
        assert_eq!(response.status(), StatusCode::OK);
        let headers = response.headers();
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
        assert!(
            headers[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"));
        assert!(!csp.contains("youtube"), "the dashboard needs no YouTube");

        let overlay = respond(Section::Overlay, "");
        let csp = overlay.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("https://www.youtube.com"));
        assert!(!csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"));
    }

    #[test]
    fn missing_and_unsafe_paths_are_404() {
        for path in ["nope.js", "../Cargo.toml", "dir/", "a\\b"] {
            assert_eq!(
                respond(Section::Overlay, path).status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }
}
