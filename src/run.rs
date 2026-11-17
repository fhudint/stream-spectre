//! Starting the program: wires the configuration, the pipeline, the chat
//! readers, the savers and the server together.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::app::{
    Credentials, Pipeline, SETTLE, autosave, autosave_history, history_path, load_history,
};
use crate::chat::{tiktok_source, youtube_source};
use crate::config::{ChatSourceKind, KeySource, Loaded};
use crate::i18n::I18n;
use crate::server;
use crate::youtube::YoutubeClient;

/// Runs the whole program for `loaded` until `shutdown` completes: the server,
/// both chat readers and the savers. Settings and history are written before
/// it returns. `main.rs` passes the system's stop signal; the window program
/// passes the closing of its window. `listening` is called once the server has
/// its port and is about to answer.
pub async fn serve(
    loaded: Loaded,
    shutdown: impl Future<Output = ()> + Send + 'static,
    listening: impl FnOnce(),
) -> anyhow::Result<()> {
    let Loaded {
        config,
        secrets,
        path,
        baseline,
        saved_path,
        saved_tables,
        api_key_source,
        secrets_path,
        env_api_key,
        ..
    } = loaded;
    if config.chat.tiktok.username().is_empty() && config.chat.youtube.video.trim().is_empty() {
        tracing::info!(
            "no chat to read yet: set a TikTok username or a YouTube live on the dashboard's Account page"
        );
    }
    if saved_tables.is_empty() {
        tracing::info!(path = %saved_path.display(), "settings changed in the dashboard are saved here");
    } else {
        tracing::info!(
            path = %saved_path.display(),
            tables = %saved_tables.join(", "),
            "saved dashboard settings replace these tables from config.toml; delete the file to go back to config.toml"
        );
    }

    let i18n = I18n::load(config.general.language)?;
    match api_key_source {
        KeySource::None => tracing::warn!(
            "no YouTube API key: video requests are refused until one is entered on the dashboard's Account page (or YOUTUBE_API_KEY is set)"
        ),
        KeySource::Environment => tracing::info!("YouTube API key: from the environment"),
        KeySource::Saved => tracing::info!(
            "YouTube API key: saved from the dashboard (it replaces YOUTUBE_API_KEY, if set)"
        ),
    }
    let youtube = YoutubeClient::new(
        secrets.youtube_api_key.as_ref(),
        Duration::from_secs(config.youtube.request_timeout_secs),
    )?;
    let pipeline = Arc::new(Pipeline::new(&config, i18n, youtube)?);
    let history_file = history_path(&path);
    pipeline.restore_history(load_history(&history_file));
    tracing::info!(path = %history_file.display(), "the request history is kept here");
    let credentials = Arc::new(Credentials::new(
        secrets_path,
        env_api_key,
        // Only a key that came from the secrets file counts as saved.
        if api_key_source == KeySource::Saved {
            secrets.youtube_api_key.clone()
        } else {
            None
        },
        secrets.dashboard_token.is_some(),
    ));

    let address = SocketAddr::new(config.server.bind, config.server.port);
    let listener = TcpListener::bind(address).await.with_context(|| {
        format!("cannot listen on {address} (is another copy already running?)")
    })?;
    listening();
    let hint = if secrets.dashboard_token.is_some() {
        "#token=<DASHBOARD_TOKEN>"
    } else {
        ""
    };
    let host = server::browser_host(address);
    tracing::info!("dashboard: http://{host}/dashboard/{hint}");
    tracing::info!(
        "overlay (OBS browser source): http://{host}/overlay/mediashare/{}",
        hint.replace('#', "?")
    );

    let (stop, stopped) = watch::channel(false);

    // One reading loop for each chat source; each waits by itself until it
    // has something to read.
    let tiktok_chat = {
        let pipeline = Arc::clone(&pipeline);
        let mut stopped = stopped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = pipeline.run_chat_sources(ChatSourceKind::Tiktok, tiktok_source) => {}
                () = async { let _ = stopped.wait_for(|stop| *stop).await; } => {}
            }
        })
    };
    let youtube_chat = {
        let pipeline = Arc::clone(&pipeline);
        let mut stopped = stopped.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = pipeline.run_chat_sources(ChatSourceKind::Youtube, youtube_source) => {}
                () = async { let _ = stopped.wait_for(|stop| *stop).await; } => {}
            }
        })
    };

    let saver = tokio::spawn(autosave(
        Arc::clone(&pipeline),
        baseline,
        saved_path,
        SETTLE,
        stopped.clone(),
    ));

    let history_saver = tokio::spawn(autosave_history(
        Arc::clone(&pipeline),
        history_file,
        SETTLE,
        stopped.clone(),
    ));

    tokio::spawn(async move {
        shutdown.await;
        tracing::info!("stopping");
        let _ = stop.send(true);
    });

    server::serve(
        listener,
        pipeline,
        credentials,
        secrets.dashboard_token.clone(),
        stopped,
    )
    .await?;
    let _ = tiktok_chat.await;
    let _ = youtube_chat.await;
    // Writes any change that was still waiting to be saved.
    let _ = saver.await;
    let _ = history_saver.await;
    Ok(())
}
