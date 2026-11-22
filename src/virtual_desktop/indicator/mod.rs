//! On-taskbar indicator for virtual desktops.
//!
//! The indicator is a layered **child window of `Shell_TrayWnd`** that paints one
//! dot per virtual desktop and handles mouse input to switch desktops or move
//! windows. Being a child keeps it in the taskbar's own z-order band, so it stays
//! visible over the taskbar's XAML content - including while Task View is open.
//!
//! This file is only the module's table of contents. The behaviour lives in files
//! named after their primary type:
//!
//! * `IndicatorWindow`   - owns the Win32 window, its state and its message loop.
//! * `IndicatorSettings` - the indicator's user-facing options (from config).
//! * `DotPainter`        - paints the dots and hover highlights onto a `Canvas`.
//! * `DesktopMover`      - "move window to desktop" actions and their context menu.
//! * `IndicatorGeometry` - dot size/length and where the window is placed.
//! * `FullscreenDetector` - the fullscreen app that owns the taskbar's monitor.
//! * `Canvas`            - logical 32-bit ARGB drawing surface.
//! * `utils`             - small helpers over the winvd API.

mod canvas;
mod desktop_mover;
mod dot_painter;
mod fullscreen_detector;
mod indicator_geometry;
mod indicator_mode;
mod indicator_position;
mod indicator_settings;
mod indicator_window;
mod taskbar_watcher;
mod utils;

pub use indicator_mode::IndicatorMode;
pub use indicator_settings::IndicatorSettings;
pub use indicator_window::IndicatorWindow;

/// 1-based number of the active virtual desktop, for the tray-icon mode.
pub fn current_desktop_number() -> usize {
    utils::current_index() + 1
}

use windows::Win32::UI::WindowsAndMessaging::WM_USER;

/// Posted to the indicator window when the active virtual desktop changes.
const WM_APP_VD_EVENT: u32 = WM_USER + 0x100;

/// Posted to the indicator window when the taskbar changes position (slide hide/unhide).
const WM_APP_TASKBAR_MOVED: u32 = WM_USER + 0x101;

/// Posted to the indicator window when the foreground window changed, which may
/// have started or ended a fullscreen app on the taskbar's monitor.
const WM_APP_FULLSCREEN_CHANGED: u32 = WM_USER + 0x102;

/// Gap kept between the indicator and the taskbar edge / system tray.
const INDICATOR_MARGIN: i32 = 6;

/// Command-ID base for the "Move to Desktop" context menu items.
const MENU_MOVE_BASE: usize = 1000;
