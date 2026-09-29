//! Placement preset of the indicator.

/// Placement of the virtual desktop indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorPosition {
    /// Alignment-aware: the start edge (left / top of a vertical bar) or just
    /// before the system tray, mirroring the taskbar alignment.
    Auto = 0,
    /// Fixed at the start of the taskbar (left edge / top of a vertical bar).
    Left = 1,
    /// Fixed just before the system tray (right / bottom of a vertical bar).
    Right = 2,
}

impl IndicatorPosition {
    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Left,
            2 => Self::Right,
            _ => Self::Auto,
        }
    }
}
