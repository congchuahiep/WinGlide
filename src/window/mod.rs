//! Win32 *window* interop: finding, describing and activating desktop windows.

pub mod activate;
pub mod aumid;
pub mod context;
pub mod enumerate;
pub mod system_class;
mod types;

pub use types::WindowInfo;
