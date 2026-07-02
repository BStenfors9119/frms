//! Clipboard access for the app. The real, platform-specific implementations
//! live in the port layer (`port::unix::clipboard` / `port::windows::clipboard`);
//! this module just re-exports the neutral entry points so existing callers keep
//! using `crate::clipboard::{read, write}`.

pub use crate::port::clipboard::{read, write};
