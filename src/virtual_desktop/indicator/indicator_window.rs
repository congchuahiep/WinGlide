//! The indicator window: owns the Win32 window, its state and its message loop.

use std::cell::Cell;
use std::ffi::c_void;
use std::slice::from_raw_parts_mut;

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse;
use windows::Win32::UI::WindowsAndMessaging::{self, *};

use super::canvas::Canvas;
use super::desktop_mover::{DesktopMover, MoveTarget};
use super::dot_painter::DotPainter;
use super::indicator_geometry::{hovered_dot, IndicatorGeometry};
use super::indicator_settings::IndicatorSettings;
use super::utils;
use super::{MENU_MOVE_BASE, WM_APP_VD_EVENT};
use crate::taskbar::Taskbar;

/// Indicator window displaying virtual-desktop status on the taskbar.
///
/// It uses [`WS_EX_LAYERED`] with [`UpdateLayeredWindow`] to draw 32-bit graphics
/// with an alpha (transparent) channel.
pub struct IndicatorWindow {
    pub hwnd: HWND,
    /// State shared with the static `window_proc`; boxed so the address stored in
    /// `GWLP_USERDATA` stays valid when this struct is moved.
    inner: Box<State>,
    _desktop_event_thread: Option<winvd::DesktopEventThread>,
}

/// Mutable window state, borrowed shared-only by the message handler.
///
/// Uses interior mutability (`Cell`) so re-entrant messages — e.g. the nested
/// message loop inside `TrackPopupMenu` — can never alias a `&mut`.
struct State {
    /// Visual options from config (placement, accent color, background).
    settings: Cell<IndicatorSettings>,
    hover: Cell<Option<usize>>,
    move_mode: Cell<bool>,
    move_target: Cell<Option<MoveTarget>>,
}

impl IndicatorWindow {
    /// Initializes a new Indicator window.
    ///
    /// **WARN: Task View Issue (Win+Tab)**
    /// This window is an owned window of `Shell_TrayWnd` so it stays above the
    /// taskbar. On Windows 11 the DWM "cloaks" all owned windows of the taskbar
    /// while Task View is open, so the indicator temporarily disappears. Known
    /// limitation, no complete workaround yet.
    pub unsafe fn new(settings: IndicatorSettings) -> anyhow::Result<Self> {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = w!("TaskbarSwitcherIndicator");

        let wnd_class = WNDCLASSW {
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: class_name,
            lpfnWndProc: Some(Self::window_proc),
            ..Default::default()
        };
        let _ = RegisterClassW(&wnd_class);

        let taskbar =
            Taskbar::primary().ok_or_else(|| anyhow::anyhow!("primary taskbar is unavailable"))?;
        let geometry = IndicatorGeometry::for_taskbar(&taskbar)
            .ok_or_else(|| anyhow::anyhow!("taskbar geometry is unavailable"))?;
        let (window_width, window_height) = geometry.physical_size();
        let (window_x, window_y) = geometry.origin(&taskbar, settings.position);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Indicator"),
            WS_POPUP | WS_VISIBLE,
            window_x,
            window_y,
            window_width,
            window_height,
            Some(taskbar.hwnd()),
            None,
            Some(hinstance.into()),
            None,
        )?;

        let mut inner = Box::new(State::new(settings));
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::as_mut(&mut inner) as *mut State as isize,
        );

        let this = Self {
            hwnd,
            inner,
            _desktop_event_thread: None,
        };
        this.inner.render(hwnd);

        Ok(this)
    }

    /// Applies the indicator settings and re-renders the indicator.
    pub fn set_config(&mut self, settings: IndicatorSettings) {
        self.inner.settings.set(settings);
        self.inner.render(self.hwnd);
    }

    /// Re-renders the indicator, recomputing its position against the current
    /// taskbar / system tray bounds.
    pub fn refresh(&self) {
        self.inner.render(self.hwnd);
    }

    /// Starts a thread that watches virtual-desktop switches and posts
    /// `WM_APP_VD_EVENT` so the main thread re-renders the current dot.
    pub fn run(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel::<winvd::DesktopEvent>();
        let hwnd_ind_ptr = self.hwnd.0 as isize;

        match winvd::listen_desktop_events(tx) {
            Ok(thread) => {
                std::thread::spawn(move || {
                    while let Ok(_event) = rx.recv() {
                        unsafe {
                            let hwnd_ind = HWND(hwnd_ind_ptr as *mut _);
                            let _ =
                                PostMessageW(Some(hwnd_ind), WM_APP_VD_EVENT, WPARAM(0), LPARAM(0));
                        }
                    }
                });
                self._desktop_event_thread = Some(thread);
            }
            Err(e) => tracing::error!("Failed to start winvd desktop event listener: {:?}", e),
        }
    }

    /// Static window procedure: recovers the [`State`] from `GWLP_USERDATA` and
    /// forwards the message to it. Falls back to `DefWindowProcW` when the state
    /// isn't installed yet (messages sent during window creation).
    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
        let state = if state_ptr.is_null() {
            None
        } else {
            Some(&*state_ptr)
        };

        match (msg, state) {
            (WM_APP_VD_EVENT, Some(s)) => {
                s.render(hwnd);
                LRESULT(0)
            }
            (WM_DISPLAYCHANGE | WM_SETTINGCHANGE, _) => {
                // Post instead of rendering inline: leaves any "Input Sync Call"
                // first, so the re-render sees the latest display state.
                let _ = PostMessageW(Some(hwnd), WM_APP_VD_EVENT, WPARAM(0), LPARAM(0));
                LRESULT(0)
            }
            (WM_MOUSEMOVE, Some(s)) => {
                let (x, y) = lparam_point(lparam);
                s.on_mouse_move(hwnd, x, y)
            }
            (0x02A3 /* WM_MOUSELEAVE */, Some(s)) => s.on_mouse_leave(hwnd),
            (WM_LBUTTONUP, Some(s)) => {
                let (x, y) = lparam_point(lparam);
                s.on_lbutton_up(x, y)
            }
            (WM_RBUTTONUP, Some(s)) => {
                let (x, y) = lparam_point(lparam);
                s.on_rbutton_up(hwnd, x, y)
            }
            (WM_COMMAND, Some(s)) => s.on_command(wparam),
            (WindowsAndMessaging::WM_SETCURSOR, _) => set_move_cursor(),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl Drop for IndicatorWindow {
    fn drop(&mut self) {
        unsafe {
            // Detach state first so messages sent during destruction skip it.
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

impl State {
    fn new(settings: IndicatorSettings) -> Self {
        Self {
            settings: Cell::new(settings),
            hover: Cell::new(None),
            move_mode: Cell::new(false),
            move_target: Cell::new(None),
        }
    }

    /// Renders the dots into a 32-bit ARGB bitmap and pushes it to the screen
    /// with `UpdateLayeredWindow`.
    fn render(&self, hwnd: HWND) {
        unsafe {
            let Some(taskbar) = Taskbar::primary() else {
                return;
            };
            let Some(geometry) = IndicatorGeometry::for_taskbar(&taskbar) else {
                return;
            };
            let (phys_width, phys_height) = geometry.physical_size();

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

            let mut ppvbits: *mut c_void = std::ptr::null_mut();
            let screen_dc = GetDC(None);
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            let hbitmap =
                CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut ppvbits, None, 0);

            if let Ok(bmp) = hbitmap {
                let old_bmp = SelectObject(mem_dc, HGDIOBJ(bmp.0 as _));

                // Fill with alpha = 0 => fully invisible.
                let buffer =
                    from_raw_parts_mut(ppvbits as *mut u32, (phys_width * phys_height) as usize);
                buffer.fill(0);

                let settings = self.settings.get();
                let painter = DotPainter::new(geometry, settings.accent_color, settings.background);
                let mut canvas =
                    Canvas::new(buffer, phys_width, phys_height, geometry.dots_vertical());
                painter.paint(
                    &mut canvas,
                    self.hover.get(),
                    utils::current_index(),
                    move_modifier_down(),
                );

                let (window_x, window_y) = geometry.origin(&taskbar, settings.position);
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

    fn on_mouse_move(&self, hwnd: HWND, x: i32, y: i32) -> LRESULT {
        let hovered = hovered_dot(x, y);
        let mode_changed = self.update_move_mode();
        if self.hover.get() != hovered || mode_changed {
            self.hover.set(hovered);
            self.render(hwnd);
            if hovered.is_some() {
                unsafe { track_mouse_leave(hwnd) };
            }
        }
        LRESULT(0)
    }

    fn on_mouse_leave(&self, hwnd: HWND) -> LRESULT {
        self.hover.set(None);
        self.render(hwnd);
        LRESULT(0)
    }

    fn on_lbutton_up(&self, x: i32, y: i32) -> LRESULT {
        if let Some(idx) = hovered_dot(x, y) {
            if let Ok(desktops) = winvd::get_desktops() {
                if let Some(desktop) = desktops.get(idx) {
                    if move_modifier_down() {
                        // Alt + click: move the foreground window to that desktop.
                        DesktopMover::move_foreground(desktop);
                    } else {
                        let _ = winvd::switch_desktop(*desktop);
                    }
                }
            }
        }
        LRESULT(0)
    }

    fn on_rbutton_up(&self, hwnd: HWND, x: i32, y: i32) -> LRESULT {
        // The right-clicked dot tells us exactly which desktop N to act on, and
        // the target window is captured before the menu takes foreground.
        if let Some(idx) = hovered_dot(x, y) {
            if let Some(target_hwnd) = DesktopMover::foreground_target() {
                let target = MoveTarget {
                    hwnd: target_hwnd,
                    desktop_index: idx,
                };
                self.move_target.set(Some(target));
                DesktopMover::show_menu(hwnd, target);
            }
        }
        LRESULT(0)
    }

    fn on_command(&self, wparam: WPARAM) -> LRESULT {
        let id = (wparam.0 as u32) & 0xFFFF;
        if id == MENU_MOVE_BASE as u32 || id == (MENU_MOVE_BASE + 1) as u32 {
            if let Some(target) = self.move_target.get() {
                DesktopMover::handle_command(id, target);
            }
        }
        LRESULT(0)
    }

    /// Re-reads the Alt (move) modifier; returns whether it changed.
    fn update_move_mode(&self) -> bool {
        let current = move_modifier_down();
        let changed = self.move_mode.get() != current;
        self.move_mode.set(current);
        changed
    }
}

/// True while the "move window to desktop" modifier (Alt) is held.
fn move_modifier_down() -> bool {
    unsafe { KeyboardAndMouse::GetAsyncKeyState(KeyboardAndMouse::VK_MENU.0 as i32) < 0 }
}

/// Splits the client `x`/`y` packed into a mouse message's `LPARAM`.
fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    let packed = lparam.0 as i32;
    ((packed << 16) >> 16, packed >> 16)
}

/// Asks for a `WM_MOUSELEAVE` once the pointer leaves the indicator.
unsafe fn track_mouse_leave(hwnd: HWND) {
    let mut tme = KeyboardAndMouse::TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<KeyboardAndMouse::TRACKMOUSEEVENT>() as u32,
        dwFlags: KeyboardAndMouse::TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = KeyboardAndMouse::TrackMouseEvent(&mut tme);
}

/// Sets the hand cursor, or the "move" cursor while Alt is held.
fn set_move_cursor() -> LRESULT {
    unsafe {
        let cursor_id = if move_modifier_down() {
            IDC_SIZEALL
        } else {
            IDC_HAND
        };
        if let Ok(cursor) = LoadCursorW(None, cursor_id) {
            let _ = SetCursor(Some(cursor));
        }
    }
    LRESULT(1)
}
