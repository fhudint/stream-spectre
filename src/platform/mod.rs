//! OS-specific helpers.
//!
//! Anything that needs `#[cfg(target_os = ...)]` belongs here, in its own
//! submodule (`linux.rs`, `windows.rs`, `macos.rs`), so the rest of the code
//! stays platform-neutral.

use std::path::PathBuf;

/// Per-user configuration directory for the current OS, without the app
/// subfolder.
///
/// Linux: `$XDG_CONFIG_HOME` or `~/.config`. Windows: `%APPDATA%`.
/// macOS: `~/Library/Application Support`.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir()
}

/// Completes when the user asks the program to stop: Ctrl+C, and on Unix
/// also `SIGTERM` (what `systemctl stop` and `kill` send).
#[cfg(unix)]
pub async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            // Cannot listen for SIGTERM; Ctrl+C still works.
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        () = terminate => {}
    }
}

/// Completes when the user presses Ctrl+C.
#[cfg(not(unix))]
pub async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
