//! "Move current window to desktop N" actions and their right-click menu.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetClassNameW, GetCursorPos, GetForegroundWindow,
    GetWindowLongW, GetWindowTextW, IsWindowVisible, SetForegroundWindow, SetWindowLongW,
    TrackPopupMenu, GWL_EXSTYLE, MF_DISABLED, MF_GRAYED, MF_SEPARATOR, MF_STRING, TPM_LEFTALIGN,
    TPM_RIGHTBUTTON, WS_EX_NOACTIVATE,
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

/// Stateless helper for the move-to-desktop feature.
pub(super) struct DesktopMover;

impl DesktopMover {
    /// The current foreground application window, rejecting system/hidden windows.
    pub(super) fn foreground_target() -> Option<HWND> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() {
                return None;
            }
            let mut class_buf = [0u16; 256];
            let len = GetClassNameW(hwnd, &mut class_buf);
            if len > 0 {
                let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
                // Never treat our own windows (tray / indicator) as move targets.
                if is_system_class(&class_name)
                    || class_name == "WinGlideTray"
                    || class_name == "TaskbarSwitcherIndicator"
                {
                    return None;
                }
            }
            Some(hwnd)
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
    pub(super) fn show_menu(indicator_hwnd: HWND, target: MoveTarget) {
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

            // TrackPopupMenu needs its owner window to be foreground. The
            // indicator is WS_EX_NOACTIVATE, so temporarily clear that style
            // while the menu is shown.
            let ex_style = GetWindowLongW(indicator_hwnd, GWL_EXSTYLE);
            let _ = SetWindowLongW(
                indicator_hwnd,
                GWL_EXSTYLE,
                (ex_style as u32 & !WS_EX_NOACTIVATE.0) as i32,
            );
            let _ = SetForegroundWindow(indicator_hwnd);
            let _ = TrackPopupMenu(
                hmenu,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                indicator_hwnd,
                None,
            );
            let _ = SetWindowLongW(indicator_hwnd, GWL_EXSTYLE, ex_style);

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
