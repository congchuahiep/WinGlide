//! Event hooks that feed the app's message loop.
//!
//! | File | Responsibility |
//! | --- | --- |
//! | `winevent` | `EVENT_OBJECT_SHOW` hook -> uncombine newly created windows. |
//! | `uia` | UIA `StructureChanged` hook -> invalidate the button cache. |

mod uia;
mod winevent;

pub use uia::*;
pub use winevent::*;
