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
//!   • **Two tiers.** *Essential* events — one `install` per install and a
//!     `launch` per app open — are always sent: they're the licensing/adoption
//!     count the product needs, and the notice discloses them up front.
//!     *Optional* events (feature usage, errors, panics) are on by default and
//!     off the instant the user unchecks the Profile toggle or sets
//!     `DO_NOT_TRACK` / `FRMS_NO_TELEMETRY`. Both tiers carry the same anonymous
//!     payload — an install id, version, OS, arch — and never any PII.
//!
//! Transport is `curl` — the app deliberately carries no TLS crate (same
//! shell-out approach as `chat_api`, `stats`, and `clipboard`). The endpoint
//! defaults to the TSR backend and is overridable with `FRMS_TELEMETRY_URL`
//! ("until we move it" — one env var, no rebuild).

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

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
    /// First launch on this install — the anonymous id was just minted.
    /// Essential tier: always sent (see [`Event::is_essential`]).
    Install,
    /// App started. Essential tier: always sent.
    Launch,
    /// An agent pane was created (`kind` = "build" | "research" | "chat").
    AgentCreated { kind: &'static str },
    /// A session was created (`kind` = "terminal" | "browser").
    SessionCreated { kind: &'static str },
    /// The user retargeted `count` existing agent panes at another model of
    /// `tool` (= "claude" | "windsurf"). No model id — just the tool and how
    /// many panes moved at once.
    AgentModelChanged { tool: &'static str, count: usize },
    /// A chat reply finished successfully against `model`.
    ChatCompleted { model: &'static str },
    /// A non-fatal error surfaced to the user. `detail` is scrubbed + truncated.
    Error { kind: &'static str, detail: String },
    /// A panic was caught by the hook. `detail` is scrubbed + truncated.
    Panic { detail: String },
}

impl Event {
    /// Essential events are the install/adoption count: one per install, one per
    /// app open. They carry no usage detail beyond the shared anonymous context
    /// and are not covered by the opt-out — everything else is.
    fn is_essential(&self) -> bool {
        matches!(self, Event::Install | Event::Launch)
    }
}

/// Handle held by the app. Cheap to call; does nothing when disabled.
pub struct Telemetry {
    tx: Option<SyncSender<Event>>,
    /// Whether optional (non-essential) events are allowed through. Shared so
    /// the Profile toggle applies instantly without restarting the sender.
    optional: Arc<AtomicBool>,
}

impl Telemetry {
    /// A no-op handle — sends nothing at all. Used as the pre-init placeholder;
    /// the running app always holds a [`Telemetry::start`] handle so essential
    /// events still flow.
    pub fn disabled() -> Self {
        Self { tx: None, optional: Arc::new(AtomicBool::new(false)) }
    }

    /// Start the sender thread. Essential events (install / launch) are always
    /// sent; `optional` — the user's pref, further gated by the env opt-out —
    /// decides whether feature-usage and diagnostic events go too.
    pub fn start(optional: bool) -> Self {
        let url = endpoint();
        let (ctx, fresh_install) = Context::gather();
        let (tx, rx) = sync_channel::<Event>(QUEUE_CAP);
        let spawned = std::thread::Builder::new()
            .name("frms-telemetry".into())
            .spawn(move || worker(rx, url, ctx))
            .is_ok();
        // If the thread failed to spawn, drop the sender so `record` is a no-op.
        let this = Self {
            tx:       spawned.then_some(tx),
            optional: Arc::new(AtomicBool::new(optional && !env_opt_out())),
        };
        // A brand-new anonymous id means this is the first run of a fresh
        // install — report it once, before anything else in the queue.
        if fresh_install {
            this.record(Event::Install);
        }
        this
    }

    /// Apply the Profile toggle. Essential events are unaffected.
    pub fn set_optional(&self, optional: bool) {
        self.optional.store(optional && !env_opt_out(), Ordering::Relaxed);
    }

    /// Queue an event. Non-blocking: returns immediately, dropping the event if
    /// the queue is backed up. Never touches the network on the calling thread.
    pub fn record(&self, event: Event) {
        if !event.is_essential() && !self.optional.load(Ordering::Relaxed) {
            return;
        }
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(event);
        }
    }
}

/// True if the user set a standard opt-out env var (`DO_NOT_TRACK` is the
/// cross-app convention; `FRMS_NO_TELEMETRY` is ours). Any non-empty, non-"0"
/// value counts. Like the Profile toggle, this silences the optional tier —
/// install/launch counts still go out.
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
    /// Build the context, reporting whether the anonymous id had to be minted —
    /// i.e. whether this is the first run of a fresh install.
    fn gather() -> (Self, bool) {
        let (id, fresh) = anon_id();
        let ctx = Self {
            id,
            version: crate::version::VERSION,
            os:      std::env::consts::OS,
            arch:    std::env::consts::ARCH,
        };
        (ctx, fresh)
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
        Event::Install                 => ("install",        json!({})),
        Event::Launch                  => ("launch",         json!({})),
        Event::AgentCreated { kind }   => ("agent_created",  json!({ "kind":  kind })),
        Event::SessionCreated { kind } => ("session_created",json!({ "kind":  kind })),
        Event::AgentModelChanged { tool, count } =>
            ("agent_model_changed", json!({ "tool": tool, "count": count })),
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
    Some(crate::port::dirs::config()?.join("telemetry_id"))
}

/// Persistent random id (UUID-like, 16 bytes hex). Created once, then reused —
/// it identifies an install, never a person. No network, no hardware ids.
///
/// Returns `(id, fresh)`, where `fresh` means the id was just minted — the
/// install signal. It's written to disk immediately, so a crash right after
/// this can lose the install event but never duplicate it. If the config dir
/// is unwritable the id can't persist, so `fresh` stays `false` after the
/// first call in that process rather than counting an install on every launch.
fn anon_id() -> (String, bool) {
    let path = id_path();
    if let Some(p) = &path {
        if let Ok(s) = std::fs::read_to_string(p) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return (s, false);
            }
        }
    }
    let id = random_hex16();
    let mut stored = false;
    if let Some(p) = &path {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        stored = std::fs::write(p, &id).is_ok();
    }
    (id, stored)
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
    if let Some(home) = crate::port::dirs::home().and_then(|h| h.to_str().map(str::to_owned)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The opt-out covers usage + diagnostics; install/launch counts are
    /// outside it. Guards the tier split itself — `record` can't be exercised
    /// here without starting the sender thread (which would POST for real).
    #[test]
    fn only_install_and_launch_are_essential() {
        assert!(Event::Install.is_essential());
        assert!(Event::Launch.is_essential());
        for e in [
            Event::AgentCreated { kind: "build" },
            Event::SessionCreated { kind: "terminal" },
            Event::ChatCompleted { model: "m" },
            Event::Error { kind: "chat", detail: String::new() },
            Event::Panic { detail: String::new() },
        ] {
            assert!(!e.is_essential());
        }
    }
}

// ── panic reporting ─────────────────────────────────────────────────────────

/// Install a panic hook that best-effort reports a (scrubbed) panic event, then
/// chains the previous hook. Crash reports are optional-tier, so this is gated
/// on the user's pref (and the env opt-out) — an opted-out user reports nothing
/// here. Runs synchronously (the app is already dying) but is bounded by the
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
        let (ctx, _) = Context::gather();
        let body = encode(&ctx, &Event::Panic { detail: format!("{where_} {msg}") });
        post(&endpoint(), &body);
        previous(info);
    }));
}
