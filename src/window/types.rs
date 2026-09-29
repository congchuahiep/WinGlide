//! A visible desktop window, used for matching taskbar buttons to windows.

use windows::Win32::Foundation::{HWND, RECT};

/// Visible window on the desktop, used for matching buttons to windows.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    /// Window handle (HWND)
    pub hwnd: HWND,

    /// Window title
    pub title: String,

    /// Process ID
    pub process_id: u32,

    /// Position and size
    pub rect: RECT,

    /// Executable file name (e.g., "chrome.exe")
    pub process_name: String,

    /// AppUserModelID (AUMID) of the window
    pub aumid: Option<String>,
}
