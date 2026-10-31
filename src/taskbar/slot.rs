//! One slot per taskbar button, resolved to real windows by exact identity.
//!
//! # Why identity instead of heuristics
//!
//! The previous model rebuilt a button→window map on every keypress by trying five
//! fuzzy strategies and letting earlier buttons consume windows first. That made the
//! result depend on enumeration order and on which window happened to sort first, so
//! two instances of the same app could swap places between presses.
//!
//! Two facts make an exact model possible:
//!
//! 1. The taskbar's automation id is `Appid: <AUMID>`, i.e. the shell tells us which
//!    AUMID a button represents.
//! 2. WinGlide's uncombine feature writes `TaskbarSwitcher_<HWND>` as the AUMID of every
//!    window it splits, so for those windows the button id contains the **window
//!    handle** itself.
//!
//! Matching is therefore: parse the id, compare AUMIDs exactly (never `contains`), and
//! fall back to heuristics only when the button exposes no usable id. A window belongs
//! to at most one slot; a slot holds several windows only when the shell itself groups
//! them (same AUMID).

use tracing::debug;
use windows::Win32::Foundation::{HWND, RECT};

use super::explorer_process;
use super::types::TaskbarButton;
use super::utils;
use crate::shared::text::truncate;
use crate::window::LogicalWindow;

/// Prefix the shell puts in front of the AUMID in `automation_id`.
const AUMID_PREFIX: &str = "appid:";

/// Prefix written by WinGlide's uncombine feature; the rest is the window handle.
const UNCOMBINE_PREFIX: &str = "taskbarswitcher_";

/// All taskbar slots for one monitor, plus the windows they point into.
pub(crate) struct SlotPlan {
    /// Logical windows the slots can target; `Slot::targets` indexes into this list.
    pub windows: Vec<LogicalWindow>,
    pub slots: Vec<Slot>,
}

/// One taskbar button and the windows it represents.
pub(crate) struct Slot {
    /// Position on the taskbar, left to right.
    pub index: usize,
    /// Button label, for logs only.
    pub name: String,
    /// Button rectangle, used by the click fallback.
    pub rect: RECT,
    /// Stable identity of the button (`hwnd:<n>` / `aumid:<id>` / `hint:<n>`).
    pub key: String,
    /// Indices into [`SlotPlan::windows`], sorted by primary handle for a stable order.
    pub targets: Vec<usize>,
}

impl SlotPlan {
    /// Builds the slot plan from the UIA buttons and the visible logical windows.
    pub(crate) fn build(buttons: &[TaskbarButton], windows: Vec<LogicalWindow>) -> Self {
        let explorer_pid = explorer_process::get_explorer_pid() as i32;
        let identities: Vec<Identity> = buttons.iter().map(parse_identity).collect();
        let mut claimed = vec![false; windows.len()];

        let mut slots: Vec<Slot> = buttons
            .iter()
            .enumerate()
            .map(|(index, button)| Slot {
                index,
                name: button.name.clone(),
                rect: button.rect,
                key: identity_key(&identities[index], index, &button.name),
                targets: Vec::new(),
            })
            .collect();

        // Pass 1: exact identity. A window claimed here can never be taken by another slot.
        for (slot_index, identity) in identities.iter().enumerate() {
            match identity {
                Identity::Hwnd(value) => {
                    let hwnd = HWND(*value as *mut _);
                    let Some(window_index) = windows.iter().position(|w| w.contains(hwnd)) else {
                        debug!("Slot {slot_index}: no live window for {hwnd:?} (stale button)");
                        continue;
                    };

                    let expected = format!("{UNCOMBINE_PREFIX}{value}");
                    if !windows[window_index]
                        .aumids()
                        .any(|aumid| aumid.eq_ignore_ascii_case(&expected))
                    {
                        debug!(
                            "Slot {slot_index}: {hwnd:?} does not read back AUMID '{expected}'"
                        );
                    }

                    assign(&mut slots, &mut claimed, slot_index, window_index);
                }
                Identity::Aumid(aumid) => {
                    let mut matches = 0;
                    for (window_index, window) in windows.iter().enumerate() {
                        if window
                            .aumids()
                            .any(|candidate| candidate.eq_ignore_ascii_case(aumid))
                        {
                            assign(&mut slots, &mut claimed, slot_index, window_index);
                            matches += 1;
                        }
                    }
                    if matches == 0 {
                        debug!("Slot {slot_index}: no window with AUMID '{aumid}'");
                    }
                }
                Identity::Hint => {}
            }
        }

        // Pass 2: buttons without a usable identity fall back to the old heuristics, but
        // only over windows no exact slot claimed.
        for slot_index in 0..slots.len() {
            if !slots[slot_index].targets.is_empty() {
                continue;
            }
            let matches = hint_matches(&buttons[slot_index], &windows, &claimed, explorer_pid);
            for window_index in matches {
                assign(&mut slots, &mut claimed, slot_index, window_index);
            }
        }

        // Stable order inside a group: ascending primary handle. Rebuilding the plan on
        // every press must not reorder a group, otherwise repeated presses could bounce
        // between two windows of the same group forever.
        for slot in &mut slots {
            slot.targets
                .sort_by_key(|&index| windows[index].primary.0 as isize);
        }

        Self { windows, slots }
    }

    /// Compact one-line description used in verbose logs.
    pub(crate) fn summary(&self) -> String {
        self.slots
            .iter()
            .filter(|slot| !slot.targets.is_empty())
            .map(|slot| {
                let targets = slot
                    .targets
                    .iter()
                    .map(|&index| format!("{:?}", self.windows[index].primary))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "[{}/{}] {} -> {}",
                    slot.index,
                    slot.key,
                    truncate(&slot.name, 28),
                    targets
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

/// Records `window_index` as a target of `slot_index` unless another slot already has it.
fn assign(slots: &mut [Slot], claimed: &mut [bool], slot_index: usize, window_index: usize) {
    if claimed[window_index] {
        return;
    }
    claimed[window_index] = true;
    slots[slot_index].targets.push(window_index);
}

/// The identity encoded in a button's `automation_id`.
#[derive(Debug)]
enum Identity {
    /// `TaskbarSwitcher_<hwnd>`: exact window handle.
    Hwnd(isize),
    /// Any other AUMID, lowercased.
    Aumid(String),
    /// No usable automation id; use heuristics.
    Hint,
}

fn parse_identity(button: &TaskbarButton) -> Identity {
    let Some(raw) = button.automation_id.as_deref() else {
        return Identity::Hint;
    };

    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    let aumid = lower
        .strip_prefix(AUMID_PREFIX)
        .unwrap_or(&lower)
        .trim();

    if aumid.is_empty() {
        return Identity::Hint;
    }

    if let Some(value) = aumid.strip_prefix(UNCOMBINE_PREFIX) {
        if let Ok(hwnd) = value.parse::<isize>() {
            return Identity::Hwnd(hwnd);
        }
    }

    Identity::Aumid(aumid.to_string())
}

fn identity_key(identity: &Identity, index: usize, name: &str) -> String {
    match identity {
        Identity::Hwnd(value) => format!("hwnd:{value}"),
        Identity::Aumid(aumid) => format!("aumid:{aumid}"),
        Identity::Hint => format!("hint:{index}:{name}"),
    }
}

/// Fallback matching for buttons whose automation id is empty or unusable.
///
/// Strategies run strongest first and never look at windows an exact slot claimed.
fn hint_matches(
    button: &TaskbarButton,
    windows: &[LogicalWindow],
    claimed: &[bool],
    explorer_pid: i32,
) -> Vec<usize> {
    let available = |index: usize| !claimed[index];

    // 1. Process id, unless the button belongs to explorer itself.
    if button.process_id != 0 && button.process_id != explorer_pid {
        let matches = collect(
            windows,
            |index, window| {
                available(index)
                    && window
                        .process_ids()
                        .any(|pid| pid == button.process_id as u32)
            },
        );
        if !matches.is_empty() {
            return rank(matches, windows, "");
        }
    }

    let label = utils::normalize_button_label(&button.name);
    if label.is_empty() {
        return Vec::new();
    }

    // 2. Window title (fuzzy, but only over unclaimed windows).
    let matches = collect(windows, |index, window| {
        available(index) && !window.title.is_empty() && title_matches(&label, &window.title)
    });
    if !matches.is_empty() {
        return rank(matches, windows, &label);
    }

    // 3. Process name (allows windows with an empty title).
    let label_lower = label.to_lowercase();
    let matches = collect(windows, |index, window| {
        let process = window.process_name();
        if !available(index) || process.is_empty() {
            return false;
        }
        let stem = process.strip_suffix(".exe").unwrap_or(process);
        stem.contains(&label_lower) || label_lower.contains(stem)
    });

    rank(matches, windows, &label)
}

fn collect(
    windows: &[LogicalWindow],
    predicate: impl Fn(usize, &LogicalWindow) -> bool,
) -> Vec<usize> {
    windows
        .iter()
        .enumerate()
        .filter(|(index, window)| predicate(*index, window))
        .map(|(index, _)| index)
        .collect()
}

/// Sorts matches so an exact title wins, then by stable handle order.
fn rank(mut matches: Vec<usize>, windows: &[LogicalWindow], label: &str) -> Vec<usize> {
    matches.sort_by_key(|&index| {
        let exact = !label.is_empty() && windows[index].title.eq_ignore_ascii_case(label);
        (!exact, windows[index].primary.0 as isize)
    });
    matches
}

fn title_matches(label: &str, title: &str) -> bool {
    let cleaned = utils::normalize_button_label(title);
    if cleaned.is_empty() {
        return false;
    }
    let label = label.to_lowercase();
    let cleaned = cleaned.to_lowercase();
    cleaned == label || cleaned.contains(&label) || label.contains(&cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(automation_id: Option<&str>) -> TaskbarButton {
        TaskbarButton {
            name: "App".to_string(),
            rect: RECT::default(),
            process_id: 0,
            automation_id: automation_id.map(str::to_string),
        }
    }

    #[test]
    fn parses_the_uncombine_identity_as_an_exact_handle() {
        match parse_identity(&button(Some("Appid: TaskbarSwitcher_2557428"))) {
            Identity::Hwnd(value) => assert_eq!(value, 2_557_428),
            other => panic!("unexpected identity: {other:?}"),
        }
    }

    #[test]
    fn parses_a_plain_aumid_case_insensitively() {
        match parse_identity(&button(Some("Appid: MSEdge"))) {
            Identity::Aumid(aumid) => assert_eq!(aumid, "msedge"),
            other => panic!("unexpected identity: {other:?}"),
        }
    }

    #[test]
    fn tolerates_a_missing_space_after_the_prefix() {
        match parse_identity(&button(Some("Appid:TaskbarSwitcher_42"))) {
            Identity::Hwnd(value) => assert_eq!(value, 42),
            other => panic!("unexpected identity: {other:?}"),
        }
    }

    #[test]
    fn missing_or_empty_ids_fall_back_to_hints() {
        assert!(matches!(parse_identity(&button(None)), Identity::Hint));
        assert!(matches!(parse_identity(&button(Some(""))), Identity::Hint));
        assert!(matches!(
            parse_identity(&button(Some("Appid: "))),
            Identity::Hint
        ));
    }
}
