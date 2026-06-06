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
    #[default]
    Off,
    ThirtySec,
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
    let seven_day_resets = body["seven_day"]["resets_at"].as_str().map(format_reset);

    Some(ClaudeStats { five_hour_pct, seven_day_pct, five_hour_resets, seven_day_resets })
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
