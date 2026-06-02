//! WinEvent hook để detect cửa sổ mới và uncombine ngay lập tức.
//!
//! Dùng `SetWinEventHook` với `EVENT_OBJECT_SHOW` để hook khi
//! một cửa sổ mới xuất hiện. Khi callback trigger, gửi custom message
//! đến main thread để xử lý uncombine.

use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use tracing::{debug, instrument};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindowVisible, PostThreadMessageW, EVENT_OBJECT_SHOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};

use crate::uncombine::is_window_tracked;

pub const WM_APP_UNCOMBINE: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 0x100;

static HOOK_HANDLE: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

pub unsafe fn install_hook() -> anyhow::Result<()> {
    MAIN_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

    let hook = SetWinEventHook(
        EVENT_OBJECT_SHOW,
        EVENT_OBJECT_SHOW,
        None,
        Some(win_event_proc),
        0,
        0,
        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
    );

    if hook.is_invalid() {
        anyhow::bail!("Failed to install WinEvent hook");
    }

    HOOK_HANDLE.store(hook.0, Ordering::SeqCst);
    debug!("WinEvent hook installed (EVENT_OBJECT_SHOW)");
    Ok(())
}

pub unsafe fn uninstall_hook() {
    let handle_ptr = HOOK_HANDLE.load(Ordering::SeqCst);
    if !handle_ptr.is_null() {
        let hook = HWINEVENTHOOK(handle_ptr);
        let _ = UnhookWinEvent(hook);
        HOOK_HANDLE.store(std::ptr::null_mut(), Ordering::SeqCst);
        debug!("WinEvent hook uninstalled");
    }
}

#[instrument(level = "debug", skip_all)]
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if id_object != 0 || id_child != 0 {
        return;
    }

    if event != EVENT_OBJECT_SHOW {
        return;
    }

    if hwnd.0.is_null() {
        return;
    }

    if !IsWindowVisible(hwnd).as_bool() {
        return;
    }

    if is_window_tracked(hwnd) {
        return;
    }

    debug!(
        "WinEvent: EVENT_OBJECT_SHOW: {event}, hwnd={:?}, id_object={id_object}, id_child={id_child}, hook={:?}",
        hwnd, _hook
    );

    let thread_id = MAIN_THREAD_ID.load(Ordering::SeqCst);
    if thread_id == 0 {
        return;
    }

    let _ = PostThreadMessageW(
        thread_id,
        WM_APP_UNCOMBINE,
        windows::Win32::Foundation::WPARAM(hwnd.0 as usize),
        windows::Win32::Foundation::LPARAM(0),
    );
}
