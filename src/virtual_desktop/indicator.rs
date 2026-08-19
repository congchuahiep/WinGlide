//! Indicator window displaying status on the Taskbar.

use std::slice::from_raw_parts_mut;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU8, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse;
use windows::Win32::UI::WindowsAndMessaging::{self, *};

use crate::utils;
use crate::win32::activate::force_activate;

const WM_APP_VD_EVENT: u32 = WM_USER + 0x100;

/// Gap kept between the indicator and the taskbar edge / system tray.
const INDICATOR_MARGIN: i32 = 6;
/// Command-ID base for the "Move to Desktop" context menu items.
const MENU_MOVE_BASE: usize = 1000;

/// Shared geometry of the virtual-desktop dots. Used both when rendering and
/// when hit-testing the mouse, so the hover zones always line up with the dots.
struct DotLayout {
    radius: f32,
    spacing: f32,
    start_x: f32,
}

fn dot_layout(height: i32) -> DotLayout {
    // Taskbar at 1080p is usually 48px high, at 4K (200%) it's 96px high.
    // Binding to Taskbar height keeps the aspect ratio 100% accurate on all screens.
    let radius = height as f32 * 0.07; // Radius = 7% height (equivalent to ~3.36px at 1080p)
    let spacing = radius * 5.0;
    let start_x = 10.0 + radius;
    DotLayout {
        radius,
        spacing,
        start_x,
    }
}

/// Total window width needed so the last dot (and its hover hitbox) is fully
/// visible. Derived from the desktop count instead of a fixed constant: adding
/// desktops never clips the indicator, and few desktops don't leave a dead zone.
fn indicator_width(height: i32, count: usize) -> i32 {
    let layout = dot_layout(height);
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

fn get_hovered_index(x: i32) -> Option<usize> {
    unsafe {
        let mut tray_rect = RECT::default();
        if let Ok(taskbar_hwnd) = FindWindowW(w!("Shell_TrayWnd"), None) {
            let _ = GetWindowRect(taskbar_hwnd, &mut tray_rect);
        }
        let height = tray_rect.bottom - tray_rect.top;
        if height <= 0 {
            return None;
        }

        let count = winvd::get_desktop_count().unwrap_or(1) as usize;
        let layout = dot_layout(height);
        let half_spacing = layout.spacing / 2.0;
        let px = x as f32;

        for i in 0..count {
            let cx = layout.start_x + (i as f32) * layout.spacing;
            // Switch to rectangular Hit-box (like a div block), connected continuously without gaps
            if px >= cx - half_spacing && px <= cx + half_spacing {
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

/// Placement of the virtual desktop indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorPosition {
    /// Alignment-aware: left edge (Center-aligned taskbar) or just left of the
    /// system tray (Left-aligned taskbar).
    Auto = 0,
    /// Fixed at the left edge of the taskbar.
    Left = 1,
    /// Fixed just left of the system tray (right side).
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
fn indicator_left(taskbar: RECT, width: i32) -> i32 {
    let position = IndicatorPosition::from_u8(INDICATOR_POSITION.load(Ordering::Relaxed));
    let tray_left = tray_notify_left();
    let left_of_tray = match tray_left {
        Some(l) => (l - INDICATOR_MARGIN - width).max(taskbar.left),
        None => taskbar.right - INDICATOR_MARGIN - width,
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
        let taskbar_height = tray_rect.bottom - tray_rect.top;
        let count = winvd::get_desktop_count().unwrap_or(1) as usize;
        let width = indicator_width(taskbar_height, count);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Indicator"),
            WS_POPUP | WS_VISIBLE,
            indicator_left(tray_rect, width),
            tray_rect.top,
            width,
            taskbar_height,
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
            let mut tray_rect = RECT::default();
            if let Ok(taskbar_hwnd) = FindWindowW(w!("Shell_TrayWnd"), None) {
                let _ = GetWindowRect(taskbar_hwnd, &mut tray_rect);
            }
            let height = tray_rect.bottom - tray_rect.top;
            if height <= 0 {
                return;
            }

            let count = winvd::get_desktop_count().unwrap_or(1) as usize;
            let layout = dot_layout(height);
            let width = indicator_width(height, count);

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down
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
                let buffer = from_raw_parts_mut(ppvbits as *mut u32, (width * height) as usize);
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
                let cy = height as f32 / 2.0;
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

                for i in 0..count {
                    let cx = start_x + (i as f32) * spacing;
                    let is_hovered = hover_idx == (i as isize);

                    // Draw invisible Div block (Alpha = 1) as a Hitbox surrounding the dot.
                    // If hovering, draw a slight rounded background (Alpha = 0.15)
                    Self::draw_hitbox_and_bg(
                        buffer,
                        width,
                        height,
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
                        buffer,
                        width,
                        height,
                        cx,
                        cy,
                        current_radius,
                        button_theme_color,
                        base_alpha,
                    );
                }

                // Update to screen
                let mut pt_src = POINT { x: 0, y: 0 };
                let mut size = SIZE {
                    cx: width,
                    cy: height,
                };
                let mut pt_dst = POINT {
                    x: indicator_left(tray_rect, width),
                    y: tray_rect.top,
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
        buffer: &mut [u32],
        width: i32,
        height: i32,
        cx: f32,
        spacing: f32,
        is_hovered: bool,
        theme_color: (u8, u8, u8),
        move_mode: bool,
    ) {
        let half_spacing = spacing / 2.0;
        let min_x = (cx - half_spacing).floor().max(0.0) as i32;
        let max_x = (cx + half_spacing).ceil().min((width - 1) as f32) as i32;
        let min_y = 0;
        let max_y = height - 1;

        let cy = height as f32 / 2.0;

        // bg_rw: Horizontal radius of hover background (Width = bg_rw * 2)
        // Instead of subtracting margin, we leave `spacing / 2.0` so hover backgrounds touch continuously (no gap)
        let bg_rw = spacing / 2.0;

        // bg_rh: Vertical radius of hover background (Height = bg_rh * 2)
        // Subtract 6px to create padding from the top/bottom edges of the Taskbar
        let bg_rh = (height as f32) / 2.0 - 6.0;

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
                let idx = (y * width + x) as usize;
                if idx >= buffer.len() {
                    continue;
                }

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
                        buffer[idx] = (a << 24) | (pr << 16) | (pg << 8) | pb;
                        continue;
                    }
                }

                // Invisible hitbox
                if buffer[idx] == 0 {
                    buffer[idx] = 0x01000000;
                }
            }
        }
    }

    /// Draws an anti-aliased SDF circle directly onto a 32-bit ARGB buffer.
    fn draw_aa_circle(
        buffer: &mut [u32],
        width: i32,
        height: i32,
        cx: f32,
        cy: f32,
        radius: f32,
        color: (u8, u8, u8),
        base_alpha: f32,
    ) {
        let (r, g, b) = color;

        let min_x = (cx - radius - 1.0).floor().max(0.0) as i32;
        let max_x = (cx + radius + 1.0).ceil().min((width - 1) as f32) as i32;
        let min_y = (cy - radius - 1.0).floor().max(0.0) as i32;
        let max_y = (cy + radius + 1.0).ceil().min((height - 1) as f32) as i32;

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

                    let new_pixel = (a << 24) | (pr << 16) | (pg << 8) | pb;
                    let idx = (y * width + x) as usize;

                    if idx < buffer.len() {
                        buffer[idx] = new_pixel;
                    }
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
                let x = ((lparam.0 as i32) << 16) >> 16;
                let hovered = get_hovered_index(x);
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
                let x = ((lparam.0 as i32) << 16) >> 16;
                if let Some(idx) = get_hovered_index(x) {
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
                let x = ((lparam.0 as i32) << 16) >> 16;
                // The right-clicked dot tells us exactly which desktop N to act on.
                // Capture the target first: showing the menu temporarily takes
                // foreground, so we can't derive it again from GetForegroundWindow.
                if let Some(idx) = get_hovered_index(x) {
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
