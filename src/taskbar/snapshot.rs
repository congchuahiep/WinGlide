//! A snapshot of the primary taskbar window (`Shell_TrayWnd`).
//!
//! [`Taskbar`] records which screen edge the bar is on, its thickness, DPI and
//! labels mode. The desktop indicator uses it to size and place itself;
//! secondary taskbars (`Shell_SecondaryTrayWnd`) are not handled here.

use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetWindowRect};

/// Standard thickness (px) of an icons-only taskbar at 96 DPI. A vertical
/// taskbar whose width is much larger than this is showing button labels.
const ICON_TASKBAR_THICKNESS: i32 = 48;

/// Which screen edge the primary taskbar occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl TaskbarEdge {
    /// `true` for a left/right (vertical) taskbar, `false` for top/bottom.
    pub fn is_vertical(self) -> bool {
        matches!(self, TaskbarEdge::Left | TaskbarEdge::Right)
    }
}

/// A snapshot of the primary taskbar (`Shell_TrayWnd`).
#[derive(Clone, Copy)]
pub struct Taskbar {
    hwnd: HWND,
    rect: RECT,
    edge: TaskbarEdge,
}

impl Taskbar {
    /// Snapshot the primary taskbar, or `None` when it can't be found/measured.
    pub fn primary() -> Option<Self> {
        unsafe {
            let hwnd = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).ok()?;
            Some(Self {
                hwnd,
                rect,
                edge: detect_edge(rect),
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn rect(&self) -> RECT {
        self.rect
    }

    pub fn edge(&self) -> TaskbarEdge {
        self.edge
    }

    /// Short dimension of the bar: its height when horizontal, its width when
    /// vertical.
    pub fn thickness(&self) -> i32 {
        if self.edge.is_vertical() {
            self.rect.right - self.rect.left
        } else {
            self.rect.bottom - self.rect.top
        }
    }

    /// Thickness of an icons-only taskbar at this taskbar's DPI.
    pub fn icon_scale(&self) -> i32 {
        (ICON_TASKBAR_THICKNESS * self.dpi() as i32) / 96
    }

    /// True when a vertical taskbar is wide enough to be showing button labels.
    pub fn is_label_mode(&self) -> bool {
        if !self.edge.is_vertical() {
            return false;
        }
        self.thickness() > self.icon_scale() * 3 / 2
    }

    /// `true` when taskbar alignment is "Left" (buttons start at the left edge).
    /// Read from `HKCU\...\Explorer\Advanced\TaskbarAl` (0 = Left, 1 = Center).
    pub fn alignment_left(&self) -> bool {
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let res = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
                w!("TaskbarAl"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut value as *mut _ as *mut _),
                Some(&mut size),
            )
        };
        res.is_ok() && value == 0
    }

    /// Left edge (screen coords) of the system tray, if present.
    pub fn tray_left(&self) -> Option<i32> {
        self.tray_notify_rect().map(|r| r.left)
    }

    /// Top edge (screen coords) of the system tray, if present.
    pub fn tray_top(&self) -> Option<i32> {
        self.tray_notify_rect().map(|r| r.top)
    }

    fn dpi(&self) -> u32 {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        if dpi == 0 {
            96
        } else {
            dpi
        }
    }

    fn tray_notify_rect(&self) -> Option<RECT> {
        unsafe {
            let notify = FindWindowExW(Some(self.hwnd), None, w!("TrayNotifyWnd"), None).ok()?;
            if notify.is_invalid() {
                return None;
            }
            let mut rect = RECT::default();
            GetWindowRect(notify, &mut rect).ok()?;
            Some(rect)
        }
    }
}

/// Detects the screen edge of the primary taskbar.
///
/// Prefers the documented appbar API `SHAppBarMessage(ABM_GETTASKBARPOS)` and
/// falls back to measuring the rectangle when that query fails (e.g. right after
/// an Explorer restart). A vertical taskbar is tall and narrow, so
/// `width < height` is a reliable fallback signal.
fn detect_edge(taskbar_rect: RECT) -> TaskbarEdge {
    unsafe {
        let mut abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            ..Default::default()
        };
        if SHAppBarMessage(ABM_GETTASKBARPOS, &mut abd) != 0 {
            return if abd.uEdge == ABE_LEFT {
                TaskbarEdge::Left
            } else if abd.uEdge == ABE_RIGHT {
                TaskbarEdge::Right
            } else if abd.uEdge == ABE_TOP {
                TaskbarEdge::Top
            } else {
                // ABE_BOTTOM, or any unexpected value, means bottom.
                TaskbarEdge::Bottom
            };
        }
    }

    let width = taskbar_rect.right - taskbar_rect.left;
    let height = taskbar_rect.bottom - taskbar_rect.top;
    if width >= height {
        if taskbar_rect.top <= 0 {
            TaskbarEdge::Top
        } else {
            TaskbarEdge::Bottom
        }
    } else if taskbar_rect.left <= 0 {
        TaskbarEdge::Left
    } else {
        TaskbarEdge::Right
    }
}
