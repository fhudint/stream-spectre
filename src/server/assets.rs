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
/// needs the YouTube player and TikTok's embedded player (a frame, nothing
/// more). Tightened or loosened as the pages require.
const OVERLAY_CSP: &str = "default-src 'self'; \
    script-src 'self' https://www.youtube.com https://s.ytimg.com; \
    frame-src https://www.youtube.com https://www.youtube-nocookie.com https://www.tiktok.com; \
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

    /// YouTube's embedded player needs to know which site embeds it, so the
    /// overlay sends other sites its origin only (never the path or query,
    /// where the token may be). The dashboard sends nothing at all.
    fn referrer_policy(self) -> &'static str {
        match self {
            Self::Overlay => "strict-origin-when-cross-origin",
            Self::Dashboard => "no-referrer",
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

/// Browsers ask for `/favicon.ico` by themselves, whatever the page says.
/// Send them to the dashboard's icon.
pub async fn favicon() -> Redirect {
    Redirect::temporary("/dashboard/favicon-32.png")
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
            (
                header::REFERRER_POLICY,
                section.referrer_policy().to_owned(),
            ),
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
        assert!(!csp.contains("tiktok"), "the dashboard needs no TikTok");

        let overlay = respond(Section::Overlay, "");
        assert_eq!(
            overlay.headers()[header::REFERRER_POLICY],
            "strict-origin-when-cross-origin"
        );
        let csp = overlay.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("https://www.youtube.com"));
        // TikTok is only a frame: no scripts or images are loaded from it.
        assert!(csp.contains("frame-src") && csp.contains("https://www.tiktok.com"));
        let script_src = csp
            .split(';')
            .find(|d| d.trim().starts_with("script-src"))
            .unwrap();
        assert!(!script_src.contains("tiktok"), "{script_src}");
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

    fn page_source(section: &str) -> String {
        let file = Assets::get(&format!("{section}/index.html")).expect("index.html");
        String::from_utf8(file.data.into_owned()).unwrap()
    }

    /// The pages run under a strict Content-Security-Policy (no inline script
    /// or style), so a page that breaks the rules would silently do nothing.
    fn assert_csp_clean(section: &str) {
        use regex::Regex;

        let html = page_source(section);
        let script = Regex::new(r"(?is)<script\b([^>]*)>(.*?)</script>").unwrap();
        for caps in script.captures_iter(&html) {
            assert!(
                caps[1].contains("src="),
                "{section}: script tag without src"
            );
            assert!(caps[2].trim().is_empty(), "{section}: inline script code");
        }
        let forbidden = [
            (r"(?i)<style\b", "style block"),
            (r"(?i)\sstyle\s*=", "style attribute"),
            (r"(?i)\son[a-z]+\s*=", "inline event handler"),
            (r"(?i)javascript:", "javascript: URL"),
        ];
        for (pattern, what) in forbidden {
            assert!(
                !Regex::new(pattern).unwrap().is_match(&html),
                "{section}: {what}"
            );
        }
        let reference = Regex::new(r#"(?i)\b(?:src|href)\s*=\s*"([^"]+)""#).unwrap();
        for caps in reference.captures_iter(&html) {
            let target = &caps[1];
            // A link to a place on the same page is not a file.
            if target.starts_with('#') {
                continue;
            }
            assert!(
                !target.contains("://") && !target.starts_with("//"),
                "{section}: external reference {target}"
            );
            assert!(
                Assets::get(&format!("{section}/{target}")).is_some(),
                "{section}: {target} is referenced but missing"
            );
        }
    }

    #[test]
    fn the_pages_obey_their_own_content_security_policy() {
        assert_csp_clean("overlay");
        assert_csp_clean("dashboard");
    }

    #[test]
    fn page_scripts_never_build_markup_from_text() {
        let banned = [
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "eval(",
            "new Function",
        ];
        let mut scanned = 0;
        for name in Assets::iter() {
            if !name.ends_with(".js") {
                continue;
            }
            let file = Assets::get(&name).unwrap();
            let source = String::from_utf8(file.data.into_owned()).unwrap();
            for api in banned {
                assert!(!source.contains(api), "{name} uses {api}");
            }
            scanned += 1;
        }
        assert!(scanned >= 2, "expected to find the overlay scripts");
    }
}
