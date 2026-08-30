//! Persists session list across app launches as JSON at
//! `$HOME/.frms/sessions.json`. Mirrors the design of `prefs.rs`:
//! hand-rolled serialization on top of `serde_json::Value` so the file
//! is forward-compatible — unknown fields are ignored, missing fields
//! fall back to defaults.
//!
//! What we persist (per session): kind, name, working_dir, the browser
//! url state, the last-open editor file, the center-tab choice, and the
//! plugin panel layout (dock side, split, tabs, sizes, visibility).
//! What we deliberately don't: live PTY contents, DB connection state,
//! screenshots, diff lists — anything that has to be rebuilt fresh on
//! launch anyway.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::agent::{AgentKind, AiTool};
use crate::plugin_panel::{DockSide, PluginPanel, PluginTab};
use crate::session::SessionKind;
use crate::terminal::Container;

/// Persisted form of the session's center-pane tab. We deliberately don't
/// round-trip Claude terminal ids since those are reassigned on launch —
/// `claude(N)` records the index into the rebuilt terminal list instead.
#[derive(Debug, Clone, Copy)]
pub enum PersistedCenterTab {
    Editor,
    Database,
    Claude(usize),
}

/// Persisted form of one pinned Terminals-plugin shell. Records the name the
/// user gave it, the directory its shell was sitting in when last saved, and
/// the toolbox it was inside — so a restored terminal reopens where it was left
/// instead of at the project root, and back in the same container.
#[derive(Debug, Clone)]
pub struct PersistedTerminal {
    pub name: String,
    /// Where the shell was; `None` when unknown (platform can't read it, or the
    /// file predates this field). Restore falls back to the session's dir.
    pub cwd:  Option<PathBuf>,
    /// The toolbox/distrobox the shell was inside, or `None` if it was on the
    /// host (or the file predates this field). Restore types the matching
    /// `… enter <name>` into the new shell.
    pub container: Option<Container>,
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
    /// Per-tab agent kind (build/research/chat), indexed positionally. Empty
    /// when the file predates this field — restore then treats every tab as a
    /// `Build` pane, matching the old behaviour.
    pub claude_kinds: Vec<AgentKind>,
    /// Per-tab AI tool for `Build` panes (`claude`/`windsurf`), indexed
    /// positionally. Chat panes record `Claude`. Empty when the file predates
    /// this field — restore then treats every tab as Claude, matching how older
    /// saves (which only ran `claude`) behaved.
    pub claude_tools: Vec<AiTool>,
    /// Per-tab model *token* (e.g. `"opus-4-8"`, `"swe-1-6"`), indexed
    /// positionally and resolved against that tab's tool on restore. A chat
    /// pane records the Claude model the user picked for it, or `"default"` to
    /// stay on its kind's own model. Empty when the file predates this field —
    /// restore then uses each tool's default. (Old files stored Claude tokens
    /// here, which still resolve since `CLAUDE_MODELS` shares those tokens.)
    pub claude_models: Vec<String>,
    /// The session's *pinned* Terminals-plugin shell terminals. Each is
    /// respawned as a fresh shell on the next launch — rooted in its saved
    /// `cwd` if still valid, else `working_dir`. Unpinned plugin terminals are
    /// ephemeral and omitted.
    pub plugin_terminals: Vec<PersistedTerminal>,
    /// Whether the user pinned this session for persistence. Only pinned
    /// sessions are written, so this is always true on load — kept explicit
    /// so the restored session stays pinned without re-pinning.
    pub pinned:       bool,
    /// Whether the user hid the file explorer pane. `false` when the file
    /// predates this field, so older saves come back with the browser showing.
    pub file_browser_collapsed: bool,
    /// Plugin panel layout — dock side, split, tabs, sizes, visibility.
    /// `None` when the file predates this field; restore keeps defaults.
    pub plugin_panel: Option<PluginPanel>,
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
    Some(crate::port::dirs::config()?.join("sessions.json"))
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

fn plugin_tab_as_str(t: PluginTab) -> &'static str {
    match t {
        PluginTab::Profile   => "profile",
        PluginTab::Notes     => "notes",
        PluginTab::Docs      => "docs",
        PluginTab::Terminals => "terminals",
        PluginTab::Receivers => "receivers",
    }
}

fn plugin_tab_from_str(s: &str) -> Option<PluginTab> {
    match s {
        // "db" is a legacy value — the database tooling is no longer a plugin
        // tab, so an old persisted "db" falls back to the default tab.
        "profile"   => Some(PluginTab::Profile),
        "notes"     => Some(PluginTab::Notes),
        "docs"      => Some(PluginTab::Docs),
        "terminals" => Some(PluginTab::Terminals),
        "receivers" => Some(PluginTab::Receivers),
        _           => None,
    }
}

fn plugin_panel_to_value(p: &PluginPanel) -> Value {
    json!({
        "visible":    p.visible,
        "expanded":   p.expanded,
        "dock":       match p.dock { DockSide::Left => "left", DockSide::Right => "right" },
        "active_tab": plugin_tab_as_str(p.active_tab),
        "bottom_tab": plugin_tab_as_str(p.bottom_tab),
        "split":      p.split,
        "top_height": p.top_height,
        "width":      p.width,
        "hidden":     p.hidden.iter().map(|t| plugin_tab_as_str(*t)).collect::<Vec<_>>(),
    })
}

fn plugin_panel_from_value(v: &Value) -> Option<PluginPanel> {
    let obj = v.as_object()?;
    let mut p = PluginPanel::default();
    if let Some(b) = obj.get("visible").and_then(Value::as_bool) { p.visible = b; }
    if let Some(b) = obj.get("expanded").and_then(Value::as_bool) { p.expanded = b; }
    if let Some(d) = obj.get("dock").and_then(Value::as_str) {
        p.dock = match d {
            "left" => DockSide::Left,
            _      => DockSide::Right,
        };
    }
    if let Some(t) = obj.get("active_tab").and_then(Value::as_str).and_then(plugin_tab_from_str) {
        p.active_tab = t;
    }
    if let Some(t) = obj.get("bottom_tab").and_then(Value::as_str).and_then(plugin_tab_from_str) {
        p.bottom_tab = t;
    }
    if let Some(b) = obj.get("split").and_then(Value::as_bool) { p.split = b; }
    // Route sizes through the setters so hand-edited or stale values are
    // clamped to the same bounds the drag handles enforce.
    if let Some(h) = obj.get("top_height").and_then(Value::as_f64) { p.set_top_height(h as f32); }
    if let Some(w) = obj.get("width").and_then(Value::as_f64)      { p.set_width(w as f32); }
    if let Some(arr) = obj.get("hidden").and_then(Value::as_array) {
        p.hidden = arr.iter()
            .filter_map(|v| v.as_str().and_then(plugin_tab_from_str))
            .collect();
    }
    Some(p)
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
            "claude_kinds": s.claude_kinds.iter()
                .map(|k| Value::String(k.as_str().to_string()))
                .collect::<Vec<_>>(),
            "claude_tools": s.claude_tools.iter()
                .map(|t| Value::String(t.as_str().to_string()))
                .collect::<Vec<_>>(),
            "claude_models": s.claude_models.iter()
                .map(|m| Value::String(m.clone()))
                .collect::<Vec<_>>(),
            "plugin_terminals": s.plugin_terminals.iter()
                .map(|t| json!({
                    "name": t.name,
                    "cwd":  t.cwd.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    "container": t.container.as_ref().map(|c| json!({
                        "name":    c.name,
                        "runtime": c.runtime,
                    })),
                }))
                .collect::<Vec<_>>(),
            "pinned":       s.pinned,
            "file_browser_collapsed": s.file_browser_collapsed,
            "plugin_panel": s.plugin_panel.as_ref().map(plugin_panel_to_value),
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
    let claude_kinds = v.get("claude_kinds")
        .and_then(Value::as_array)
        .map(|arr| arr.iter()
            .filter_map(|k| k.as_str().and_then(AgentKind::from_str))
            .collect())
        .unwrap_or_default();
    let claude_tools = v.get("claude_tools")
        .and_then(Value::as_array)
        .map(|arr| arr.iter()
            .map(|t| t.as_str().and_then(AiTool::from_str).unwrap_or_default())
            .collect())
        .unwrap_or_default();
    // Model tokens are kept as raw strings and resolved against each tab's tool
    // on restore (a token means nothing without its tool). Old files stored
    // Claude model tokens here — still valid, since `CLAUDE_MODELS` uses them.
    let claude_models = v.get("claude_models")
        .and_then(Value::as_array)
        .map(|arr| arr.iter()
            .map(|m| m.as_str().unwrap_or("default").to_string())
            .collect())
        .unwrap_or_default();
    // Accept both the old shape (array of name strings) and the new one
    // (array of {name, cwd, container} objects), so files written by an older
    // frms still load — a bare string just has no saved cwd or container.
    let plugin_terminals = v.get("plugin_terminals")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|t| {
            if let Some(name) = t.as_str() {
                return Some(PersistedTerminal {
                    name: name.to_owned(), cwd: None, container: None,
                });
            }
            let name = t.get("name").and_then(Value::as_str)?.to_owned();
            let cwd = t.get("cwd").and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let container = t.get("container").and_then(|c| {
                let name = c.get("name").and_then(Value::as_str)
                    .filter(|s| !s.is_empty())?
                    .to_owned();
                let runtime = c.get("runtime").and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                Some(Container { name, runtime })
            });
            Some(PersistedTerminal { name, cwd, container })
        }).collect())
        .unwrap_or_default();
    // Default to true: a session present in the file was written because it
    // was pinned, so it should come back pinned even if the field predates
    // this version of the format.
    let pinned = v.get("pinned").and_then(Value::as_bool).unwrap_or(true);

    let file_browser_collapsed = v.get("file_browser_collapsed")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let plugin_panel = v.get("plugin_panel").and_then(plugin_panel_from_value);

    Some(PersistedSession {
        id, kind, name, working_dir,
        url_input, browser_url, open_file, center_tab, claude_count, claude_names,
        claude_kinds,
        claude_tools,
        claude_models,
        plugin_terminals,
        pinned,
        file_browser_collapsed,
        plugin_panel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A minimal session JSON rooted at a directory that exists (parse_session
    /// drops sessions whose working_dir is gone), with the given per-tab arrays.
    fn session_json(extra: Value) -> Value {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let mut base = json!({
            "id": 1, "kind": "terminal", "name": "s", "working_dir": dir,
        });
        let obj = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
        base
    }

    #[test]
    fn new_format_round_trips_tool_and_model_per_tab() {
        let v = session_json(json!({
            "claude_count":  2,
            "claude_kinds":  ["build", "build"],
            "claude_tools":  ["claude", "windsurf"],
            "claude_models": ["opus-4-8", "swe-1-6"],
        }));
        let ps = parse_session(&v).expect("session parses");
        assert_eq!(ps.claude_tools, vec![AiTool::Claude, AiTool::Windsurf]);
        assert_eq!(ps.claude_models, vec!["opus-4-8".to_string(), "swe-1-6".to_string()]);
        // Each token resolves against its own tab's tool.
        assert!(AiTool::Windsurf.model_by_token(&ps.claude_models[1]).is_some());
    }

    #[test]
    fn old_format_without_tools_loads_as_claude() {
        // A pre-tool save file: no `claude_tools`, Claude model tokens only.
        let v = session_json(json!({
            "claude_count":  1,
            "claude_kinds":  ["build"],
            "claude_models": ["opus-4-8"],
        }));
        let ps = parse_session(&v).expect("session parses");
        // No tools recorded → restore defaults every tab to Claude.
        assert!(ps.claude_tools.is_empty());
        // The old Claude token survives verbatim and still resolves.
        assert_eq!(ps.claude_models, vec!["opus-4-8".to_string()]);
        assert!(AiTool::Claude.model_by_token("opus-4-8").is_some());
    }

    #[test]
    fn pinned_terminals_round_trip_their_toolbox() {
        let dir = std::env::temp_dir().to_string_lossy().into_owned();
        let v = session_json(json!({
            "plugin_terminals": [
                { "name": "build", "cwd": dir, "container": { "name": "bottle", "runtime": "toolbox" } },
                { "name": "host",  "cwd": dir },
                // Pre-container format: a bare name string.
                "ancient",
            ],
        }));
        let ps = parse_session(&v).expect("session parses");
        assert_eq!(ps.plugin_terminals[0].container,
                   Some(Container { name: "bottle".into(), runtime: "toolbox".into() }));
        // A shell left on the host, and an older save, both restore on the host.
        assert!(ps.plugin_terminals[1].container.is_none());
        assert!(ps.plugin_terminals[2].container.is_none());
        assert!(ps.plugin_terminals[2].cwd.is_none());
    }

    #[test]
    fn unknown_model_token_falls_back_to_default_on_resolve() {
        // A token no longer offered by the tool resolves to None, so restore
        // uses the tool's default (exercised here via the same lookup).
        let v = session_json(json!({
            "claude_count":  1,
            "claude_kinds":  ["build"],
            "claude_tools":  ["windsurf"],
            "claude_models": ["some-retired-model"],
        }));
        let ps = parse_session(&v).expect("session parses");
        assert_eq!(ps.claude_tools, vec![AiTool::Windsurf]);
        assert!(AiTool::Windsurf.model_by_token(&ps.claude_models[0]).is_none());
    }
}
