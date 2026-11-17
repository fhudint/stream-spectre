use std::process::ExitCode;

use anyhow::Context;
use stream_spectre::cli::{self, Command};
use stream_spectre::config::{self, LoadOptions};
use stream_spectre::{platform, run};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match start() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("{err:#}");
            ExitCode::FAILURE
        }
    }
}

fn start() -> anyhow::Result<()> {
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
            runtime.block_on(run::serve(loaded, platform::shutdown_signal(), || {}))?;
        }
    }
    Ok(())
}
