//! On-taskbar indicator for virtual desktops.
//!
//! The indicator is a layered popup owned by `Shell_TrayWnd` that paints one dot
//! per virtual desktop and handles mouse input to switch desktops or move windows.
//!
//! This file is only the module's table of contents. The behaviour lives in files
//! named after their primary type:
//!
//! * `IndicatorWindow`   - owns the Win32 window, its state and its message loop.
//! * `IndicatorSettings` - the indicator's user-facing options (from config).
//! * `DotPainter`        - paints the dots and hover highlights onto a `Canvas`.
//! * `DesktopMover`      - "move window to desktop" actions and their context menu.
//! * `IndicatorGeometry` - dot size/length and where the window is placed.
//! * `Canvas`            - logical 32-bit ARGB drawing surface.
//! * `utils`             - small helpers over the winvd API.

mod canvas;
mod desktop_mover;
mod dot_painter;
mod indicator_geometry;
mod indicator_position;
mod indicator_settings;
mod indicator_window;
mod utils;

pub use indicator_settings::IndicatorSettings;
pub use indicator_window::IndicatorWindow;

use windows::Win32::UI::WindowsAndMessaging::WM_USER;

/// Posted to the indicator window when the active virtual desktop changes.
const WM_APP_VD_EVENT: u32 = WM_USER + 0x100;

/// Gap kept between the indicator and the taskbar edge / system tray.
const INDICATOR_MARGIN: i32 = 6;

/// Command-ID base for the "Move to Desktop" context menu items.
const MENU_MOVE_BASE: usize = 1000;
