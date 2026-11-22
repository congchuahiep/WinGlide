//! Detects a fullscreen app that owns the taskbar's monitor.
//!
//! The indicator is a child of `Shell_TrayWnd`, so it paints wherever the bar
//! paints. Windows does not always move the bar off screen for a fullscreen app
//! (a game in exclusive mode, F11 in a browser, a fullscreen video): the bar stays
//! where it is, covered by the app, and the dots would keep painting over it.
//! [`FullscreenDetector::active`] answers the one question the window needs: *is a
//! fullscreen app in the foreground on the bar's monitor right now?*
//!
//! The rules are the ones taskbar replacements converged on - ManagedShell's
//! `FullScreenHelper`, and Bevel95's Rust port of it - because each one closes a
//! false positive that a plain "does it fill the monitor" test gets wrong.

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use tracing::debug;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetClientRect, GetForegroundWindow, GetPropW, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostMessageW,
    EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE, GWL_STYLE,
    INDEXID_CONTAINER, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WS_BORDER,
    WS_CAPTION, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_EX_WINDOWEDGE, WS_POPUP, WS_THICKFRAME,
};

use super::WM_APP_FULLSCREEN_CHANGED;
use crate::taskbar::Taskbar;
use crate::window::system_class::is_system_class;

/// Window class of the shell's own full-screen surfaces, which must never count
/// as "a fullscreen app".
///
/// Windows 11 only, like the rest of the app: Windows 10's Task View
/// (`MultitaskingViewFrame`) is deliberately absent, because the XAML taskbar this
/// app drives does not exist there.
///
/// These are deliberately not in [`is_system_class`]: that list also drives
/// activation and taskbar cycling, where a UWP content window or the Task View
/// host has a different meaning.
const SHELL_OVERLAY_CLASSES: &[&str] = &[
    "XamlExplorerHostIslandWindow", // Task View / Alt-Tab
    "TaskSwitcherWnd",              // Alt-Tab switcher (legacy)
    "ForegroundStaging",            // shell staging window during app launches
    "Windows.Internal.Shell.TabProxyWindow",
    "StartMenuSizingFrame",
    "Shell_LightDismissOverlay",
];

static INDICATOR_HWND: AtomicIsize = AtomicIsize::new(0);
/// Foreground window at the last foreground event, so the location hook can
/// ignore every window the user is not looking at.
static FOREGROUND_HWND: AtomicIsize = AtomicIsize::new(0);
static PENDING: AtomicBool = AtomicBool::new(false);

/// Watches the events that can change the answer and answers "is a fullscreen app
/// on the taskbar's monitor?".
///
/// Purely event driven, no polling: the foreground hook covers switching apps, the
/// location hook covers a window that resizes itself into fullscreen while it keeps
/// the focus (Alt+Enter, a player's own fullscreen button), which fires no
/// foreground event.
///
/// Created on the indicator thread and dropped when its window goes away; the
/// callbacks only post a message, so they never do real work.
pub(super) struct FullscreenDetector {
    foreground_hook: HWINEVENTHOOK,
    location_hook: HWINEVENTHOOK,
}

impl FullscreenDetector {
    /// Installs both hooks; they post [`WM_APP_FULLSCREEN_CHANGED`] to `indicator`.
    pub(super) fn new(indicator: HWND) -> Option<Self> {
        INDICATOR_HWND.store(indicator.0 as isize, Ordering::SeqCst);
        PENDING.store(false, Ordering::SeqCst);
        unsafe {
            FOREGROUND_HWND.store(GetForegroundWindow().0 as isize, Ordering::SeqCst);
        }

        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let foreground_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(foreground_proc),
                0,
                0,
                flags,
            )
        };
        if foreground_hook.is_invalid() {
            debug!("Failed to install the fullscreen foreground hook");
            return None;
        }

        let location_hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(location_proc),
                0,
                0,
                flags,
            )
        };
        if location_hook.is_invalid() {
            // Switching apps still works; only a window that resizes itself into
            // fullscreen while keeping the focus would be missed.
            debug!("Failed to install the fullscreen location hook");
        }

        debug!("Fullscreen detector hooks installed (foreground + location)");
        Some(Self {
            foreground_hook,
            location_hook,
        })
    }

    /// Resets the coalescing flag so the next event posts a message.
    pub(super) fn reset_pending() {
        PENDING.store(false, Ordering::SeqCst);
    }

    /// The app window that owns the taskbar's monitor, or `None` when the dots
    /// belong on screen.
    ///
    /// `previous` is the window found by the last check. It is kept while it is
    /// still fullscreen and something that is *not* a competing app took the
    /// foreground - a topmost volume OSD, a game overlay, or a window on another
    /// monitor - so a floating popup cannot make the dots flicker back in.
    pub(super) fn active(taskbar: &Taskbar, previous: Option<HWND>) -> Option<HWND> {
        let screen = taskbar.monitor_rect()?;
        unsafe {
            let foreground = GetForegroundWindow();
            if is_fullscreen_on(foreground, screen) {
                return Some(foreground);
            }
            previous.filter(|&app| {
                is_fullscreen_on(app, screen)
                    && (foreground.is_invalid()
                        || floats_above(foreground)
                        || on_another_monitor(foreground, screen))
            })
        }
    }
}

impl Drop for FullscreenDetector {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.foreground_hook);
            if !self.location_hook.is_invalid() {
                let _ = UnhookWinEvent(self.location_hook);
            }
        }
        INDICATOR_HWND.store(0, Ordering::SeqCst);
        FOREGROUND_HWND.store(0, Ordering::SeqCst);
        PENDING.store(false, Ordering::SeqCst);
        debug!("Fullscreen detector hooks uninstalled");
    }
}

/// Posts the re-check, at most once per pending message.
fn post_recheck() {
    if PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let indicator = INDICATOR_HWND.load(Ordering::Relaxed);
    if indicator != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(indicator as _)),
                WM_APP_FULLSCREEN_CHANGED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Static callback invoked by Windows when the foreground window changes.
unsafe extern "system" fn foreground_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND {
        return;
    }
    FOREGROUND_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    post_recheck();
}

/// Static callback invoked when a window moves or resizes.
///
/// This fires for every window on the system, so everything but the foreground one
/// is dropped on a pointer compare. It exists for the window that turns itself
/// fullscreen without changing the focus.
unsafe extern "system" fn location_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event != EVENT_OBJECT_LOCATIONCHANGE
        || id_object != OBJID_WINDOW.0
        || id_child != INDEXID_CONTAINER as i32
    {
        return;
    }
    if hwnd.0 as isize != FOREGROUND_HWND.load(Ordering::Relaxed) {
        return;
    }
    post_recheck();
}

/// Whether `hwnd` is a live app window that owns the whole of `screen`.
unsafe fn is_fullscreen_on(hwnd: HWND, screen: RECT) -> bool {
    if hwnd.is_invalid()
        || !IsWindow(Some(hwnd)).as_bool()
        || !IsWindowVisible(hwnd).as_bool()
        || IsIconic(hwnd).as_bool()
    {
        return false;
    }

    // WinGlide's own windows (settings, tray host, this indicator) are never the
    // app that owns the screen.
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == unsafe { GetCurrentProcessId() } {
        return false;
    }

    can_own_screen(hwnd) && unsafe { covers(hwnd, screen) }
}

/// Whether `hwnd` is of a kind that can own the whole screen: not a shell window,
/// not a cloaked window, and not a see-through overlay.
unsafe fn can_own_screen(hwnd: HWND) -> bool {
    unsafe {
        // Since Windows 8 the shell marks its own panels and the desktop with this
        // property, which is a better answer than a class list the shell may extend.
        if !GetPropW(hwnd, w!("NonRudeHWND")).is_invalid() {
            return false;
        }

        // Cloaked windows are not drawn: without this, a fullscreen app on another
        // virtual desktop would hide the dots on this one.
        let mut cloaked: u32 = 0;
        let cloaked_ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok();
        if cloaked_ok && cloaked != 0 {
            return false;
        }

        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, &mut class_buf);
        let class_name = String::from_utf16_lossy(&class_buf[..class_len.max(0) as usize]);
        if is_system_class(&class_name) || SHELL_OVERLAY_CLASSES.contains(&class_name.as_str()) {
            return false;
        }

        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;

        // A UWP app's frame carries the edge style; the shell's own panels reuse the
        // same class without it.
        if matches!(
            class_name.as_str(),
            "ApplicationFrameWindow" | "Windows.UI.Core.CoreWindow"
        ) && ex_style & WS_EX_WINDOWEDGE.0 == 0
        {
            return false;
        }

        // Overlays that float over an app (game overlays, recorders, screenshot
        // tools) can be as large as the screen but are not the app being used.
        if ex_style & WS_EX_LAYERED.0 != 0
            && ex_style & (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) != 0
        {
            return false;
        }

        true
    }
}

/// Whether `hwnd` covers the whole of `screen` by the rectangle the user can see.
unsafe fn covers(hwnd: HWND, screen: RECT) -> bool {
    let Some(rect) = (unsafe { effective_rect(hwnd) }) else {
        return false;
    };
    rect.left <= screen.left
        && rect.top <= screen.top
        && rect.right >= screen.right
        && rect.bottom >= screen.bottom
}

/// The rectangle the user actually sees.
///
/// A framed window is measured by its **client area**: with auto-hide on, Windows
/// reports the whole monitor as the work area, so a maximized window fills the
/// monitor rectangle while its title bar keeps the app itself one pixel short of
/// fullscreen. Measuring the client area is what tells those two cases apart.
unsafe fn effective_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let framed = |bits: u32| style & bits == bits;
        let has_frame = framed(WS_CAPTION.0 | WS_THICKFRAME.0)
            || framed(WS_POPUP.0 | WS_THICKFRAME.0)
            || framed(WS_POPUP.0 | WS_BORDER.0);

        if !has_frame {
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).ok()?;
            return Some(rect);
        }

        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).ok()?;
        let mut origin = POINT { x: 0, y: 0 };
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
        Some(RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + client.right,
            bottom: origin.y + client.bottom,
        })
    }
}

/// Whether `hwnd` has `WS_EX_TOPMOST`, i.e. it floats above a fullscreen app
/// instead of replacing it.
unsafe fn floats_above(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 }
}

/// Whether `hwnd` is on a monitor other than the one whose bounds are `screen`.
unsafe fn on_another_monitor(hwnd: HWND, screen: RECT) -> bool {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info).as_bool() && info.rcMonitor != screen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what the detector sees right now, for diagnosing "the dots hid in the
    /// wrong app" reports, plus what one check costs (this is what the 1.5 s tick
    /// runs): `cargo test print_fullscreen_state -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the live desktop"]
    fn print_fullscreen_state() {
        let Some(taskbar) = Taskbar::primary() else {
            println!("no primary taskbar");
            return;
        };
        println!("taskbar monitor: {:?}", taskbar.monitor_rect());
        println!("foreground: {:?}", unsafe { GetForegroundWindow() });

        let start = std::time::Instant::now();
        let mut found = None;
        for _ in 0..100 {
            found = FullscreenDetector::active(&taskbar, None);
        }
        println!(
            "fullscreen app: {found:?} ({:?} per check)",
            start.elapsed() / 100
        );
    }
}
