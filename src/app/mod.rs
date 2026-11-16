//! WinGlide's application core: state, message loop and process lifecycle.
//!
//! * [`orchestrator`]    - `App`, the message loop and its message handlers.
//! * [`autostart`]       - start with Windows, via `StartupTask` or the Run key.
//! * [`update_checker`]  - GitHub release check; MSI/ZIP builds only.
//! * [`messages`]        - the custom window messages used by the loop and IPC.
//! * [`single_instance`] - named-mutex guard against duplicate instances.
//! * [`dpi_awareness`]   - per-monitor DPI awareness setup.

pub mod autostart;
mod dpi_awareness;
pub mod messages;
mod orchestrator;
mod single_instance;
#[cfg(feature = "updater")]
pub mod update_checker;

pub use dpi_awareness::setup_dpi_awareness;
pub use orchestrator::App;
pub use single_instance::{ensure_single_instance, InstanceType};
