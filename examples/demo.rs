//! Runs the whole application with pretend viewers, so the dashboard and the
//! overlay can be tried in a browser or in OBS without TikTok, a YouTube API
//! key, or a live stream.
//!
//! ```text
//! cargo run --example demo
//! ```
//!
//! Then open <http://localhost:8787/dashboard/> and add
//! <http://localhost:8787/overlay/> as a browser source (use `localhost`, not
//! `127.0.0.1`: YouTube's player refuses to play from an IP address). A pretend viewer
//! asks for a real, public YouTube video every few seconds; approve one in the
//! dashboard and it plays in the overlay.
//!
//! `DEMO_PORT` changes the port. `DASHBOARD_TOKEN` turns on the token check, as
//! in the real application. Titles are made up; the videos themselves are real,
//! so the overlay plays them for real.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use stream_spectre::app::Pipeline;
use stream_spectre::chat::{ChatMessage, ChatStatus, Role, Sender};
use stream_spectre::config::{Config, Secret};
use stream_spectre::i18n::I18n;
use stream_spectre::youtube::{LiveStatus, VideoDetails, VideoId, VideoSource, YoutubeError};
use stream_spectre::{platform, server};
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Public videos the pretend viewers ask for.
const VIDEOS: [&str; 5] = [
    "dQw4w9WgXcQ",
    "jNQXAC9IVRw",
    "9bZkp7q19f0",
    "kJQP7kiw5Fk",
    "M7lc1UVf-VE",
];

const VIEWERS: [&str; 4] = ["Budi", "Sari", "Dewi", "Agus"];

/// Ordinary chat, so the dashboard's chat view has something besides requests.
const SMALL_TALK: [&str; 4] = [
    "hello from the demo",
    "great stream!",
    "what is this song?",
    "first time here",
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
        }))
    }
}

async fn request(pipeline: &Pipeline<DemoVideos>, round: usize) {
    let viewer = VIEWERS[round % VIEWERS.len()];
    let video = VIDEOS[round % VIDEOS.len()];
    let sender = Sender::new(format!("demo-{viewer}"), viewer, Role::Follower);
    pipeline
        .handle_chat(ChatMessage {
            sender,
            text: format!("!v {video}"),
        })
        .await;
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

async fn feed_chat(pipeline: Arc<Pipeline<DemoVideos>>, mut stopped: watch::Receiver<bool>) {
    for round in 0..3 {
        chatter(&pipeline, round).await;
        request(&pipeline, round).await;
    }
    let mut round = 3;
    let mut tick = tokio::time::interval(Duration::from_secs(6));
    loop {
        tokio::select! {
            () = async { let _ = stopped.wait_for(|stop| *stop).await; } => return,
            _ = tick.tick() => {
                chatter(&pipeline, round).await;
                request(&pipeline, round).await;
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

    let mut config = Config::default();
    config.chat.tiktok.username = "demo_streamer".to_owned();
    config.moderation.cooldown_secs = 0;

    let i18n = I18n::load(config.general.language)?;
    let pipeline = Arc::new(Pipeline::new(&config, i18n, DemoVideos)?);

    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on {address}"))?;
    let host = server::browser_host(address);
    println!("dashboard: http://{host}/dashboard/");
    println!("overlay:   http://{host}/overlay/");

    pipeline.chat_status(&ChatStatus::Connected {
        channel: config.chat.tiktok.username().to_owned(),
    });

    let (stop, stopped) = watch::channel(false);
    let feeder = tokio::spawn(feed_chat(Arc::clone(&pipeline), stopped.clone()));
    tokio::spawn(async move {
        platform::shutdown_signal().await;
        let _ = stop.send(true);
    });

    server::serve(listener, pipeline, token, stopped).await?;
    let _ = feeder.await;
    Ok(())
}
