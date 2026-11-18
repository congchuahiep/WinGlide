//! Taskbar data structures.

use windows::Win32::Foundation::RECT;

/// Taskbar button on the Windows 11 XAML taskbar, read from UI Automation.
///
/// It does not have a separate HWND because XAML elements are not Win32 windows.
#[derive(Debug, Clone)]
pub struct TaskbarButton {
    /// The display name of the button (e.g., "Chrome", "Edge - 3 running windows")
    pub name: String,

    /// Position and size on the screen (in pixels)
    pub rect: RECT,

    /// PID of the process owning the button (usually explorer.exe on Win11)
    pub process_id: i32,

    /// Automation ID from UIA. On Windows 11 this is one of:
    /// - `Appid: <AUMID>` for a combined button;
    /// - `Window: 0x<hwnd>` for a labelled ("never combine") button, which is the
    ///   exact window handle;
    /// - `Appid: TaskbarSwitcher_<hwnd>` for a window split by the uncombine feature.
    pub automation_id: Option<String>,
}
