//! Single-instance enforcement via named mutexes.
//!
//! Two independent mutexes exist because the background engine and the Settings
//! UI are separate processes that may each run at most once. When a second
//! instance starts, it instead signals the running one to come to the front.

use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SetForegroundWindow, ShowWindow, SW_RESTORE, WM_COMMAND,
};

use crate::tray::IDM_SETTINGS;

/// Specifies the type of application instance to check for uniqueness.
pub enum InstanceType {
    /// The invisible background WinGlide engine.
    Background,
    /// The foreground XAML settings user interface.
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
                                WPARAM(IDM_SETTINGS as usize),
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
