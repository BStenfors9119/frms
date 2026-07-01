/// Fetches live Claude usage from the Anthropic OAuth usage API.
///
/// Credentials are read from `~/.claude/.credentials.json` (written by
/// Claude Code). The API returns utilization as 0-100 percentages for
/// the current 5-hour session window and the current 7-day weekly window.

#[derive(Debug, Clone, Default)]
pub struct ClaudeStats {
    /// Current 5-hour session usage (0–100 %).
    pub five_hour_pct:  f32,
    /// Current 7-day weekly usage (0–100 %).
    pub seven_day_pct:  f32,
    /// Human-readable reset time for the 5-hour window, if available.
    pub five_hour_resets: Option<String>,
    /// Human-readable reset time for the 7-day window, if available.
    pub seven_day_resets: Option<String>,
}

impl ClaudeStats {
    pub fn five_hour_fraction(&self) -> f32 { (self.five_hour_pct / 100.0).clamp(0.0, 1.0) }
    pub fn seven_day_fraction(&self) -> f32 { (self.seven_day_pct  / 100.0).clamp(0.0, 1.0) }
}

/// User-selectable auto-refresh cadence for the usage stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefreshInterval {
    Off,
    ThirtySec,
    #[default]
    OneMin,
    FiveMin,
}

impl RefreshInterval {
    pub const ALL: [RefreshInterval; 4] = [
        RefreshInterval::Off,
        RefreshInterval::ThirtySec,
        RefreshInterval::OneMin,
        RefreshInterval::FiveMin,
    ];

    pub fn as_duration(self) -> Option<std::time::Duration> {
        match self {
            RefreshInterval::Off       => None,
            RefreshInterval::ThirtySec => Some(std::time::Duration::from_secs(30)),
            RefreshInterval::OneMin    => Some(std::time::Duration::from_secs(60)),
            RefreshInterval::FiveMin   => Some(std::time::Duration::from_secs(300)),
        }
    }
}

impl std::fmt::Display for RefreshInterval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RefreshInterval::Off       => "Off",
            RefreshInterval::ThirtySec => "30s",
            RefreshInterval::OneMin    => "1m",
            RefreshInterval::FiveMin   => "5m",
        };
        f.write_str(s)
    }
}

/// Async: fetch usage via `curl` using the stored OAuth token.
/// Returns `None` on failure (missing credentials, network blip, non-2xx) so
/// callers can preserve the previously-known stats instead of clobbering them
/// with zeros on every transient hiccup.
pub async fn load() -> Option<ClaudeStats> {
    tokio::task::spawn_blocking(load_sync).await.ok().flatten()
}

pub fn load_sync() -> Option<ClaudeStats> {
    read_usage()
}

fn read_usage() -> Option<ClaudeStats> {
    let home  = std::env::var("HOME").ok()?;
    let creds = std::fs::read(format!("{home}/.claude/.credentials.json")).ok()?;
    let creds: serde_json::Value = serde_json::from_slice(&creds).ok()?;
    let token = creds["claudeAiOauth"]["accessToken"].as_str()?.to_string();

    // Call the API via curl (avoids needing a Rust HTTPS client dependency)
    let output = std::process::Command::new("curl")
        .args([
            "-sf",                                       // silent + fail on HTTP error
            "-H", &format!("Authorization: Bearer {token}"),
            "-H", "anthropic-beta: oauth-2025-04-20",
            "-H", "Content-Type: application/json",
            "--max-time", "8",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .output()
        .ok()?;

    if !output.status.success() { return None; }

    let body: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    let five_hour_pct    = body["five_hour"]["utilization"].as_f64().unwrap_or(0.0) as f32;
    let seven_day_pct    = body["seven_day"]["utilization"].as_f64().unwrap_or(0.0) as f32;
    let five_hour_resets = body["five_hour"]["resets_at"].as_str().map(format_reset);
    let seven_day_resets = body["seven_day"]["resets_at"].as_str().map(format_reset_dated);

    Some(ClaudeStats { five_hour_pct, seven_day_pct, five_hour_resets, seven_day_resets })
}

// ── Claude process tracker ──────────────────────────────────────────────────
//
// A live snapshot of every `claude` process running on the machine (not just
// the panes this IDE spawned), plus the system RAM total so the UI can show
// how much of the box Claude is eating.

#[derive(Debug, Clone)]
pub struct ClaudeProc {
    pub rss_kb:  u64,
    pub cpu_pct: f32,
}

#[derive(Debug, Clone, Default)]
pub struct ProcStats {
    pub procs:        Vec<ClaudeProc>,
    /// Total physical RAM on the system, in KiB (0 if unknown).
    pub sys_total_kb: u64,
}

impl ProcStats {
    pub fn count(&self) -> usize { self.procs.len() }
    pub fn total_rss_kb(&self) -> u64 { self.procs.iter().map(|p| p.rss_kb).sum() }
    pub fn total_cpu_pct(&self) -> f32 { self.procs.iter().map(|p| p.cpu_pct).sum() }

    /// Fraction of system RAM held by all Claude processes (0–1).
    pub fn mem_fraction(&self) -> f32 {
        if self.sys_total_kb == 0 { return 0.0; }
        (self.total_rss_kb() as f32 / self.sys_total_kb as f32).clamp(0.0, 1.0)
    }
}

/// Async: scan the process table for `claude` processes off the UI thread.
pub async fn load_procs() -> ProcStats {
    tokio::task::spawn_blocking(read_procs).await.unwrap_or_default()
}

fn read_procs() -> ProcStats {
    let self_pid     = std::process::id();
    let sys_total_kb = mem_total_kb();
    let mut procs    = Vec::new();

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
                else { continue };

                let pid: u32 = match pid.parse() { Ok(v) => v, Err(_) => continue };
                if pid == self_pid { continue; }

                // Match the process binary (`claude`, `/usr/bin/claude`,
                // `node …/claude.js`) by basename, not bare substring — keeps
                // `.claude/` config-path references from counting as processes.
                let is_claude = comm.starts_with("claude")
                    || it.clone().any(is_claude_exec);
                if !is_claude { continue; }

                procs.push(ClaudeProc {
                    rss_kb:  rss.parse().unwrap_or(0),
                    cpu_pct: cpu.parse().unwrap_or(0.0),
                });
            }
        }
    }

    ProcStats { procs, sys_total_kb }
}

/// True if a command-line token's file name looks like the Claude executable.
fn is_claude_exec(token: &str) -> bool {
    let name = token.rsplit('/').next().unwrap_or(token);
    name.starts_with("claude")
}

/// Total physical RAM from `/proc/meminfo`, in KiB (0 if unavailable).
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

/// Trim an ISO 8601 timestamp down to just the time portion ("08:00 UTC").
fn format_reset(ts: &str) -> String {
    // "2026-04-10T08:00:00.200835+00:00"  →  "08:00 UTC"
    ts.find('T')
        .map(|i| {
            let time = &ts[i + 1..];
            let hm   = time.get(..5).unwrap_or(time);
            format!("{hm} UTC")
        })
        .unwrap_or_else(|| ts.to_string())
}

const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Short weekday name for a Gregorian date via Zeller's congruence — avoids
/// pulling in a date crate (the project keeps its dependency set lean and
/// C-free). `h` is 0=Sat … 6=Fri.
fn weekday_name(year: i64, month: u32, day: u32) -> &'static str {
    // Zeller counts Jan/Feb as months 13/14 of the previous year.
    let (m, y) = if month < 3 { (month as i64 + 12, year - 1) } else { (month as i64, year) };
    let (k, j) = (y % 100, y / 100);
    let h = (day as i64 + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    ["Sat", "Sun", "Mon", "Tue", "Wed", "Thu", "Fri"][h as usize]
}

/// Like [`format_reset`] but prefixed with the weekday and calendar date —
/// "Fri Apr 10, 08:00 UTC". Used for the weekly bar, whose reset is days out so
/// the date is what matters; the 5-hour bar stays time-only. Falls back to the
/// time-only form if the timestamp can't be parsed.
fn format_reset_dated(ts: &str) -> String {
    // "2026-04-10T08:00:00.200835+00:00"  →  "Fri Apr 10, 08:00 UTC"
    let dated = (|| {
        let t     = ts.find('T')?;
        let mut d = ts[..t].split('-');
        let year:  i64 = d.next()?.parse().ok()?;
        let month: u32 = d.next()?.parse().ok()?;
        let day:   u32 = d.next()?.parse().ok()?;
        let hm   = ts[t + 1..].get(..5)?;
        let mon  = MONTHS.get((month as usize).checked_sub(1)?)?;
        Some(format!("{} {} {}, {} UTC", weekday_name(year, month, day), mon, day, hm))
    })();
    dated.unwrap_or_else(|| format_reset(ts))
}

#[cfg(test)]
mod reset_tests {
    use super::*;
    #[test]
    fn weekday_known_dates() {
        assert_eq!(weekday_name(2000, 1, 1), "Sat");   // Y2K was a Saturday
        assert_eq!(weekday_name(2026, 4, 10), "Fri");
        assert_eq!(weekday_name(2026, 6, 25), "Thu");  // today
        assert_eq!(weekday_name(2024, 2, 29), "Thu");  // leap day
    }
    #[test]
    fn dated_format_and_fallback() {
        assert_eq!(format_reset_dated("2026-04-10T08:00:00.2+00:00"), "Fri Apr 10, 08:00 UTC");
        assert_eq!(format_reset_dated("garbage"), "garbage"); // falls back
    }
}
