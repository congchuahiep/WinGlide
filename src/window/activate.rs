//! Brings a window to the foreground with verification, trying a bounded ladder of
//! fallbacks instead of a single `SetForegroundWindow` call.
//!
//! # Why a ladder
//!
//! Windows restricts which processes may set the foreground window. A background app
//! such as WinGlide can satisfy those conditions in some situations and not others:
//! a window that is minimized, owned by a suspended UWP app, or simply on the wrong
//! side of the foreground lock will ignore a plain `SetForegroundWindow`. Each step
//! below covers a different failure mode, and **every step is verified** against the
//! actual foreground window before the next one runs.
//!
//! Verification cannot compare `GetForegroundWindow() == target`: UWP activation may
//! hand focus to the content window (a child of the frame) instead of the frame we
//! asked for. It therefore compares the whole [`LogicalWindow`] and its top-level
//! ancestor.

use std::mem::size_of;
use std::thread::sleep;
use std::time::{Duration, Instant};

use tracing::{debug, instrument};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, SetActiveWindow, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE,
    KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSE_EVENT_FLAGS, MOUSEINPUT, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowThreadProcessId, IsIconic, SetCursorPos, SetForegroundWindow, ShowWindowAsync,
    SwitchToThisWindow, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SW_RESTORE,
};

use super::logical_window::LogicalWindow;

/// How long to wait for a step to take effect before trying the next one.
const STEP_TIMEOUT: Duration = Duration::from_millis(90);

/// The taskbar click is asynchronous inside Explorer, so it gets a longer window.
const CLICK_TIMEOUT: Duration = Duration::from_millis(400);

/// The step that finally put the window in the foreground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationMethod {
    /// Plain `SetForegroundWindow`.
    SetForegroundWindow,
    /// Attached our input queue to the foreground/target thread, then raised the window.
    AttachThreadInput,
    /// The shell's `SwitchToThisWindow` (the Alt+Tab style switch).
    SwitchToThisWindow,
    /// Tapped ALT to unlock `SetForegroundWindow`, as documented by `LockSetForegroundWindow`.
    AltUnlock,
    /// Clicked the taskbar button, letting Explorer perform the activation itself.
    TaskbarClick,
}

/// Activates `target`, verifying the result of every step.
///
/// `button_rect` is the matching taskbar button's rectangle and enables the final
/// click fallback; the caller passes it only for buttons that map to a single window,
/// because clicking a grouped button opens the thumbnail flyout instead of switching.
///
/// Returns `None` when the window is already in the foreground (a no-op must never be
/// reported as progress) or when every step failed.
#[instrument(level = "debug", skip_all)]
pub fn activate(target: &LogicalWindow, button_rect: Option<RECT>) -> Option<ActivationMethod> {
    unsafe {
        if is_foreground(target) {
            debug!("'{}' is already in the foreground", target.title);
            return None;
        }

        restore_if_minimized(target);

        let _ = SetForegroundWindow(target.primary);
        if wait_until_foreground(target, STEP_TIMEOUT) {
            return Some(ActivationMethod::SetForegroundWindow);
        }

        attach_and_raise(target);
        if wait_until_foreground(target, STEP_TIMEOUT) {
            return Some(ActivationMethod::AttachThreadInput);
        }

        SwitchToThisWindow(target.primary, true);
        if wait_until_foreground(target, STEP_TIMEOUT) {
            return Some(ActivationMethod::SwitchToThisWindow);
        }

        tap_alt();
        let _ = SetForegroundWindow(target.primary);
        if wait_until_foreground(target, STEP_TIMEOUT) {
            return Some(ActivationMethod::AltUnlock);
        }

        if let Some(rect) = button_rect {
            if click_button(rect) && wait_until_foreground(target, CLICK_TIMEOUT) {
                return Some(ActivationMethod::TaskbarClick);
            }
        }

        None
    }
}

/// True when the foreground window is the target or one of its native windows.
fn is_foreground(target: &LogicalWindow) -> bool {
    target.contains(unsafe { GetForegroundWindow() })
}

fn wait_until_foreground(target: &LogicalWindow, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if is_foreground(target) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(10));
    }
}

/// Restores a minimized window before trying to focus it.
fn restore_if_minimized(target: &LogicalWindow) {
    let minimized = target.is_minimized || unsafe { IsIconic(target.primary).as_bool() };
    if minimized {
        debug!("Restoring minimized window '{:?}'", target.primary);
        let _ = unsafe { ShowWindowAsync(target.primary, SW_RESTORE) };
        sleep(Duration::from_millis(40));
    }
}

/// Attaches our input queue to the foreground and target threads, then raises the window.
///
/// Sharing the input state is what lets `SetActiveWindow`/`SetFocus` act on a window owned
/// by another thread, and is the classic way around the foreground lock.
fn attach_and_raise(target: &LogicalWindow) {
    unsafe {
        let current = GetCurrentThreadId();
        let foreground = GetForegroundWindow();
        let foreground_thread = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let target_thread = GetWindowThreadProcessId(target.primary, None);

        let mut attached = [0u32; 2];
        let mut attached_count = 0;

        for thread in [foreground_thread, target_thread] {
            if thread == 0 || thread == current || attached[..attached_count].contains(&thread) {
                continue;
            }
            if AttachThreadInput(current, thread, true).as_bool() {
                attached[attached_count] = thread;
                attached_count += 1;
            }
        }

        let _ = SetActiveWindow(target.primary);
        let _ = SetFocus(Some(target.primary));
        let _ = SetForegroundWindow(target.primary);
        let _ = BringWindowToTop(target.primary);

        for &thread in &attached[..attached_count] {
            let _ = AttachThreadInput(current, thread, false);
        }
    }
}

/// Taps ALT, which makes Windows unlock `SetForegroundWindow` (see `LockSetForegroundWindow`).
fn tap_alt() {
    let inputs = [alt_input(false), alt_input(true)];
    unsafe {
        SendInput(&inputs, size_of::<INPUT>() as i32);
    }
}

fn alt_input(key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_MENU,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Clicks the centre of a taskbar button, letting Explorer activate the window.
///
/// The cursor is moved to the button and restored immediately afterwards. `SendInput`
/// returns 0 when UIPI blocks the injection, which is reported instead of retried.
fn click_button(rect: RECT) -> bool {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return false;
    }

    let x = rect.left + (rect.right - rect.left) / 2;
    let y = rect.top + (rect.bottom - rect.top) / 2;
    if !is_on_virtual_screen(x, y) {
        debug!("Taskbar button is off-screen at ({x}, {y}); not clicking");
        return false;
    }

    let mut previous = POINT::default();
    let have_previous = unsafe { GetCursorPos(&mut previous) }.is_ok();

    if unsafe { SetCursorPos(x, y) }.is_err() {
        return false;
    }
    sleep(Duration::from_millis(30));

    let inputs = [mouse_input(MOUSEEVENTF_LEFTDOWN), mouse_input(MOUSEEVENTF_LEFTUP)];
    let inserted = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };

    sleep(Duration::from_millis(30));
    if have_previous {
        let _ = unsafe { SetCursorPos(previous.x, previous.y) };
    }

    if inserted != inputs.len() as u32 {
        debug!("Taskbar click blocked ({inserted}/2 events inserted); UIPI or input lock");
        return false;
    }
    true
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn is_on_virtual_screen(x: i32, y: i32) -> bool {
    unsafe {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        width > 0
            && height > 0
            && x >= left
            && x < left + width
            && y >= top
            && y < top + height
    }
}

/// Kept for callers that only have a raw handle, e.g. the virtual-desktop jump path.
#[instrument(level = "debug", skip_all)]
pub(crate) unsafe fn force_activate(hwnd: HWND) -> bool {
    let foreground = GetForegroundWindow();
    if foreground == hwnd {
        return true;
    }

    if IsIconic(hwnd).as_bool() {
        let _ = ShowWindowAsync(hwnd, SW_RESTORE);
    }

    let _ = SetForegroundWindow(hwnd);
    let _ = BringWindowToTop(hwnd);

    if GetForegroundWindow() == hwnd {
        return true;
    }

    let current_thread = GetCurrentThreadId();
    let foreground_thread = if foreground.0.is_null() {
        0
    } else {
        GetWindowThreadProcessId(foreground, None)
    };

    if foreground_thread == 0 || foreground_thread == current_thread {
        return GetForegroundWindow() == hwnd;
    }

    let attached = AttachThreadInput(current_thread, foreground_thread, true).as_bool();
    let _ = SetForegroundWindow(hwnd);
    let _ = BringWindowToTop(hwnd);
    if attached {
        let _ = AttachThreadInput(current_thread, foreground_thread, false);
    }

    GetForegroundWindow() == hwnd
}

