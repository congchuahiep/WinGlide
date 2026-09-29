//! Taskbar system - the core feature of WinGlide.
//!
//! Manages the taskbar lifecycle: enumerating buttons via UI Automation, mapping
//! buttons to windows, cycling between them, and uncombining buttons.
//!
//! | File | Responsibility |
//! | --- | --- |
//! | `snapshot` | [`Taskbar`]/[`TaskbarEdge`]: edge, thickness, DPI, labels mode. |
//! | `enumerator` | [`TaskbarEnumerator`]: the UIA button list + 1s TTL cache. |
//! | `button_window` | [`ButtonWindowMap`]: matching buttons to real windows. |
//! | `uncombine` | [`UncombineManager`]: unique AUMID per window. |
//! | `explorer_process` | Explorer's PID (buttons owned by the shell). |
//! | `types` | [`TaskbarButton`], [`TargetWindow`]. |
//! | `utils` | Button-name cleanup helpers. |

mod button_window;
mod enumerator;
mod explorer_process;
mod snapshot;
mod types;
mod uncombine;
mod utils;

pub use enumerator::{CycleDirection, TaskbarEnumerator};
pub use snapshot::{Taskbar, TaskbarEdge};
pub use types::{TaskbarButton, TargetWindow};
pub use uncombine::UncombineManager;
