//! Windows clipboard I/O.
//!
//! Mirrors the unix impl's "shell out instead of linking a clipboard library"
//! approach (see `../unix/clipboard.rs`), keeping frms free of extra Win32
//! dependencies: `clip.exe` sets the clipboard from stdin, and PowerShell's
//! `Get-Clipboard` reads it back. Unlike the Wayland case there's no missing-EOF
//! hazard here — both tools EOF normally — but PowerShell has a slow cold start,
//! so reads use a generous safety timeout rather than the unix 400 ms.

use super::Sys;
use crate::port::api::Clipboard;
use std::process::Stdio;
use std::time::Duration;

/// Upper bound on a read. `Get-Clipboard` returns as soon as PowerShell starts
/// and prints, which can take several hundred ms cold; this only caps a hang.
const READ_DEADLINE: Duration = Duration::from_secs(5);

impl Clipboard for Sys {
    async fn read() -> Option<String> {
        use tokio::io::AsyncReadExt;

        let mut child = tokio::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", "Get-Clipboard -Raw"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .ok()?;

        let mut buf = Vec::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = tokio::time::timeout(READ_DEADLINE, out.read_to_end(&mut buf)).await;
        }
        let _ = child.start_kill();

        if buf.is_empty() {
            return None;
        }
        // `Get-Clipboard -Raw` appends a trailing CRLF; drop one so paste round-
        // trips don't accumulate blank lines.
        let mut text = String::from_utf8_lossy(&buf).into_owned();
        if text.ends_with("\r\n") {
            text.truncate(text.len() - 2);
        } else if text.ends_with('\n') {
            text.truncate(text.len() - 1);
        }
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    async fn write(text: String) {
        use tokio::io::AsyncWriteExt;

        // `clip.exe` reads stdin and puts it on the clipboard. It exits promptly
        // (no daemonization needed, unlike Wayland's wl-copy).
        let child = tokio::process::Command::new("clip")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(_) => return,
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
        let _ = child.wait().await;
    }
}
