//! System tray icon feature.
//!
//! Shows the WinGlide icon in the notification area and turns its right-click
//! into the application menu (Exit / Settings / Debug Console).

mod tray_icon;

pub use tray_icon::{TrayIcon, IDM_EXIT, IDM_SETTINGS, IDM_SHOW_CONSOLE};
