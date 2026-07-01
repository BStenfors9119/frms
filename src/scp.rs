//! Copy local files to a TSR receiver with `sshpass` + `scp`.
//!
//! Mirrors the project's "shell out instead of linking a library" approach
//! (see `clipboard.rs` and the `curl` networking in `chat_api.rs`). The
//! receiver's password is handed to `sshpass` through the `SSHPASS`
//! environment variable rather than the argument list (which is world-readable
//! via `/proc`). Files land in the receiver's `/home/<username>/` directory.

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

    // `scp -P <port> -o accept-new <files…> user@host:/home/user/`
    let dest = format!("{user}@{host}:/home/{user}/");
    let mut cmd = tokio::process::Command::new("sshpass");
    cmd.arg("-e") // read password from $SSHPASS
        .arg("scp")
        .arg("-P").arg(port.to_string())
        .arg("-o").arg("StrictHostKeyChecking=accept-new");
    for f in &files {
        cmd.arg(f);
    }
    cmd.arg(&dest)
        .env("SSHPASS", &password)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| format!("could not run sshpass/scp: {e}"))?;

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
