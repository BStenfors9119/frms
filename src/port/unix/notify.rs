//! Desktop notifications via libnotify's `notify-send`.
//!
//! Mirrors the project's "shell out instead of linking a library" approach (see
//! `clipboard.rs` and networking via `curl`): one `notify-send` invocation per
//! notification, so frms needs no D-Bus crate. It is deliberately best-effort —
//! if `notify-send` isn't installed, or no notification daemon is running, the
//! command simply fails and nothing happens.
//!
//! The command is run on a detached thread rather than on the event loop: the
//! thread owns the child and waits on it, so the brief D-Bus round-trip never
//! stalls the UI and no zombie is left behind. Notifications are rare enough
//! (one when an agent stops while frms is in the background) that a short-lived
//! thread per notification is cheap.

use super::Sys;
use crate::port::api::Notify;
use std::process::Stdio;

impl Notify for Sys {
    fn send(summary: &str, body: &str) {
        let summary = summary.to_owned();
        let body = body.to_owned();
        std::thread::spawn(move || {
            // `--app-name`/`--icon=frms` tie the toast to frms' installed
            // `.desktop` icon where the daemon honours it; both are ignored
            // gracefully when it doesn't.
            let _ = std::process::Command::new("notify-send")
                .args(["--app-name=frms", "--icon=frms", &summary, &body])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
    }
}
