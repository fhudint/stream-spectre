//! stream-spectre: OBS browser-source stream overlay.
//!
//! Seluruh logika berada di library ini supaya mudah diuji;
//! `main.rs` hanya bootstrap.

pub mod chat;
pub mod cli;
pub mod command;
pub mod config;
pub mod i18n;
pub mod platform;
pub mod queue;
pub mod youtube;

/// Nama aplikasi, diambil dari `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// Versi aplikasi, diambil dari `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
