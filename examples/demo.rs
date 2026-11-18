//! Runs the whole application with pretend viewers, so the dashboard and the
//! overlay can be tried in a browser or in OBS without TikTok, a YouTube API
//! key, or a live stream.
//!
//! ```text
//! cargo run --example demo
//! ```
//!
//! Then open <http://localhost:8787/dashboard/> and add
//! <http://localhost:8787/overlay/mediashare/> as a browser source (use `localhost`, not
//! `127.0.0.1`: YouTube's player refuses to play from an IP address). A pretend
//! viewer asks for a video every few seconds (most add a few words, which the
//! overlay shows), mostly YouTube and now and then
//! TikTok; approve one in the dashboard and it plays in the overlay. A TikTok
//! video has to be skipped by hand when it is over.
//!
//! **Which videos.** By default a short built-in list. To try your own, copy
//! `demo-videos.example.json` to `demo-videos.json` (git-ignored, like `.env`)
//! and put the videos in it:
//!
//! ```json
//! { "youtube": ["dQw4w9WgXcQ"], "tiktok": ["7700124541145599252"] }
//! ```
//!
//! An entry is what a viewer would type after the command: for YouTube an ID or
//! a link, for TikTok an ID, the code of a short link, or a link. (A TikTok
//! short link or code is opened with one request to TikTok, as in the real
//! program; an ID needs none.) A key left out keeps the built-in list for it,
//! an empty list `[]` turns that source off, and an entry that would not be
//! understood is reported and skipped. `DEMO_VIDEOS` names another file.
//!
//! The settings are those of `config.example.toml` (so the commands are the
//! ones written there, `!yt` and `!vt`), with a made-up TikTok username and no
//! cooldown. Nothing is saved to disk. `DEMO_PORT` changes the port.
//! `DASHBOARD_TOKEN` turns on the token check, as in the real application.
//! YouTube titles are made up; the videos themselves are real, so the overlay
//! plays them for real.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use stream_spectre::app::{Credentials, Pipeline};
use stream_spectre::chat::{ChatMessage, ChatStatus, Role, Sender};
use stream_spectre::config::{self, ChatSourceKind, CommandConfig, CommandSource, Secret};
use stream_spectre::i18n::I18n;
use stream_spectre::tiktok::extract_tiktok_ref;
use stream_spectre::youtube::{
    LiveStatus, VideoDetails, VideoId, VideoSource, YoutubeError, extract_video_id,
};
use stream_spectre::{platform, server};
use tokio::net::TcpListener;
use tokio::sync::watch;

/// The file the videos come from when it exists, next to `Cargo.toml`.
const VIDEOS_FILE: &str = "demo-videos.json";

/// Public videos the pretend viewers ask for when no file says otherwise.
const DEFAULT_YOUTUBE: [&str; 5] = [
    "dQw4w9WgXcQ",
    "jNQXAC9IVRw",
    "9bZkp7q19f0",
    "kJQP7kiw5Fk",
    "M7lc1UVf-VE",
];
const DEFAULT_TIKTOK: [&str; 2] = ["7700124541145599252", "7219950637247728923"];

/// The videos the pretend viewers ask for, as they would type them.
#[derive(Clone)]
struct Videos {
    youtube: Vec<String>,
    tiktok: Vec<String>,
}

/// `demo-videos.json`. A key that is left out keeps the built-in list.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VideosFile {
    youtube: Option<Vec<String>>,
    tiktok: Option<Vec<String>>,
}

impl Videos {
    fn built_in() -> Self {
        let own = |list: &[&str]| list.iter().map(|id| (*id).to_owned()).collect();
        Self {
            youtube: own(&DEFAULT_YOUTUBE),
            tiktok: own(&DEFAULT_TIKTOK),
        }
    }

    /// Reads `path`. An entry that the program would not understand is
    /// reported on the screen and left out, so a typo is found now and not as
    /// a puzzling notice in the dashboard.
    fn from_file(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let file: VideosFile = serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not valid (see demo-videos.example.json)",
                path.display()
            )
        })?;
        let built_in = Self::built_in();
        let keep = |entries: Vec<String>, source: &str, understood: fn(&str) -> bool| {
            entries
                .into_iter()
                .filter(|entry| {
                    let ok = understood(entry.trim());
                    if !ok {
                        eprintln!("demo: skipping {source} entry {entry:?}: not understood");
                    }
                    ok
                })
                .map(|entry| entry.trim().to_owned())
                .collect::<Vec<_>>()
        };
        Ok(Self {
            youtube: file.youtube.map_or(built_in.youtube, |entries| {
                keep(entries, "YouTube", |entry| {
                    extract_video_id(entry).is_some()
                })
            }),
            tiktok: file.tiktok.map_or(built_in.tiktok, |entries| {
                keep(entries, "TikTok", |entry| {
                    extract_tiktok_ref(entry).is_some()
                })
            }),
        })
    }

    /// The file named by `DEMO_VIDEOS`, else `demo-videos.json` if it is
    /// there, else the built-in lists. Says which on the screen.
    fn load() -> anyhow::Result<Self> {
        let named = std::env::var_os("DEMO_VIDEOS").map(PathBuf::from);
        let path = match named {
            Some(path) => path,
            None if Path::new(VIDEOS_FILE).exists() => PathBuf::from(VIDEOS_FILE),
            None => {
                println!(
                    "videos:    built-in list (copy demo-videos.example.json to {VIDEOS_FILE} to use your own)"
                );
                return Ok(Self::built_in());
            }
        };
        let videos = Self::from_file(&path)?;
        println!(
            "videos:    {} ({} YouTube, {} TikTok)",
            path.display(),
            videos.youtube.len(),
            videos.tiktok.len()
        );
        Ok(videos)
    }
}

/// Every how many requests one is for TikTok.
const TIKTOK_EVERY: usize = 3;

const VIEWERS: [&str; 4] = ["Budi", "Sari", "Dewi", "Agus"];

/// Ordinary chat, so the dashboard's chat view has something besides requests.
const SMALL_TALK: [&str; 4] = [
    "hello from the demo",
    "great stream!",
    "what is this song?",
    "first time here",
];

/// What some of the requests say after the link; every third has nothing.
const MESSAGES: [&str; 4] = [
    "halo ka, puterin ini dong",
    "this one is my favourite",
    "request dari Bandung!",
    "",
];

/// Answers every ID with an ordinary three-minute video.
struct DemoVideos;

impl VideoSource for DemoVideos {
    async fn fetch(&self, id: &VideoId) -> Result<Option<VideoDetails>, YoutubeError> {
        Ok(Some(VideoDetails {
            id: id.clone(),
            title: format!("Demo video {id}"),
            channel: "Demo channel".to_owned(),
            duration_secs: 213,
            embeddable: true,
            age_restricted: false,
            live: LiveStatus::None,
            private: false,
            category_id: 10,
        }))
    }
}

/// What the pretend viewers say: the commands and the videos.
#[derive(Clone)]
struct Plan {
    commands: Commands,
    videos: Videos,
}

/// What viewers type to ask for a video, as the settings say: `!yt`, `!vt`.
#[derive(Clone)]
struct Commands {
    youtube: Option<String>,
    tiktok: Option<String>,
}

impl Commands {
    fn from_config(config: &CommandConfig) -> Self {
        let first = |source| {
            config
                .commands
                .iter()
                .find(|command| command.source == source)
                .map(|command| format!("{}{}", config.prefix, command.name))
        };
        Self {
            youtube: first(CommandSource::Youtube),
            tiktok: first(CommandSource::Tiktok),
        }
    }
}

/// The text of round `round`'s request: mostly YouTube, every third one
/// TikTok. A source with no command in the settings or no video in the list is
/// left out, and the other one is used for everything.
fn request_text(commands: &Commands, videos: &Videos, round: usize) -> Option<String> {
    let youtube = commands
        .youtube
        .as_ref()
        .filter(|_| !videos.youtube.is_empty())
        .map(|command| (command, &videos.youtube));
    let tiktok = commands
        .tiktok
        .as_ref()
        .filter(|_| !videos.tiktok.is_empty())
        .map(|command| (command, &videos.tiktok));
    let wants_tiktok = round % TIKTOK_EVERY == TIKTOK_EVERY - 1;
    let (command, list) = match (youtube, tiktok) {
        (_, Some(chosen)) if wants_tiktok => chosen,
        (Some(chosen), _) | (None, Some(chosen)) => chosen,
        (None, None) => return None,
    };
    // Each source keeps its own turn, so every video comes up in time.
    let turn = if wants_tiktok {
        round / TIKTOK_EVERY
    } else {
        round - round / TIKTOK_EVERY
    };
    let message = MESSAGES[round % MESSAGES.len()];
    Some(format!("{command} {} {message}", list[turn % list.len()]))
}

async fn request(pipeline: &Pipeline<DemoVideos>, plan: &Plan, round: usize) {
    let Some(text) = request_text(&plan.commands, &plan.videos, round) else {
        return;
    };
    let viewer = VIEWERS[round % VIEWERS.len()];
    let sender = Sender::new(format!("demo-{viewer}"), viewer, Role::Follower);
    pipeline.handle_chat(ChatMessage { sender, text }).await;
}

async fn chatter(pipeline: &Pipeline<DemoVideos>, round: usize) {
    let viewer = VIEWERS[(round + 1) % VIEWERS.len()];
    let sender = Sender::new(format!("demo-{viewer}"), viewer, Role::Everyone);
    pipeline
        .handle_chat(ChatMessage {
            sender,
            text: SMALL_TALK[round % SMALL_TALK.len()].to_owned(),
        })
        .await;
}

async fn feed_chat(
    pipeline: Arc<Pipeline<DemoVideos>>,
    plan: Plan,
    mut stopped: watch::Receiver<bool>,
) {
    for round in 0..3 {
        chatter(&pipeline, round).await;
        request(&pipeline, &plan, round).await;
    }
    let mut round = 3;
    let mut tick = tokio::time::interval(Duration::from_secs(6));
    loop {
        tokio::select! {
            () = async { let _ = stopped.wait_for(|stop| *stop).await; } => return,
            _ = tick.tick() => {
                chatter(&pipeline, round).await;
                request(&pipeline, &plan, round).await;
                round += 1;
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::var("DEMO_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8787);
    let token = std::env::var("DASHBOARD_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .map(Secret::new);

    // The same settings as the example config, so the demo shows what a new
    // user gets. Only the username and the cooldown are the demo's own.
    let mut config = config::parse(
        include_str!("../config.example.toml"),
        Path::new("config.example.toml"),
    )?;
    config.chat.tiktok.username = "demo_streamer".to_owned();
    config.moderation.cooldown_secs = 0;
    config.validate()?;
    let plan = Plan {
        commands: Commands::from_config(&config.command),
        videos: Videos::load()?,
    };

    let i18n = I18n::load(config.general.language)?;
    let pipeline = Arc::new(Pipeline::new(&config, i18n, DemoVideos)?);

    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on {address}"))?;
    let host = server::browser_host(address);
    println!("dashboard: http://{host}/dashboard/");
    println!("overlay:   http://{host}/overlay/mediashare/");

    pipeline.chat_status(
        ChatSourceKind::Tiktok,
        &ChatStatus::Connected {
            channel: config.chat.tiktok.username().to_owned(),
        },
    );

    let (stop, stopped) = watch::channel(false);
    let feeder = tokio::spawn(feed_chat(Arc::clone(&pipeline), plan, stopped.clone()));
    tokio::spawn(async move {
        platform::shutdown_signal().await;
        let _ = stop.send(true);
    });

    // The demo has no real key; anything entered on the Account page goes to a
    // throwaway file and is never sent anywhere.
    let credentials = Arc::new(Credentials::new(
        std::env::temp_dir().join(format!(
            "stream-spectre-demo-{}.secrets.toml",
            std::process::id()
        )),
        None,
        None,
        token.is_some(),
    ));
    // The demo asks nobody about newer versions.
    let updates = Arc::new(stream_spectre::update::Updates::new(false)?);
    server::serve(listener, pipeline, credentials, updates, token, stopped).await?;
    let _ = feeder.await;
    Ok(())
}
