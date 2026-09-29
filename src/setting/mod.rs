//! The Settings UI and the app-level preferences it controls.
//!
//! Built on `windows-reactor` (a native WinUI 3 binding). Besides the window
//! itself, this feature owns the preferences that have no other home:
//! autostart, administrator relaunch and the update check.

mod autostart;
mod hotkey_button;
mod setting_item;
mod settings_window;
mod update_checker;

pub use settings_window::*;
