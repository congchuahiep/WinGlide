//! Small helpers over the winvd virtual-desktop API.

/// Number of virtual desktops (at least 1).
pub(super) fn count() -> usize {
    winvd::get_desktop_count().unwrap_or(1) as usize
}

/// Index of the currently active desktop (0 when it can't be determined).
pub(super) fn current_index() -> usize {
    let current = winvd::get_current_desktop().ok();
    let desktops = winvd::get_desktops().unwrap_or_default();
    match current {
        Some(c) => desktops.iter().position(|d| *d == c).unwrap_or(0),
        None => 0,
    }
}
