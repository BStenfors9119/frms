//! Unix (Linux/macOS) implementations of the platform seams.
//!
//! No `cfg` appears in this subtree — it is compiled only when the `cfg(unix)`
//! gate in [`port`](crate::port) selects it, so every file here is plain
//! single-platform code.

mod clipboard;
mod dirs;
mod fs;
mod shell;
mod window;

/// Zero-sized handle that carries this platform's trait impls (see
/// [`crate::port::api`]).
pub struct Sys;
