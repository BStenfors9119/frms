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
    /// Raw reset instants (unix seconds) behind the strings above. Kept so the
    /// per-model breakdown can scan *the same* window the bar is measuring —
    /// the window runs from `resets_at - length` to `resets_at`, which is not
    /// the same as the last five hours / seven days from right now.
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_resets_at: Option<i64>,
}

impl ClaudeStats {
    pub fn five_hour_fraction(&self) -> f32 { (self.five_hour_pct / 100.0).clamp(0.0, 1.0) }
    pub fn seven_day_fraction(&self) -> f32 { (self.seven_day_pct  / 100.0).clamp(0.0, 1.0) }

    /// Whether these stats came from a real response. A successful fetch always
    /// carries at least one reset instant, so their absence means the default
    /// value — never fetched, or every fetch so far has failed. The header uses
    /// this to say "unavailable" instead of drawing two confident 0% bars.
    pub fn has_reading(&self) -> bool {
        self.five_hour_resets_at.is_some() || self.seven_day_resets_at.is_some()
    }

    /// Unix-second start of `window`, anchored on the API's reset instant when
    /// we have one and falling back to "`length` ago" when we don't (no
    /// credentials, offline, or a response without `resets_at`).
    pub fn window_start(&self, window: UsageWindow) -> i64 {
        let resets = match window {
            UsageWindow::FiveHour => self.five_hour_resets_at,
            UsageWindow::SevenDay => self.seven_day_resets_at,
        };
        // A reset instant that has already passed describes a window that is
        // over — it only survives in stats we failed to refresh. Scanning it
        // would report a span that ended, so fall back to the trailing one.
        match resets {
            Some(at) if at > now_epoch() => at - window.length_secs(),
            _                            => now_epoch() - window.length_secs(),
        }
    }

    /// Start of the span a breakdown should scan, per the reader's chosen
    /// [`UsageScope`].
    pub fn scan_start(&self, window: UsageWindow, scope: UsageScope) -> i64 {
        match scope {
            UsageScope::LimitWindow => self.window_start(window),
            UsageScope::Trailing    => now_epoch() - window.length_secs(),
        }
    }
}

/// Which span the per-model breakdown scans. The two differ by more than they
/// look: a weekly limit that resets Sunday 23:00 has a *current* window only a
/// couple of days old by Tuesday, so the limit-window scan legitimately covers
/// nearly the same span as the 5-hour card and omits models used earlier in the
/// calendar week. Both readings are useful, so the card offers both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageScope {
    /// Exactly what the bar measures: `resets_at - length` … `resets_at`.
    #[default]
    LimitWindow,
    /// A plain trailing span ending now — "what have I been running lately",
    /// regardless of where the limit window happens to have reset.
    Trailing,
}

impl UsageScope {
    pub fn label(self, window: UsageWindow) -> String {
        match self {
            UsageScope::LimitWindow => "Limit window".to_string(),
            UsageScope::Trailing    => format!("Last {}", window.span_label()),
        }
    }
}

/// Which of the two header bars a breakdown belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageWindow {
    FiveHour,
    SevenDay,
}

impl UsageWindow {
    pub fn label(self) -> &'static str {
        match self {
            UsageWindow::FiveHour => "Session (5h)",
            UsageWindow::SevenDay => "Week (7d)",
        }
    }

    pub fn length_secs(self) -> i64 {
        match self {
            UsageWindow::FiveHour => 5 * 60 * 60,
            UsageWindow::SevenDay => 7 * 24 * 60 * 60,
        }
    }

    /// The window's length as a noun phrase — "5 hours", "7 days".
    pub fn span_label(self) -> &'static str {
        match self {
            UsageWindow::FiveHour => "5 hours",
            UsageWindow::SevenDay => "7 days",
        }
    }
}

pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
    let creds_path = crate::port::dirs::home()?.join(".claude").join(".credentials.json");
    let creds = std::fs::read(creds_path).ok()?;
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
    let five_hour_resets_at = body["five_hour"]["resets_at"].as_str().and_then(iso_to_epoch);
    let seven_day_resets_at = body["seven_day"]["resets_at"].as_str().and_then(iso_to_epoch);

    Some(ClaudeStats {
        five_hour_pct, seven_day_pct,
        five_hour_resets, seven_day_resets,
        five_hour_resets_at, seven_day_resets_at,
    })
}

// ── per-model token breakdown ───────────────────────────────────────────────
//
// The header bars come from the API as a single utilization percentage each,
// with no model-level detail. The detail *is* on disk: Claude Code appends one
// JSON record per assistant reply to `~/.claude/projects/<project>/<id>.jsonl`,
// each carrying the model that answered, that reply's token counts, and a
// timestamp. Summing those over a window shows where the window actually went
// — see [`breakdown_sync`] for what that number can and can't tell you.

/// One model's token total inside a window.
#[derive(Debug, Clone, Default)]
pub struct ModelTokens {
    pub model:       String,
    pub input:       u64,
    pub output:      u64,
    pub cache_read:  u64,
    pub cache_write: u64,
    /// Number of assistant replies this model produced in the window.
    pub replies:     u64,
}

impl ModelTokens {
    /// Every token billed against the window, cache included — cache reads
    /// dominate agentic use, so leaving them out would misrepresent the split.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// Per-model totals for one usage window, plus how the scan went.
#[derive(Debug, Clone, Default)]
pub struct ModelBreakdown {
    /// Sorted by total tokens, descending.
    pub rows:         Vec<ModelTokens>,
    /// Transcript files that fell inside the window and were read.
    pub files:        usize,
    /// True when no transcripts were found at all — usually means Claude Code
    /// has never run as this user, so the UI can say that rather than show an
    /// empty table that reads like "you used nothing".
    pub no_transcripts: bool,
}

impl ModelBreakdown {
    pub fn total(&self) -> u64 { self.rows.iter().map(ModelTokens::total).sum() }
    pub fn replies(&self) -> u64 { self.rows.iter().map(|r| r.replies).sum() }
}

/// Async: aggregate `~/.claude/projects/**/*.jsonl` by model, off the UI thread.
pub async fn load_model_breakdown(since: i64) -> ModelBreakdown {
    tokio::task::spawn_blocking(move || breakdown_sync(since))
        .await
        .unwrap_or_default()
}

/// Walk every project transcript and total each model's tokens since `since`.
///
/// Caveats worth knowing before trusting a number here:
/// * This counts *local* Claude Code activity — every project on this machine,
///   not just frms panes, but nothing done on another machine or in the web app.
/// * Tokens are not the same quantity as the bar's utilization percentage. The
///   limit weighs models differently, so a model's token share is not its share
///   of the bar.
fn breakdown_sync(since: i64) -> ModelBreakdown {
    match crate::port::dirs::home().map(|h| h.join(".claude").join("projects")) {
        Some(root) => breakdown_under(&root, since),
        None       => ModelBreakdown { no_transcripts: true, ..Default::default() },
    }
}

/// [`breakdown_sync`] against an explicit transcript root — the seam that lets
/// the tests run on a fixture instead of the user's real `~/.claude`.
fn breakdown_under(root: &std::path::Path, since: i64) -> ModelBreakdown {
    let mut out = ModelBreakdown::default();

    let Ok(projects) = std::fs::read_dir(root) else {
        out.no_transcripts = true;
        return out;
    };

    let mut by_model: std::collections::HashMap<String, ModelTokens> = std::collections::HashMap::new();
    // A resumed or forked conversation can replay earlier records into a new
    // file; the per-record uuid is stable across those copies, so dedupe on it
    // rather than double-counting the same reply.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut any_transcript = false;

    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
            any_transcript = true;
            // A file last written before the window opened cannot hold a record
            // inside it — skip without opening. This is what keeps the scan
            // cheap on a machine with months of transcripts.
            let touched = entry.metadata().ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64);
            if matches!(touched, Some(t) if t < since) { continue; }

            let Ok(file) = std::fs::File::open(&path) else { continue };
            out.files += 1;
            let reader = std::io::BufReader::new(file);
            for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
                // Cheap prefilter: only assistant replies carry a usage block,
                // and they're a small slice of the lines in a transcript.
                if !line.contains("\"usage\"") { continue; }
                let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if rec["type"].as_str() != Some("assistant") { continue; }
                let Some(ts) = rec["timestamp"].as_str().and_then(iso_to_epoch) else { continue };
                if ts < since { continue; }
                if let Some(uuid) = rec["uuid"].as_str() {
                    if !seen.insert(uuid.to_string()) { continue; }
                }

                let message = &rec["message"];
                let model = message["model"].as_str().unwrap_or("unknown").to_string();
                let usage = &message["usage"];
                let n = |key: &str| usage[key].as_u64().unwrap_or(0);

                let row = by_model.entry(model.clone()).or_insert_with(|| ModelTokens {
                    model,
                    ..Default::default()
                });
                row.input       += n("input_tokens");
                row.output      += n("output_tokens");
                row.cache_read  += n("cache_read_input_tokens");
                row.cache_write += n("cache_creation_input_tokens");
                row.replies     += 1;
            }
        }
    }

    out.no_transcripts = !any_transcript;
    out.rows = by_model.into_values().collect();
    out.rows.sort_by(|a, b| b.total().cmp(&a.total()));
    out
}

/// Parse an ISO 8601 UTC timestamp to unix seconds. Both sources here are UTC
/// (`…Z` in transcripts, `+00:00` from the API), so the offset is ignored and
/// anything sub-second is dropped — a second's precision is far finer than the
/// windows being measured.
fn iso_to_epoch(ts: &str) -> Option<i64> {
    let (date, rest) = ts.split_once('T')?;
    let mut d = date.split('-');
    let year:  i64 = d.next()?.parse().ok()?;
    let month: u32 = d.next()?.parse().ok()?;
    let day:   u32 = d.next()?.parse().ok()?;

    let mut t = rest.get(..8)?.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let min:  i64 = t.next()?.parse().ok()?;
    let sec:  i64 = t.next()?.parse().ok()?;

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + min * 60 + sec)
}

/// Render a unix instant the same way the weekly bar renders its reset —
/// "Fri Apr 10, 08:00 UTC". Used for the scanned window's start.
pub fn format_epoch_dated(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let mon = MONTHS.get((month as usize).saturating_sub(1)).copied().unwrap_or("???");
    format!(
        "{} {} {}, {:02}:{:02} UTC",
        weekday_name(year, month, day), mon, day, secs / 3600, (secs % 3600) / 60,
    )
}

/// Days between 1970-01-01 and the given Gregorian date (Howard Hinnant's
/// `days_from_civil`). Same reason as [`weekday_name`]: no date crate.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;                                  // [0, 399]
    let m   = month as i64;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;          // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`] — days since the epoch back to `(y, m, d)`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z   = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;                                          // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;     // [0, 399]
    let y   = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);                    // [0, 365]
    let mp  = (5 * doy + 2) / 153;                                        // [0, 11]
    let d   = (doy - (153 * mp + 2) / 5 + 1) as u32;                      // [1, 31]
    let m   = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;             // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
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
    // Enumeration + the `claude` match live in the port layer (`ps`/`/proc` on
    // unix, CIM on Windows); here we just adapt the samples to the UI's type.
    let procs = crate::port::proc::claude_procs()
        .into_iter()
        .map(|s| ClaudeProc { rss_kb: s.rss_kb, cpu_pct: s.cpu_pct })
        .collect();
    let sys_total_kb = crate::port::proc::mem_total_kb();
    ProcStats { procs, sys_total_kb }
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

    /// Both timestamp flavours the breakdown meets: `…Z` in transcripts and
    /// `+00:00` from the usage API.
    #[test]
    fn iso_to_epoch_parses_both_flavours() {
        assert_eq!(iso_to_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso_to_epoch("2026-08-01T01:01:14.941Z"), Some(1_785_546_074));
        assert_eq!(iso_to_epoch("2026-08-01T01:01:14+00:00"), Some(1_785_546_074));
        // Leap day, and a plain rejection of anything unparseable.
        assert_eq!(iso_to_epoch("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(iso_to_epoch("garbage"), None);
    }

    /// The scan itself, over a fixture transcript tree — never the user's real
    /// `~/.claude`, which no test should depend on or touch.
    #[test]
    fn breakdown_totals_by_model_within_the_window() {
        let root = std::env::temp_dir().join("frms-breakdown-test").join("projects");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
        std::fs::create_dir_all(root.join("proj-a")).unwrap();
        std::fs::create_dir_all(root.join("proj-b")).unwrap();

        // `since` sits between the two timestamps used below.
        let since = iso_to_epoch("2026-08-01T00:00:00Z").unwrap();
        let reply = |uuid: &str, ts: &str, model: &str, out_tokens: u64| format!(
            r#"{{"type":"assistant","uuid":"{uuid}","timestamp":"{ts}","message":{{"model":"{model}",
               "usage":{{"input_tokens":1,"output_tokens":{out_tokens},
               "cache_read_input_tokens":10,"cache_creation_input_tokens":100}}}}}}"#
        ).replace('\n', "");

        std::fs::write(root.join("proj-a").join("s1.jsonl"), format!(
            "{}\n{}\n{}\n",
            reply("u1", "2026-08-01T01:00:00Z", "claude-opus-5", 5),
            // Before the window opened — must not count.
            reply("u2", "2026-07-31T23:00:00Z", "claude-opus-5", 999),
            // A user record with no usage block — must not count.
            r#"{"type":"user","uuid":"u3","timestamp":"2026-08-01T01:05:00Z"}"#,
        )).unwrap();

        std::fs::write(root.join("proj-b").join("s2.jsonl"), format!(
            "{}\n{}\n{}\n",
            reply("u4", "2026-08-01T02:00:00Z", "claude-sonnet-5", 7),
            // Same uuid as s1's first record — a forked/resumed transcript
            // replaying it. Counting it twice would inflate the window.
            reply("u1", "2026-08-01T01:00:00Z", "claude-opus-5", 5),
            "not json at all",
        )).unwrap();

        let b = breakdown_under(&root, since);
        assert_eq!(b.files, 2);
        assert!(!b.no_transcripts);
        assert_eq!(b.replies(), 2, "one reply per model, the duplicate dropped");

        // Sorted by total, descending: sonnet's larger output puts it first.
        let models: Vec<&str> = b.rows.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["claude-sonnet-5", "claude-opus-5"]);
        assert_eq!(b.rows[0].total(), 1 + 7 + 10 + 100);
        assert_eq!(b.rows[1].total(), 1 + 5 + 10 + 100);
        assert_eq!(b.total(), 234);

        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    /// A machine that has never run Claude Code says so, rather than reporting
    /// a window in which nothing was used.
    #[test]
    fn missing_transcript_root_is_flagged() {
        let missing = std::env::temp_dir().join("frms-breakdown-nope");
        let _ = std::fs::remove_dir_all(&missing);
        let b = breakdown_under(&missing, 0);
        assert!(b.no_transcripts);
        assert!(b.rows.is_empty());
    }

    /// `civil_from_days` has to invert `days_from_civil` exactly — the card's
    /// "scanned since" line is wrong by a day otherwise.
    #[test]
    fn civil_round_trips_and_formats() {
        for (y, m, d) in [(1970, 1, 1), (2024, 2, 29), (2026, 8, 1), (1999, 12, 31)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(format_epoch_dated(0), "Thu Jan 1, 00:00 UTC");
        assert_eq!(format_epoch_dated(1_785_546_074), "Sat Aug 1, 01:01 UTC");
    }

    /// The window the breakdown scans has to be the one the bar measures: it
    /// ends at the API's reset instant, not at "now".
    #[test]
    fn window_start_anchors_on_the_reset_instant() {
        // Both resets ahead of now — the live case. (A reset already in the past
        // is covered by `a_reset_instant_in_the_past_falls_back_to_trailing`.)
        let resets = now_epoch() + 3_600;
        let stats = ClaudeStats {
            five_hour_resets_at: Some(resets),
            seven_day_resets_at: Some(resets),
            ..Default::default()
        };
        assert_eq!(stats.window_start(UsageWindow::FiveHour), resets - 5 * 3600);
        assert_eq!(stats.window_start(UsageWindow::SevenDay), resets - 7 * 86_400);

        // With no reset instant (offline, no credentials) it falls back to a
        // trailing window ending now.
        let blind = ClaudeStats::default();
        let slack = (now_epoch() - 5 * 3600 - blind.window_start(UsageWindow::FiveHour)).abs();
        assert!(slack <= 2, "fallback should be ~5h ago, off by {slack}s");
    }

    /// The weekly reset can be days out, which makes the *current* limit window
    /// only a fraction of a week old — the case where the 7-day card collapses
    /// onto the 5-hour one. `Trailing` is the escape hatch, and it must ignore
    /// the reset instant entirely.
    #[test]
    fn trailing_scope_ignores_the_reset_instant() {
        let now = now_epoch();
        let stats = ClaudeStats {
            // Weekly window reset ~2 days ago: resets 5 days from now.
            seven_day_resets_at: Some(now + 5 * 86_400),
            ..Default::default()
        };

        let limit = stats.scan_start(UsageWindow::SevenDay, UsageScope::LimitWindow);
        assert_eq!(limit, now + 5 * 86_400 - 7 * 86_400, "starts at the last reset");
        assert!(now - limit < 3 * 86_400, "only ~2 days of that week have elapsed");

        let trailing = stats.scan_start(UsageWindow::SevenDay, UsageScope::Trailing);
        let slack = (now - 7 * 86_400 - trailing).abs();
        assert!(slack <= 2, "trailing should be ~7d ago, off by {slack}s");
        assert!(trailing < limit, "trailing reaches further back than the fresh window");
    }

    /// Stats we failed to refresh keep a reset instant that has since passed.
    /// Anchoring on it would scan a window that is over, so it falls back to
    /// the trailing span — and `has_reading` still reports that the numbers,
    /// stale as they are, came from a real response.
    #[test]
    fn a_reset_instant_in_the_past_falls_back_to_trailing() {
        let now = now_epoch();
        let stale = ClaudeStats {
            five_hour_resets_at: Some(now - 90 * 60), // reset an hour and a half ago
            ..Default::default()
        };
        let slack = (now - 5 * 3600 - stale.window_start(UsageWindow::FiveHour)).abs();
        assert!(slack <= 2, "expired reset should fall back to ~5h ago, off by {slack}s");

        assert!(stale.has_reading(), "it is stale, not absent");
        assert!(!ClaudeStats::default().has_reading(), "never fetched");
    }

    #[test]
    fn scope_labels_name_their_span() {
        assert_eq!(UsageScope::LimitWindow.label(UsageWindow::SevenDay), "Limit window");
        assert_eq!(UsageScope::Trailing.label(UsageWindow::SevenDay), "Last 7 days");
        assert_eq!(UsageScope::Trailing.label(UsageWindow::FiveHour), "Last 5 hours");
    }
}
