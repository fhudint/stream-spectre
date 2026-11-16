//! stream-spectre: OBS browser-source stream overlay.
//!
//! Seluruh logika berada di library ini supaya mudah diuji;
//! `main.rs` hanya bootstrap.

/// Nama aplikasi, diambil dari `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// Versi aplikasi, diambil dari `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
