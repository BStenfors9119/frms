//! Clipboard I/O that bypasses iced's built-in `smithay-clipboard` backend.
//!
//! On this GNOME/mutter Wayland session the compositor delivers clipboard
//! *content* immediately but never EOFs the data-transfer fd. iced reads the
//! clipboard synchronously, to EOF, on its event-loop thread — so a stalled
//! read freezes the entire app (the right-click-to-paste lockup). We sidestep
//! the whole problem by shelling out to `wl-copy`/`wl-paste` (with `xclip`/
//! `xsel` X11 fallbacks) inside an async `Task`, off the event loop, reading
//! stdout with a deadline rather than waiting for an EOF that may never arrive.
//!
//! This mirrors the project's existing "shell out instead of fighting a
//! library" approach (see networking via `curl`).

use super::Sys;
use crate::port::api::Clipboard;
use std::process::Stdio;
use std::time::Duration;

/// How long to wait for clipboard *content* after launching the reader. The
/// data arrives effectively instantly; the wait only exists because the
/// compositor may leave the transfer fd open forever instead of sending EOF.
const READ_DEADLINE: Duration = Duration::from_millis(400);

impl Clipboard for Sys {
    /// Read the system clipboard, returning `None` when it is empty or no reader
    /// binary is available. Never blocks longer than [`READ_DEADLINE`].
    async fn read() -> Option<String> {
        if let Some(text) = read_with("wl-paste", &["-n"]).await {
            return Some(text);
        }
        if let Some(text) = read_with("xclip", &["-selection", "clipboard", "-o"]).await {
            return Some(text);
        }
        read_with("xsel", &["--clipboard", "--output"]).await
    }

    /// Write `text` to the system clipboard. A Wayland clipboard source must stay
    /// alive to serve future pastes, so `wl-copy` daemonizes; we detach a reaper
    /// so the eventual exit doesn't leave a zombie.
    async fn write(text: String) {
        if write_with("wl-copy", &[], &text).await {
            return;
        }
        if write_with("xclip", &["-selection", "clipboard"], &text).await {
            return;
        }
        let _ = write_with("xsel", &["--clipboard", "--input"], &text).await;
    }
}

/// Spawn `prog args…`, collecting whatever it writes to stdout within
/// [`READ_DEADLINE`], then kill it. Returns `None` if the binary is missing or
/// produced no output (e.g. an empty clipboard).
async fn read_with(prog: &str, args: &[&str]) -> Option<String> {
    use tokio::io::AsyncReadExt;

    let mut child = tokio::process::Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let mut buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        // `read_to_end` blocks until EOF; on this compositor EOF may never
        // come, so cap the wait. On timeout the future is dropped but `buf`
        // keeps the bytes already read into it.
        let _ = tokio::time::timeout(READ_DEADLINE, out.read_to_end(&mut buf)).await;
    }
    let _ = child.start_kill();

    if buf.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

/// Pipe `text` into `prog args…` via stdin. Returns `true` if the binary was
/// found and fed; `false` if it could not be spawned (try the next fallback).
async fn write_with(prog: &str, args: &[&str], text: &str) -> bool {
    use tokio::io::AsyncWriteExt;

    let child = tokio::process::Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return false,
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes()).await;
        let _ = stdin.shutdown().await; // let the writer proceed / daemonize
    }

    // The source process lingers (it owns the selection); reap it in the
    // background so we neither block here nor leak a zombie on its exit.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    true
}
