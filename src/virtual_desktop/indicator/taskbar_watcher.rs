//! WinEvent hook monitoring `EVENT_OBJECT_LOCATIONCHANGE` on `Shell_TrayWnd`.
//!
//! When the primary taskbar slides off-screen (auto-hide) or unhides, Windows
//! fires `EVENT_OBJECT_LOCATIONCHANGE`. This watcher posts `WM_APP_TASKBAR_MOVED`
//! to the indicator window so it can update its position and visibility without
//! any polling or timer.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use tracing::debug;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowThreadProcessId, PostMessageW, EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE,
    EVENT_OBJECT_SHOW, INDEXID_CONTAINER, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};

use super::WM_APP_TASKBAR_MOVED;

static TARGET_TASKBAR_HWND: AtomicIsize = AtomicIsize::new(0);
static INDICATOR_HWND: AtomicIsize = AtomicIsize::new(0);
static SYNC_PENDING: AtomicBool = AtomicBool::new(false);

/// The WinEvent ranges the watcher listens to, all filtered to the shell's process.
///
/// The bar sliding for auto-hide reports a location change; the bar coming back
/// from a fullscreen app reports a **show**, which carries no location change at
/// all. Without that second range the dots stayed hidden until something else
/// happened to move the bar.
const WATCHED_EVENTS: [(u32, u32); 2] = [
    (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
    (EVENT_OBJECT_SHOW, EVENT_OBJECT_HIDE),
];

/// RAII wrapper around the hooks that watch the taskbar for movement and for being
/// shown again.
pub(super) struct TaskbarWatcher {
    hooks: Mutex<Vec<HWINEVENTHOOK>>,
    taskbar_hwnd: AtomicIsize,
}

/// Installs one hook per watched range for the shell's process `pid`.
fn install_hooks(pid: u32) -> Vec<HWINEVENTHOOK> {
    WATCHED_EVENTS
        .iter()
        .map(|&(min, max)| unsafe {
            SetWinEventHook(
                min,
                max,
                None,
                Some(taskbar_event_proc),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        })
        .filter(|hook| !hook.is_invalid())
        .collect()
}

fn unhook_all(hooks: &[HWINEVENTHOOK]) {
    for hook in hooks {
        unsafe {
            let _ = UnhookWinEvent(*hook);
        }
    }
}

impl TaskbarWatcher {
    /// Installs a WinEvent hook targeting `taskbar_hwnd`'s owning process.
    pub(super) fn new(indicator_hwnd: HWND, taskbar_hwnd: HWND) -> Option<Self> {
        INDICATOR_HWND.store(indicator_hwnd.0 as isize, Ordering::SeqCst);
        TARGET_TASKBAR_HWND.store(taskbar_hwnd.0 as isize, Ordering::SeqCst);
        SYNC_PENDING.store(false, Ordering::SeqCst);

        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(taskbar_hwnd, Some(&mut pid));
        }

        let hooks = install_hooks(pid);

        if hooks.is_empty() {
            debug!("Failed to install TaskbarWatcher hooks");
            None
        } else {
            debug!("TaskbarWatcher hooks installed for PID {}", pid);
            Some(Self {
                hooks: Mutex::new(hooks),
                taskbar_hwnd: AtomicIsize::new(taskbar_hwnd.0 as isize),
            })
        }
    }

    /// Re-installs the hook if the taskbar HWND has changed (e.g. after Explorer restart).
    pub(super) fn update_taskbar(&self, indicator_hwnd: HWND, new_taskbar_hwnd: HWND) {
        if self.taskbar_hwnd.load(Ordering::Relaxed) != new_taskbar_hwnd.0 as isize {
            if let Ok(mut hooks) = self.hooks.lock() {
                unhook_all(&hooks);
                INDICATOR_HWND.store(indicator_hwnd.0 as isize, Ordering::SeqCst);
                TARGET_TASKBAR_HWND.store(new_taskbar_hwnd.0 as isize, Ordering::SeqCst);
                SYNC_PENDING.store(false, Ordering::SeqCst);

                let mut pid = 0;
                unsafe {
                    GetWindowThreadProcessId(new_taskbar_hwnd, Some(&mut pid));
                }

                *hooks = install_hooks(pid);
                self.taskbar_hwnd
                    .store(new_taskbar_hwnd.0 as isize, Ordering::SeqCst);
                debug!("TaskbarWatcher hooks updated for new PID {}", pid);
            }
        }
    }

    /// Resets the coalescing flag so subsequent location events post a new message.
    pub(super) fn reset_pending() {
        SYNC_PENDING.store(false, Ordering::SeqCst);
    }
}

impl Drop for TaskbarWatcher {
    fn drop(&mut self) {
        if let Ok(hooks) = self.hooks.lock() {
            unhook_all(&hooks);
        }
        TARGET_TASKBAR_HWND.store(0, Ordering::SeqCst);
        INDICATOR_HWND.store(0, Ordering::SeqCst);
        SYNC_PENDING.store(false, Ordering::SeqCst);
        debug!("TaskbarWatcher hook uninstalled");
    }
}

/// Static callback invoked by Windows when the taskbar moves or is shown/hidden.
unsafe extern "system" fn taskbar_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if !matches!(
        event,
        EVENT_OBJECT_LOCATIONCHANGE | EVENT_OBJECT_SHOW | EVENT_OBJECT_HIDE
    ) {
        return;
    }
    if id_object != OBJID_WINDOW.0 || id_child != INDEXID_CONTAINER as i32 {
        return;
    }

    let target = TARGET_TASKBAR_HWND.load(Ordering::Relaxed);
    if target == 0 || hwnd.0 as isize != target {
        return;
    }

    // Coalesce rapid animation frames so we don't flood the message queue.
    if !SYNC_PENDING.swap(true, Ordering::SeqCst) {
        let ind = INDICATOR_HWND.load(Ordering::Relaxed);
        if ind != 0 {
            tracing::debug!(
                "Shell_TrayWnd event 0x{:X} – event_time={}ms",
                event, _event_time
            );

            let _ = PostMessageW(
                Some(HWND(ind as _)),
                WM_APP_TASKBAR_MOVED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}
