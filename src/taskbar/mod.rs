//! Taskbar system - the core feature of WinGlide.
//!
//! Manages the taskbar lifecycle: enumerating buttons via UI Automation, resolving each
//! button to its window(s), cycling between them, and uncombining buttons.
//!
//! | File | Responsibility |
//! | --- | --- |
//! | `snapshot` | [`Taskbar`]/[`TaskbarEdge`]: edge, thickness, DPI, labels mode. |
//! | `enumerator` | [`TaskbarEnumerator`]: the UIA button list + per-monitor TTL cache. |
//! | `slot` | Slot model: exact button identity -> windows. |
//! | `cycle_state` | Cursor, candidate ordering and anti-stuck bookkeeping. |
//! | `uncombine` | [`UncombineManager`]: unique AUMID per window. |
//! | `explorer_process` | Explorer's PID (buttons owned by the shell). |
//! | `types` | [`TaskbarButton`]. |
//! | `utils` | Button-name cleanup helpers. |

mod cycle_state;
mod enumerator;
mod explorer_process;
mod slot;
mod snapshot;
mod types;
mod uncombine;
mod utils;

pub use enumerator::{CycleDirection, TaskbarEnumerator};
pub use snapshot::{Taskbar, TaskbarEdge};
pub use types::TaskbarButton;
pub use uncombine::UncombineManager;
