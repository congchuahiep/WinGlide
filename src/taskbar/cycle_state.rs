//! Explicit position of the cycle and anti-stuck bookkeeping.
//!
//! The previous implementation derived the next target from the foreground window plus a
//! cached index. When the foreground window could not be matched to a button (UWP apps),
//! the stale index pointed back at the same window and every subsequent press became a
//! verified no-op: the user was stuck.
//!
//! This module keeps the cycle position explicitly:
//! - the cursor records the target of the last press, successful or not, and is used
//!   when the foreground cannot be resolved to a slot;
//! - windows that just failed to activate are blocked for a few seconds, so the next
//!   press skips them instead of retrying the same dead target;
//! - the source is the window that currently has focus, or the cursor when focus cannot
//!   be resolved.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use super::enumerator::CycleDirection;
use super::slot::SlotPlan;
use crate::window::LogicalWindow;

/// How long a window that refused activation stays out of the rotation.
const BLOCKED_TTL: Duration = Duration::from_secs(10);

/// How many different windows one keypress may try before giving up.
const MAX_CANDIDATES: usize = 3;

/// One window inside the flattened cycle order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// Index into [`SlotPlan::slots`].
    pub slot: usize,
    /// Index into [`SlotPlan::windows`].
    pub window: usize,
}

/// The targets one keypress should try, in order.
#[derive(Debug)]
pub(crate) struct PressPlan {
    pub candidates: Vec<Candidate>,
    /// Window the press started from, when it could be resolved.
    pub source: Option<Candidate>,
}

/// Position of the cycle plus the set of temporarily unactivatable windows.
#[derive(Debug, Default)]
pub(crate) struct CycleState {
    /// Primary handle of the last intended target.
    cursor: Option<isize>,
    /// Windows that failed to activate, with the time of the failure.
    blocked: HashMap<isize, Instant>,
}

impl CycleState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Computes the targets for one press.
    ///
    /// The cycle order is the taskbar order with grouped buttons expanded in place, so a
    /// grouped button is walked window by window before moving on to the next button.
    /// Group order is stable (see [`SlotPlan::build`]) so repeated presses cannot bounce
    /// between two windows forever.
    pub(crate) fn plan_press(
        &mut self,
        plan: &SlotPlan,
        foreground: Option<HWND>,
        direction: CycleDirection,
    ) -> PressPlan {
        self.prune_blocked();

        let flat = flatten(plan);
        if flat.is_empty() {
            return PressPlan {
                candidates: Vec::new(),
                source: None,
            };
        }

        // Resolve where we are. `source` is authoritative when the focused window is in
        // the plan; otherwise the cursor decides where the cycle continues.
        let source = foreground.and_then(|hwnd| {
            flat.iter()
                .position(|candidate| plan.windows[candidate.window].contains(hwnd))
        });

        if let Some(index) = source {
            // The user is on a window we know: keep the cursor in sync with reality.
            self.cursor = Some(plan.windows[flat[index].window].primary.0 as isize);
        }

        let cursor = self.cursor.and_then(|hwnd| {
            flat.iter()
                .position(|candidate| plan.windows[candidate.window].primary.0 as isize == hwnd)
        });

        let mut position = source.or(cursor);
        let mut candidates = Vec::with_capacity(MAX_CANDIDATES);
        let mut steps = 0;

        while candidates.len() < MAX_CANDIDATES && steps < flat.len() {
            steps += 1;
            position = match (position, direction) {
                (None, CycleDirection::Forward) => Some(0),
                (None, CycleDirection::Backward) => Some(flat.len() - 1),
                (Some(current), CycleDirection::Forward) => Some((current + 1) % flat.len()),
                (Some(current), CycleDirection::Backward) => {
                    Some(if current == 0 { flat.len() - 1 } else { current - 1 })
                }
            };

            let index = position.expect("position is assigned on every iteration");
            let candidate = flat[index];
            let window = &plan.windows[candidate.window];

            // Never target the window that already has focus: that would turn the press
            // into a no-op and is exactly the "stuck" failure mode.
            if foreground.is_some_and(|hwnd| window.contains(hwnd)) {
                continue;
            }

            if self.is_blocked(window.primary) {
                continue;
            }

            candidates.push(candidate);
        }

        PressPlan {
            candidates,
            source: source.map(|index| flat[index]),
        }
    }

    /// Records a verified activation and moves the cursor onto it.
    pub(crate) fn record_success(&mut self, target: &LogicalWindow) {
        let hwnd = target.primary.0 as isize;
        self.cursor = Some(hwnd);
        self.blocked.remove(&hwnd);
    }

    /// Records a press where every candidate failed.
    ///
    /// The cursor still advances to the first attempted window and all attempts are
    /// blocked, so the next press continues past the dead targets instead of repeating
    /// them for the rest of the session.
    pub(crate) fn record_failure(&mut self, first: HWND, attempted: &[HWND]) {
        self.cursor = Some(first.0 as isize);

        let now = Instant::now();
        for hwnd in attempted {
            self.blocked.insert(hwnd.0 as isize, now);
        }
    }

    fn is_blocked(&self, hwnd: HWND) -> bool {
        self.blocked
            .get(&(hwnd.0 as isize))
            .is_some_and(|since| since.elapsed() < BLOCKED_TTL)
    }

    fn prune_blocked(&mut self) {
        self.blocked
            .retain(|_, since| since.elapsed() < BLOCKED_TTL);
    }
}

/// Flattens the plan into the order the cycle walks: button by button, target by target.
fn flatten(plan: &SlotPlan) -> Vec<Candidate> {
    let mut flat = Vec::new();
    for slot in &plan.slots {
        for &window in &slot.targets {
            flat.push(Candidate {
                slot: slot.index,
                window,
            });
        }
    }
    flat
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taskbar::slot::Slot;
    use crate::window::{LogicalWindow, WindowInfo};
    use windows::Win32::Foundation::RECT;

    fn window(hwnd: isize) -> LogicalWindow {
        let info = WindowInfo {
            hwnd: HWND(hwnd as *mut _),
            title: format!("window-{hwnd}"),
            process_id: 1,
            rect: RECT::default(),
            process_name: "test.exe".to_string(),
            aumid: None,
            class_name: "TestWindow".to_string(),
        };

        LogicalWindow {
            primary: info.hwnd,
            members: vec![info.clone()],
            title: info.title.clone(),
            is_minimized: false,
            class_name: info.class_name.clone(),
        }
    }

    /// Builds a plan where each entry is one slot and its target handles.
    fn plan(groups: &[&[isize]]) -> SlotPlan {
        let mut windows = Vec::new();
        let mut slots = Vec::new();

        for (index, group) in groups.iter().enumerate() {
            let mut targets = Vec::new();
            for &hwnd in group.iter() {
                targets.push(windows.len());
                windows.push(window(hwnd));
            }
            slots.push(Slot {
                index,
                name: format!("slot-{index}"),
                rect: RECT::default(),
                key: format!("key-{index}"),
                targets,
            });
        }

        SlotPlan { windows, slots }
    }

    fn hwnd(value: isize) -> HWND {
        HWND(value as *mut _)
    }

    fn target_hwnds(plan: &SlotPlan, press: &PressPlan) -> Vec<isize> {
        press
            .candidates
            .iter()
            .map(|candidate| plan.windows[candidate.window].primary.0 as isize)
            .collect()
    }

    #[test]
    fn forward_moves_to_the_next_slot() {
        let plan = plan(&[&[10], &[20], &[30]]);
        let press = CycleState::new().plan_press(&plan, Some(hwnd(10)), CycleDirection::Forward);

        assert_eq!(press.source.map(|c| c.slot), Some(0));
        // The window that already has focus is never a candidate, even after wrapping.
        assert_eq!(target_hwnds(&plan, &press), vec![20, 30]);
    }

    #[test]
    fn backward_wraps_around() {
        let plan = plan(&[&[10], &[20], &[30]]);
        let press = CycleState::new().plan_press(&plan, Some(hwnd(10)), CycleDirection::Backward);

        assert_eq!(target_hwnds(&plan, &press), vec![30, 20]);
    }

    #[test]
    fn unknown_foreground_continues_from_the_cursor() {
        let plan = plan(&[&[10], &[20], &[30]]);
        let mut state = CycleState::new();

        // Simulate a previous press that ended on 20.
        state.record_success(&plan.windows[1]);

        let press = state.plan_press(&plan, Some(hwnd(999)), CycleDirection::Forward);
        assert_eq!(target_hwnds(&plan, &press), vec![30, 10, 20]);
    }

    #[test]
    fn unknown_foreground_without_cursor_starts_at_the_first_slot() {
        let plan = plan(&[&[10], &[20]]);
        let press = CycleState::new().plan_press(&plan, None, CycleDirection::Forward);

        assert_eq!(target_hwnds(&plan, &press), vec![10, 20]);
    }

    #[test]
    fn grouped_button_is_walked_before_leaving() {
        let plan = plan(&[&[10, 11], &[20]]);
        let mut state = CycleState::new();

        let first = state.plan_press(&plan, Some(hwnd(10)), CycleDirection::Forward);
        assert_eq!(target_hwnds(&plan, &first), vec![11, 20]);

        let second = state.plan_press(&plan, Some(hwnd(11)), CycleDirection::Forward);
        assert_eq!(target_hwnds(&plan, &second), vec![20, 10]);
    }

    #[test]
    fn grouped_button_is_walked_backwards_too() {
        let plan = plan(&[&[10, 11], &[20]]);
        let press = CycleState::new().plan_press(&plan, Some(hwnd(11)), CycleDirection::Backward);

        assert_eq!(target_hwnds(&plan, &press), vec![10, 20]);
    }

    #[test]
    fn empty_slots_are_skipped() {
        let plan = plan(&[&[10], &[], &[30]]);
        let press = CycleState::new().plan_press(&plan, Some(hwnd(10)), CycleDirection::Forward);

        assert_eq!(target_hwnds(&plan, &press), vec![30]);
    }

    #[test]
    fn blocked_windows_are_skipped() {
        let plan = plan(&[&[10], &[20], &[30]]);
        let mut state = CycleState::new();
        state.record_failure(hwnd(20), &[hwnd(20)]);

        let press = state.plan_press(&plan, Some(hwnd(10)), CycleDirection::Forward);
        assert_eq!(target_hwnds(&plan, &press), vec![30]);
    }

    #[test]
    fn failed_press_still_advances_the_cursor() {
        let plan = plan(&[&[10], &[20], &[30]]);
        let mut state = CycleState::new();

        let press = state.plan_press(&plan, Some(hwnd(10)), CycleDirection::Forward);
        assert_eq!(press.candidates.len(), 2);

        state.record_failure(hwnd(20), &[hwnd(20)]);
        state.record_failure(hwnd(30), &[hwnd(30)]);

        let next = state.plan_press(&plan, Some(hwnd(999)), CycleDirection::Forward);
        assert_eq!(target_hwnds(&plan, &next), vec![10]);
    }
}
