//! "Move current window to desktop N" actions and their right-click menu.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowTextW,
    IsWindowVisible, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    TrackPopupMenu, WINDOW_EX_STYLE, WNDCLASSW, GWLP_USERDATA, MF_DISABLED, MF_GRAYED,
    MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_COMMAND, WS_POPUP,
};

use crate::shared::text::truncate;
use crate::window::activate::force_activate;
use crate::window::system_class::is_system_class;

use super::MENU_MOVE_BASE;

/// The window and desktop a right-click menu will act on. Captured before the
/// menu takes foreground, since the foreground window can change by the time the
/// menu result arrives.
#[derive(Clone, Copy)]
pub(super) struct MoveTarget {
    pub(super) hwnd: HWND,
    pub(super) desktop_index: usize,
}

/// Last window that had the keyboard focus and can be a move target.
///
/// Clicking the indicator makes Windows drop the foreground window: the click
/// deactivates the current app, but the indicator is a child of the taskbar and
/// cannot take the focus itself. [`DesktopMover::capture_foreground`] records the
/// window from `WM_MOUSEACTIVATE`, which arrives before that happens.
static LAST_FOREGROUND: AtomicIsize = AtomicIsize::new(0);

/// `hwnd` when it is a window a user would expect to move: visible, and not one
/// of the shell's or WinGlide's own windows.
unsafe fn move_target(hwnd: HWND) -> Option<HWND> {
    if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() {
        return None;
    }
    let mut class_buf = [0u16; 256];
    let len = GetClassNameW(hwnd, &mut class_buf);
    let class_name = String::from_utf16_lossy(&class_buf[..len.max(0) as usize]);
    if is_system_class(&class_name)
        || class_name == "WinGlideTray"
        || class_name == "TaskbarSwitcherIndicator"
    {
        return None;
    }
    Some(hwnd)
}

/// Stateless helper for the move-to-desktop feature.
pub(super) struct DesktopMover;

impl DesktopMover {
    /// Creates the hidden top-level window that owns the indicator's
    /// right-click menu.
    ///
    /// `TrackPopupMenu` needs an owner that can be the foreground window, and a
    /// child of the taskbar never can. This 0x0 window is never shown: it exists
    /// only to own the menu and to forward the chosen command back to the
    /// indicator, which holds the move target.
    pub(super) fn create_menu_host(indicator: HWND) -> HWND {
        unsafe {
            let Ok(hinstance) = GetModuleHandleW(None) else {
                return HWND(std::ptr::null_mut());
            };
            let wnd_class = WNDCLASSW {
                hInstance: HINSTANCE(hinstance.0),
                lpszClassName: w!("TaskbarIndicatorMenuHost"),
                lpfnWndProc: Some(menu_host_proc),
                ..Default::default()
            };
            let _ = RegisterClassW(&wnd_class);

            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("TaskbarIndicatorMenuHost"),
                w!("IndicatorMenuHost"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance.into()),
                None,
            ) else {
                return HWND(std::ptr::null_mut());
            };

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, indicator.0 as isize);
            hwnd
        }
    }

    /// Destroys a menu owner window created by [`Self::create_menu_host`].
    pub(super) fn destroy_menu_host(hwnd: HWND) {
        if !hwnd.is_invalid() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }

    /// The window a "move to desktop" action should act on: the one that had the
    /// focus before the indicator was clicked, falling back to the live
    /// foreground window.
    pub(super) fn foreground_target() -> Option<HWND> {
        let cached = LAST_FOREGROUND.load(Ordering::Relaxed);
        if cached != 0 {
            let hwnd = HWND(cached as *mut _);
            // Guard against the window having been closed since the event.
            if unsafe { IsWindowVisible(hwnd).as_bool() } {
                tracing::debug!("move target: 0x{cached:X} (last focused window)");
                return Some(hwnd);
            }
        }

        let live = unsafe { move_target(GetForegroundWindow()) };
        live
    }

    /// Records the current foreground window if it can be a move target.
    ///
    /// Called while the pointer hovers over the indicator and on
    /// `WM_MOUSEACTIVATE` - both happen before a click clears the foreground
    /// window. Cheap when nothing changed: the cached handle is compared first.
    pub(super) fn capture_foreground() {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0 as isize == LAST_FOREGROUND.load(Ordering::Relaxed) {
            return;
        }
        if let Some(target) = unsafe { move_target(hwnd) } {
            LAST_FOREGROUND.store(target.0 as isize, Ordering::Relaxed);
            tracing::debug!(
                "move target captured before click: 0x{:X}",
                target.0 as isize
            );
        }
    }

    /// Gives the focus back to the window the user was working in.
    ///
    /// Clicking the indicator drops the foreground window, so a click that did
    /// not land on a dot would otherwise leave the desktop without one.
    pub(super) fn restore_foreground() {
        if let Some(target) = Self::foreground_target() {
            unsafe {
                let _ = SetForegroundWindow(target);
            }
        }
    }

    /// Moves the current foreground window to `target` desktop and jumps to it.
    pub(super) fn move_foreground(target: &winvd::Desktop) {
        if let Some(hwnd) = Self::foreground_target() {
            move_and_jump_to_desktop(hwnd, target);
        }
    }

    /// Runs the action selected in the popup menu. `id` is the raw `WM_COMMAND`
    /// id (`MENU_MOVE_BASE` = move only, `+1` = move and jump).
    pub(super) fn handle_command(id: u32, target: MoveTarget) {
        let Ok(desktops) = winvd::get_desktops() else {
            return;
        };
        let Some(desktop) = desktops.get(target.desktop_index) else {
            return;
        };
        if id == MENU_MOVE_BASE as u32 {
            move_window_to_desktop_only(target.hwnd, desktop);
        } else {
            move_and_jump_to_desktop(target.hwnd, desktop);
        }
    }

    /// Shows the "Move to Desktop N" popup menu. The window title goes in a
    /// disabled header so the menu items stay short.
    ///
    /// `menu_host` is the hidden top-level window from
    /// [`Self::create_menu_host`]; it owns the menu and receives the command.
    pub(super) fn show_menu(menu_host: HWND, target: MoveTarget) {
        unsafe {
            let Ok(desktops) = winvd::get_desktops() else {
                return;
            };
            let Some(desktop) = desktops.get(target.desktop_index) else {
                return;
            };
            let n = target.desktop_index + 1; // 1-based label

            let mut title_buf = [0u16; 256];
            let len = GetWindowTextW(target.hwnd, &mut title_buf);
            let title = if len > 0 {
                truncate(&String::from_utf16_lossy(&title_buf[..len as usize]), 40)
            } else {
                "current window".to_string()
            };

            // Gray "move" out when the window is already on the clicked desktop.
            let current = winvd::get_desktop_by_window(std::mem::transmute(target.hwnd)).ok();
            let already_here = Some(desktop) == current.as_ref();

            let Ok(hmenu) = CreatePopupMenu() else {
                return;
            };

            let header = wide(&format!("\"{title}\""));
            let _ = AppendMenuW(
                hmenu,
                MF_STRING | MF_DISABLED | MF_GRAYED,
                0,
                PCWSTR(header.as_ptr()),
            );
            let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

            let move_label = wide(&format!("Move to Desktop {n}"));
            let _ = AppendMenuW(
                hmenu,
                if already_here {
                    MF_STRING | MF_DISABLED | MF_GRAYED
                } else {
                    MF_STRING
                },
                MENU_MOVE_BASE,
                PCWSTR(move_label.as_ptr()),
            );

            let jump_label = wide(&format!("Move and jump to Desktop {n}"));
            let _ = AppendMenuW(
                hmenu,
                MF_STRING,
                MENU_MOVE_BASE + 1,
                PCWSTR(jump_label.as_ptr()),
            );

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);

            // The hidden owner is a top-level window, so it can be foreground;
            // that is what makes the menu close on an outside click.
            let _ = SetForegroundWindow(menu_host);
            let _ = TrackPopupMenu(
                hmenu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                menu_host,
                None,
            );

            let _ = DestroyMenu(hmenu);

            // Give focus back to the app window so the indicator doesn't linger
            // as the foreground window (which would make the next right-click
            // target the indicator itself instead of the app).
            force_activate(target.hwnd);
        }
    }
}

/// Moves `hwnd` to `target` desktop without switching. Returns `true` when the
/// window was actually moved (not pinned, not already there).
fn move_window_to_desktop_only(hwnd: HWND, target: &winvd::Desktop) -> bool {
    // winvd links a different `windows` crate version, so the HWND must be
    // transmuted (both are transparent wrappers over a pointer).
    let whwnd = unsafe { std::mem::transmute(hwnd) };

    // Pinned (all-desktops) windows can't be moved.
    if winvd::is_pinned_window(whwnd).unwrap_or(false) {
        return false;
    }

    // Already on the target desktop -> nothing to do.
    if let Ok(win_desktop) = winvd::get_desktop_by_window(whwnd) {
        if &win_desktop == target {
            return false;
        }
    }

    winvd::move_window_to_desktop(*target, &whwnd).is_ok()
}

/// Moves `hwnd` to `target` desktop, switches to it and re-activates the window.
fn move_and_jump_to_desktop(hwnd: HWND, target: &winvd::Desktop) {
    if move_window_to_desktop_only(hwnd, target) {
        let _ = winvd::switch_desktop(*target);
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe { force_activate(hwnd) };
    }
}

/// Null-terminated UTF-16 copy of `s` for Win32 string parameters.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Window procedure of the hidden menu owner: hands the menu result to the
/// indicator (via `GWLP_USERDATA`) and does nothing else.
unsafe extern "system" fn menu_host_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COMMAND {
        let indicator = HWND(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _);
        if !indicator.is_invalid() {
            let _ = SendMessageW(indicator, WM_COMMAND, Some(wparam), Some(lparam));
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
