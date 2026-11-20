//! A snapshot of the current foreground window, monitor and virtual desktop.

use windows::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONEAREST},
    UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow},
};
use winvd::{get_current_desktop, Desktop};

#[allow(dead_code)]
/// The desktop context around the user: the foreground window, the monitor it
/// is on, and the active virtual desktop.
pub struct WindowContext {
    pub foreground_window: HWND,
    pub monitor: HMONITOR,
    pub virtual_desktop: Option<Desktop>,
}

impl WindowContext {
    /// Returns the current state of the window context.
    ///
    /// The monitor is taken from the **foreground window**, because that is the taskbar
    /// the user is working on. It used to be taken from the cursor first, which broke
    /// cycling whenever the mouse rested on another monitor: the button list and the
    /// window list then described a different monitor than the focused window, so the
    /// cycle could never resolve its source button. The cursor is only a fallback for
    /// when there is no foreground window at all (desktop, lock screen transition).
    pub fn current_state() -> WindowContext {
        unsafe {
            let fg = GetForegroundWindow();

            let monitor = if fg.0.is_null() {
                cursor_monitor()
            } else {
                let from_window = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
                if from_window.is_invalid() {
                    cursor_monitor()
                } else {
                    from_window
                }
            };

            let virtual_desktop = get_current_desktop().ok();

            WindowContext {
                foreground_window: fg,
                monitor,
                virtual_desktop,
            }
        }
    }
}

/// Monitor under the cursor, or a null handle when the cursor position is unavailable.
fn cursor_monitor() -> HMONITOR {
    let mut pt = POINT::default();
    match unsafe { GetCursorPos(&mut pt) } {
        Ok(()) => unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) },
        Err(_) => HMONITOR::default(),
    }
}
