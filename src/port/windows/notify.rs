//! Desktop notifications on Windows.
//!
//! Deferred, like the other best-effort Windows seams (see `../windows/mod.rs`
//! and the `net`/`proc` notes): a native toast needs either the WinRT
//! `Windows.UI.Notifications` surface or a bundled AppUserModelID, neither of
//! which the port layer carries yet. Until that lands this is a no-op, which is
//! a valid `Notify` impl — the trait is explicitly best-effort, and the caller
//! never depends on a notification actually appearing.
//
// TODO(windows): raise a real toast (PowerShell `Windows.UI.Notifications`
// template, or a `NotifyIcon` balloon as a fallback) once an AppUserModelID is
// registered for frms.

use super::Sys;
use crate::port::api::Notify;

impl Notify for Sys {
    fn send(_summary: &str, _body: &str) {
        // Intentionally does nothing yet — see module docs.
    }
}
