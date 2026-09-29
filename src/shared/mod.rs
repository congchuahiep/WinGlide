//! Small helpers shared by more than one feature.
//!
//! Everything here is used by at least two features, so it has no natural home
//! inside a single one. Each file covers exactly one concern:
//!
//! * `elevation`    - Administrator-privilege checks and elevated restart.
//! * `system_theme` - Is Windows currently using the light theme?
//! * `text`         - String helpers.

pub mod elevation;
pub mod system_theme;
pub mod text;
