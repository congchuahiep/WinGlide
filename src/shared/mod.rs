//! Small helpers shared by more than one feature.
//!
//! Everything here is used by at least two features, so it has no natural home
//! inside a single one. Each file covers exactly one concern:
//!
//! * `integrity`     - Is a process (or the process owning a window) elevated?
//! * `package`       - Is this process running inside an MSIX package?
//! * `system_theme`  - Is Windows currently using the light theme?
//! * `text`          - String helpers.

pub mod integrity;
pub mod package;
pub mod system_theme;
pub mod text;
