//! Gives the Settings window a title bar icon.
//!
//! WinUI 3 does not inherit the executable's icon: without an explicit `WM_SETICON`
//! the title bar keeps the generic placeholder. `windows-reactor` creates the window
//! without exposing its HWND, so the icon is applied from a short-lived thread that
//! waits for the first visible window of this process.

use std::sync::OnceLock;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible, LoadImageW,
    SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON,
    WM_SETICON,
};

/// `winresource` embeds `assets/app.ico` as resource id 1.
const APP_ICON_RESOURCE: PCWSTR = PCWSTR(1 as *const u16);

/// Windows does not take ownership of the icons handed to `WM_SETICON`, so they are
/// kept alive for the lifetime of the process.
static ICONS: OnceLock<(isize, isize)> = OnceLock::new();

struct Search {
    pid: u32,
    found: Option<HWND>,
}

/// Applies the application icon to the first visible window of this process, retrying
/// for a few seconds because the window does not exist yet when the UI starts.
pub fn apply() {
    std::thread::spawn(|| {
        let pid = std::process::id();
        for _ in 0..200 {
            let mut search = Search { pid, found: None };
            unsafe {
                let _ = EnumWindows(
                    Some(enum_proc),
                    LPARAM(&mut search as *mut Search as isize),
                );
            }
            if let Some(hwnd) = search.found {
                unsafe { set_icons(hwnd) };
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    });
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    let mut pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let titled = unsafe { GetWindowTextLengthW(hwnd) } > 0;
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
    if pid == search.pid && visible && titled {
        search.found = Some(hwnd);
        return BOOL(0);
    }
    BOOL(1)
}

unsafe fn set_icons(hwnd: HWND) {
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let sizes = [
        (unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16), ICON_SMALL),
        (unsafe { GetSystemMetricsForDpi(SM_CXICON, dpi) }.max(32), ICON_BIG),
    ];

    let mut handles = (0isize, 0isize);
    for (index, (size, slot)) in sizes.into_iter().enumerate() {
        let loaded = unsafe {
            LoadImageW(
                Some(HINSTANCE(module.0)),
                APP_ICON_RESOURCE,
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
        };
        if let Ok(icon) = loaded {
            unsafe {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(slot as usize)),
                    Some(LPARAM(icon.0 as isize)),
                )
            };
            if index == 0 {
                handles.0 = icon.0 as isize;
            } else {
                handles.1 = icon.0 as isize;
            }
        }
    }
    let _ = ICONS.set(handles);
}
