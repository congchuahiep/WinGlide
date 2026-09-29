//! Configures the application's structured logging system.
//!
//! This module sets up `tracing_subscriber` with custom formatters to output logs
//! to both a rolling file and a detached console window via Named Pipes.
//!
//! | File | Responsibility |
//! | --- | --- |
//! | `logger` | [`setup_logger`]: wires the file + console layers. |
//! | `formatter` | Forest-style console formatting ([`CleanFormatter`]). |
//! | `console` | The detached console window over a named pipe. |
//! | `debug_console` | Attaches/allocates a console for `--debug`. |

pub mod console;
pub mod debug_console;
mod formatter;
mod logger;

pub use formatter::*;
pub use logger::*;
