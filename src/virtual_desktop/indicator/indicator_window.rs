//! The indicator window: owns the Win32 window, its state and its message loop.

use std::cell::Cell;
use std::ffi::c_void;
use std::slice::from_raw_parts_mut;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse;
use windows::Win32::UI::WindowsAndMessaging::{self, *};

use super::canvas::Canvas;
use super::desktop_mover::{DesktopMover, MoveTarget};
use super::dot_painter::DotPainter;
use super::indicator_geometry::{hovered_dot, IndicatorGeometry};
use super::indicator_settings::IndicatorSettings;
use super::taskbar_watcher::TaskbarWatcher;
use super::utils;
use super::{MENU_MOVE_BASE, WM_APP_TASKBAR_MOVED, WM_APP_VD_EVENT};
use crate::taskbar::Taskbar;

/// Timer id for the periodic "am I still above the taskbar's other children" check.
const TIMER_Z_ORDER: usize = 1;
/// How often that check runs. It is a single `GetWindow` call and only acts when
/// Explorer re-stacked the taskbar's children.
const Z_ORDER_CHECK_MS: u32 = 1500;

/// Indicator window displaying virtual-desktop status on the taskbar.
///
/// It uses [`WS_EX_LAYERED`] with [`UpdateLayeredWindow`] to draw 32-bit graphics
/// with an alpha (transparent) channel.
///
/// The Win32 window lives on its own thread (see [`run_indicator_thread`]); this
/// handle only posts messages to it. That is deliberate: a child of the taskbar
/// shares the shell's input queue, so Explorer sends it messages and waits for
/// the answer. The main thread regularly blocks inside UI Automation calls into
/// Explorer, and answering from there would deadlock both threads.
pub struct IndicatorWindow {
    /// The indicator window, owned by the indicator thread.
    pub hwnd: HWND,
    /// Settings shared with the indicator thread.
    settings: Arc<Mutex<IndicatorSettings>>,
    thread: Option<JoinHandle<()>>,
    _desktop_event_thread: Option<winvd::DesktopEventThread>,
}

/// Mutable window state, borrowed shared-only by the message handler.
///
/// Uses interior mutability (`Cell`) so re-entrant messages — e.g. the nested
/// message loop inside `TrackPopupMenu` — can never alias a `&mut`.
struct State {
    /// Visual options from config (placement, accent color, background).
    settings: Arc<Mutex<IndicatorSettings>>,
    hover: Cell<Option<usize>>,
    move_mode: Cell<bool>,
    move_target: Cell<Option<MoveTarget>>,
    /// Tracks whether the indicator is currently hidden due to taskbar autohide.
    taskbar_hidden: Cell<bool>,
    /// Last known taskbar rect, to avoid redundant re-renders when the bar has not moved.
    last_taskbar_rect: Cell<Option<RECT>>,
    /// Hidden top-level window that owns the right-click menu (see
    /// [`DesktopMover::create_menu_host`]).
    menu_host: HWND,
    /// Watches the taskbar for auto-hide slides and Explorer restarts.
    watcher: Option<TaskbarWatcher>,
}

impl IndicatorWindow {
    /// Initializes a new Indicator window.
    ///
    /// The window is a **child of `Shell_TrayWnd`**, not a popup owned by it.
    /// Windows keeps a child inside its parent's z-order band, so the dots stay
    /// above the taskbar's own XAML content for as long as the bar exists - also
    /// while Task View (`Win+Tab`) covers every ordinary window. [`State::render`]
    /// keeps it at `HWND_TOP` among the taskbar's children, because
    /// `Windows.UI.Composition.DesktopWindowContentBridge` is a full-size
    /// sibling that would otherwise paint over the dots.
    pub unsafe fn new(settings: IndicatorSettings) -> anyhow::Result<Self> {
        // Fail fast (and with the same error as before) instead of spawning a
        // thread that would only report the missing taskbar back.
        if Taskbar::primary().is_none() {
            anyhow::bail!("primary taskbar is unavailable");
        }

        let settings = Arc::new(Mutex::new(settings));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, String>>();
        let thread_settings = Arc::clone(&settings);
        let thread = std::thread::Builder::new()
            .name("indicator".into())
            .spawn(move || unsafe { run_indicator_thread(thread_settings, ready_tx) })?;

        let hwnd = match ready_rx.recv() {
            Ok(Ok(handle)) => HWND(handle as *mut _),
            Ok(Err(e)) => {
                let _ = thread.join();
                anyhow::bail!("indicator window: {e}");
            }
            Err(e) => {
                let _ = thread.join();
                anyhow::bail!("indicator thread stopped: {e}");
            }
        };

        Ok(Self {
            hwnd,
            settings,
            thread: Some(thread),
            _desktop_event_thread: None,
        })
    }

    /// Applies the indicator settings and re-renders the indicator.
    pub fn set_config(&mut self, settings: IndicatorSettings) {
        if let Ok(mut shared) = self.settings.lock() {
            *shared = settings;
        }
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_APP_VD_EVENT, WPARAM(0), LPARAM(0));
        }
    }

    /// Re-renders the indicator, recomputing its position against the current
    /// taskbar / system tray bounds.
    ///
    /// `WM_APP_TASKBAR_MOVED` re-syncs the taskbar snapshot (including a new
    /// HWND after an Explorer restart) and then draws.
    pub fn refresh(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_APP_TASKBAR_MOVED, WPARAM(0), LPARAM(0));
        }
    }

    /// Whether the underlying window still exists.
    ///
    /// A child window can be destroyed together with its parent, so an Explorer
    /// restart may leave `self.hwnd` dangling; callers rebuild the indicator in
    /// that case instead of drawing into a dead window.
    pub fn is_alive(&self) -> bool {
        unsafe { IsWindow(Some(self.hwnd)).as_bool() }
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
            (WM_APP_TASKBAR_MOVED, Some(s)) => {
                TaskbarWatcher::reset_pending();
                s.sync_taskbar(hwnd);
                // `refresh()` reuses this message for theme/DPI changes, where
                // the taskbar did not move but the dots still need a repaint.
                if !s.taskbar_hidden.get() {
                    s.render(hwnd);
                }
                LRESULT(0)
            }
            (WM_APP_VD_EVENT, Some(s)) => {
                s.render(hwnd);
                LRESULT(0)
            }
            (WM_DISPLAYCHANGE | WM_SETTINGCHANGE, Some(s)) => {
                s.sync_taskbar(hwnd);
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
                s.on_rbutton_up(x, y)
            }
            (WM_COMMAND, Some(s)) => s.on_command(wparam),
            (WM_TIMER, Some(s)) if wparam.0 == TIMER_Z_ORDER => {
                s.keep_on_top(hwnd);
                LRESULT(0)
            }
            // The message loop of the indicator thread exits here; the owning
            // `IndicatorWindow` is then joined by its `Drop`.
            (WM_DESTROY, _) => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            // UI Automation asking for this window's accessibility object. Answer
            // "nothing": accessibility clients (WinGlide's own taskbar walk
            // included) would otherwise make a synchronous cross-process call
            // into this thread, which can freeze the shell while the thread is
            // busy. FluentFlyout, which also lives on the taskbar, does the same.
            (WM_GETOBJECT, _) => LRESULT(0),
            // The indicator is a child of the shell's window, so a plain click
            // would activate the taskbar. Keep the click without taking focus.
            (WM_MOUSEACTIVATE, _) => {
                // Still before the click clears the foreground window.
                DesktopMover::capture_foreground();
                LRESULT(MA_NOACTIVATE as isize)
            }
            // A right-click on a child window that leaves WM_CONTEXTMENU to
            // `DefWindowProc` gets it forwarded to the parent, so the shell opens
            // its taskbar menu on top of ours. Handle the right-click ourselves.
            (WM_CONTEXTMENU, _) => LRESULT(0),
            (WindowsAndMessaging::WM_SETCURSOR, _) => set_move_cursor(),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

impl Drop for IndicatorWindow {
    fn drop(&mut self) {
        // The window belongs to the indicator thread. Ask it to close
        // (DefWindowProc destroys the window, WM_DESTROY quits the loop) and
        // wait for that thread to drop the window state.
        let _ = unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        DesktopMover::destroy_menu_host(self.menu_host);
    }
}

/// Entry point of the thread that owns the indicator window.
///
/// A child of the taskbar shares the shell's input queue: Explorer sends it
/// messages (accessibility queries, show/hide notifications) and waits for the
/// answer. The main thread regularly blocks inside UI Automation calls into
/// Explorer, so answering from there deadlocks the taskbar. This thread only
/// pumps messages and paints, so it always answers.
unsafe fn run_indicator_thread(
    settings: Arc<Mutex<IndicatorSettings>>,
    ready: mpsc::Sender<Result<isize, String>>,
) {
    // winvd (virtual desktop queries) needs an STA on the thread that uses it.
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

    match create_indicator_window(settings) {
        Ok((hwnd, state)) => {
            let _ = ready.send(Ok(hwnd.0 as isize));
            run_message_loop();
            // Drops the menu owner window and unhooks the taskbar watcher.
            drop(state);
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
        }
    }

    CoUninitialize();
}

/// Creates the window, its hidden menu owner and the taskbar watcher on the
/// indicator thread.
unsafe fn create_indicator_window(
    settings: Arc<Mutex<IndicatorSettings>>,
) -> anyhow::Result<(HWND, Box<State>)> {
    let hinstance = GetModuleHandleW(None)?;
    let class_name = w!("TaskbarSwitcherIndicator");

    let wnd_class = WNDCLASSW {
        hInstance: HINSTANCE(hinstance.0),
        lpszClassName: class_name,
        lpfnWndProc: Some(IndicatorWindow::window_proc),
        ..Default::default()
    };
    let _ = RegisterClassW(&wnd_class);

    let taskbar =
        Taskbar::primary().ok_or_else(|| anyhow::anyhow!("primary taskbar is unavailable"))?;
    let geometry = IndicatorGeometry::for_taskbar(&taskbar)
        .ok_or_else(|| anyhow::anyhow!("taskbar geometry is unavailable"))?;
    let (window_width, window_height) = geometry.physical_size();

    // Created hidden as a popup, then turned into a child right away: the window
    // must not appear while it is still a top-level popup. `render` shows it once
    // it sits in the taskbar's client area.
    //
    // `WS_EX_NOPARENTNOTIFY` keeps the shell from receiving create/click
    // notifications for a foreign child window that it never expected.
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOPARENTNOTIFY,
        class_name,
        w!("Indicator"),
        WS_POPUP,
        0,
        0,
        window_width,
        window_height,
        None,
        None,
        Some(hinstance.into()),
        None,
    )?;
    reparent_to_taskbar(hwnd, taskbar.hwnd());

    // Safety net for shell re-layouts: if Explorer ever puts its XAML content
    // bridge back above the dots, the next tick restores the order.
    SetTimer(Some(hwnd), TIMER_Z_ORDER, Z_ORDER_CHECK_MS, None);

    let menu_host = DesktopMover::create_menu_host(hwnd);
    let watcher = TaskbarWatcher::new(hwnd, taskbar.hwnd());
    let mut state = Box::new(State::new(
        settings,
        taskbar.is_hidden(),
        menu_host,
        watcher,
    ));
    SetWindowLongPtrW(
        hwnd,
        GWLP_USERDATA,
        Box::as_mut(&mut state) as *mut State as isize,
    );

    if !state.taskbar_hidden.get() {
        state.render(hwnd);
    }

    Ok((hwnd, state))
}

/// Runs the indicator thread's message loop until the window is destroyed.
unsafe fn run_message_loop() {
    let mut msg = std::mem::zeroed();
    while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

impl State {
    fn new(
        settings: Arc<Mutex<IndicatorSettings>>,
        initially_hidden: bool,
        menu_host: HWND,
        watcher: Option<TaskbarWatcher>,
    ) -> Self {
        Self {
            settings,
            hover: Cell::new(None),
            move_mode: Cell::new(false),
            move_target: Cell::new(None),
            taskbar_hidden: Cell::new(initially_hidden),
            last_taskbar_rect: Cell::new(None),
            menu_host,
            watcher,
        }
    }

    /// Syncs indicator visibility, position and parent against the current
    /// taskbar state.
    ///
    /// If the taskbar is auto-hidden off-screen, the indicator hides (`SW_HIDE`).
    /// When the taskbar reappears, moves, or was recreated by Explorer, the
    /// indicator re-parents and draws itself again.
    fn sync_taskbar(&self, hwnd: HWND) {
        unsafe {
            let Some(taskbar) = Taskbar::primary() else {
                if !self.taskbar_hidden.get() {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    self.taskbar_hidden.set(true);
                }
                return;
            };

            let is_hidden = taskbar.is_hidden();
            let rect = taskbar.rect();
            let was_hidden = self.taskbar_hidden.get();
            let last_rect = self.last_taskbar_rect.get();
            let reattached = self.attach(hwnd, taskbar.hwnd());
            if let Some(watcher) = &self.watcher {
                watcher.update_taskbar(hwnd, taskbar.hwnd());
            }

            if is_hidden {
                if !was_hidden {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    self.taskbar_hidden.set(true);
                }
            } else if was_hidden || reattached || Some(rect) != last_rect {
                self.render(hwnd);
            }
        }
    }

    /// Re-parents the indicator to `taskbar_hwnd` when it is not a child of it
    /// yet (Explorer recreated the taskbar). A child follows its parent, so a
    /// stale parent means the dots would move or vanish with a window that no
    /// longer exists.
    ///
    /// Returns `true` when the parent changed.
    fn attach(&self, hwnd: HWND, taskbar_hwnd: HWND) -> bool {
        unsafe {
            if let Ok(parent) = GetParent(hwnd) {
                if parent == taskbar_hwnd {
                    return false;
                }
            }
            let _ = SetParent(hwnd, Some(taskbar_hwnd));
            true
        }
    }

    /// Puts the dots back above the taskbar's other children when something
    /// ended up over them.
    ///
    /// Explorer's XAML content bridge is a full-size sibling of the indicator,
    /// and a shell re-layout can raise it again. The check is one `GetWindow`
    /// call and does nothing while the order is already right.
    fn keep_on_top(&self, hwnd: HWND) {
        unsafe {
            // `Err` means "no window above me", i.e. the order is already right.
            if GetWindow(hwnd, GW_HWNDPREV).is_err() {
                return;
            }
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Renders the dots into a 32-bit ARGB bitmap and pushes it to the screen
    /// with `UpdateLayeredWindow`.
    fn render(&self, hwnd: HWND) {
        unsafe {
            let Some(taskbar) = Taskbar::primary() else {
                let _ = ShowWindow(hwnd, SW_HIDE);
                self.taskbar_hidden.set(true);
                return;
            };

            if taskbar.is_hidden() {
                let _ = ShowWindow(hwnd, SW_HIDE);
                self.taskbar_hidden.set(true);
                return;
            }

            // Explorer may have restarted since the last frame; the dots must
            // follow the *current* taskbar, not the dead one.
            self.attach(hwnd, taskbar.hwnd());

            self.last_taskbar_rect.set(Some(taskbar.rect()));

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

                let Ok(settings) = self.settings.lock().map(|s| *s) else {
                    return;
                };
                let painter = DotPainter::new(geometry, settings.accent_color, settings.background);
                let mut canvas =
                    Canvas::new(buffer, phys_width, phys_height, geometry.dots_vertical());
                painter.paint(
                    &mut canvas,
                    self.hover.get(),
                    utils::current_index(),
                    move_modifier_down(),
                );

                let (screen_x, screen_y) = geometry.origin(&taskbar, settings.position);
                let (client_x, client_y) = client_point(taskbar.hwnd(), screen_x, screen_y);

                // Position inside the taskbar and stay on top of its children:
                // Explorer's XAML content bridge spans the whole bar and would
                // otherwise paint over the dots. Showing the window before the
                // first paint is harmless - a layered window has no surface yet,
                // so it stays fully transparent until `UpdateLayeredWindow` runs.
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    client_x,
                    client_y,
                    phys_width,
                    phys_height,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );

                let mut pt_src = POINT { x: 0, y: 0 };
                let mut size = SIZE {
                    cx: phys_width,
                    cy: phys_height,
                };
                let mut blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };

                // No destination point: the position comes from `SetWindowPos`
                // above, in the parent's client coordinates.
                let _ = UpdateLayeredWindow(
                    hwnd,
                    None,
                    None,
                    Some(&mut size as *mut _),
                    Some(mem_dc),
                    Some(&mut pt_src as *mut _),
                    COLORREF(0),
                    Some(&mut blend as *mut _),
                    ULW_ALPHA,
                );

                self.taskbar_hidden.set(false);

                SelectObject(mem_dc, old_bmp);
                let _ = DeleteObject(HGDIOBJ(bmp.0 as _));
            }

            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
        }
    }

    fn on_mouse_move(&self, hwnd: HWND, x: i32, y: i32) -> LRESULT {
        // Remember the window the user was working in while the pointer is still
        // only hovering: the click itself clears the foreground window.
        DesktopMover::capture_foreground();
        if self.taskbar_hidden.get() {
            return LRESULT(0);
        }
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
        let dot = hovered_dot(x, y);
        tracing::debug!("indicator left-click at ({x}, {y}) hit {dot:?}");
        let Some(idx) = dot else {
            // The click landed on the transparent part of the window; give the
            // focus back to the window the user was working in.
            DesktopMover::restore_foreground();
            return LRESULT(0);
        };
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
        LRESULT(0)
    }

    fn on_rbutton_up(&self, x: i32, y: i32) -> LRESULT {
        // The right-clicked dot tells us exactly which desktop N to act on, and
        // the target window is captured before the menu takes foreground.
        let dot = hovered_dot(x, y);
        tracing::debug!("indicator right-click at ({x}, {y}) hit {dot:?}");
        let Some(idx) = dot else {
            DesktopMover::restore_foreground();
            return LRESULT(0);
        };
        if let Some(target_hwnd) = DesktopMover::foreground_target() {
            let target = MoveTarget {
                hwnd: target_hwnd,
                desktop_index: idx,
            };
            self.move_target.set(Some(target));
            DesktopMover::show_menu(self.menu_host, target);
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

/// Turns `hwnd` into a child window of `taskbar` - the indicator's whole trick.
///
/// `SetParent` deliberately leaves `WS_POPUP`/`WS_CHILD` alone for compatibility
/// reasons, so the style has to be flipped by hand first. As a child, the window
/// is clipped to the taskbar, follows it when it moves, hides with it, and is
/// composited inside the taskbar's z-order band.
unsafe fn reparent_to_taskbar(hwnd: HWND, taskbar: HWND) {
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let child = (style & !(WS_POPUP.0 as isize)) | WS_CHILD.0 as isize;
    SetWindowLongPtrW(hwnd, GWL_STYLE, child);
    let _ = SetParent(hwnd, Some(taskbar));
}

/// Converts a screen point into the client coordinates of `parent`, which is
/// what a child window's position is expressed in.
unsafe fn client_point(parent: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut pt = POINT { x, y };
    let _ = ScreenToClient(parent, &mut pt);
    (pt.x, pt.y)
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
