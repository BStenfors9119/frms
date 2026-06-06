//! Persists session list across app launches as JSON at
//! `$HOME/.frms/sessions.json`. Mirrors the design of `prefs.rs`:
//! hand-rolled serialization on top of `serde_json::Value` so the file
//! is forward-compatible — unknown fields are ignored, missing fields
//! fall back to defaults.
//!
//! What we persist (per session): kind, name, working_dir, the browser
//! url state, the last-open editor file, and the center-tab choice.
//! What we deliberately don't: live PTY contents, DB connection state,
//! screenshots, diff lists — anything that has to be rebuilt fresh on
//! launch anyway.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::session::SessionKind;

/// Persisted form of the session's center-pane tab. We deliberately don't
/// round-trip Claude terminal ids since those are reassigned on launch —
/// `claude(N)` records the index into the rebuilt terminal list instead.
#[derive(Debug, Clone, Copy)]
pub enum PersistedCenterTab {
    Editor,
    Database,
    Claude(usize),
}

#[derive(Debug, Clone)]
pub struct PersistedSession {
    pub id:           usize,
    pub kind:         SessionKind,
    pub name:         String,
    pub working_dir:  PathBuf,
    pub url_input:    Option<String>,
    pub browser_url:  Option<String>,
    pub open_file:    Option<PathBuf>,
    pub center_tab:   PersistedCenterTab,
    /// Number of Claude terminals to recreate on launch (at least 1).
    pub claude_count: usize,
    /// Per-tab user-assigned names, indexed positionally. Shorter than
    /// `claude_count` if only some tabs were renamed.
    pub claude_names: Vec<Option<String>>,
    /// Names of the session's *pinned* Terminals-plugin shell terminals.
    /// Each is respawned as a fresh shell (rooted in `working_dir`) on the
    /// next launch. Unpinned plugin terminals are ephemeral and omitted.
    pub plugin_terminals: Vec<String>,
    /// Whether the user pinned this session for persistence. Only pinned
    /// sessions are written, so this is always true on load — kept explicit
    /// so the restored session stays pinned without re-pinning.
    pub pinned:       bool,
}

#[derive(Debug, Clone)]
pub struct PersistedSplitView {
    pub top_id:    usize,
    pub bottom_id: usize,
}

#[derive(Debug, Clone, Default)]
pub struct PersistedState {
    pub sessions:          Vec<PersistedSession>,
    pub active_session_id: Option<usize>,
    pub split_view:        Option<PersistedSplitView>,
}

fn path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".frms").join("sessions.json"))
}

fn kind_as_str(k: SessionKind) -> &'static str {
    match k {
        SessionKind::Terminal => "terminal",
        SessionKind::Browser  => "browser",
    }
}

fn kind_from_str(s: &str) -> Option<SessionKind> {
    match s {
        "terminal" => Some(SessionKind::Terminal),
        "browser"  => Some(SessionKind::Browser),
        _          => None,
    }
}

fn center_tab_to_value(t: PersistedCenterTab) -> Value {
    match t {
        PersistedCenterTab::Editor      => json!("editor"),
        PersistedCenterTab::Database    => json!("database"),
        PersistedCenterTab::Claude(idx) => json!({ "kind": "claude", "index": idx }),
    }
}

fn center_tab_from_value(v: &Value) -> Option<PersistedCenterTab> {
    if let Some(s) = v.as_str() {
        return match s {
            "editor"   => Some(PersistedCenterTab::Editor),
            "database" => Some(PersistedCenterTab::Database),
            _          => None,
        };
    }
    let obj = v.as_object()?;
    match obj.get("kind").and_then(Value::as_str)? {
        "editor"   => Some(PersistedCenterTab::Editor),
        "database" => Some(PersistedCenterTab::Database),
        "claude"   => {
            let idx = obj.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            Some(PersistedCenterTab::Claude(idx))
        }
        _ => None,
    }
}

impl PersistedState {
    pub fn load() -> Self {
        let Some(p)    = path()                               else { return Self::default(); };
        let Ok(bytes)  = std::fs::read(&p)                    else { return Self::default(); };
        let Ok(v)      = serde_json::from_slice::<Value>(&bytes) else { return Self::default(); };

        let sessions = v.get("sessions")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(parse_session).collect())
            .unwrap_or_default();

        let active_session_id = v.get("active_session_id")
            .and_then(Value::as_u64)
            .map(|n| n as usize);

        let split_view = v.get("split_view").and_then(|sv| {
            let top    = sv.get("top_id")?.as_u64()? as usize;
            let bottom = sv.get("bottom_id")?.as_u64()? as usize;
            Some(PersistedSplitView { top_id: top, bottom_id: bottom })
        });

        Self { sessions, active_session_id, split_view }
    }

    pub fn save(&self) {
        let Some(p) = path() else { return; };
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let sessions: Vec<Value> = self.sessions.iter().map(|s| json!({
            "id":           s.id,
            "kind":         kind_as_str(s.kind),
            "name":         s.name,
            "working_dir":  s.working_dir.to_string_lossy(),
            "url_input":    s.url_input,
            "browser_url":  s.browser_url,
            "open_file":    s.open_file.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "center_tab":   center_tab_to_value(s.center_tab),
            "claude_count": s.claude_count.max(1),
            "claude_names": s.claude_names.iter()
                .map(|n| n.clone().map(Value::String).unwrap_or(Value::Null))
                .collect::<Vec<_>>(),
            "plugin_terminals": s.plugin_terminals,
            "pinned":       s.pinned,
        })).collect();

        let split_view = self.split_view.as_ref().map(|sv| json!({
            "top_id":    sv.top_id,
            "bottom_id": sv.bottom_id,
        }));

        let doc = json!({
            "sessions":          sessions,
            "active_session_id": self.active_session_id,
            "split_view":        split_view,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
            let _ = std::fs::write(&p, bytes);
        }
    }
}

fn parse_session(v: &Value) -> Option<PersistedSession> {
    let id          = v.get("id")?.as_u64()? as usize;
    let kind        = v.get("kind").and_then(Value::as_str).and_then(kind_from_str)?;
    let name        = v.get("name").and_then(Value::as_str)?.to_string();
    let working_dir = v.get("working_dir").and_then(Value::as_str).map(PathBuf::from)?;

    // Skip restoring this session if the directory no longer exists. The
    // user has likely moved or deleted the project; reopening a session
    // pointing at a missing path would spawn shells in $HOME and show an
    // empty file browser — surprising and unrecoverable.
    if !working_dir.is_dir() {
        return None;
    }

    let url_input   = v.get("url_input").and_then(Value::as_str).map(str::to_owned);
    let browser_url = v.get("browser_url").and_then(Value::as_str).map(str::to_owned);
    let open_file   = v.get("open_file")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file());
    let center_tab  = v.get("center_tab")
        .and_then(center_tab_from_value)
        .unwrap_or(PersistedCenterTab::Editor);
    let claude_count = v.get("claude_count")
        .and_then(Value::as_u64)
        .map(|n| n.max(1) as usize)
        .unwrap_or(1);
    let claude_names = v.get("claude_names")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(|n| n.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    let plugin_terminals = v.get("plugin_terminals")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|n| n.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    // Default to true: a session present in the file was written because it
    // was pinned, so it should come back pinned even if the field predates
    // this version of the format.
    let pinned = v.get("pinned").and_then(Value::as_bool).unwrap_or(true);

    Some(PersistedSession {
        id, kind, name, working_dir,
        url_input, browser_url, open_file, center_tab, claude_count, claude_names,
        plugin_terminals,
        pinned,
    })
}
