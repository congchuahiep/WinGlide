//! Enumerates buttons on the Windows 11 taskbar in correct order from left to right.
//!
//! # Why not use `FindWindow` directly?
//!
//! On Windows 10, taskbar buttons are `ToolbarWindow32`, a standard Windows control.
//! We can use the `TB_GETBUTTON` message to get information directly. But on **Windows 11**,
//! Microsoft rewrote the taskbar using **XAML** (UWP/WinRT). Buttons are no longer distinct `HWND`s,
//! they are **XAML elements** inside `Windows.UI.Composition.DesktopWindowContentBridge`.
//!
//! Therefore we must use **UI Automation (UIAutomation)**, a COM-based API that allows accessing UI
//! elements regardless of the underlying technology (Win32, XAML, WebView, etc.).
//!
//! # Key concept: IUIAutomation
//!
//! **IUIAutomation** is like a "screen reader" for the visually impaired. It describes everything on the screen as a **hierarchy tree**:
//!
//! ```text
//! Root (Desktop)
//!  └── Shell_TrayWnd (Taskbar)
//!       └── Windows.UI.Composition.DesktopWindowContentBridge
//!            └── Taskbar.TaskListButtonAutomationPeer  ← these are the buttons!
//!            └── Taskbar.TaskListButtonAutomationPeer
//!            └── ...
//! ```

use std::cell::{Cell, RefCell};
use std::time::Instant;
use tracing::{debug, error, instrument, warn};
use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, HMONITOR, MONITOR_DEFAULTTONULL};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    AutomationElementMode_None, CUIAutomation, IUIAutomation, IUIAutomationCacheRequest,
    IUIAutomationCondition, IUIAutomationElementArray, TreeScope_Descendants,
    UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId,
    UIA_NamePropertyId, UIA_ProcessIdPropertyId,
};
use windows::Win32::UI::Shell::IVirtualDesktopManager;
use windows::Win32::UI::Shell::VirtualDesktopManager;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, GetForegroundWindow};

use super::cycle_state::CycleState;
use super::explorer_process::invalidate_explorer_pid_cache;
use super::slot::SlotPlan;
use super::TaskbarButton;
use crate::event::UiaEventHook;
use crate::window::activate;
use crate::window::context::WindowContext;
use crate::window::enumerate::find_visible_windows;
use crate::window::logical_window::group_logical_windows;

/// Cache TTL: 1 second. If no WinEvent invalidates it, the cache auto-expires after 1s.
const CACHE_TTL_SECS: f64 = 1.0;

/// Button cache with timestamp and the monitor it belongs to.
///
/// The monitor is part of the key: with a taskbar on several monitors, a cache that only
/// remembered the buttons would return another monitor's buttons for up to a second.
struct ButtonCache {
    monitor: HMONITOR,
    buttons: Vec<TaskbarButton>,
    created_at: Instant,
}

/// Cycle direction: left or right on the taskbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleDirection {
    Forward,
    Backward,
}

/// Enumerates taskbar buttons on Windows 11.
pub struct TaskbarEnumerator {
    /// COM interface IUIAutomation, UI "scanner".
    automation: IUIAutomation,

    /// Caches the button list to avoid re-enumerating on every keystroke.
    ///
    /// [`RefCell`] allows mutation from `&self` methods (no need for `&mut self`).
    /// The cache is invalidated by UIA events or when TTL (1 second) expires.
    button_cache: RefCell<Option<ButtonCache>>,

    /// Virtual Desktop ID of the most recent foreground window.
    ///
    /// Used to detect desktop switching, if ID changes -> invalidate cache.
    last_desktop_id: RefCell<Option<windows::core::GUID>>,

    /// List of HWNDs of the taskbar windows (both primary and secondary monitors' taskbars).
    taskbars: RefCell<Vec<HWND>>,

    /// UIA StructureChanged event hooks.
    uia_hooks: RefCell<Vec<UiaEventHook>>,

    /// UIA StructureChanged event hook - auto subscribes/unsubscribes.
    ///
    /// Upon explorer restart, [`Self::refresh_taskbar_hwnd`] will drop the old and create a new one.
    uia_thread_id: Cell<u32>,

    /// Explicit cycle position and anti-stuck bookkeeping.
    cycle_state: RefCell<CycleState>,
}

/// Life cycle implementation
impl TaskbarEnumerator {
    /// Creates a new enumerator and initializes COM (STA apartment).
    ///
    /// # COM Apartments
    ///
    /// Windows COM has 2 apartment types:
    /// - **STA (Single-Threaded Apartment)**: Each thread has its own message queue, using
    /// `GetMessageW`.
    /// - **MTA (Multi-Threaded Apartment)**: No message queue, using
    /// `CoWaitForMultipleObjects`.
    ///
    /// IUIAutomation works well with both, but STA is recommended for simplicity.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let enumerator = TaskbarEnumerator::new()?;
    /// let buttons = enumerator.enumerate_current_monitor_buttons()?;
    /// ```
    pub fn new() -> anyhow::Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;

            let taskbars = Self::get_all_taskbar_hwnds();
            if taskbars.is_empty() {
                anyhow::bail!(
                    "No taskbar found, possibly running in portable mode or the taskbar is disabled"
                )
            }

            Ok(Self {
                automation,
                taskbars: RefCell::new(taskbars),
                button_cache: RefCell::new(None),
                last_desktop_id: RefCell::new(None),
                uia_hooks: RefCell::new(Vec::new()),
                uia_thread_id: Cell::new(0),
                cycle_state: RefCell::new(CycleState::new()),
            })
        }
    }

    /// Registers UIA StructureChanged event handler on taskbar elements.
    ///
    /// Called once from `App::run()`. After explorer restarts, [`Self::refresh_taskbar_hwnd`]
    /// automatically re-subscribes.
    pub fn install_uia_hook(&self, main_thread_id: u32) -> anyhow::Result<()> {
        self.uia_thread_id.set(main_thread_id);
        let taskbars = self.taskbars.borrow();
        let mut hooks = self.uia_hooks.borrow_mut();
        hooks.clear();
        for &hwnd in taskbars.iter() {
            if let Ok(hook) =
                unsafe { UiaEventHook::install(&self.automation, hwnd, main_thread_id) }
            {
                hooks.push(hook);
            }
        }
        Ok(())
    }

    /// Self-recovers after explorer.exe crashes and restarts.
    ///
    /// NOTE: This logic is called at [`Self::enumerate_buttons`], which might sound incorrect
    /// because this method should ideally be called when the explorer restart event occurs. The
    /// reason for not that approach is because it's a bit cumbersome, but in the future it's still
    /// better to catch the event instead of calling it manually.
    pub fn refresh_taskbar_hwnd(&self) -> anyhow::Result<()> {
        let taskbars = Self::get_all_taskbar_hwnds();
        if taskbars.is_empty() {
            anyhow::bail!("No taskbars found after explorer restart");
        }

        *self.taskbars.borrow_mut() = taskbars;
        self.invalidate_cache();
        invalidate_explorer_pid_cache();

        let mut hooks = self.uia_hooks.borrow_mut();
        hooks.clear();
        for &hwnd in self.taskbars.borrow().iter() {
            if let Ok(hook) =
                unsafe { UiaEventHook::install(&self.automation, hwnd, self.uia_thread_id.get()) }
            {
                hooks.push(hook);
            }
        }

        debug!("Taskbar HWNDs refreshed, UIA re-subscribed");
        Ok(())
    }
}

/// Core implementation
impl TaskbarEnumerator {
    /// Cycles to the next window on the taskbar and activates it.
    ///
    /// The order is the taskbar order of the current monitor, with grouped buttons
    /// expanded in place. The source is the focused window when it can be resolved, and
    /// the stored cursor otherwise, so an app the matcher cannot identify (UWP) still
    /// moves the cycle forward instead of trapping it.
    ///
    /// # Errors
    ///
    /// Returns an error only when the taskbar buttons cannot be enumerated.
    #[instrument(level = "debug", skip_all)]
    pub fn cycle_to_neighbor(&self, direction: CycleDirection) -> anyhow::Result<()> {
        let context = WindowContext::current_state();
        let buttons = self.enumerate_buttons_by_monitor(context.monitor)?;
        if buttons.is_empty() {
            warn!("No taskbar buttons on the current monitor");
            return Ok(());
        }

        let windows = group_logical_windows(find_visible_windows(Some(&context)));
        let plan = SlotPlan::build(&buttons, windows);

        let press = {
            let mut state = self.cycle_state.borrow_mut();
            state.plan_press(&plan, Some(context.foreground_window), direction)
        };

        debug!(
            "cycle {:?}: source={:?} | {}",
            direction,
            press.source.map(|c| c.slot),
            plan.summary()
        );

        if press.candidates.is_empty() {
            warn!("No window left to cycle to");
            return Ok(());
        }

        let mut attempted = Vec::new();
        for candidate in &press.candidates {
            let target = &plan.windows[candidate.window];
            let slot = &plan.slots[candidate.slot];

            // The click fallback is only safe for a button that represents a single
            // window; clicking a grouped button opens the thumbnail flyout instead of
            // switching, which would leave the ui in a state the user did not ask for.
            let button_rect = (slot.targets.len() == 1).then_some(slot.rect);

            debug!(
                "cycle: trying '{}' class='{}' primary={:?} from slot [{}] '{}'",
                target.title, target.class_name, target.primary, slot.index, slot.name
            );

            match activate::activate(target, button_rect) {
                Some(method) => {
                    self.cycle_state.borrow_mut().record_success(target);
                    debug!(
                        "cycle: activated '{}' ({:?}) via {:?}",
                        target.title, target.primary, method
                    );
                    return Ok(());
                }
                None => {
                    warn!(
                        "cycle: cannot activate '{}' ({:?})",
                        target.title, target.primary
                    );
                    attempted.push(target.primary);
                }
            }
        }

        if let Some(first) = press.candidates.first() {
            let first_primary = plan.windows[first.window].primary;
            self.cycle_state
                .borrow_mut()
                .record_failure(first_primary, &attempted);
        }

        Ok(())
    }

    /// Gets all HWNDs of taskbars (primary and secondary).
    fn get_all_taskbar_hwnds() -> Vec<HWND> {
        let mut hwnds = Vec::new();
        unsafe {
            if let Ok(primary) = FindWindowW(w!("Shell_TrayWnd"), None) {
                if !primary.0.is_null() {
                    hwnds.push(primary);
                }
            }

            let mut secondary = HWND::default();
            loop {
                secondary =
                    FindWindowExW(None, Some(secondary), w!("Shell_SecondaryTrayWnd"), None)
                        .unwrap_or_default();

                if secondary.0.is_null() {
                    break;
                }
                hwnds.push(secondary);
            }
        }
        hwnds
    }

    /// Returns taskbar buttons on the given monitor.
    ///
    /// Cached with a 1-second TTL, invalidated by UIA events or desktop switches. The
    /// monitor is part of the cache key so moving between monitors never serves the
    /// other monitor's buttons.
    pub fn enumerate_buttons_by_monitor(
        &self,
        monitor: HMONITOR,
    ) -> anyhow::Result<Vec<TaskbarButton>> {
        if self.desktop_changed() {
            self.invalidate_cache();
        }

        if let Some(ref cache) = *self.button_cache.borrow() {
            let age = cache.created_at.elapsed().as_secs_f64();
            if cache.monitor == monitor && age < CACHE_TTL_SECS {
                debug!("Using cached buttons (age: {:.0}ms)", age * 1000.0);
                return Ok(cache.buttons.clone());
            }
        }

        unsafe {
            // Copy the target handle out and drop the borrow before enumerating:
            // `enumerate_buttons` may call `refresh_taskbar_hwnd`, which takes a
            // mutable borrow of the same `RefCell` when Explorer restarted mid-call.
            let target_taskbar = {
                let taskbars = self.taskbars.borrow();
                taskbars
                    .iter()
                    .find(|&&hwnd| {
                        let hmonitor_tb = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
                        !hmonitor_tb.is_invalid() && hmonitor_tb.0 == monitor.0
                    })
                    .copied()
                    .unwrap_or_else(|| *taskbars.first().unwrap_or(&HWND::default()))
            };

            let buttons = self.enumerate_buttons(target_taskbar)?;
            *self.button_cache.borrow_mut() = Some(ButtonCache {
                monitor,
                buttons: buttons.clone(),
                created_at: Instant::now(),
            });
            Ok(buttons)
        }
    }

    /// Checks if the virtual desktop has changed via `IVirtualDesktopManager::GetWindowDesktopId`.
    fn desktop_changed(&self) -> bool {
        let fg = unsafe { GetForegroundWindow() };
        if fg.0.is_null() {
            return false;
        }

        let mgr: IVirtualDesktopManager =
            match unsafe { CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_LOCAL_SERVER) } {
                Ok(m) => m,
                Err(_) => return false,
            };

        let current_id = match unsafe { mgr.GetWindowDesktopId(fg) } {
            Ok(id) => id,
            Err(_) => return false,
        };

        let mut last = self.last_desktop_id.borrow_mut();
        let changed = match *last {
            Some(ref prev) => prev != &current_id,
            None => false,
        };

        *last = Some(current_id);
        changed
    }

    /// Detects `EVENT_E_ALL_SUBSCRIBERS_FAILED (0x80040201)` error upon explorer restart.
    fn is_subscribers_failed(e: &anyhow::Error) -> bool {
        e.chain().any(|c| c.to_string().contains("0x80040201"))
    }

    /// Enumerates taskbar buttons of a specific taskbar HWND via UIA.
    ///
    /// Fallback: if `FindAllBuildCache` returns empty, tries `enumerate_via_bridge_windows`.
    /// Self-recovers if `EVENT_E_ALL_SUBSCRIBERS_FAILED` (explorer restart) is encountered.
    #[instrument(level = "debug", skip_all)]
    unsafe fn enumerate_buttons(&self, target_taskbar: HWND) -> anyhow::Result<Vec<TaskbarButton>> {
        /// Creates a CacheRequest to batch 4 UIA properties (name, rect, PID, automation_id)
        /// so it only takes 1 COM call instead of 4 separate calls.
        unsafe fn create_button_cache_request(
            automation: &IUIAutomation,
        ) -> anyhow::Result<IUIAutomationCacheRequest> {
            let cache = automation.CreateCacheRequest()?;

            cache.AddProperty(UIA_NamePropertyId)?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
            cache.AddProperty(UIA_ProcessIdPropertyId)?;
            cache.AddProperty(UIA_AutomationIdPropertyId)?;

            cache.SetAutomationElementMode(AutomationElementMode_None)?;

            Ok(cache)
        }

        let try_enumerate_buttons = |taskbar_hwnd| -> anyhow::Result<Vec<TaskbarButton>> {
            let class_condition = self.automation.CreatePropertyCondition(
                UIA_ClassNamePropertyId,
                &VARIANT::from("Taskbar.TaskListButtonAutomationPeer"),
            )?;

            let cache_request = create_button_cache_request(&self.automation)?;

            let root_element = self.automation.ElementFromHandle(taskbar_hwnd)?;
            let items = root_element.FindAllBuildCache(
                TreeScope_Descendants,
                &class_condition,
                &cache_request,
            )?;

            let mut all_buttons = Vec::new();
            self.collect_buttons(&items, &mut all_buttons)?;

            if all_buttons.is_empty() {
                self.enumerate_via_bridge_windows(
                    taskbar_hwnd,
                    &class_condition,
                    &cache_request,
                    &mut all_buttons,
                )?;
            }

            all_buttons.sort_by_key(|b| b.rect.left);
            Ok(all_buttons)
        };

        match try_enumerate_buttons(target_taskbar) {
            Ok(buttons) => Ok(buttons),
            Err(ref e) if Self::is_subscribers_failed(e) => {
                warn!("UIA subscribers failed (explorer restart?), recovering...");
                self.refresh_taskbar_hwnd().map_err(|e2| {
                    error!("Failed to recover taskbar: {e2}");
                    anyhow::anyhow!("{e}")
                })?;

                let new_target = self
                    .taskbars
                    .borrow()
                    .first()
                    .copied()
                    .unwrap_or(target_taskbar);
                try_enumerate_buttons(new_target)
            }
            Err(e) => Err(e),
        }
    }

    /// Extracts information from the UIA element array via cached properties.
    ///
    /// Uses `Cached*` methods (no separate COM calls needed) because properties were already
    /// batch-read via `CacheRequest` in `FindAllBuildCache`.
    #[instrument(level = "debug", skip_all)]
    unsafe fn collect_buttons(
        &self,
        items: &IUIAutomationElementArray,
        buttons: &mut Vec<TaskbarButton>,
    ) -> anyhow::Result<()> {
        let count = items.Length()?;

        debug!("collect_buttons: count={}", count);

        for i in 0..count {
            let item = items.GetElement(i)?;

            let name = item
                .CachedName()
                .ok()
                .map(|b| b.to_string())
                .unwrap_or_default();

            let rect = match item.CachedBoundingRectangle() {
                Ok(r) => r,
                Err(_) => continue,
            };

            let process_id = item.CachedProcessId().unwrap_or(0);
            let automation_id = item.CachedAutomationId().ok().map(|s| s.to_string());

            buttons.push(TaskbarButton {
                name,
                rect,
                process_id,
                automation_id,
            });
        }

        Ok(())
    }

    /// Fallback: finds buttons via `DesktopWindowContentBridge` child windows of the taskbar.
    ///
    /// Win11 can render buttons inside the bridge window (XAML island)
    /// instead of directly under Shell_TrayWnd.
    #[instrument(level = "debug", skip_all)]
    unsafe fn enumerate_via_bridge_windows(
        &self,
        root_hwnd: HWND,
        condition: &IUIAutomationCondition,
        cache_request: &IUIAutomationCacheRequest,
        buttons: &mut Vec<TaskbarButton>,
    ) -> anyhow::Result<()> {
        let mut child_hwnd = HWND::default();

        loop {
            child_hwnd = FindWindowExW(
                Some(root_hwnd),
                Some(child_hwnd),
                w!("Windows.UI.Composition.DesktopWindowContentBridge"),
                None,
            )
            .unwrap_or_default();

            if child_hwnd.0.is_null() {
                break;
            }

            if let Ok(bridge_element) = self.automation.ElementFromHandle(child_hwnd) {
                if let Ok(items) = bridge_element.FindAllBuildCache(
                    TreeScope_Descendants,
                    condition,
                    cache_request,
                ) {
                    self.collect_buttons(&items, buttons)?;
                }
            }

            if !buttons.is_empty() {
                break;
            }
        }

        Ok(())
    }
}

/// Cached implementation
impl TaskbarEnumerator {
    /// Invalidates button cache - called upon receiving UIA StructureChanged event.
    pub fn invalidate_cache(&self) {
        let mut cache = self.button_cache.borrow_mut();
        if cache.is_some() {
            debug!("Button cache invalidated (event)");
            *cache = None;
        }
    }
}

#[cfg(test)]
mod live_probe {
    use super::*;

    /// Manual probe: prints how the live taskbar maps to windows.
    /// Run with `cargo test print_slot_plan -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the live desktop"]
    fn print_slot_plan() {
        let enumerator = TaskbarEnumerator::new().expect("enumerator");
        let context = WindowContext::current_state();
        let buttons = enumerator
            .enumerate_buttons_by_monitor(context.monitor)
            .expect("buttons");
        let windows = group_logical_windows(find_visible_windows(Some(&context)));
        let plan = SlotPlan::build(&buttons, windows);

        println!("foreground: {:?}", context.foreground_window);
        println!("buttons: {}", buttons.len());
        for slot in &plan.slots {
            println!(
                "slot {} '{}' key={} targets={:?}",
                slot.index,
                slot.name,
                slot.key,
                slot.targets
                    .iter()
                    .map(|&index| (plan.windows[index].primary, plan.windows[index].title.as_str()))
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Manual probe: presses the cycle once and restores the original window.
    #[test]
    #[ignore = "activates a real window"]
    fn live_cycle_once() {
        let enumerator = TaskbarEnumerator::new().expect("enumerator");
        let original = WindowContext::current_state().foreground_window;
        println!("before: {original:?}");

        enumerator
            .cycle_to_neighbor(CycleDirection::Forward)
            .expect("forward press");
        println!("after forward: {:?}", unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
        });

        enumerator
            .cycle_to_neighbor(CycleDirection::Backward)
            .expect("backward press");
        println!("after backward: {:?}", unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
        });

        let context = WindowContext::current_state();
        if context.foreground_window != original {
            let windows = group_logical_windows(find_visible_windows(Some(&context)));
            if let Some(target) = windows.iter().find(|window| window.contains(original)) {
                println!("restoring via {:?}", activate::activate(target, None));
            }
        }
        println!("after restore: {:?}", unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow()
        });
    }
}
