//! Best-effort process/memory stats on Windows.
//!
//! Claude runs as `node.exe` executing the Claude Code CLI script, so we can't
//! match on image name alone — we query the full command line via CIM. CPU% is
//! not sampled (it needs two snapshots or a perf counter); it's reported as
//! `0.0`, so the stats panel shows process count + RAM but not CPU on Windows.

use super::Sys;
use crate::port::api::{Proc, ProcSample};

/// Run a PowerShell snippet and return stdout, or `None` on any failure.
fn powershell(script: &str) -> Option<String> {
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Proc for Sys {
    fn claude_procs() -> Vec<ProcSample> {
        let self_pid = std::process::id();

        // Emit "pid workingSetBytes" for every process whose command line
        // mentions claude. WorkingSetSize is bytes; caller wants KiB.
        let script = "Get-CimInstance Win32_Process | \
                      Where-Object { $_.CommandLine -match 'claude' } | \
                      ForEach-Object { \"$($_.ProcessId) $($_.WorkingSetSize)\" }";

        let Some(text) = powershell(script) else {
            return Vec::new();
        };

        let mut procs = Vec::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let (Some(pid), Some(rss_bytes)) = (it.next(), it.next()) else {
                continue;
            };
            let pid: u32 = match pid.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            if pid == self_pid {
                continue;
            }
            let rss_bytes: u64 = rss_bytes.parse().unwrap_or(0);
            procs.push(ProcSample {
                rss_kb:  rss_bytes / 1024,
                cpu_pct: 0.0, // not sampled on Windows
            });
        }
        procs
    }

    fn mem_total_kb() -> u64 {
        // TotalVisibleMemorySize is already reported in KiB.
        powershell("(Get-CimInstance Win32_OperatingSystem).TotalVisibleMemorySize")
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }
}
