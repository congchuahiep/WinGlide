//! WinGlide's application core: state, message loop and process lifecycle.
//!
//! * [`orchestrator`]    - `App`, the message loop and its message handlers.
//! * [`messages`]        - the custom window messages used by the loop and IPC.
//! * [`single_instance`] - named-mutex guard against duplicate instances.
//! * [`dpi_awareness`]   - per-monitor DPI awareness setup.

mod dpi_awareness;
pub mod messages;
mod orchestrator;
mod single_instance;

pub use dpi_awareness::setup_dpi_awareness;
pub use orchestrator::App;
pub use single_instance::{ensure_single_instance, InstanceType};
