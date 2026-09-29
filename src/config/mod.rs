//! User preferences and their JSON persistence.
//!
//! [`AppConfig`] is serialized to `%APPDATA%/WinGlide/config.json`.

mod app_config;

pub use app_config::AppConfig;
