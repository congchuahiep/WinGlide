//! Indicator window displaying status on the Taskbar.

use std::slice::from_raw_parts_mut;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU8, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse;
use windows::Win32::UI::Shell::{
    SHAppBarMessage, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETTASKBARPOS, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{self, *};

use crate::utils;
use crate::win32::activate::force_activate;

const WM_APP_VD_EVENT: u32 = WM_USER + 0x100;

/// Gap kept between the indicator and the taskbar edge / system tray.
const INDICATOR_MARGIN: i32 = 6;
/// Command-ID base for the "Move to Desktop" context menu items.
const MENU_MOVE_BASE: usize = 1000;

/// Which screen edge the primary taskbar occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskbarEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl TaskbarEdge {
    /// `true` for a left/right (vertical) taskbar, `false` for top/bottom.
    fn is_vertical(self) -> bool {
        matches!(self, TaskbarEdge::Left | TaskbarEdge::Right)
    }
}

/// Detects the screen edge of the primary taskbar.
///
/// Prefers the documented appbar API `SHAppBarMessage(ABM_GETTASKBARPOS)` and
/// falls back to measuring the `Shell_TrayWnd` rectangle when that query fails
/// (e.g. right after an Explorer restart). A vertical taskbar is tall and
/// narrow, so `width < height` is a reliable fallback signal.
fn primary_taskbar_edge(taskbar_rect: RECT) -> TaskbarEdge {
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

/// The taskbar's short dimension (across the bar): its height for a horizontal
/// taskbar, its width for a vertical one. This is what sizes the dots.
fn taskbar_thickness(taskbar_rect: RECT, edge: TaskbarEdge) -> i32 {
    if edge.is_vertical() {
        taskbar_rect.right - taskbar_rect.left
    } else {
        taskbar_rect.bottom - taskbar_rect.top
    }
}

/// Standard thickness (px) of an icons-only taskbar at 96 DPI. A vertical
/// taskbar whose width is much larger than this is showing button labels.
const ICON_TASKBAR_THICKNESS: i32 = 48;

/// DPI of the taskbar window (96 when unavailable).
fn taskbar_dpi(taskbar_hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(taskbar_hwnd) };
    if dpi == 0 {
        96
    } else {
        dpi
    }
}

/// Thickness of an icons-only taskbar at the given DPI.
fn icon_scale(dpi: u32) -> i32 {
    (ICON_TASKBAR_THICKNESS * dpi as i32) / 96
}

/// Detects whether a vertical taskbar is wide enough to be showing button labels.
/// Icons-only columns stay close to [`ICON_TASKBAR_THICKNESS`], while labels need
/// far more width. Horizontal taskbars are never in label mode.
fn vertical_label_mode(taskbar_hwnd: HWND, taskbar: RECT, edge: TaskbarEdge) -> bool {
    if !edge.is_vertical() {
        return false;
    }
    let icon = icon_scale(taskbar_dpi(taskbar_hwnd));
    taskbar_thickness(taskbar, edge) > icon * 3 / 2
}

/// Largest dot scale no bigger than `preferred` for which the whole indicator
/// still fits inside `available` px along the dots' axis.
fn fit_scale(preferred: i32, available: i32, count: usize) -> i32 {
    if indicator_length(preferred, count) <= available {
        return preferred.max(1);
    }
    // indicator_length is affine in the scale: length ≈ 11 + k * scale.
    let k = 0.245 + 0.35 * (count.saturating_sub(1) as f32);
    if k <= 0.0 {
        return 1;
    }
    (((available - 11) as f32 / k).floor() as i32).max(1)
}

/// Everything the indicator needs to render / place itself, resolved from the
/// taskbar edge and (for a vertical taskbar) whether labels are shown.
///
/// * horizontal taskbar -> dots run along X, cross-axis is the bar's height;
/// * vertical, icons only -> dots run along Y inside the narrow column;
/// * vertical, with labels -> the column is wide, so the dots run along X as a
///   short horizontal row (a vertical stack would look crude).
#[derive(Clone, Copy)]
struct IndicatorGeometry {
    edge: TaskbarEdge,
    /// Dots are stacked along the taskbar's vertical axis (icon-only vertical bar).
    dots_vertical: bool,
    /// Cross-axis size in px; sizes the dots and the window's short side.
    cross: i32,
    /// Along-axis length in px needed to fit every desktop dot.
    length: i32,
}

impl IndicatorGeometry {
    /// Resolves the geometry for the primary taskbar, or `None` when there is no
    /// usable taskbar rectangle.
    fn detect(taskbar_hwnd: HWND, taskbar: RECT) -> Option<Self> {
        let edge = primary_taskbar_edge(taskbar);
        let thickness = taskbar_thickness(taskbar, edge);
        if thickness <= 0 {
            return None;
        }

        let count = winvd::get_desktop_count().unwrap_or(1) as usize;
        let label_mode = vertical_label_mode(taskbar_hwnd, taskbar, edge);
        let dots_vertical = edge.is_vertical() && !label_mode;

        // In label mode the dots form a horizontal row sized against a standard
        // taskbar thickness (not the whole wide column), shrunk if needed so the
        // row still fits inside the column's width.
        let cross = if label_mode {
            fit_scale(icon_scale(taskbar_dpi(taskbar_hwnd)), thickness, count)
        } else {
            thickness
        };

        Some(Self {
            edge,
            dots_vertical,
            cross,
            length: indicator_length(cross, count),
        })
    }

    /// Physical bitmap / window size `(width, height)`.
    fn physical_size(&self) -> (i32, i32) {
        if self.dots_vertical {
            (self.cross, self.length)
        } else {
            (self.length, self.cross)
        }
    }
}

/// A 32-bit ARGB drawing surface addressed in *logical* coordinates:
/// `x` runs along the taskbar (its length) and `y` runs across it (its
/// thickness). For a vertical taskbar the pixels are transposed while writing,
/// so all the dot layout math stays axis-agnostic.
struct Canvas<'a> {
    buffer: &'a mut [u32],
    phys_width: i32,
    phys_height: i32,
    vertical: bool,
}

impl<'a> Canvas<'a> {
    fn new(buffer: &'a mut [u32], phys_width: i32, phys_height: i32, vertical: bool) -> Self {
        Self {
            buffer,
            phys_width,
            phys_height,
            vertical,
        }
    }

    /// Maps logical `(x, y)` to a buffer index, or `None` when out of bounds.
    #[inline]
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 {
            return None;
        }
        let (px, py) = if self.vertical { (y, x) } else { (x, y) };
        if px >= self.phys_width || py >= self.phys_height {
            return None;
        }
        Some(py as usize * self.phys_width as usize + px as usize)
    }

    #[inline]
    fn get(&self, x: i32, y: i32) -> u32 {
        self.index(x, y).map(|i| self.buffer[i]).unwrap_or(0)
    }

    #[inline]
    fn set(&mut self, x: i32, y: i32, value: u32) {
        if let Some(i) = self.index(x, y) {
            self.buffer[i] = value;
        }
    }
}

/// Shared geometry of the virtual-desktop dots. Used both when rendering and
/// when hit-testing the mouse, so the hover zones always line up with the dots.
struct DotLayout {
    radius: f32,
    spacing: f32,
    start_x: f32,
}

fn dot_layout(thickness: i32) -> DotLayout {
    // Taskbar at 1080p is usually 48px thick, at 4K (200%) it's 96px thick.
    // Binding to the taskbar thickness keeps the dot size consistent everywhere.
    let radius = thickness as f32 * 0.07; // Radius = 7% thickness (equivalent to ~3.36px at 1080p)
    let spacing = radius * 5.0;
    let start_x = 10.0 + radius;
    DotLayout {
        radius,
        spacing,
        start_x,
    }
}

/// Total length needed along the taskbar so the last dot (and its hover hitbox)
/// is fully visible. Derived from the desktop count instead of a fixed
/// constant: adding desktops never clips the indicator, and few desktops don't
/// leave a dead zone.
fn indicator_length(thickness: i32, count: usize) -> i32 {
    let layout = dot_layout(thickness);
    let last_center = layout.start_x + count.saturating_sub(1) as f32 * layout.spacing;
    // The last hitbox extends half a spacing past its center; the enlarged dot
    // (1.25x radius when active/hovered) is always smaller than that.
    (last_center + layout.spacing / 2.0).ceil() as i32 + 1
}

static HOVER_INDEX: AtomicIsize = AtomicIsize::new(-1);

/// Tracks whether the "move window" modifier (Alt) is down, so the indicator
/// can re-render (and switch cursor) when the mode toggles.
static MOVE_MODE: AtomicBool = AtomicBool::new(false);

/// The window targeted by the right-click "Move to Desktop" menu (captured
/// before the indicator temporarily takes foreground to show the menu).
static MOVE_TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

/// The desktop index of the dot that was right-clicked (for the move menu).
static MOVE_TARGET_INDEX: AtomicI32 = AtomicI32::new(-1);

/// Where the virtual desktop indicator is placed (see [`IndicatorPosition`]).
/// Stored as `AtomicU8` because the static [`Self::window_proc`] needs it
/// during `render()` without access to the struct.
static INDICATOR_POSITION: AtomicU8 = AtomicU8::new(0);

/// Returns the index of the dot under a mouse message, using the coordinate
/// along the dots' axis (`x` for a horizontal dot row, `y` for a vertical dot
/// column). `x`/`y` are the client coordinates from the message's `LPARAM`.
fn get_hovered_index(x: i32, y: i32) -> Option<usize> {
    unsafe {
        let taskbar_hwnd = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let mut tray_rect = RECT::default();
        let _ = GetWindowRect(taskbar_hwnd, &mut tray_rect);
        let geom = IndicatorGeometry::detect(taskbar_hwnd, tray_rect)?;

        let count = winvd::get_desktop_count().unwrap_or(1) as usize;
        let layout = dot_layout(geom.cross);
        let half_spacing = layout.spacing / 2.0;
        let along = (if geom.dots_vertical { y } else { x }) as f32;

        for i in 0..count {
            let cx = layout.start_x + (i as f32) * layout.spacing;
            // Switch to rectangular Hit-box (like a div block), connected continuously without gaps
            if along >= cx - half_spacing && along <= cx + half_spacing {
                return Some(i);
            }
        }
        None
    }
}

/// Returns `true` when the taskbar alignment is "Left" (buttons start at the left
/// edge of the taskbar). Read from `HKCU\...\Explorer\Advanced\TaskbarAl`
/// (0 = Left, 1 = Center). Defaults to `false` (Center) when the value is absent.
fn taskbar_alignment_left() -> bool {
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let res = unsafe {
        windows::Win32::System::Registry::RegGetValueW(
            windows::Win32::System::Registry::HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
            w!("TaskbarAl"),
            windows::Win32::System::Registry::RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        )
    };
    res.is_ok() && value == 0
}

/// Left edge (screen coords) of the primary taskbar's system tray
/// (`TrayNotifyWnd`, which contains the notification icons and the clock).
fn tray_notify_left() -> Option<i32> {
    unsafe {
        let tray = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let notify = FindWindowExW(Some(tray), None, w!("TrayNotifyWnd"), None).ok()?;
        if notify.is_invalid() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(notify, &mut rect).ok()?;
        Some(rect.left)
    }
}

/// Top edge (screen coords) of the primary taskbar's system tray
/// (`TrayNotifyWnd`). Used to keep the indicator above the tray on a vertical
/// taskbar, mirroring [`tray_notify_left`] for the horizontal case.
fn tray_notify_top() -> Option<i32> {
    unsafe {
        let tray = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let notify = FindWindowExW(Some(tray), None, w!("TrayNotifyWnd"), None).ok()?;
        if notify.is_invalid() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(notify, &mut rect).ok()?;
        Some(rect.top)
    }
}

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

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

/// Computes the screen X where the indicator should sit, according to the
/// configured [`IndicatorPosition`].
fn indicator_left(taskbar: RECT, length: i32) -> i32 {
    let position = IndicatorPosition::from_u8(INDICATOR_POSITION.load(Ordering::Relaxed));
    let tray_left = tray_notify_left();
    let left_of_tray = match tray_left {
        Some(l) => (l - INDICATOR_MARGIN - length).max(taskbar.left),
        None => taskbar.right - INDICATOR_MARGIN - length,
    };

    match position {
        IndicatorPosition::Left => taskbar.left + 10,
        IndicatorPosition::Right => left_of_tray,
        IndicatorPosition::Auto => {
            if taskbar_alignment_left() {
                left_of_tray
            } else {
                taskbar.left + 10
            }
        }
    }
}

/// Screen coordinates (top-left) at which the indicator window is placed.
///
/// For a horizontal taskbar the indicator is offset along X (via
/// [`indicator_left`]). For a vertical taskbar it lies inside the column: offset
/// along Y by the position preset, and centered across the column for the
/// horizontal (label-mode) dot row.
fn indicator_origin(taskbar: RECT, geom: IndicatorGeometry) -> (i32, i32) {
    if !geom.edge.is_vertical() {
        return (indicator_left(taskbar, geom.length), taskbar.top);
    }

    let thickness = taskbar.right - taskbar.left;
    let x = if geom.dots_vertical {
        taskbar.left
    } else {
        taskbar.left + (thickness - geom.length).max(0) / 2
    };
    (x, vertical_anchor(taskbar, geom.length))
}

/// Vertical position of the indicator along a vertical taskbar: `Left` pins it
/// to the top (start), `Right` just above the system tray (end), and `Auto`
/// mirrors the horizontal alignment rule instead of centering on the taskbar.
fn vertical_anchor(taskbar: RECT, length: i32) -> i32 {
    let position = IndicatorPosition::from_u8(INDICATOR_POSITION.load(Ordering::Relaxed));
    let above_tray = match tray_notify_top() {
        Some(top) => (top - INDICATOR_MARGIN - length).max(taskbar.top),
        None => taskbar.bottom - INDICATOR_MARGIN - length,
    };
    match position {
        IndicatorPosition::Left => taskbar.top + 10,
        IndicatorPosition::Right => above_tray,
        IndicatorPosition::Auto => {
            if taskbar_alignment_left() {
                above_tray
            } else {
                taskbar.top + 10
            }
        }
    }
}

/// True while the "move window to desktop" modifier (Alt) is held.
fn move_modifier_down() -> bool {
    unsafe { (KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_MENU.0 as i32)) < 0 }
}

/// Returns `true` if the move-modifier state changed since the last check,
/// so the indicator knows to re-render when the user toggles Alt while hovering.
fn move_mode_changed() -> bool {
    let current = move_modifier_down();
    let old = MOVE_MODE.load(Ordering::Relaxed);
    if current != old {
        MOVE_MODE.store(current, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// The current foreground application window, rejecting system/hidden windows.
/// Returns `None` if there is no suitable window to move.
fn foreground_target_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let mut class_buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
            // Never treat our own windows (tray / indicator) as move targets.
            if utils::is_system_class(&class_name)
                || class_name == "WinGlideTray"
                || class_name == "TaskbarSwitcherIndicator"
            {
                return None;
            }
        }
        Some(hwnd)
    }
}

/// Moves `hwnd` to `target` desktop without switching. Returns `true` when the
/// window was actually moved (it can't be when the window is pinned or already
/// on `target`).
fn move_window_to_desktop_only(hwnd: HWND, target: &winvd::Desktop) -> bool {
    // winvd links a different `windows` crate version, so the HWND must be
    // transmuted (both are transparent wrappers over a pointer).
    let whwnd = unsafe { std::mem::transmute(hwnd) };

    // Pinned (all-desktops) windows can't be moved.
    if winvd::is_pinned_window(whwnd).unwrap_or(false) {
        return false;
    }

    // Already on the target desktop -> nothing to do.
    if let Ok(win_desktop) = winvd::get_desktop_by_window(whwnd) {
        if &win_desktop == target {
            return false;
        }
    }

    winvd::move_window_to_desktop(*target, &whwnd).is_ok()
}

/// Moves `hwnd` to `target` desktop, switches to it and re-activates the window.
fn move_and_jump_to_desktop(hwnd: HWND, target: &winvd::Desktop) {
    if move_window_to_desktop_only(hwnd, target) {
        let _ = winvd::switch_desktop(*target);
        std::thread::sleep(std::time::Duration::from_millis(50));
        unsafe { force_activate(hwnd) };
    }
}

/// Moves the current foreground window to `target` desktop and jumps to it.
fn move_foreground_to_desktop(target: &winvd::Desktop) {
    if let Some(hwnd) = foreground_target_window() {
        move_and_jump_to_desktop(hwnd, target);
    }
}

/// Null-terminated UTF-16 copy of `s` for Win32 string parameters.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shows the "Move to Desktop N" context menu. `desktop_idx` is the index of
/// the desktop whose dot was right-clicked, so `N` is known.
/// The window title goes in a disabled header so the menu items stay short.
fn show_move_menu(indicator_hwnd: HWND, target_hwnd: HWND, desktop_idx: usize) {
    unsafe {
        let desktops = match winvd::get_desktops() {
            Ok(d) => d,
            Err(_) => return,
        };
        if desktop_idx >= desktops.len() {
            return;
        }

        let target = &desktops[desktop_idx];
        let n = desktop_idx + 1; // 1-based label

        let mut title_buf = [0u16; 256];
        let len = GetWindowTextW(target_hwnd, &mut title_buf);
        let title = if len > 0 {
            utils::truncate(&String::from_utf16_lossy(&title_buf[..len as usize]), 40)
        } else {
            "current window".to_string()
        };

        // Gray "move" out when the window is already on the right-clicked desktop.
        let current = winvd::get_desktop_by_window(std::mem::transmute(target_hwnd)).ok();
        let already_here = Some(target) == current.as_ref();

        let Ok(hmenu) = CreatePopupMenu() else { return };

        // Header: the window title (disabled), then the two short actions.
        let header_wide = wide(&format!("\"{title}\""));
        let _ = AppendMenuW(
            hmenu,
            MF_STRING | MF_DISABLED | MF_GRAYED,
            0,
            PCWSTR(header_wide.as_ptr()),
        );
        let _ = AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null());

        let move_label = wide(&format!("Move to Desktop {n}"));
        let _ = AppendMenuW(
            hmenu,
            if already_here {
                MF_STRING | MF_DISABLED | MF_GRAYED
            } else {
                MF_STRING
            },
            MENU_MOVE_BASE,
            PCWSTR(move_label.as_ptr()),
        );

        let jump_label = wide(&format!("Move and jump to Desktop {n}"));
        let _ = AppendMenuW(
            hmenu,
            MF_STRING,
            MENU_MOVE_BASE + 1,
            PCWSTR(jump_label.as_ptr()),
        );

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);

        // TrackPopupMenu needs its owner window to be foreground. The indicator
        // is WS_EX_NOACTIVATE, so temporarily clear that style while the menu is
        // shown (the target window was captured before we took foreground).
        let ex_style = GetWindowLongW(indicator_hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(
            indicator_hwnd,
            GWL_EXSTYLE,
            (ex_style as u32 & !WS_EX_NOACTIVATE.0) as i32,
        );
        let _ = SetForegroundWindow(indicator_hwnd);
        let _ = TrackPopupMenu(
            hmenu,
            TPM_LEFTALIGN | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            None,
            indicator_hwnd,
            None,
        );
        let _ = SetWindowLongW(indicator_hwnd, GWL_EXSTYLE, ex_style);

        let _ = DestroyMenu(hmenu);

        // Give focus back to the app window so the indicator doesn't linger as
        // the foreground window (which would make the next right-click target
        // the indicator itself instead of the app).
        force_activate(target_hwnd);
    }
}

/// Indicator window displaying status on the Taskbar.
/// It uses the `WS_EX_LAYERED` flag combined with `UpdateLayeredWindow` to draw 32-bit graphics with an Alpha (transparent) channel.
pub struct IndicatorWindow {
    pub hwnd: HWND,
    _desktop_event_thread: Option<winvd::DesktopEventThread>,
}

impl IndicatorWindow {
    /// Initializes a new Indicator window.
    ///
    /// **WARN: Task View Issue (Win+Tab)**
    /// Currently this window is set as an Owned Window of `Shell_TrayWnd` (Taskbar) so it always stays
    /// on top of the Taskbar. However, on Windows 11, when opening Task View (Win+Tab), the DWM system automatically
    /// uses "Cloaking" technique to hide all Owned windows of the Taskbar. As a result,
    /// the Indicator will disappear while Task View is open, and usually only reappears when the Taskbar receives
    /// focus. This is a current technical limitation with no complete workaround yet.
    pub unsafe fn new(position: IndicatorPosition) -> anyhow::Result<Self> {
        INDICATOR_POSITION.store(position.to_u8(), Ordering::Relaxed);
        let hinstance = GetModuleHandleW(None)?;
        let class_name = w!("TaskbarSwitcherIndicator");

        let wnd_class = WNDCLASSW {
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: class_name,
            lpfnWndProc: Some(Self::window_proc),
            ..Default::default()
        };

        let _ = RegisterClassW(&wnd_class);

        let taskbar_hwnd = FindWindowW(w!("Shell_TrayWnd"), None)?;
        let mut tray_rect = RECT::default();
        let _ = GetWindowRect(taskbar_hwnd, &mut tray_rect);
        let geom = IndicatorGeometry::detect(taskbar_hwnd, tray_rect)
            .ok_or_else(|| anyhow::anyhow!("taskbar rectangle is unavailable"))?;
        let (window_width, window_height) = geom.physical_size();
        let (window_x, window_y) = indicator_origin(tray_rect, geom);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Indicator"),
            WS_POPUP | WS_VISIBLE,
            window_x,
            window_y,
            window_width,
            window_height,
            Some(taskbar_hwnd),
            None,
            Some(hinstance.into()),
            None,
        )?;

        let this = Self {
            hwnd,
            _desktop_event_thread: None,
        };

        // Initial render
        Self::render(hwnd);

        Ok(this)
    }

    /// Changes the placement preset and re-renders the indicator.
    pub fn set_position(&mut self, position: IndicatorPosition) {
        INDICATOR_POSITION.store(position.to_u8(), Ordering::Relaxed);
        Self::render(self.hwnd);
    }

    /// Re-renders the indicator, recomputing its position against the current
    /// taskbar / system tray bounds.
    pub fn refresh(&self) {
        Self::render(self.hwnd);
    }

    /// Starts a thread to monitor Desktop switching events (Virtual Desktop).
    /// When it detects the user switching Desktops, it sends a `WM_APP_VD_EVENT` message to the main UI thread
    /// to request a re-render of the Indicator dots, accurately displaying the current Desktop.
    pub fn run(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel::<winvd::DesktopEvent>();
        let hwnd_ind_ptr = self.hwnd.0 as isize;

        match winvd::listen_desktop_events(tx) {
            Ok(thread) => {
                std::thread::spawn(move || {
                    while let Ok(_event) = rx.recv() {
                        unsafe {
                            let hwnd_ind = windows::Win32::Foundation::HWND(hwnd_ind_ptr as *mut _);
                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                Some(hwnd_ind),
                                WM_APP_VD_EVENT,
                                windows::Win32::Foundation::WPARAM(0),
                                windows::Win32::Foundation::LPARAM(0),
                            );
                        }
                    }
                });

                self._desktop_event_thread = Some(thread);
            }
            Err(e) => tracing::error!("Failed to start winvd desktop event listener: {:?}", e),
        }
    }

    /// Renders Indicator content to a buffer, then pushes it directly to the screen.
    ///
    /// Instead of using standard GDI (which causes jagged black border artifacts when combined with `LWA_COLORKEY`),
    /// this function initializes a 32-bit ARGB bitmap (DIBSection), draws circular dots using the SDF
    /// (Signed Distance Field) algorithm for smooth anti-aliasing, then uses `UpdateLayeredWindow`
    /// to apply the entire Alpha channel onto the Desktop.
    ///
    /// To be honest, I don't even know how this function works anymore :P
    pub fn render(hwnd: HWND) {
        unsafe {
            let taskbar_hwnd = match FindWindowW(w!("Shell_TrayWnd"), None) {
                Ok(h) => h,
                Err(_) => return,
            };
            let mut tray_rect = RECT::default();
            let _ = GetWindowRect(taskbar_hwnd, &mut tray_rect);
            let geom = match IndicatorGeometry::detect(taskbar_hwnd, tray_rect) {
                Some(g) => g,
                None => return,
            };

            let count = winvd::get_desktop_count().unwrap_or(1) as usize;
            let layout = dot_layout(geom.cross);
            let length = geom.length;
            let thickness = geom.cross;
            let (phys_width, phys_height) = geom.physical_size();

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: phys_width,
                    biHeight: -phys_height, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0 as u32,
                    ..Default::default()
                },
                ..Default::default()
            };

            let mut ppvbits: *mut std::ffi::c_void = std::ptr::null_mut();
            let screen_dc = GetDC(None);
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            let hbitmap =
                CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut ppvbits, None, 0);

            if let Ok(bmp) = hbitmap {
                let old_bmp = SelectObject(mem_dc, HGDIOBJ(bmp.0 as _));

                // Point rust array to image buffer
                // Fill with black background but 0 transparency (Alpha = 0) => 100% invisible
                let buffer =
                    from_raw_parts_mut(ppvbits as *mut u32, (phys_width * phys_height) as usize);
                buffer.fill(0);

                let light_mode = utils::is_light_theme();
                let button_theme_color = match light_mode {
                    true => (20, 20, 20),
                    false => (255, 255, 255),
                };
                let hitbox_theme_color = match light_mode {
                    true => (255, 255, 255),
                    false => (100, 100, 100),
                };

                let radius = layout.radius;
                let spacing = layout.spacing;
                let start_x = layout.start_x;
                let cy = thickness as f32 / 2.0;
                let current = winvd::get_current_desktop().ok();
                let desktops = winvd::get_desktops().unwrap_or_default();
                let mut current_idx = 0;
                if let Some(c) = current {
                    for (i, d) in desktops.iter().enumerate() {
                        if *d == c {
                            current_idx = i;
                            break;
                        }
                    }
                }

                let hover_idx = HOVER_INDEX.load(Ordering::Relaxed);
                let move_mode = move_modifier_down();

                let mut canvas = Canvas::new(buffer, phys_width, phys_height, geom.dots_vertical);

                for i in 0..count {
                    let cx = start_x + (i as f32) * spacing;
                    let is_hovered = hover_idx == (i as isize);

                    // Draw invisible Div block (Alpha = 1) as a Hitbox surrounding the dot.
                    // If hovering, draw a slight rounded background (Alpha = 0.15)
                    Self::draw_hitbox_and_bg(
                        &mut canvas,
                        length,
                        thickness,
                        cx,
                        spacing,
                        is_hovered,
                        hitbox_theme_color,
                        move_mode,
                    );

                    let is_active = i == current_idx;

                    let mut current_radius = radius;
                    let mut base_alpha = if is_active {
                        current_radius *= 1.25; // Active indicator is as big as when hovered
                        1.0
                    } else {
                        0.5
                    };

                    // Hover effect
                    if is_hovered {
                        current_radius = radius * 1.25; // Ensure 25% enlargement (don't double if both active and hovered)
                        if base_alpha < 0.8 {
                            base_alpha = 0.8; // Brighten up
                        }
                    }

                    Self::draw_aa_circle(
                        &mut canvas,
                        length,
                        thickness,
                        cx,
                        cy,
                        current_radius,
                        button_theme_color,
                        base_alpha,
                    );
                }

                // Update to screen
                let (window_x, window_y) = indicator_origin(tray_rect, geom);
                let mut pt_src = POINT { x: 0, y: 0 };
                let mut size = SIZE {
                    cx: phys_width,
                    cy: phys_height,
                };
                let mut pt_dst = POINT {
                    x: window_x,
                    y: window_y,
                };
                let mut blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };

                let _ = UpdateLayeredWindow(
                    hwnd,
                    None,
                    Some(&mut pt_dst as *mut _),
                    Some(&mut size as *mut _),
                    Some(mem_dc),
                    Some(&mut pt_src as *mut _),
                    COLORREF(0),
                    Some(&mut blend as *mut _),
                    ULW_ALPHA,
                );

                SelectObject(mem_dc, old_bmp);
                let _ = DeleteObject(HGDIOBJ(bmp.0 as _));
            }

            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
        }
    }

    /// Draws an "invisible" rectangular block (Hitbox) and blurred rounded background on Hover
    fn draw_hitbox_and_bg(
        canvas: &mut Canvas<'_>,
        length: i32,
        thickness: i32,
        cx: f32,
        spacing: f32,
        is_hovered: bool,
        theme_color: (u8, u8, u8),
        move_mode: bool,
    ) {
        let half_spacing = spacing / 2.0;
        let min_x = (cx - half_spacing).floor().max(0.0) as i32;
        let max_x = (cx + half_spacing).ceil().min((length - 1) as f32) as i32;
        let min_y = 0;
        let max_y = thickness - 1;

        let cy = thickness as f32 / 2.0;

        // bg_rw: radius of the hover background along the taskbar (Width = bg_rw * 2)
        // Instead of subtracting margin, we leave `spacing / 2.0` so hover backgrounds touch continuously (no gap)
        let bg_rw = spacing / 2.0;

        // bg_rh: radius of the hover background across the taskbar (Height = bg_rh * 2)
        // Subtract 6px to create padding from the taskbar edges
        let bg_rh = (thickness as f32) / 2.0 - 6.0;

        // Corner radius of hover background (Larger means rounder, max is bg_rh)
        let corner_radius = 6.0;

        let inner_w = bg_rw - corner_radius;
        let inner_h = bg_rh - corner_radius;
        // In "move" mode (Alt held) tint the hover background with an accent color
        // to signal that a click will move the current window to that desktop.
        let (r, g, b) = if move_mode {
            if utils::is_light_theme() {
                (0, 92, 175)
            } else {
                (110, 170, 255)
            }
        } else {
            theme_color
        };

        let base_alpha = 0.3; // Transparency of hover background

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;

                if is_hovered {
                    let dx = (px - cx).abs() - inner_w;
                    let dy = (py - cy).abs() - inner_h;
                    let dist = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - corner_radius;

                    let mut alpha = if dist <= -0.5 {
                        1.0
                    } else if dist >= 0.5 {
                        0.0
                    } else {
                        0.5 - dist
                    };

                    alpha *= base_alpha;

                    if alpha > 0.0 {
                        let a = (alpha * 255.0) as u32;
                        let pr = (r as f32 * alpha) as u32;
                        let pg = (g as f32 * alpha) as u32;
                        let pb = (b as f32 * alpha) as u32;
                        canvas.set(x, y, (a << 24) | (pr << 16) | (pg << 8) | pb);
                        continue;
                    }
                }

                // Invisible hitbox
                if canvas.get(x, y) == 0 {
                    canvas.set(x, y, 0x01000000);
                }
            }
        }
    }

    /// Draws an anti-aliased SDF circle onto the logical canvas.
    fn draw_aa_circle(
        canvas: &mut Canvas<'_>,
        length: i32,
        thickness: i32,
        cx: f32,
        cy: f32,
        radius: f32,
        color: (u8, u8, u8),
        base_alpha: f32,
    ) {
        let (r, g, b) = color;

        let min_x = (cx - radius - 1.0).floor().max(0.0) as i32;
        let max_x = (cx + radius + 1.0).ceil().min((length - 1) as f32) as i32;
        let min_y = (cy - radius - 1.0).floor().max(0.0) as i32;
        let max_y = (cy + radius + 1.0).ceil().min((thickness - 1) as f32) as i32;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let dx = px - cx;
                let dy = py - cy;
                let distance = (dx * dx + dy * dy).sqrt();

                // Anti-aliasing (SDF)
                let mut alpha = if distance <= radius - 0.5 {
                    1.0
                } else if distance >= radius + 0.5 {
                    0.0
                } else {
                    0.5 - (distance - radius)
                };

                alpha *= base_alpha;

                if alpha > 0.0 {
                    let a = (alpha * 255.0) as u32;
                    let pr = (r as f32 * alpha) as u32;
                    let pg = (g as f32 * alpha) as u32;
                    let pb = (b as f32 * alpha) as u32;

                    canvas.set(x, y, (a << 24) | (pr << 16) | (pg << 8) | pb);
                }
            }
        }
    }

    /// Core Message Procedure for the Indicator window.
    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_APP_VD_EVENT => {
                Self::render(hwnd);
                LRESULT(0)
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                // Exit "Input Sync Call" before calling render (COM), which helps
                // get the latest state when rerendering
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                        Some(hwnd),
                        WM_APP_VD_EVENT,
                        windows::Win32::Foundation::WPARAM(0),
                        windows::Win32::Foundation::LPARAM(0),
                    );
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let packed = lparam.0 as i32;
                let x = (packed << 16) >> 16;
                let y = packed >> 16;
                let hovered = get_hovered_index(x, y);
                let old = HOVER_INDEX.load(std::sync::atomic::Ordering::Relaxed);
                let new_val = hovered.map(|i| i as isize).unwrap_or(-1);

                if old != new_val || move_mode_changed() {
                    HOVER_INDEX.store(new_val, std::sync::atomic::Ordering::Relaxed);
                    Self::render(hwnd);

                    if new_val != -1 {
                        let mut tme = KeyboardAndMouse::TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<KeyboardAndMouse::TRACKMOUSEEVENT>() as u32,
                            dwFlags: KeyboardAndMouse::TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = unsafe { KeyboardAndMouse::TrackMouseEvent(&mut tme) };
                    }
                }
                LRESULT(0)
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                HOVER_INDEX.store(-1, std::sync::atomic::Ordering::Relaxed);
                Self::render(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let packed = lparam.0 as i32;
                let x = (packed << 16) >> 16;
                let y = packed >> 16;
                if let Some(idx) = get_hovered_index(x, y) {
                    if let Ok(desktops) = winvd::get_desktops() {
                        if idx < desktops.len() {
                            if move_modifier_down() {
                                // Alt + click: move the foreground window to that desktop.
                                move_foreground_to_desktop(&desktops[idx]);
                            } else {
                                let _ = winvd::switch_desktop(desktops[idx]);
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                let packed = lparam.0 as i32;
                let x = (packed << 16) >> 16;
                let y = packed >> 16;
                // The right-clicked dot tells us exactly which desktop N to act on.
                // Capture the target first: showing the menu temporarily takes
                // foreground, so we can't derive it again from GetForegroundWindow.
                if let Some(idx) = get_hovered_index(x, y) {
                    if let Some(target) = foreground_target_window() {
                        MOVE_TARGET_HWND.store(target.0 as isize, Ordering::Relaxed);
                        MOVE_TARGET_INDEX.store(idx as i32, Ordering::Relaxed);
                        show_move_menu(hwnd, target, idx);
                    }
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                let id = (wparam.0 as u32) & 0xFFFF;
                if id == MENU_MOVE_BASE as u32 || id == (MENU_MOVE_BASE + 1) as u32 {
                    let ptr = MOVE_TARGET_HWND.load(Ordering::Relaxed);
                    let idx = MOVE_TARGET_INDEX.load(Ordering::Relaxed);
                    if ptr != 0 && idx >= 0 {
                        let target = HWND(ptr as *mut _);
                        if let Ok(desktops) = winvd::get_desktops() {
                            let i = idx as usize;
                            if i < desktops.len() {
                                if id == MENU_MOVE_BASE as u32 {
                                    move_window_to_desktop_only(target, &desktops[i]);
                                } else {
                                    move_and_jump_to_desktop(target, &desktops[i]);
                                }
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WindowsAndMessaging::WM_SETCURSOR => {
                unsafe {
                    let cursor_id = if move_modifier_down() {
                        WindowsAndMessaging::IDC_SIZEALL
                    } else {
                        WindowsAndMessaging::IDC_HAND
                    };
                    if let Ok(cursor) = WindowsAndMessaging::LoadCursorW(None, cursor_id) {
                        let _ = WindowsAndMessaging::SetCursor(Some(cursor));
                    }
                }
                LRESULT(1)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl Drop for IndicatorWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
