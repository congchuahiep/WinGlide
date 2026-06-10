use std::sync::atomic::Ordering;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE, WM_COMMAND,
};

pub enum InstanceType {
    Background,
    SettingsUI,
}

/// Ensures that only a single instance of the application is running.
///
/// # Returns
/// - `true` if this is the single instance (allowed to run).
/// - `false` if another instance is already running, opening the existing instance instead.
pub fn ensure_single_instance(instance_type: InstanceType) -> bool {
    unsafe {
        match instance_type {
            InstanceType::SettingsUI => {
                let mutex_name = windows::core::w!("Global\\WinGlide_SettingsUIMutex");
                let _ = CreateMutexW(None, false, mutex_name).unwrap_or_default();
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    if let Ok(hwnd) =
                        FindWindowW(windows::core::PCWSTR::null(), windows::core::w!("WinGlide"))
                    {
                        if !hwnd.is_invalid() {
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                            let _ = SetForegroundWindow(hwnd);
                        }
                    }
                    return false;
                }
            }
            InstanceType::Background => {
                let mutex_name = windows::core::w!("Global\\WinGlide_BackgroundMutex");
                let _ = CreateMutexW(None, false, mutex_name).unwrap_or_default();
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    if let Ok(hwnd) = FindWindowW(
                        windows::core::w!("WinGlideTray"),
                        windows::core::PCWSTR::null(),
                    ) {
                        if !hwnd.is_invalid() {
                            let _ = PostMessageW(
                                Some(hwnd),
                                WM_COMMAND,
                                WPARAM(crate::tray_icon::IDM_SETTINGS as usize),
                                LPARAM(0),
                            );
                        }
                    }
                    return false;
                }
            }
        }
        true
    }
}

pub fn attach_debug_console() {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
    crate::logging::console::DEBUG_CLI_MODE.store(true, Ordering::SeqCst);
}

pub fn setup_dpi_awareness() {
    unsafe {
        let _ =
            HiDpi::SetProcessDpiAwarenessContext(HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
