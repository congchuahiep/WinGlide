//! A visible desktop window, used for matching taskbar buttons to windows.

use windows::Win32::Foundation::{HWND, RECT};

/// Window class of the UWP frame hosted by `ApplicationFrameHost.exe`.
pub const UWP_FRAME_CLASS: &str = "ApplicationFrameWindow";

/// Window class of the actual UWP content window (`Windows.UI.Core.CoreWindow`).
///
/// On Windows 11 this window is usually a child of the [`UWP_FRAME_CLASS`] window,
/// but it can also be enumerated as a top-level sibling while the app is suspended.
pub const UWP_CONTENT_CLASS: &str = "Windows.UI.Core.CoreWindow";

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

    /// Win32 window class name (e.g. `Zed::Window`, `ApplicationFrameWindow`)
    pub class_name: String,
}

impl WindowInfo {
    /// True when this window is the UWP frame hosted by `ApplicationFrameHost.exe`.
    pub fn is_uwp_frame(&self) -> bool {
        self.class_name == UWP_FRAME_CLASS
    }

    /// True when this window is the UWP content window of a packaged app.
    pub fn is_uwp_content(&self) -> bool {
        self.class_name == UWP_CONTENT_CLASS
    }
}
