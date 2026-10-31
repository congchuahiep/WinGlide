//! Groups raw top-level windows into *logical app windows*.
//!
//! A UWP/packaged app on Windows 11 is represented by two windows:
//! - `ApplicationFrameWindow`, owned by `ApplicationFrameHost.exe` — the frame the
//!   shell draws and the window the taskbar associates with the button;
//! - `Windows.UI.Core.CoreWindow`, owned by the app process — the actual content.
//!
//! The content window is normally a child of the frame, but it can also show up as a
//! separate top-level window (observed while the app is suspended). Treating the two
//! as independent candidates makes the cycle pick a window it cannot activate, or fail
//! to recognise the foreground window. [`LogicalWindow`] keeps them together:
//! activation always targets [`LogicalWindow::primary`] (the frame), while
//! [`LogicalWindow::contains`] recognises either handle as "this app window".

use windows_core::BOOL;
use windows::Win32::Foundation::{FALSE, HWND, LPARAM, RECT, TRUE};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetAncestor, GetClassNameW, IsIconic, GA_ROOT, GA_ROOTOWNER,
};

use super::enumerate::find_window_by_hwnd;
use super::types::{WindowInfo, UWP_CONTENT_CLASS};

/// How close a top-level content window must be to its frame to be paired with it.
/// The content window sits inside the frame's client area, so a small tolerance is
/// enough to absorb DPI rounding without ever pairing unrelated windows.
const PAIR_TOLERANCE_PX: i32 = 8;

/// One logical app window, backed by one or more native windows.
#[derive(Debug, Clone)]
pub struct LogicalWindow {
    /// Window that should be activated (the frame for UWP apps).
    pub primary: HWND,

    /// All native windows that represent this app window.
    pub members: Vec<WindowInfo>,

    /// Display title, taken from `primary` when available.
    pub title: String,

    /// Whether the window is minimized.
    pub is_minimized: bool,

    /// Window class of `primary`, used in diagnostics.
    pub class_name: String,
}

impl LogicalWindow {
    fn new(primary: WindowInfo, members: Vec<WindowInfo>) -> Self {
        let is_minimized = unsafe { IsIconic(primary.hwnd).as_bool() };
        let title = if primary.title.is_empty() {
            members
                .iter()
                .map(|m| m.title.as_str())
                .find(|t| !t.is_empty())
                .unwrap_or_default()
                .to_string()
        } else {
            primary.title.clone()
        };

        Self {
            primary: primary.hwnd,
            members,
            title,
            is_minimized,
            class_name: primary.class_name,
        }
    }

    /// True when `hwnd` belongs to this app window.
    ///
    /// Besides the recorded members, this resolves child windows to their top-level
    /// ancestor and owned windows (dialogs) to their root owner. That covers the usual
    /// UWP layout where the content window is a child of the frame, and dialogs whose
    /// foreground handle is not the window the taskbar button represents.
    pub fn contains(&self, hwnd: HWND) -> bool {
        if hwnd.0.is_null() {
            return false;
        }
        if self.members.iter().any(|m| m.hwnd == hwnd) {
            return true;
        }

        unsafe {
            GetAncestor(hwnd, GA_ROOT) == self.primary
                || GetAncestor(hwnd, GA_ROOTOWNER) == self.primary
        }
    }

    /// AUMIDs of every member window, used for exact taskbar-button matching.
    pub fn aumids(&self) -> impl Iterator<Item = &str> {
        self.members
            .iter()
            .filter_map(|m| m.aumid.as_deref())
            .filter(|a| !a.is_empty())
    }

    /// Process IDs of every member window.
    pub fn process_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.members.iter().map(|m| m.process_id)
    }

    /// Executable name of the app, ignoring the shared `ApplicationFrameHost.exe` host.
    pub fn process_name(&self) -> &str {
        self.members
            .iter()
            .map(|m| m.process_name.as_str())
            .find(|name| !name.is_empty() && *name != "applicationframehost.exe")
            .unwrap_or("")
    }
}

/// Builds [`LogicalWindow`]s from a raw top-level window enumeration.
///
/// Order is preserved (the enumeration order is the z-order), so the first window of a
/// group is the one closest to the top of the z-order. Unpaired content windows are
/// dropped: they are never a valid activation target on their own.
pub fn group_logical_windows(windows: Vec<WindowInfo>) -> Vec<LogicalWindow> {
    let mut used = vec![false; windows.len()];
    let mut result: Vec<(usize, LogicalWindow)> = Vec::new();

    // 1. Every frame becomes a logical window; pair it with its top-level content window
    //    when the app is suspended and the content window is enumerated separately.
    for (index, frame) in windows.iter().enumerate() {
        if !frame.is_uwp_frame() {
            continue;
        }

        used[index] = true;
        let mut members = vec![frame.clone()];

        if let Some(content) = best_content_match(&windows, &used, frame) {
            used[content] = true;
            members.push(windows[content].clone());
        } else if let Some(content) = find_content_child(frame.hwnd) {
            // Normal layout: the content window is a child of the frame and therefore
            // never appears in the top-level enumeration. Reading it gives us the app
            // process and its package AUMID, which the frame itself does not expose.
            members.push(content);
        }

        result.push((index, LogicalWindow::new(frame.clone(), members)));
    }

    // 2. Content windows without a frame are not app windows; everything else stands alone.
    for (index, window) in windows.iter().enumerate() {
        if used[index] || window.is_uwp_content() {
            continue;
        }
        result.push((index, LogicalWindow::new(window.clone(), vec![window.clone()])));
    }

    result.sort_by_key(|(order, _)| *order);
    result.into_iter().map(|(_, window)| window).collect()
}

/// Finds the content window that belongs to `frame`: same title, contained in the frame
/// rectangle, and not already paired. The smallest match wins when several qualify.
fn best_content_match(windows: &[WindowInfo], used: &[bool], frame: &WindowInfo) -> Option<usize> {
    let frame_rect = frame.rect;

    windows
        .iter()
        .enumerate()
        .filter(|(index, window)| {
            !used[*index]
                && window.is_uwp_content()
                && !window.title.is_empty()
                && window.title.eq_ignore_ascii_case(&frame.title)
        })
        .filter(|(_, window)| rect_contains(frame_rect, window.rect))
        .min_by_key(|(_, window)| rect_area(window.rect))
        .map(|(index, _)| index)
}

fn rect_contains(outer: RECT, inner: RECT) -> bool {
    inner.left >= outer.left - PAIR_TOLERANCE_PX
        && inner.top >= outer.top - PAIR_TOLERANCE_PX
        && inner.right <= outer.right + PAIR_TOLERANCE_PX
        && inner.bottom <= outer.bottom + PAIR_TOLERANCE_PX
}

fn rect_area(rect: RECT) -> i64 {
    let width = (rect.right - rect.left).max(0) as i64;
    let height = (rect.bottom - rect.top).max(0) as i64;
    width * height
}

/// Finds the `Windows.UI.Core.CoreWindow` child of a UWP frame, if there is one.
fn find_content_child(frame: HWND) -> Option<WindowInfo> {
    struct Search {
        found: Option<HWND>,
    }

    unsafe extern "system" fn callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = &mut *(lparam.0 as *mut Search);

        let mut buffer = [0u16; 256];
        let length = GetClassNameW(hwnd, &mut buffer);
        if length > 0
            && String::from_utf16_lossy(&buffer[..length as usize]) == UWP_CONTENT_CLASS
        {
            search.found = Some(hwnd);
            return FALSE;
        }

        TRUE
    }

    let mut search = Search { found: None };
    let _ = unsafe {
        EnumChildWindows(
            Some(frame),
            Some(callback),
            LPARAM(&mut search as *mut _ as isize),
        )
    };

    search
        .found
        .map(|hwnd| unsafe { find_window_by_hwnd(hwnd) })
}
