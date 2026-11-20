//! System Tray Icon management via the `Shell_NotifyIconW` API.
//!
//! This module is responsible for displaying and managing the application's icon in the Windows
//! System Tray.
//!
//! The tray slot is sized by the DPI of the taskbar that hosts the notification area, so the
//! embedded ICO carries one entry per standard DPI step (16/20/24/28/32/40/48 px) and
//! [`TrayIcon::resync`] rebuilds the HICON whenever that size or the system theme changes.
//! Handing the shell an icon that already matches the slot avoids the blurry downscale it
//! would otherwise do from a single large bitmap.
//!
//! Implementation pattern is referenced from [window-switcher](https://github.com/sigoden/window-switcher/blob/main/src/trayicon.rs).

use tracing::debug;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::shared::system_theme::is_light_theme;

const ICON_LIGHT_BYTES: &[u8] = include_bytes!("../../assets/tray-light.ico");
const ICON_DARK_BYTES: &[u8] = include_bytes!("../../assets/tray-dark.ico");

pub const IDM_EXIT: u32 = 1;
pub const IDM_SHOW_CONSOLE: u32 = 3;
pub const IDM_SETTINGS: u32 = 4;

const TEXT_SHOW_CONSOLE: PCWSTR = w!("Debug Console");
const TEXT_SETTINGS: PCWSTR = w!("Settings...");
const TEXT_EXIT: PCWSTR = w!("Exit");

/// Manages the lifecycle and behavior of the icon in the Windows system tray.
pub struct TrayIcon {
    /// Data structure containing the configuration information of the system tray icon.
    data: NOTIFYICONDATAW,
    /// Pixel size, theme and badge the current `hIcon` was built for, so `resync` can skip
    /// the whole rebuild when nothing relevant changed.
    icon_key: (i32, bool, Option<usize>),
    /// Desktop number drawn into the icon, or `None` for the plain glyph.
    badge: Option<usize>,
}

impl TrayIcon {
    /// Creates a new `TrayIcon` instance not yet associated with any window.
    ///
    /// Loads the icon entry that matches the taskbar's current DPI and sets the default tooltip.
    pub fn create() -> Self {
        let size = Self::icon_size();
        let light = is_light_theme();
        let data = Self::create_nid(Self::get_hicon(size, light, None));
        Self {
            data,
            icon_key: (size, light, None),
            badge: None,
        }
    }

    /// Shows `number` inside the icon, or restores the plain glyph with `None`.
    ///
    /// Called when the desktop changes while the indicator lives in the
    /// notification area. Safe to call before [`Self::register`]: the handle is
    /// swapped and the shell is only told once the icon exists.
    pub fn set_badge(&mut self, number: Option<usize>) {
        if self.badge == number {
            return;
        }
        self.badge = number;

        let size = Self::icon_size();
        let light = is_light_theme();
        let new_hicon = Self::get_hicon(size, light, number);
        let old_hicon = self.data.hIcon;
        self.data.hIcon = new_hicon;
        self.icon_key = (size, light, number);

        unsafe {
            if !self.data.hWnd.is_invalid() {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data);
            }
            let _ = DestroyIcon(old_hicon);
        }
        debug!("TrayIcon badge updated ({number:?})");
    }

    /// Rebuilds the icon when the tray's size or the system theme no longer matches what the
    /// current `hIcon` was created for, and pushes it to the shell with `NIM_MODIFY`.
    ///
    /// Cheap enough to call on every relevant broadcast: it only reads the taskbar DPI and the
    /// theme, and returns without touching the shell when both are unchanged.
    pub fn resync(&mut self) {
        let size = Self::icon_size();
        let light = is_light_theme();
        if self.icon_key == (size, light, self.badge) {
            return;
        }

        let new_hicon = Self::get_hicon(size, light, self.badge);
        let old_hicon = self.data.hIcon;
        self.data.hIcon = new_hicon;
        self.icon_key = (size, light, self.badge);

        unsafe {
            // `NIM_MODIFY` needs a registered icon; at startup the window handle is still null.
            if !self.data.hWnd.is_invalid() {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data);
            }
            let _ = DestroyIcon(old_hicon);
        }
        debug!("TrayIcon resynced (size={size}px, light_mode={light})");
    }

    /// Registers the system tray icon with the Windows Shell and associates it with the message receiving window.
    ///
    /// # Errors
    /// Returns an error if the `Shell_NotifyIconW` API fails to add the icon (`NIM_ADD`).
    pub fn register(&mut self, hwnd: HWND) -> anyhow::Result<()> {
        self.data.hWnd = hwnd;
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &self.data)
                .ok()
                .map_err(|e| anyhow::anyhow!("Shell_NotifyIconW(NIM_ADD): {e}"))?;
        }
        debug!("TrayIcon registered, hwnd={:?}", hwnd);
        Ok(())
    }

    /// Checks if the current system tray icon exists.
    ///
    /// Performed by sending a modify command (`NIM_MODIFY`). Returns `true` if successful.
    #[allow(dead_code)]
    pub fn exists(&mut self) -> bool {
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data) }.as_bool()
    }

    /// Displays a Popup context menu at the current mouse cursor position.
    ///
    /// # Important Note (Thread-safety)
    /// This function **must be called from the WndProc thread** (the same STA thread managing the hidden message
    /// window). The reason is that the `TrackPopupMenu` API needs to process `WM_COMMAND` messages
    /// synchronously.
    pub fn show(&self, console_visible: bool) -> anyhow::Result<()> {
        let hwnd = self.data.hWnd;
        let mut cursor = POINT::default();
        unsafe {
            // Bring the hidden window to the foreground so that when clicking outside the menu, the menu automatically closes
            // (required by Win32 documentation).
            let _ = SetForegroundWindow(hwnd);
            GetCursorPos(&mut cursor)?;
            let hmenu = Self::create_menu(console_visible)?;
            let _ = TrackPopupMenu(
                hmenu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
                cursor.x,
                cursor.y,
                None,
                hwnd,
                None,
            );
            DestroyMenu(hmenu)?;
        }
        Ok(())
    }

    /// Reregisters the icon with the system tray.
    ///
    /// Usually called after receiving the `WM_TASKBARCREATED` message notifying that Windows
    /// Explorer (explorer.exe) has just restarted and the system tray was cleared beforehand.
    pub fn reregister(&mut self) -> anyhow::Result<()> {
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &self.data)
                .ok()
                .map_err(|e| anyhow::anyhow!("Shell_NotifyIconW(NIM_ADD) reregister: {e}"))?;
        }
        debug!("TrayIcon re-registered after TaskbarCreated");
        Ok(())
    }

    /// Builds the HICON that best matches `size`, picking the light or dark artwork.
    ///
    /// The ICO directory is parsed here instead of calling `LookupIconIdFromDirectoryEx`: on
    /// Windows 11 that API returns the first directory entry for in-memory ICO data regardless of
    /// the requested size, which would upscale the 16 px artwork at every other DPI. The entry
    /// closest to `size` is decoded at exactly `size`, preferring the larger one on a tie so an
    /// unusual DPI scales down rather than up.
    fn get_hicon(size: i32, light: bool, badge: Option<usize>) -> HICON {
        if let Some(number) = badge {
            if let Some(icon) = super::tray_badge::build(size, light, number) {
                return icon;
            }
        }

        let bytes = Self::artwork_bytes(light);
        let (_, offset, length) =
            Self::pick_entry(bytes, size).expect("embedded tray icon has no usable entry");
        unsafe {
            CreateIconFromResourceEx(
                &bytes[offset..offset + length],
                true,
                0x00030000,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
            .expect("Failed to decode embedded tray icon")
        }
    }

    /// ICO bytes of the light or dark tray artwork.
    pub(super) fn artwork_bytes(light: bool) -> &'static [u8] {
        if light {
            ICON_LIGHT_BYTES
        } else {
            ICON_DARK_BYTES
        }
    }

    /// `(offset, length)` of the ICO entry closest to `size`, for the badge renderer.
    pub(super) fn artwork_entry(size: i32, light: bool) -> Option<(usize, usize)> {
        Self::pick_entry(Self::artwork_bytes(light), size).map(|(_, offset, length)| (offset, length))
    }

    /// Returns `(dimension, offset, length)` of the ICO entry closest to `size`.
    fn pick_entry(bytes: &[u8], size: i32) -> Option<(i32, usize, usize)> {
        // ICONDIR: reserved u16, type u16, count u16, then 16 bytes per entry.
        let count = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?) as usize;
        let mut best: Option<(i32, i32, usize, usize)> = None; // distance, dimension, offset, length
        for index in 0..count {
            let at = 6 + index * 16;
            let Some(entry) = bytes.get(at..at + 16) else {
                break;
            };
            let dimension = if entry[0] == 0 {
                256
            } else {
                entry[0] as i32
            };
            let length = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
            if offset + length > bytes.len() {
                continue;
            }
            let distance = (dimension - size).abs();
            let better = match best {
                None => true,
                Some((best_distance, best_dimension, _, _)) => {
                    distance < best_distance
                        || (distance == best_distance && dimension > best_dimension)
                }
            };
            if better {
                best = Some((distance, dimension, offset, length));
            }
        }
        best.map(|(_, dimension, offset, length)| (dimension, offset, length))
    }

    /// Size of a taskbar notification-area icon at the taskbar's current DPI.
    ///
    /// Windows draws the tray buttons at `SM_CXSMICON` for the monitor that hosts the notification
    /// area, which on Windows 11 is the primary taskbar (`Shell_TrayWnd`).
    fn icon_size() -> i32 {
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, Self::tray_dpi()) };
        if size > 0 {
            size
        } else {
            16
        }
    }

    /// DPI of the taskbar that owns the notification area, falling back to the system DPI.
    fn tray_dpi() -> u32 {
        unsafe {
            if let Ok(hwnd) = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) {
                let dpi = GetDpiForWindow(hwnd);
                if dpi != 0 {
                    return dpi;
                }
            }
            GetDpiForSystem()
        }
    }

    /// Creates and sets the default `NOTIFYICONDATAW` configuration information for the system tray icon.
    fn create_nid(hicon: HICON) -> NOTIFYICONDATAW {
        let mut tooltip: Vec<u16> = "WinGlide".encode_utf16().collect();
        tooltip.resize(128, 0);
        let tooltip: [u16; 128] = tooltip.try_into().expect("tooltip too long");

        NOTIFYICONDATAW {
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_USER + 0x200, // WM_USER_TRAYICON
            hIcon: hicon,
            szTip: tooltip,
            ..Default::default()
        }
    }

    /// Creates a Popup menu containing application configuration options.
    ///
    /// The menu includes:
    /// - "Debug Console" option
    /// - "Settings..." option
    /// - Separator
    /// - "Exit" option to close the application
    fn create_menu(console_visible: bool) -> anyhow::Result<HMENU> {
        unsafe {
            let hmenu = CreatePopupMenu()?;

            let console_flags = MF_STRING
                | if crate::logging::console::DEBUG_CLI_MODE
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    MF_GRAYED | MF_DISABLED
                } else if console_visible {
                    MF_CHECKED
                } else {
                    MF_UNCHECKED
                };
            AppendMenuW(
                hmenu,
                console_flags,
                IDM_SHOW_CONSOLE as usize,
                TEXT_SHOW_CONSOLE,
            )?;

            AppendMenuW(hmenu, MF_STRING, IDM_SETTINGS as usize, TEXT_SETTINGS)?;
            AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null())?;
            AppendMenuW(hmenu, MF_STRING, IDM_EXIT as usize, TEXT_EXIT)?;

            Ok(hmenu)
        }
    }
}

/// When the `TrayIcon` instance is destroyed (Out of scope / Drop), automatically remove the icon from the system tray.
impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
            let _ = DestroyIcon(self.data.hIcon);
        }
        debug!("TrayIcon dropped, icon removed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HGDIOBJ};
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    /// Dimensions of the colour bitmap behind an HICON, which is what the shell draws.
    fn hicon_size(hicon: HICON) -> (i32, i32) {
        unsafe {
            let mut info = ICONINFO::default();
            GetIconInfo(hicon, &mut info).expect("GetIconInfo");
            let mut bitmap = BITMAP::default();
            let read = GetObjectW(
                HGDIOBJ(info.hbmColor.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bitmap as *mut _ as *mut core::ffi::c_void),
            );
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
            let _ = DestroyIcon(hicon);
            assert_ne!(read, 0, "GetObjectW failed");
            (bitmap.bmWidth, bitmap.bmHeight)
        }
    }

    #[test]
    fn pick_entry_matches_the_requested_size() {
        for size in [16, 20, 24, 28, 32, 40, 48] {
            let (dimension, _, _) =
                TrayIcon::pick_entry(ICON_LIGHT_BYTES, size).expect("no entry");
            assert_eq!(dimension, size, "no exact entry for {size}px");
        }
    }

    #[test]
    fn pick_entry_rounds_up_for_unsupported_sizes() {
        // 225% and 275% scaling ask for sizes the artwork does not cover. Both are a tie between
        // the neighbouring entries, and the larger one has to win so the shell scales the icon
        // down instead of up.
        for size in [36, 44] {
            let (dimension, _, _) =
                TrayIcon::pick_entry(ICON_LIGHT_BYTES, size).expect("no entry");
            assert!(
                dimension > size,
                "{size}px should round up, got {dimension}px"
            );
        }
    }

    #[test]
    fn every_entry_decodes_at_the_requested_size() {
        for size in [16, 20, 24, 28, 32, 40, 48] {
            assert_eq!(hicon_size(TrayIcon::get_hicon(size, true)), (size, size));
            assert_eq!(hicon_size(TrayIcon::get_hicon(size, false)), (size, size));
        }
    }
}
