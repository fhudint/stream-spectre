//! stream-spectre: OBS browser-source stream overlay.
//!
//! All the logic lives in this library so it is easy to test; `main.rs` only
//! starts things up.

pub mod app;
pub mod chat;
pub mod cli;
pub mod command;
pub mod config;
pub mod i18n;
pub mod platform;
pub mod queue;
pub mod run;
pub mod server;
pub mod text;
pub mod tiktok;
pub mod update;
pub mod youtube;

/// Application name, taken from `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");

/// Application version, taken from `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
