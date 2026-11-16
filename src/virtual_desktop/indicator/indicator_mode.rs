//! Where the desktop indicator is drawn.

/// Rendering mode of the desktop indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorMode {
    /// Dots drawn on the taskbar (the default).
    Taskbar,
    /// The current desktop number inside the notification-area icon.
    TrayIcon,
}

impl IndicatorMode {
    /// Reads the mode from `AppConfig::indicator_mode`.
    ///
    /// 1 is reserved for the automatic mode that will switch to the tray icon
    /// only while the taskbar is crowded; until that exists it behaves like the
    /// taskbar dots.
    pub fn from_u8(value: u8) -> Self {
        match value {
            2 => Self::TrayIcon,
            _ => Self::Taskbar,
        }
    }

    /// `true` when the number lives in the notification area instead of on the taskbar.
    pub fn is_tray(self) -> bool {
        matches!(self, Self::TrayIcon)
    }
}