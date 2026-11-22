//! The indicator's user-facing settings, mirrored from [`AppConfig`].

use super::indicator_position::IndicatorPosition;
use crate::app_setting::AppConfig;

/// Visual settings of the desktop indicator.
///
/// Bundled so the window can carry them in a single `Cell` and the config ->
/// indicator mapping lives in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndicatorSettings {
    /// Draw the dots on the taskbar. The window itself can outlive this: while the
    /// tray badge is on it still listens for desktop changes, it just paints
    /// nothing.
    pub dots: bool,
    /// Placement preset on the taskbar.
    pub position: IndicatorPosition,
    /// Paint the dots with the Windows accent color.
    pub accent_color: bool,
    /// Draw a translucent background behind the dot row.
    pub background: bool,
}

impl IndicatorSettings {
    /// Reads the indicator settings out of the app configuration.
    pub fn from_config(config: &AppConfig) -> Self {
        Self {
            dots: config.desktop_indicator,
            position: IndicatorPosition::from_u8(config.indicator_position),
            accent_color: config.indicator_accent_color,
            background: config.indicator_background,
        }
    }
}
