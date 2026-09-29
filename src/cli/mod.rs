//! Command-line argument parsing.
//!
//! WinGlide parses its arguments manually (no clap); see [`args`] for the
//! accepted flags.

mod args;

pub use args::{parse_args, print_help, RunMode};
