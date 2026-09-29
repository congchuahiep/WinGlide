//! The custom window messages of WinGlide.
//!
//! These IDs are the IPC contract between the background app (which owns the
//! hidden `WinGlideTray` window and the message loop) and the rest of the
//! process: event hooks, the tray icon, and the Settings UI.

use windows::Win32::UI::WindowsAndMessaging::WM_USER;

/// Thread message: a newly shown window should be uncombined.
pub const WM_APP_UNCOMBINE: u32 = WM_USER + 0x100;

/// Thread message: the taskbar button cache must be invalidated.
pub const WM_APP_INVALIDATE_CACHE: u32 = WM_USER + 0x101;

/// Posted to the hidden window to make the background app reload its config.
pub const WM_APP_RELOAD_CONFIG: u32 = WM_USER + 0x102;

/// Posted to the hidden window to restart the app elevated.
pub const WM_APP_RESTART_AS_ADMIN: u32 = WM_USER + 0x103;

/// `Shell_NotifyIconW` callback message delivered to the hidden window.
pub const WM_USER_TRAYICON: u32 = WM_USER + 0x200;
