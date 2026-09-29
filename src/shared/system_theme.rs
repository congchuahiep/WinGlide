//! Reads the Windows theme preference and the accent color.

use windows::core::{w, PCWSTR};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

/// `true` when Windows is configured to use the light system theme.
///
/// Read from `HKCU\...\Themes\Personalize\SystemUsesLightTheme` (0 = dark,
/// 1 = light).
pub fn is_light_theme() -> bool {
    read_dword(
        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
        w!("SystemUsesLightTheme"),
    )
    .map_or(false, |value| value != 0)
}

/// The Windows accent color as `(r, g, b)`, or `None` when it can't be read.
///
/// Read from `HKCU\Software\Microsoft\Windows\DWM\AccentColor` - the value the
/// Windows 11 shell paints taskbar/Start highlights with. It is stored as a
/// DWORD in ABGR order (`0xAABBGGRR`), e.g. `0xFFD77800` is `#0078D7`.
pub fn accent_color() -> Option<(u8, u8, u8)> {
    let value = read_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))?;
    Some((
        (value & 0xFF) as u8,
        ((value >> 8) & 0xFF) as u8,
        ((value >> 16) & 0xFF) as u8,
    ))
}

/// Reads a `REG_DWORD` under `HKEY_CURRENT_USER`, or `None` when absent.
fn read_dword(sub_key: PCWSTR, value_name: PCWSTR) -> Option<u32> {
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub_key,
            value_name,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    res.is_ok().then_some(value)
}
