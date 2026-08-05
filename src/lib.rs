//! Library surface for integration tests.

pub mod app;
pub mod assets;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod markdown;
pub mod media;
pub mod models;
pub mod routes;
pub mod templates;
pub mod tray;

pub use app::{AppState, build_router};
pub use config::{Cli, Config};
