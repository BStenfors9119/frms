use super::Sys;
use crate::port::api::{Proc, ProcSample};

impl Proc for Sys {
    fn claude_procs() -> Vec<ProcSample> {
        let self_pid = std::process::id();
        let mut procs = Vec::new();

        // pid / rss(KiB) / %cpu / comm / full args — `=` suffixes suppress headers.
        let output = std::process::Command::new("ps")
            .args(["-eo", "pid=,rss=,pcpu=,comm=,args="])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    let mut it = line.split_whitespace();
                    let (Some(pid), Some(rss), Some(cpu), Some(comm)) =
                        (it.next(), it.next(), it.next(), it.next())
                    else {
                        continue;
                    };

                    let pid: u32 = match pid.parse() {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if pid == self_pid {
                        continue;
                    }

                    // Match the process binary (`claude`, `/usr/bin/claude`,
                    // `node …/claude.js`) by basename, not bare substring — keeps
                    // `.claude/` config-path references from counting as processes.
                    let is_claude = comm.starts_with("claude") || it.clone().any(is_claude_exec);
                    if !is_claude {
                        continue;
                    }

                    procs.push(ProcSample {
                        rss_kb:  rss.parse().unwrap_or(0),
                        cpu_pct: cpu.parse().unwrap_or(0.0),
                    });
                }
            }
        }

        procs
    }

    fn mem_total_kb() -> u64 {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0)
    }
}

/// True if a command-line token's file name looks like the Claude executable.
fn is_claude_exec(token: &str) -> bool {
    let name = token.rsplit('/').next().unwrap_or(token);
    name.starts_with("claude")
}
