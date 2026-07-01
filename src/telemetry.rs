//! Lightweight, privacy-respecting usage telemetry.
//!
//! Sends small JSON events to the backend so we can see adoption (which builds
//! run where), coarse feature usage, and crash/error rates. Three design rules:
//!
//!   • **No PII / nothing sensitive.** Events carry an anonymous random id, the
//!     app version, OS + arch, and coarse labels/counters — never paths, file
//!     names, project names, hostnames, usernames, prompts, DB rows, keys, or
//!     URLs. Free-form error/panic text is scrubbed (home dir → `~`, secret-
//!     shaped tokens redacted) and truncated before it ever leaves the machine.
//!   • **Zero UI impact.** [`Telemetry::record`] is a non-blocking channel send;
//!     a single background thread owns all network I/O (one short-lived `curl`
//!     per event, reaped so no zombies). A full queue drops events rather than
//!     ever blocking the app.
//!   • **On by default, trivially opt-out.** Off the instant the user toggles it
//!     in the Profile tab, or if `DO_NOT_TRACK` / `FRMS_NO_TELEMETRY` is set.
//!
//! Transport is `curl` — the app deliberately carries no TLS crate (same
//! shell-out approach as `chat_api`, `stats`, and `clipboard`). The endpoint
//! defaults to the TSR backend and is overridable with `FRMS_TELEMETRY_URL`
//! ("until we move it" — one env var, no rebuild).

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

/// Where events are POSTed unless `FRMS_TELEMETRY_URL` overrides it.
const DEFAULT_URL: &str = "https://api.thesportsremote.com/api/telemetry";
/// Bounded queue between the app and the sender thread; full → events drop.
const QUEUE_CAP: usize = 64;
/// Hard cap on each `curl` so a hung endpoint can't pile up senders.
const HTTP_TIMEOUT_SECS: u32 = 5;
/// Upper bound on scrubbed free-form text — we want a hint, not a payload.
const MAX_DETAIL: usize = 500;

/// A telemetry event. Variants stay coarse on purpose — labels and counters,
/// never identifying content.
pub enum Event {
    /// App started.
    Launch,
    /// An agent pane was created (`kind` = "build" | "research" | "chat").
    AgentCreated { kind: &'static str },
    /// A session was created (`kind` = "terminal" | "browser").
    SessionCreated { kind: &'static str },
    /// A chat reply finished successfully against `model`.
    ChatCompleted { model: &'static str },
    /// A non-fatal error surfaced to the user. `detail` is scrubbed + truncated.
    Error { kind: &'static str, detail: String },
    /// A panic was caught by the hook. `detail` is scrubbed + truncated.
    Panic { detail: String },
}

/// Handle held by the app. Cheap to call; does nothing when disabled.
pub struct Telemetry {
    tx: Option<SyncSender<Event>>,
}

impl Telemetry {
    /// A no-op handle (telemetry off).
    pub fn disabled() -> Self {
        Self { tx: None }
    }

    /// Start the sender thread when `enabled` (the user's pref) and no env
    /// opt-out is set; otherwise returns a no-op handle.
    pub fn start(enabled: bool) -> Self {
        if !enabled || env_opt_out() {
            return Self::disabled();
        }
        let url = endpoint();
        let ctx = Context::gather();
        let (tx, rx) = sync_channel::<Event>(QUEUE_CAP);
        let spawned = std::thread::Builder::new()
            .name("frms-telemetry".into())
            .spawn(move || worker(rx, url, ctx))
            .is_ok();
        // If the thread failed to spawn, drop the sender so `record` is a no-op.
        Self { tx: spawned.then_some(tx) }
    }

    /// Queue an event. Non-blocking: returns immediately, dropping the event if
    /// the queue is backed up. Never touches the network on the calling thread.
    pub fn record(&self, event: Event) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(event);
        }
    }
}

/// True if the user set a standard opt-out env var (`DO_NOT_TRACK` is the
/// cross-app convention; `FRMS_NO_TELEMETRY` is ours). Any non-empty, non-"0"
/// value counts.
fn env_opt_out() -> bool {
    ["DO_NOT_TRACK", "FRMS_NO_TELEMETRY"].iter().any(|k| {
        std::env::var(k).map(|v| !v.is_empty() && v != "0").unwrap_or(false)
    })
}

fn endpoint() -> String {
    std::env::var("FRMS_TELEMETRY_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_URL.to_string())
}

/// Static, machine-stable context attached to every event.
struct Context {
    id:      String,
    version: &'static str,
    os:      &'static str,
    arch:    &'static str,
}

impl Context {
    fn gather() -> Self {
        Self {
            id:      anon_id(),
            version: env!("CARGO_PKG_VERSION"),
            os:      std::env::consts::OS,
            arch:    std::env::consts::ARCH,
        }
    }
}

/// Drain the queue, POSTing one event at a time. Exits when the app drops the
/// sender. Waiting on each `curl` reaps it (no zombies); the timeout bounds it.
fn worker(rx: Receiver<Event>, url: String, ctx: Context) {
    while let Ok(event) = rx.recv() {
        let body = encode(&ctx, &event);
        post(&url, &body);
    }
}

fn encode(ctx: &Context, event: &Event) -> String {
    let (name, props) = describe(event);
    serde_json::json!({
        "event":       name,
        "id":          ctx.id,
        "ts":          now_secs(),
        "app":         "frms",
        "app_version": ctx.version,
        "os":          ctx.os,
        "arch":        ctx.arch,
        "props":       props,
    })
    .to_string()
}

fn describe(event: &Event) -> (&'static str, serde_json::Value) {
    use serde_json::json;
    match event {
        Event::Launch                  => ("launch",         json!({})),
        Event::AgentCreated { kind }   => ("agent_created",  json!({ "kind":  kind })),
        Event::SessionCreated { kind } => ("session_created",json!({ "kind":  kind })),
        Event::ChatCompleted { model } => ("chat_completed", json!({ "model": model })),
        Event::Error { kind, detail }  => ("error",          json!({ "kind":  kind, "detail": scrub(detail) })),
        Event::Panic { detail }        => ("panic",          json!({ "detail": scrub(detail) })),
    }
}

/// Fire one POST and reap it. Body goes on stdin (`--data-binary @-`) so it
/// stays out of the argv / `/proc`. All failures are ignored — telemetry must
/// never affect the app.
fn post(url: &str, body: &str) {
    let child = Command::new("curl")
        .arg("-sS")
        .arg("-m").arg(HTTP_TIMEOUT_SECS.to_string())
        .arg("-X").arg("POST")
        .arg("-H").arg("content-type: application/json")
        .arg("--data-binary").arg("@-")
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(body.as_bytes());
        }
        let _ = child.wait();
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── anonymous id ────────────────────────────────────────────────────────────

fn id_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".frms").join("telemetry_id"))
}

/// Persistent random id (UUID-like, 16 bytes hex). Created once, then reused —
/// it identifies an install, never a person. No network, no hardware ids.
fn anon_id() -> String {
    let path = id_path();
    if let Some(p) = &path {
        if let Ok(s) = std::fs::read_to_string(p) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    let id = random_hex16();
    if let Some(p) = &path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, &id);
    }
    id
}

fn random_hex16() -> String {
    let mut buf = [0u8; 16];
    let from_urandom = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if !from_urandom {
        // Non-crypto fallback (only if /dev/urandom is unavailable): time ⊕ pid.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mix = nanos ^ ((std::process::id() as u128) << 64);
        buf = mix.to_le_bytes();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

// ── scrubbing ───────────────────────────────────────────────────────────────

/// Strip identifying / sensitive bits from free-form text before sending:
/// replace the home dir with `~` (drops username + local layout), redact
/// secret-shaped tokens, and cap the length.
fn scrub(s: &str) -> String {
    let mut out = s.to_string();
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            out = out.replace(&home, "~");
        }
    }
    out = redact_tokens(&out);
    if out.chars().count() > MAX_DETAIL {
        out = out.chars().take(MAX_DETAIL).collect::<String>();
        out.push('…');
    }
    out
}

/// Redact whitespace-delimited tokens that look like credentials — known
/// prefixes (sk-, ghp_, xox…) or long opaque base64/hex-ish strings.
fn redact_tokens(s: &str) -> String {
    s.split_inclusive(char::is_whitespace)
        .map(|piece| {
            let tok = piece.trim();
            let looks_secret = tok.starts_with("sk-")
                || tok.starts_with("ghp_")
                || tok.starts_with("xox")
                || (tok.len() >= 40
                    && tok.chars().all(|c| {
                        c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '+' | '=')
                    }));
            if looks_secret && !tok.is_empty() {
                piece.replace(tok, "[redacted]")
            } else {
                piece.to_string()
            }
        })
        .collect()
}

// ── panic reporting ─────────────────────────────────────────────────────────

/// Install a panic hook that best-effort reports a (scrubbed) panic event, then
/// chains the previous hook. Reads the pref fresh so an opted-out user reports
/// nothing. Runs synchronously (the app is already dying) but is bounded by the
/// `curl` timeout, and never itself panics.
pub fn install_panic_hook(enabled: bool) {
    if !enabled || env_opt_out() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Location + message only — no thread name (can carry app data), no args.
        let where_ = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let ctx = Context::gather();
        let body = encode(&ctx, &Event::Panic { detail: format!("{where_} {msg}") });
        post(&endpoint(), &body);
        previous(info);
    }));
}
