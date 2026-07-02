//! Windows implementations of the platform seams.
//!
//! No `cfg` appears in this subtree — it is compiled only when the
//! `cfg(windows)` gate in [`port`](crate::port) selects it, so every file here
//! is plain single-platform code. Targeted at desktop Windows (incl. IoT
//! Enterprise / LTSC), not IoT Core.

mod clipboard;
mod dirs;
mod fs;
mod shell;
mod window;

/// Zero-sized handle that carries this platform's trait impls (see
/// [`crate::port::api`]).
pub struct Sys;
