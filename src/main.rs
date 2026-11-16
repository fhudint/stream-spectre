use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use stream_spectre::app::{Pipeline, SETTLE, autosave};
use stream_spectre::chat::tiktok_source;
use stream_spectre::cli::{self, Command};
use stream_spectre::config::{self, LoadOptions, Loaded};
use stream_spectre::i18n::I18n;
use stream_spectre::youtube::YoutubeClient;
use stream_spectre::{platform, server};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    match cli::parse(std::env::args_os().skip(1))? {
        Command::Help => print!("{}", cli::HELP),
        Command::Version => println!(
            "{} {}",
            stream_spectre::APP_NAME,
            stream_spectre::APP_VERSION
        ),
        Command::Run { config } => {
            let loaded = config::load(LoadOptions {
                explicit_path: config,
            })?;
            tracing::info!(path = %loaded.path.display(), "config loaded");
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("cannot start the async runtime")?;
            runtime.block_on(serve(loaded))?;
        }
    }
    Ok(())
}

async fn serve(loaded: Loaded) -> anyhow::Result<()> {
    let Loaded {
        config,
        secrets,
        baseline,
        saved_path,
        saved_tables,
        ..
    } = loaded;
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
    let youtube = YoutubeClient::new(
        &secrets.youtube_api_key,
        Duration::from_secs(config.youtube.request_timeout_secs),
    )?;
    let pipeline = Arc::new(Pipeline::new(&config, i18n, youtube)?);

    let address = SocketAddr::new(config.server.bind, config.server.port);
    let listener = TcpListener::bind(address).await.with_context(|| {
        format!("cannot listen on {address} (is another copy already running?)")
    })?;
    let hint = if secrets.dashboard_token.is_some() {
        "#token=<DASHBOARD_TOKEN>"
    } else {
        ""
    };
    let host = server::browser_host(address);
    tracing::info!("dashboard: http://{host}/dashboard/{hint}");
    tracing::info!(
        "overlay (OBS browser source): http://{host}/overlay/{}",
        hint.replace('#', "?")
    );

    let (stop, stopped) = watch::channel(false);

    let chat = {
        let pipeline = Arc::clone(&pipeline);
        let mut stopped = stopped.clone();
        let username = config.chat.tiktok.username().to_owned();
        tokio::spawn(async move {
            tokio::select! {
                () = pipeline.run_chat(tiktok_source(&username)) => {}
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

    tokio::spawn(async move {
        platform::shutdown_signal().await;
        tracing::info!("stopping");
        let _ = stop.send(true);
    });

    server::serve(listener, pipeline, secrets.dashboard_token.clone(), stopped).await?;
    let _ = chat.await;
    // Writes any change that was still waiting to be saved.
    let _ = saver.await;
    Ok(())
}
