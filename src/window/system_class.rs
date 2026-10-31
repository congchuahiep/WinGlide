//! Win32 window classes that belong to the shell/OS and must be ignored.

/// Checks whether a given class name is a system class.
///
/// The UWP `ApplicationFrameWindow` / `Windows.UI.Core.CoreWindow` pair is **not**
/// filtered here: the frame is the window the taskbar shows and the only handle that
/// can be activated, and [`crate::window::logical_window`] groups it with its content
/// window. Filtering either one would break UWP activation.
pub fn is_system_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman" | "WorkerW" |  // Desktop
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" |  // Taskbar
        "Windows.UI.Composition.DesktopWindowContentBridge" |  // XAML bridge
        "IME" | "MSCTFIME UI" | "IMEUI" |  // IME
        "tooltips_class32" |  // Tooltips
        "DwmWindowComposition" |  // DWM
        "SysAnimate32" // Animation
    )
}
