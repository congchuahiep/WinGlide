//! Win32 window classes that belong to the shell/OS and must be ignored.

/// Checks whether a given class name is a system class.
pub fn is_system_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "Progman" | "WorkerW" |  // Desktop
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" |  // Taskbar
        "Windows.UI.Composition.DesktopWindowContentBridge" |  // XAML bridge
        "ApplicationFrame" |  // UWP frame
        "IME" | "MSCTFIME UI" | "IMEUI" |  // IME
        "tooltips_class32" |  // Tooltips
        "DwmWindowComposition" |  // DWM
        "SysAnimate32" // Animation
    )
}
