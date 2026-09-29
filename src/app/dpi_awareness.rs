//! Per-monitor DPI awareness setup.

/// Configures the process to be Per-Monitor DPI Aware V2.
///
/// This ensures the application scales correctly on modern high-DPI displays
/// and dynamically responds to DPI changes without blurring.
pub fn setup_dpi_awareness() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
}
