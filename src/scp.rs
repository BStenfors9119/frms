//! Copy local files to a TSR receiver over SSH.
//!
//! Mirrors the project's "shell out instead of linking a library" approach
//! (see `clipboard.rs` and the `curl` networking in `chat_api.rs`). The actual
//! transport command is platform-specific and built by `port::transfer`:
//! `sshpass + scp` on unix (password in the environment, out of the argv),
//! PuTTY's `pscp -pw` on Windows. Files land in the receiver's
//! `/home/<username>/` directory.

use std::path::PathBuf;
use std::process::Stdio;

use tokio::io::AsyncReadExt;

/// Copy `files` to `/home/<user>/` on `host` over SSH, authenticating as
/// `user` with `password` on `port`. Returns a one-line success summary or an
/// error message suitable for display.
pub async fn copy(
    host:     String,
    user:     String,
    password: String,
    port:     u16,
    files:    Vec<PathBuf>,
) -> Result<String, String> {
    if files.is_empty() {
        return Err("no file selected to copy".to_string());
    }

    // Build the platform-appropriate copy command: `sshpass + scp` on unix,
    // PuTTY's `pscp -pw` on Windows (or a clear error if neither is available).
    // Files land in the receiver's `/home/<user>/` — always a Linux path,
    // since the receiver is a TSR Linux device regardless of the local OS.
    let dest = format!("{user}@{host}:/home/{user}/");
    let spec = crate::port::transfer::scp_command(&host, &user, &password, port, &files, &dest)?;

    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args);
    for (k, v) in &spec.envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", spec.program))?;

    let status = child.wait().await.map_err(|e| format!("scp wait failed: {e}"))?;
    if status.success() {
        let names: Vec<String> = files.iter()
            .map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.to_string_lossy().into_owned()))
            .collect();
        Ok(format!("Copied {} to {dest}", names.join(", ")))
    } else {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr).await;
        }
        let detail = stderr.trim();
        let detail = if detail.is_empty() { "scp failed" } else { detail };
        Err(detail.to_string())
    }
}
