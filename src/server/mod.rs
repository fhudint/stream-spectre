//! The HTTP and WebSocket server: the overlay and dashboard pages, the
//! dashboard's JSON API, and the event stream.
//!
//! Routes:
//!
//! | Route | What |
//! |-------|------|
//! | `/overlay/`, `/dashboard/` | the pages (embedded in the binary) |
//! | `/ws/overlay`, `/ws/dashboard` | event stream (WebSocket) |
//! | `/api/...` | dashboard actions and state (JSON) |
//!
//! See `guard` for how requests are checked and `docs/websocket-events.md`
//! for the events.

mod api;
mod assets;
mod guard;
mod ws;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{get, patch, post, put};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::app::{Credentials, Pipeline};
use crate::config::Secret;
use crate::youtube::VideoSource;

pub use guard::Guard;

/// The `host:port` to show people for opening the pages in a browser.
///
/// An address that only reaches this computer is shown as `localhost`:
/// YouTube's embedded player refuses to play any video, with error 150, when
/// the overlay is opened through a bare IP address such as `127.0.0.1`. Other
/// addresses are shown as they are.
pub fn browser_host(bind: SocketAddr) -> String {
    let ip = bind.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        format!("localhost:{}", bind.port())
    } else {
        bind.to_string()
    }
}

/// Requests carry tiny JSON bodies.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// After shutdown is requested, connections get this long to finish before
/// the server stops waiting for them.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Shared by all handlers.
pub struct ServerState<S> {
    pub(crate) pipeline: Arc<Pipeline<S>>,
    pub(crate) credentials: Arc<Credentials>,
    pub(crate) shutdown: watch::Receiver<bool>,
    pub(crate) connections: Arc<AtomicUsize>,
}

impl<S> Clone for ServerState<S> {
    fn clone(&self) -> Self {
        Self {
            pipeline: Arc::clone(&self.pipeline),
            credentials: Arc::clone(&self.credentials),
            shutdown: self.shutdown.clone(),
            connections: Arc::clone(&self.connections),
        }
    }
}

/// Builds the routes. Every request passes through `guard::gate` first.
pub fn router<S: VideoSource + 'static>(state: ServerState<S>, guard: Arc<Guard>) -> Router {
    Router::new()
        .route("/", get(assets::to_dashboard))
        .route("/favicon.ico", get(assets::favicon))
        .route("/dashboard", get(assets::to_dashboard))
        .route("/dashboard/", get(assets::dashboard_index))
        .route("/dashboard/{*path}", get(assets::dashboard_file))
        .route("/overlay", get(assets::to_overlay))
        .route("/overlay/", get(assets::overlay_index))
        .route("/overlay/{*path}", get(assets::overlay_file))
        .route("/ws/overlay", get(ws::overlay::<S>))
        .route("/ws/dashboard", get(ws::dashboard::<S>))
        .route("/api/state", get(api::state::<S>))
        .route("/api/i18n", get(api::catalog::<S>))
        .route("/api/skip", post(api::skip::<S>))
        .route("/api/volume", post(api::volume::<S>))
        .route("/api/queue/{id}/approve", post(api::approve::<S>))
        .route("/api/queue/{id}/reject", post(api::reject::<S>))
        .route("/api/settings", patch(api::settings::<S>))
        .route("/api/blacklist", patch(api::blacklist::<S>))
        .route("/api/dashboard", patch(api::dashboard::<S>))
        .route("/api/chat/tiktok", patch(api::tiktok::<S>))
        .route("/api/chat/connection", put(api::chat_connection::<S>))
        .route("/api/account", get(api::account::<S>))
        .route(
            "/api/account/youtube-key",
            put(api::set_youtube_key::<S>).delete(api::remove_youtube_key::<S>),
        )
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(guard, guard::gate))
        .with_state(state)
}

/// Serves until `shutdown` turns true, then closes the WebSockets and
/// finishes in-flight requests (for at most a few seconds).
///
/// The host check uses the address the listener is really bound to.
pub async fn serve<S: VideoSource + 'static>(
    listener: TcpListener,
    pipeline: Arc<Pipeline<S>>,
    credentials: Arc<Credentials>,
    token: Option<Secret>,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let local = listener.local_addr()?;
    let guard = Arc::new(Guard::new(token.as_ref(), local.ip(), local.port()));
    let state = ServerState {
        pipeline,
        credentials,
        shutdown: shutdown.clone(),
        connections: Arc::new(AtomicUsize::new(0)),
    };
    let app = router(state, guard);

    let mut graceful = shutdown.clone();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = graceful.wait_for(|stop| *stop).await;
    });

    let mut deadline = shutdown;
    tokio::select! {
        result = server => result,
        () = async {
            let _ = deadline.wait_for(|stop| *stop).await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => {
            tracing::warn!("some connections did not close in time; stopping anyway");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn people_are_shown_localhost_for_addresses_that_only_reach_this_computer() {
        let shown = |text: &str| browser_host(text.parse().unwrap());
        assert_eq!(shown("127.0.0.1:8787"), "localhost:8787");
        assert_eq!(shown("127.0.0.2:80"), "localhost:80");
        assert_eq!(shown("[::1]:9000"), "localhost:9000");
        assert_eq!(shown("0.0.0.0:8787"), "localhost:8787");
        assert_eq!(shown("[::]:8787"), "localhost:8787");
        assert_eq!(shown("192.168.1.20:8787"), "192.168.1.20:8787");
        assert_eq!(shown("[2001:db8::1]:8787"), "[2001:db8::1]:8787");
    }

    use std::net::SocketAddr;

    use futures_util::{SinkExt, StreamExt};
    use reqwest::header::{AUTHORIZATION, HOST, ORIGIN};
    use serde_json::{Value, json};
    use tokio::net::TcpStream;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    use tokio_tungstenite::tungstenite::{Error as WsError, Message};
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

    use super::*;
    use crate::chat::{ChatMessage, Role, Sender};
    use crate::config::{Config, Language};
    use crate::i18n::I18n;
    use crate::youtube::{LiveStatus, VideoDetails, VideoId, YoutubeError};

    const TOKEN: &str = "test-token-0123456789";
    const ID_A: &str = "aaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbb";

    /// Finds every video, and lets a test decide whether a YouTube API key is
    /// accepted and see which keys were put to use.
    #[derive(Default)]
    struct TestVideos {
        verdict: std::sync::Mutex<Option<YoutubeError>>,
        used: std::sync::Mutex<Vec<Option<String>>>,
    }

    impl VideoSource for TestVideos {
        fn set_api_key(&self, key: Option<&Secret>) -> Result<(), YoutubeError> {
            self.used
                .lock()
                .unwrap()
                .push(key.map(|k| k.expose().to_owned()));
            Ok(())
        }

        fn check_api_key(
            &self,
            _key: &Secret,
        ) -> impl std::future::Future<Output = Result<(), YoutubeError>> + Send {
            let verdict = self.verdict.lock().unwrap().clone();
            async move { verdict.map_or(Ok(()), Err) }
        }

        async fn fetch(&self, id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
            Ok(Some(VideoDetails {
                id: id.clone(),
                title: format!("Title {id}"),
                channel: "Chan".to_owned(),
                duration_secs: 100,
                embeddable: true,
                age_restricted: false,
                live: LiveStatus::None,
                private: false,
            }))
        }
    }

    struct Running {
        addr: SocketAddr,
        pipeline: Arc<Pipeline<TestVideos>>,
        credentials: Arc<Credentials>,
        secrets_path: std::path::PathBuf,
        stop: watch::Sender<bool>,
        task: JoinHandle<io::Result<()>>,
        http: reqwest::Client,
    }

    impl Running {
        fn url(&self, path: &str) -> String {
            format!("http://{}{path}", self.addr)
        }

        async fn submit(&self, n: u32, id: &str) {
            let sender = Sender::new(format!("user-{n}"), format!("Viewer {n}"), Role::Follower);
            self.pipeline
                .handle_chat(ChatMessage {
                    sender,
                    text: format!("!v {id}"),
                })
                .await;
        }
    }

    async fn start(token: Option<&str>) -> Running {
        let mut config = Config::default();
        config.chat.tiktok.username = "streamer".to_owned();
        config.general.language = Language::En;
        config.moderation.cooldown_secs = 0;
        let pipeline = Arc::new(
            Pipeline::new(
                &config,
                I18n::load(Language::En).unwrap(),
                TestVideos::default(),
            )
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, rx) = watch::channel(false);
        let secrets_path = crate::config::test_temp_dir("server").join("config.secrets.toml");
        let credentials = Arc::new(Credentials::new(
            secrets_path.clone(),
            None,
            None,
            token.is_some(),
        ));
        let task = tokio::spawn(serve(
            listener,
            Arc::clone(&pipeline),
            Arc::clone(&credentials),
            token.map(Secret::new),
            rx,
        ));
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Running {
            addr,
            pipeline,
            credentials,
            secrets_path,
            stop,
            task,
            http,
        }
    }

    // --- pages ---

    #[tokio::test]
    async fn pages_are_served_and_the_root_redirects() {
        let server = start(None).await;
        for path in ["/dashboard/", "/overlay/"] {
            let response = server.http.get(server.url(path)).send().await.unwrap();
            assert_eq!(response.status(), 200, "{path}");
            assert!(
                response.headers()["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("text/html")
            );
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
            assert!(response.headers().contains_key("content-security-policy"));
        }
        for (from, to) in [
            ("/", "/dashboard/"),
            ("/dashboard", "/dashboard/"),
            ("/overlay", "/overlay/"),
        ] {
            let response = server.http.get(server.url(from)).send().await.unwrap();
            assert_eq!(response.status(), 307, "{from}");
            assert_eq!(response.headers()["location"], to);
        }
        let missing = server
            .http
            .get(server.url("/dashboard/nope.js"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), 404);
        let unknown = server
            .http
            .get(server.url("/elsewhere"))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), 404);
    }

    #[tokio::test]
    async fn browsers_that_ask_for_favicon_ico_are_sent_to_the_icon() {
        let server = start(Some(TOKEN)).await;
        // No token: this is the browser's own request.
        let response = server
            .http
            .get(server.url("/favicon.ico"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 307);
        let target = response.headers()["location"].to_str().unwrap().to_owned();
        assert_eq!(target, "/dashboard/favicon-32.png");

        let icon = server.http.get(server.url(&target)).send().await.unwrap();
        assert_eq!(icon.status(), 200);
        assert_eq!(icon.headers()["content-type"], "image/png");
        let bytes = icon.bytes().await.unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn the_dashboard_icon_is_the_same_file_as_the_app_icon() {
        // One source of truth: assets/icon.svg. The copy in static/ is what the
        // program serves, so the two must not drift apart.
        assert_eq!(
            include_str!("../../assets/icon.svg"),
            include_str!("../../static/dashboard/favicon.svg")
        );
    }

    #[tokio::test]
    async fn the_page_files_are_served_with_the_right_types() {
        let server = start(Some(TOKEN)).await;
        for (path, kind) in [
            ("/overlay/overlay.js", "javascript"),
            ("/overlay/overlay-core.js", "javascript"),
            ("/overlay/overlay.css", "text/css"),
            ("/dashboard/dashboard.js", "javascript"),
            ("/dashboard/dashboard-core.js", "javascript"),
            ("/dashboard/dashboard.css", "text/css"),
            ("/dashboard/fonts/inter-latin.woff2", "font/woff2"),
            ("/dashboard/fonts/firacode-latin.woff2", "font/woff2"),
            ("/dashboard/favicon.svg", "image/svg+xml"),
            ("/dashboard/favicon-32.png", "image/png"),
            ("/dashboard/apple-touch-icon.png", "image/png"),
        ] {
            // Pages and their files need no token; they hold nothing secret.
            let response = server.http.get(server.url(path)).send().await.unwrap();
            assert_eq!(response.status(), 200, "{path}");
            let content_type = response.headers()["content-type"]
                .to_str()
                .unwrap()
                .to_owned();
            assert!(content_type.contains(kind), "{path}: {content_type}");
            assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        }
        let overlay = server
            .http
            .get(server.url("/overlay/"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            overlay.headers()["referrer-policy"],
            "strict-origin-when-cross-origin"
        );
        let dashboard = server
            .http
            .get(server.url("/dashboard/"))
            .send()
            .await
            .unwrap();
        assert_eq!(dashboard.headers()["referrer-policy"], "no-referrer");
    }

    // --- guard ---

    #[tokio::test]
    async fn without_a_token_the_api_is_open_on_loopback() {
        let server = start(None).await;
        let response = server
            .http
            .get(server.url("/api/state"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["language"], "en");
        assert_eq!(body["queue"]["pending"], json!([]));
    }

    #[tokio::test]
    async fn a_configured_token_protects_the_api_but_not_the_pages() {
        let server = start(Some(TOKEN)).await;
        let get = |path: &str| server.http.get(server.url(path));

        assert_eq!(get("/dashboard/").send().await.unwrap().status(), 200);
        assert_eq!(get("/overlay/").send().await.unwrap().status(), 200);

        let denied = get("/api/state").send().await.unwrap();
        assert_eq!(denied.status(), 401);
        assert_eq!(denied.headers()["www-authenticate"], "Bearer");

        let wrong = get("/api/state").bearer_auth("nope").send().await.unwrap();
        assert_eq!(wrong.status(), 401);
        let wrong_query = get("/api/state?token=nope").send().await.unwrap();
        assert_eq!(wrong_query.status(), 401);

        assert_eq!(
            get("/api/state")
                .bearer_auth(TOKEN)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let by_query = format!("/api/state?token={TOKEN}");
        assert_eq!(get(&by_query).send().await.unwrap().status(), 200);

        let post = server
            .http
            .post(server.url("/api/skip"))
            .send()
            .await
            .unwrap();
        assert_eq!(post.status(), 401);
    }

    #[tokio::test]
    async fn other_host_names_are_refused() {
        let server = start(None).await;
        for path in ["/api/state", "/dashboard/"] {
            let response = server
                .http
                .get(server.url(path))
                .header(HOST, "evil.example")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{path}");
        }
        let ok = server
            .http
            .get(server.url("/api/state"))
            .header(HOST, format!("localhost:{}", server.addr.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), 200);
    }

    #[tokio::test]
    async fn requests_from_other_origins_are_refused() {
        let server = start(None).await;
        let post = |origin: &str| {
            server
                .http
                .post(server.url("/api/skip"))
                .header(ORIGIN, origin.to_owned())
        };
        for bad in ["http://evil.example", "null", "http://127.0.0.1:1"] {
            assert_eq!(post(bad).send().await.unwrap().status(), 403, "{bad}");
        }
        let own = format!("http://{}", server.addr);
        assert_eq!(post(&own).send().await.unwrap().status(), 200);
        // Scripts and curl send no Origin at all.
        let none = server
            .http
            .post(server.url("/api/skip"))
            .send()
            .await
            .unwrap();
        assert_eq!(none.status(), 200);
    }

    // --- REST ---

    #[tokio::test]
    async fn the_dashboard_can_approve_reject_skip_and_set_the_volume() {
        let server = start(None).await;
        server.submit(1, ID_A).await;
        server.submit(2, ID_B).await;
        let post = |path: &str| server.http.post(server.url(path));

        assert_eq!(
            post("/api/queue/1/approve").send().await.unwrap().status(),
            204
        );
        assert_eq!(
            post("/api/queue/1/approve").send().await.unwrap().status(),
            404
        );
        assert_eq!(
            post("/api/queue/9/reject").send().await.unwrap().status(),
            404
        );
        let bad_id = post("/api/queue/abc/approve").send().await.unwrap();
        assert_eq!(bad_id.status(), 400);

        let state: Value = server
            .http
            .get(server.url("/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(state["queue"]["now_playing"]["id"], 1);
        assert_eq!(state["queue"]["pending"][0]["id"], 2);

        assert_eq!(
            post("/api/queue/2/reject").send().await.unwrap().status(),
            204
        );
        let skipped: Value = post("/api/skip")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(skipped, json!({"skipped": true}));
        let again: Value = post("/api/skip")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(again, json!({"skipped": false}));

        let loud: Value = post("/api/volume")
            .json(&json!({"volume": 100}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(loud, json!({"volume": 80}), "limited to max_volume");
        for bad in [
            json!({"volume": 101}),
            json!({"volume": -1}),
            json!({"volume": "x"}),
            json!({}),
            json!(7),
        ] {
            let response = post("/api/volume").json(&bad).send().await.unwrap();
            assert_eq!(response.status(), 400, "{bad}");
        }
    }

    #[tokio::test]
    async fn settings_and_blacklist_are_patched_not_replaced() {
        let server = start(None).await;
        let patch = |path: &str, body: Value| server.http.patch(server.url(path)).json(&body);

        let updated: Value = patch(
            "/api/settings",
            json!({"min_role": "subscriber", "cooldown_secs": 5}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
        assert_eq!(updated["settings"]["moderation"]["min_role"], "subscriber");
        assert_eq!(updated["settings"]["moderation"]["cooldown_secs"], 5);
        assert_eq!(
            updated["settings"]["moderation"]["max_queue_len"], 20,
            "untouched"
        );

        let refused = patch("/api/settings", json!({"max_queue_len": 0}))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 400);
        let body: Value = refused.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("max_queue_len"),
            "{body}"
        );
        let typo = patch("/api/settings", json!({"min_rol": "everyone"}))
            .send()
            .await
            .unwrap();
        assert_eq!(typo.status(), 400);
        // A refused change leaves everything as it was.
        let state: Value = server
            .http
            .get(server.url("/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(state["settings"]["moderation"]["max_queue_len"], 20);
        assert_eq!(state["settings"]["moderation"]["min_role"], "subscriber");

        let list: Value = patch("/api/blacklist", json!({"title_words": ["spam"]}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            list["settings"]["blacklist"]["title_words"],
            json!(["spam"])
        );
        let bad = patch("/api/blacklist", json!({"video_ids": ["short"]}))
            .send()
            .await
            .unwrap();
        assert_eq!(bad.status(), 400);
    }

    // --- the account: credentials changed from the dashboard ---

    const GOOD_KEY: &str = "AIzaSyA-1234567890abcdefghijklmnopqrstu";

    async fn put_key(server: &Running, body: Value) -> reqwest::Response {
        server
            .http
            .put(server.url("/api/account/youtube-key"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    fn keys_used(server: &Running) -> Vec<Option<String>> {
        server.pipeline.lookup_source().used.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn the_account_says_what_is_set_and_never_the_secrets() {
        let server = start(None).await;
        let body: Value = server
            .http
            .get(server.url("/api/account"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            body,
            json!({
                "youtube_key": {"set": false, "source": "none", "last4": null},
                "dashboard_token": {"set": false},
                "editable": true
            })
        );
        let protected = start(Some(TOKEN)).await;
        let with_token: Value = protected
            .http
            .get(protected.url("/api/account"))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(with_token["dashboard_token"], json!({"set": true}));
        assert!(!with_token.to_string().contains(TOKEN));
        // Like every API route, it needs the token when one is set.
        let refused = protected
            .http
            .get(protected.url("/api/account"))
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), 401);
    }

    #[tokio::test]
    async fn a_good_key_is_saved_privately_used_at_once_and_never_sent_back() {
        let server = start(None).await;
        let response = put_key(&server, json!({"key": format!(" {GOOD_KEY} ")})).await;
        assert_eq!(response.status(), 200);
        let text = response.text().await.unwrap();
        assert!(!text.contains(GOOD_KEY), "the key was sent back: {text}");
        let body: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["checked"], "valid");
        assert_eq!(
            body["youtube_key"],
            json!({"set": true, "source": "saved", "last4": "rstu"})
        );

        assert_eq!(keys_used(&server), vec![Some(GOOD_KEY.to_owned())]);
        let on_disk = std::fs::read_to_string(&server.secrets_path).unwrap();
        assert!(on_disk.contains(GOOD_KEY));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&server.secrets_path)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Nothing a dashboard can read holds the key.
        for path in ["/api/account", "/api/state", "/api/i18n"] {
            let text = server
                .http
                .get(server.url(path))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(!text.contains(GOOD_KEY), "{path} leaks the key");
        }
        let mut dashboard = connect(&server, "/ws/dashboard", None).await.unwrap();
        let hello = next_json(&mut dashboard).await;
        assert!(!hello.to_string().contains(GOOD_KEY));
    }

    #[tokio::test]
    async fn a_key_youtube_refuses_or_that_cannot_be_checked_is_not_kept() {
        let server = start(None).await;
        *server.pipeline.lookup_source().verdict.lock().unwrap() =
            Some(YoutubeError::InvalidApiKey);
        let refused = put_key(&server, json!({"key": GOOD_KEY})).await;
        assert_eq!(refused.status(), 400);
        let body: Value = refused.json().await.unwrap();
        assert_eq!(body["code"], "key_rejected");
        assert!(!body.to_string().contains(GOOD_KEY));

        *server.pipeline.lookup_source().verdict.lock().unwrap() =
            Some(YoutubeError::Network("timeout"));
        let offline = put_key(&server, json!({"key": GOOD_KEY})).await;
        assert_eq!(offline.status(), 502);
        assert_eq!(
            offline.json::<Value>().await.unwrap()["code"],
            "cannot_check"
        );

        assert!(!server.secrets_path.exists());
        assert!(keys_used(&server).is_empty());
    }

    #[tokio::test]
    async fn a_key_that_hit_the_quota_limit_is_saved_with_a_note() {
        let server = start(None).await;
        *server.pipeline.lookup_source().verdict.lock().unwrap() =
            Some(YoutubeError::QuotaExceeded);
        let body: Value = put_key(&server, json!({"key": GOOD_KEY}))
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(body["checked"], "quota_exhausted");
        assert_eq!(body["youtube_key"]["set"], true);
    }

    #[tokio::test]
    async fn bad_requests_for_a_key_are_refused_with_a_code() {
        let server = start(None).await;
        for (body, code) in [
            (json!({}), "bad_request"),
            (json!({"key": 5}), "bad_request"),
            (json!(["x"]), "bad_request"),
            (json!({"key": ""}), "key_empty"),
            (json!({"key": "  "}), "key_empty"),
            (json!({"key": "has space"}), "key_bad_format"),
            (json!({"key": "line\nbreak"}), "key_bad_format"),
            (json!({"key": "x".repeat(500)}), "key_bad_format"),
        ] {
            let response = put_key(&server, body.clone()).await;
            assert_eq!(response.status(), 400, "{body}");
            assert_eq!(
                response.json::<Value>().await.unwrap()["code"],
                code,
                "{body}"
            );
        }
        assert!(!server.secrets_path.exists());
        assert!(keys_used(&server).is_empty());
    }

    #[tokio::test]
    async fn the_saved_key_can_be_removed() {
        let server = start(None).await;
        put_key(&server, json!({"key": GOOD_KEY})).await;
        let response = server
            .http
            .delete(server.url("/api/account/youtube-key"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["youtube_key"]["set"], false);
        assert_eq!(keys_used(&server).last(), Some(&None));
        assert!(
            !std::fs::read_to_string(&server.secrets_path)
                .unwrap()
                .contains(GOOD_KEY)
        );
    }

    #[test]
    fn only_connections_from_this_computer_may_change_credentials() {
        use std::net::IpAddr;
        for allowed in ["127.0.0.1", "127.9.9.9", "::1", "::ffff:127.0.0.1"] {
            let ip: IpAddr = allowed.parse().unwrap();
            assert!(api::may_change_credentials(ip), "{allowed}");
        }
        for refused in [
            "192.168.1.5",
            "10.0.0.2",
            "8.8.8.8",
            "0.0.0.0",
            "::",
            "fe80::1",
            "2001:db8::1",
            "::ffff:192.168.1.5",
        ] {
            let ip: IpAddr = refused.parse().unwrap();
            assert!(!api::may_change_credentials(ip), "{refused}");
        }
    }

    #[tokio::test]
    async fn a_connection_from_the_network_may_look_but_not_change() {
        use axum::extract::{ConnectInfo, State};
        use axum::response::IntoResponse;

        let server = start(Some(TOKEN)).await;
        let state = ServerState {
            pipeline: Arc::clone(&server.pipeline),
            credentials: Arc::clone(&server.credentials),
            shutdown: server.stop.subscribe(),
            connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let lan: SocketAddr = "192.168.1.5:50000".parse().unwrap();

        let view = api::account(State(state.clone()), ConnectInfo(lan)).await;
        assert_eq!(view.0["editable"], false);
        assert_eq!(view.0["dashboard_token"], json!({"set": true}));

        let refused = api::set_youtube_key(
            State(state.clone()),
            ConnectInfo(lan),
            axum::Json(json!({"key": GOOD_KEY})),
        )
        .await
        .unwrap_err()
        .into_response();
        assert_eq!(refused.status(), 403);
        let body = axum::body::to_bytes(refused.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "loopback_only");
        assert!(!body.to_string().contains(GOOD_KEY));

        let refused = api::remove_youtube_key(State(state), ConnectInfo(lan))
            .await
            .unwrap_err()
            .into_response();
        assert_eq!(refused.status(), 403);

        // Nothing happened: no check, no file, no key switched.
        assert!(!server.secrets_path.exists());
        assert!(keys_used(&server).is_empty());
    }

    #[tokio::test]
    async fn the_tiktok_username_is_patched_checked_and_announced() {
        let server = start(None).await;
        let mut dashboard = connect(&server, "/ws/dashboard", None).await.unwrap();
        next_json(&mut dashboard).await;
        let patch = |body: Value| {
            server
                .http
                .patch(server.url("/api/chat/tiktok"))
                .json(&body)
                .send()
        };

        let updated: Value = patch(json!({"username": " @New.Name "}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            updated["settings"]["chat"]["tiktok"]["username"],
            "New.Name"
        );
        let event = next_json(&mut dashboard).await;
        assert_eq!(event["type"], "settings_updated");
        assert_eq!(event["data"]["chat"]["tiktok"]["username"], "New.Name");

        for bad in [
            json!({"username": "a/b"}),
            json!({"username": "a b"}),
            json!({"username": "@"}),
            json!({"username": 5}),
            json!({"name": "x"}),
            json!({"username": "x".repeat(100)}),
            json!(["x"]),
        ] {
            let refused = patch(bad.clone()).await.unwrap();
            assert_eq!(refused.status(), 400, "{bad}");
        }
        let state: Value = server
            .http
            .get(server.url("/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(state["settings"]["chat"]["tiktok"]["username"], "New.Name");

        // Emptying it is allowed: no chat is read until a name is set.
        let emptied: Value = patch(json!({"username": ""}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(emptied["settings"]["chat"]["tiktok"]["username"], "");
    }

    #[tokio::test]
    async fn the_chat_is_disconnected_and_connected_from_the_dashboard() {
        let server = start(None).await;
        let put = |body: Value| {
            server
                .http
                .put(server.url("/api/chat/connection"))
                .json(&body)
                .send()
        };
        let off: Value = put(json!({"connected": false}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(off["chat"]["state"], "paused");
        assert_eq!(off["chat"]["channel"], "streamer");

        let on: Value = put(json!({"connected": true}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(on["chat"]["state"], "reconnecting");

        for bad in [
            json!({}),
            json!({"connected": "yes"}),
            json!({"connected": 1}),
            json!(["x"]),
            json!(null),
        ] {
            let refused = put(bad.clone()).await.unwrap();
            assert_eq!(refused.status(), 400, "{bad}");
            assert_eq!(
                refused.json::<Value>().await.unwrap()["code"],
                "bad_request"
            );
        }
    }

    #[tokio::test]
    async fn the_overlay_never_hears_about_the_username() {
        let server = start(None).await;
        let mut overlay = connect(&server, "/ws/overlay", None).await.unwrap();
        let hello = next_json(&mut overlay).await;
        server
            .http
            .patch(server.url("/api/chat/tiktok"))
            .json(&json!({"username": "someone_else"}))
            .send()
            .await
            .unwrap();
        expect_silence(&mut overlay).await;
        // The username is in the settings of every hello; that is the one
        // thing the overlay's hello has in common with the dashboard's.
        assert!(hello["data"]["settings"]["chat"].is_object());
    }

    #[tokio::test]
    async fn the_chat_history_setting_is_patched_and_checked() {
        let server = start(None).await;
        let patch = |body: Value| {
            server
                .http
                .patch(server.url("/api/dashboard"))
                .json(&body)
                .send()
        };

        let updated: Value = patch(json!({"chat_history": 5}))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(updated["settings"]["dashboard"]["chat_history"], 5);

        for bad in [
            json!({"chat_history": 100_000}),
            json!({"chat_history": -1}),
            json!({"chat_history": "many"}),
            json!({"history": 5}),
            json!([5]),
        ] {
            let refused = patch(bad.clone()).await.unwrap();
            assert_eq!(refused.status(), 400, "{bad}");
        }
        let state: Value = server
            .http
            .get(server.url("/api/state"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(state["settings"]["dashboard"]["chat_history"], 5);
    }

    #[tokio::test]
    async fn the_ui_texts_are_available_in_the_configured_language() {
        let server = start(None).await;
        let texts: Value = server
            .http
            .get(server.url("/api/i18n"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(texts["dashboard.title"], "Moderation Dashboard");
    }

    #[tokio::test]
    async fn oversized_bodies_are_refused() {
        let server = start(None).await;
        let big = "x".repeat(200 * 1024);
        let response = server
            .http
            .patch(server.url("/api/settings"))
            .header("content-type", "application/json")
            .body(format!("{{\"junk\":\"{big}\"}}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 413);
    }

    // --- WebSockets ---

    type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

    async fn connect(server: &Running, path: &str, origin: Option<&str>) -> Result<Ws, WsError> {
        let mut request = format!("ws://{}{path}", server.addr)
            .into_client_request()
            .unwrap();
        if let Some(origin) = origin {
            request
                .headers_mut()
                .insert("Origin", HeaderValue::from_str(origin).unwrap());
        }
        connect_async(request).await.map(|(socket, _)| socket)
    }

    async fn next_json(ws: &mut Ws) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("timed out waiting for a message")
                .expect("socket closed")
                .expect("socket error");
            match message {
                Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
                Message::Close(_) => panic!("socket closed by the server"),
                _ => {}
            }
        }
    }

    /// Asserts that no further message arrives soon.
    async fn expect_silence(ws: &mut Ws) {
        let waited = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(waited.is_err(), "unexpected message: {waited:?}");
    }

    fn status_of(error: WsError) -> u16 {
        match error {
            WsError::Http(response) => response.status().as_u16(),
            other => panic!("expected an HTTP refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_dashboard_socket_sends_hello_then_every_event() {
        let server = start(None).await;
        server.submit(1, ID_A).await;
        let mut ws = connect(&server, "/ws/dashboard", None).await.unwrap();

        let hello = next_json(&mut ws).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["v"], 1);
        assert_eq!(hello["data"]["queue"]["pending"][0]["video_id"], ID_A);
        let hello_seq = hello["seq"].as_u64().unwrap();
        assert_eq!(hello["data"]["seq"].as_u64().unwrap(), hello_seq);

        server.submit(2, ID_B).await;
        let line = next_json(&mut ws).await;
        assert_eq!(line["type"], "chat_message");
        assert_eq!(line["seq"].as_u64().unwrap(), hello_seq + 1);
        assert_eq!(line["data"]["name"], "Viewer 2");
        assert_eq!(line["data"]["command"], true);
        let update = next_json(&mut ws).await;
        assert_eq!(update["type"], "queue_updated");
        assert_eq!(update["seq"].as_u64().unwrap(), hello_seq + 2);
        assert_eq!(update["data"]["pending"].as_array().unwrap().len(), 2);
        let stats = next_json(&mut ws).await;
        assert_eq!(stats["type"], "stats_updated");
        assert_eq!(stats["data"]["received"], 2);

        server.pipeline.set_volume(10);
        assert_eq!(next_json(&mut ws).await["type"], "volume");
    }

    #[tokio::test]
    async fn the_overlay_socket_gets_a_trimmed_hello_and_only_screen_events() {
        let server = start(None).await;
        server.submit(1, ID_A).await;
        let mut ws = connect(&server, "/ws/overlay", None).await.unwrap();

        let hello = next_json(&mut ws).await;
        assert_eq!(hello["type"], "hello");
        assert_eq!(
            hello["data"]["queue"]["pending"],
            json!([]),
            "no viewer names for the overlay"
        );
        assert_eq!(hello["data"]["queue"]["now_playing"], Value::Null);

        // Approving starts playback: the overlay hears `play`, but not the
        // `queue_updated` that follows.
        server
            .http
            .post(server.url("/api/queue/1/approve"))
            .send()
            .await
            .unwrap();
        let play = next_json(&mut ws).await;
        assert_eq!(play["type"], "play");
        assert_eq!(play["data"]["item"]["video_id"], ID_A);
        assert_eq!(play["data"]["volume"], 50);
        // Chat is for moderators only: nothing reaches the overlay.
        server.submit(2, ID_B).await;
        expect_silence(&mut ws).await;
    }

    #[tokio::test]
    async fn the_overlay_reports_back_over_its_socket() {
        let server = start(None).await;
        server.submit(1, ID_A).await;
        server.pipeline.approve(crate::queue::ItemId(1)).unwrap();
        let mut ws = connect(&server, "/ws/overlay", None).await.unwrap();
        let hello = next_json(&mut ws).await;
        assert_eq!(hello["data"]["queue"]["now_playing"]["id"], 1);

        // Noise and stale reports change nothing.
        for noise in [
            "not json",
            "{}",
            r#"{"type":"ended"}"#,
            r#"{"type":"ended","item_id":99}"#,
            r#"{"type":"player_ready"}"#,
        ] {
            ws.send(Message::text(noise)).await.unwrap();
        }
        expect_silence(&mut ws).await;
        assert_eq!(
            server.pipeline.hello().queue.now_playing.map(|i| i.id),
            Some(1)
        );

        ws.send(Message::text(r#"{"type":"ended","item_id":1}"#))
            .await
            .unwrap();
        assert_eq!(next_json(&mut ws).await["type"], "stop");
        assert_eq!(server.pipeline.hello().queue.now_playing, None);
    }

    #[tokio::test]
    async fn playback_progress_goes_from_the_overlay_to_the_dashboard_only() {
        let server = start(None).await;
        server.submit(1, ID_A).await;
        server.pipeline.approve(crate::queue::ItemId(1)).unwrap();
        let mut overlay = connect(&server, "/ws/overlay", None).await.unwrap();
        next_json(&mut overlay).await;
        let mut dashboard = connect(&server, "/ws/dashboard", None).await.unwrap();
        next_json(&mut dashboard).await;

        overlay
            .send(Message::text(
                r#"{"type":"progress","item_id":1,"position_secs":7}"#,
            ))
            .await
            .unwrap();
        let event = next_json(&mut dashboard).await;
        assert_eq!(event["type"], "playback");
        assert_eq!(event["data"], json!({"item_id": 1, "position_secs": 7}));
        expect_silence(&mut overlay).await;

        // A report for another video is dropped.
        overlay
            .send(Message::text(
                r#"{"type":"progress","item_id":99,"position_secs":1}"#,
            ))
            .await
            .unwrap();
        expect_silence(&mut dashboard).await;
    }

    #[tokio::test]
    async fn a_player_error_from_the_overlay_skips_the_video() {
        let server = start(None).await;
        server.submit(1, ID_A).await;
        server.pipeline.approve(crate::queue::ItemId(1)).unwrap();
        let mut ws = connect(&server, "/ws/overlay", None).await.unwrap();
        next_json(&mut ws).await;

        ws.send(Message::text(
            r#"{"type":"player_error","item_id":1,"code":"150"}"#,
        ))
        .await
        .unwrap();
        let skip = next_json(&mut ws).await;
        assert_eq!(skip["type"], "skip");
        assert_eq!(skip["data"]["reason"], "player_error");
        assert_eq!(next_json(&mut ws).await["type"], "stop");
    }

    #[tokio::test]
    async fn websockets_need_the_token_and_a_matching_origin() {
        let server = start(Some(TOKEN)).await;
        for path in ["/ws/dashboard", "/ws/overlay"] {
            let no_token = connect(&server, path, None).await.unwrap_err();
            assert_eq!(status_of(no_token), 401, "{path}");
            let wrong = connect(&server, &format!("{path}?token=nope"), None)
                .await
                .unwrap_err();
            assert_eq!(status_of(wrong), 401, "{path}");
        }
        let mut ws = connect(&server, &format!("/ws/dashboard?token={TOKEN}"), None)
            .await
            .unwrap();
        assert_eq!(next_json(&mut ws).await["type"], "hello");

        let foreign = connect(
            &server,
            &format!("/ws/dashboard?token={TOKEN}"),
            Some("http://evil.example"),
        )
        .await
        .unwrap_err();
        assert_eq!(status_of(foreign), 403);
        let own = format!("http://{}", server.addr);
        assert!(
            connect(&server, &format!("/ws/dashboard?token={TOKEN}"), Some(&own))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn the_bearer_header_works_for_websockets_too() {
        let server = start(Some(TOKEN)).await;
        let mut request = format!("ws://{}/ws/dashboard", server.addr)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            AUTHORIZATION.as_str(),
            HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
        );
        let (mut ws, _) = connect_async(request).await.unwrap();
        assert_eq!(next_json(&mut ws).await["type"], "hello");
    }

    #[tokio::test]
    async fn oversized_websocket_messages_end_the_connection() {
        let server = start(None).await;
        let mut ws = connect(&server, "/ws/overlay", None).await.unwrap();
        next_json(&mut ws).await;
        ws.send(Message::text("x".repeat(64 * 1024))).await.unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_) | Ok(Message::Close(_))) => return,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(
            outcome.is_ok(),
            "the server should have closed the connection"
        );
    }

    #[tokio::test]
    async fn shutting_down_closes_sockets_and_the_server_returns() {
        let server = start(None).await;
        let mut dashboard = connect(&server, "/ws/dashboard", None).await.unwrap();
        let mut overlay = connect(&server, "/ws/overlay", None).await.unwrap();
        next_json(&mut dashboard).await;
        next_json(&mut overlay).await;

        server.stop.send(true).unwrap();
        for ws in [&mut dashboard, &mut overlay] {
            let closed = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    match ws.next().await {
                        None | Some(Err(_) | Ok(Message::Close(_))) => return,
                        Some(Ok(_)) => {}
                    }
                }
            })
            .await;
            assert!(closed.is_ok());
        }
        let result = tokio::time::timeout(Duration::from_secs(3), server.task)
            .await
            .expect("serve should return promptly")
            .unwrap();
        assert!(result.is_ok());
    }
}
