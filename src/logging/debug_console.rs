//! Attaches or allocates a Win32 console for debug output.
//!
//! GUI-subsystem applications have no console by default, so `--debug` calls
//! [`attach_debug_console`] to borrow the parent's console (e.g. the terminal
//! that launched the app) or allocate a fresh one.

use std::sync::atomic::Ordering;
use windows::Win32::System::Console::{AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS};

use super::console::DEBUG_CLI_MODE;

/// Attaches to the parent process' console, allocating a new one when that fails.
pub fn attach_debug_console() {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
    DEBUG_CLI_MODE.store(true, Ordering::SeqCst);
}
