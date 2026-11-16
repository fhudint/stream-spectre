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
