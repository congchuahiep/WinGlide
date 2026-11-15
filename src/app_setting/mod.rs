//! The user's settings: what is stored, and the window that edits it.
//!
//! [`AppConfig`] is the persisted state - one JSON file per user, see
//! [`app_config`]. [`settings_window`] draws the WinUI 3 window that edits it and
//! owns the two small widgets it needs ([`setting_item`], [`hotkey_button`]) plus
//! the title bar icon workaround ([`window_icon`]).

mod app_config;
mod hotkey_button;
mod setting_item;
mod settings_window;
mod window_icon;

pub use app_config::AppConfig;
pub use settings_window::*;
