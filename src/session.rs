use std::path::PathBuf;

use crate::agent::{AgentKind, AgentPane};
use crate::claude_prompt::PendingPrompt;
use crate::db_panel::DbPanel;
use crate::editor::EditorState;
use crate::file_browser::FileBrowserState;
use crate::layout::Layout;
use crate::plugin_panel::PluginPanel;
use crate::terminal::{TerminalId, TerminalPane};
use crate::terminals::TerminalsState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SessionKind {
    Terminal,
    Browser,
}

/// True if `cmd` is an executable file reachable on `PATH`. Used to decide
/// whether the `claude` CLI is installed before we try to spawn it.
fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths)
            .any(|dir| std::fs::metadata(dir.join(cmd)).map(|m| m.is_file()).unwrap_or(false))
    })
}

/// Whether the `claude` CLI is installed (on `PATH`). Both Build panes and the
/// native chat panes drive it, so the Profile tab surfaces this to the user.
pub fn claude_on_path() -> bool {
    on_path("claude")
}

/// Spawn a Build (Claude Code) agent pane in a PTY.
///
/// When the `claude` CLI is on `PATH`, run it directly. When it isn't —
/// common right after an `.rpm`/`.deb` install, since Claude Code ships via
/// npm, not as a distro package — fall back to an interactive shell that first
/// prints install guidance. That keeps a fresh machine from crashing at launch
/// (the previous code `panic!`ed on the failed spawn) and tells the user
/// exactly how to get Claude Code, while leaving them a usable shell in the
/// pane. Re-running an agent pane after installing picks up `claude` normally.
fn spawn_agent_pane(id: TerminalId, dir: Option<&std::path::Path>) -> TerminalPane {
    if on_path("claude") {
        if let Ok(pane) = TerminalPane::spawn(id, "claude", dir) {
            return pane;
        }
    }

    // Single-quote a string for safe inclusion in a `sh -c` script: wrap in
    // quotes and replace each embedded quote with the '\'' idiom.
    fn sq(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let lines = [
        "Claude Code CLI ('claude') was not found on your PATH.",
        "Agent panes use it to run Claude. To install it:",
        "    npm install -g @anthropic-ai/claude-code",
        "(requires Node.js / npm). Then restart frms or open a new agent pane.",
        "",
    ];
    let script = format!(
        "printf '%s\\n' {}; exec {}",
        lines.iter().map(|l| sq(l)).collect::<Vec<_>>().join(" "),
        sq(&shell),
    );
    let args = vec!["-c".to_string(), script];
    TerminalPane::spawn_args(id, "/bin/sh", &args, &[], dir)
        .unwrap_or_else(|e| panic!("Failed to spawn fallback shell for agent pane {id}: {e}"))
}

/// Which tab is currently active in the center pane: the editor, the
/// database panel, or one of the session's Claude terminals (referenced by
/// its `TerminalId` so it stays stable as other Claude tabs come and go).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CenterTab {
    Editor,
    Database,
    Claude(TerminalId),
}

/// One fully independent IDE session — its own file browser, editor, and
/// a dynamic list of Claude terminals shown as tabs in the center pane.
#[allow(dead_code)]
pub struct Session {
    pub id:               usize,
    pub name:             String,
    pub kind:             SessionKind,
    pub working_dir:      PathBuf,
    pub layout:           Layout,
    /// When true the file-browser pane is hidden and its space reclaimed by
    /// the editor; a thin strip with an expand button takes its place.
    pub file_browser_collapsed: bool,
    /// When true this session is written to disk and restored on next launch.
    /// Unpinned sessions are ephemeral — they vanish when the app closes.
    pub pinned:           bool,
    pub file_browser:     FileBrowserState,
    pub editor:           EditorState,
    /// Agent panes owned by this session, in tab order. Always at least one.
    /// Each appears as a tab in the center pane. Currently every pane is a
    /// `Build` pane (Claude Code in a PTY); native chat panes arrive later.
    pub panes:            Vec<AgentPane>,
    pub active_terminal:  TerminalId,
    pub url_input:        String,
    pub browser_url:      String,
    pub screenshot_bytes: Option<Vec<u8>>,
    pub browser_loading:  bool,
    pub browser_error:    Option<String>,
    // ── prompt response ───────────────────────────────────────────────────────
    /// The Claude question currently being surfaced next to the active
    /// Claude tab (if any). Rendered inline to the right of the terminal.
    pub pending_prompt:        Option<PendingPrompt>,
    /// Which Claude terminal triggered the current prompt.
    pub prompt_source_terminal: Option<TerminalId>,
    /// Fingerprint of the last prompt the user already answered — prevents
    /// the response pane from re-opening on the same prompt before Claude moves on.
    pub last_handled_prompt:   Option<String>,
    /// When the surfaced prompt first stopped being detected. Showing the
    /// response pane shrinks the terminal, which makes Claude repaint, and a
    /// mid-repaint frame momentarily lacks the prompt — tearing the pane down
    /// on that single frame would resize the terminal again and flicker
    /// forever. We debounce: only drop the prompt once it's been gone past
    /// [`PROMPT_CLEAR_DELAY`].
    pub prompt_lost_at:        Option<std::time::Instant>,
    // ── CDP (interactive browser) ─────────────────────────────────────────────
    /// Port that the Chromium CDP instance listens on (9200 + session_id).
    pub cdp_port:         u16,
    /// True once Chromium is confirmed reachable on `cdp_port`.
    pub cdp_active:       bool,
    /// Most-recent cursor position over the screenshot image (in page pixels).
    pub browser_mouse_pos: Option<(f32, f32)>,
    // ── database side panel ───────────────────────────────────────────────────
    pub db_panel:  DbPanel,
    /// Which tab is active in the center (Editor) pane — lets the user flip
    /// between the file editor and the database query/results without
    /// changing the pane's dimensions.
    pub center_tab: CenterTab,
    /// Right-docked plugin panel state (visibility + active tab).
    pub plugin_panel: PluginPanel,
    /// Shell terminals owned by the Terminals plugin tab — independent of
    /// the Claude `terminals` list above.
    pub terminals_plugin: TerminalsState,
}

impl Session {
    pub fn new(
        id:          usize,
        name:        Option<String>,
        kind:        SessionKind,
        first_term:  TerminalId,
        working_dir: PathBuf,
    ) -> Self {
        let layout = match kind {
            SessionKind::Terminal => Layout::new_terminal(),
            SessionKind::Browser  => Layout::new_browser(),
        };
        let dir = Some(working_dir.as_path());
        let panes = vec![
            AgentPane::Terminal(spawn_agent_pane(first_term, dir)),
        ];
        let mut file_browser = FileBrowserState::new();
        file_browser.navigate(working_dir.clone());

        Self {
            id,
            name:             name
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("Session {id}")),
            kind,
            working_dir:      working_dir.clone(),
            layout,
            file_browser_collapsed: false,
            pinned:           false,
            file_browser,
            editor:           EditorState::new(),
            active_terminal:  first_term,
            panes,
            url_input:        String::from("http://localhost:3000"),
            browser_url:      String::new(),
            screenshot_bytes: None,
            browser_loading:  false,
            browser_error:    None,
            pending_prompt:         None,
            prompt_source_terminal: None,
            last_handled_prompt:    None,
            prompt_lost_at:         None,
            cdp_port:         (9200_u16).saturating_add(id as u16),
            cdp_active:       false,
            browser_mouse_pos: None,
            db_panel:          DbPanel::default(),
            center_tab:        CenterTab::Editor,
            plugin_panel:      PluginPanel::default(),
            terminals_plugin:  TerminalsState::new(),
        }
    }

    /// The underlying PTY terminal for `id`, when that pane is a Build pane.
    pub fn terminal_mut(&mut self, id: TerminalId) -> Option<&mut TerminalPane> {
        self.panes.iter_mut().find(|p| p.id() == id).and_then(|p| p.as_terminal_mut())
    }

    /// The chat state for `id`, when that pane is a Research/Chat pane.
    pub fn chat_mut(&mut self, id: TerminalId) -> Option<&mut crate::agent::ChatPane> {
        self.panes.iter_mut().find(|p| p.id() == id).and_then(|p| p.as_chat_mut())
    }

    /// Add an agent pane of the given `kind` and return its id. `Build` spawns
    /// a `claude` PTY; `Research`/`Chat` create a native chat pane that talks
    /// to the Messages API (no process spawned).
    pub fn add_agent(&mut self, id: TerminalId, kind: AgentKind) -> TerminalId {
        if kind.is_agentic() {
            let dir = Some(self.working_dir.as_path());
            self.panes.push(AgentPane::Terminal(spawn_agent_pane(id, dir)));
        } else {
            self.panes.push(AgentPane::Chat(crate::agent::ChatPane::new(id, kind)));
        }
        id
    }

    /// Close a Claude terminal owned by this session. Refuses to remove the
    /// last one (a session always has at least one Claude tab). Re-targets
    /// `active_terminal` / `center_tab` and clears any pending prompt sourced
    /// from the closed terminal. Returns true if the terminal was removed.
    pub fn close_claude_terminal(&mut self, id: TerminalId) -> bool {
        if self.panes.len() <= 1 {
            return false;
        }
        let Some(pos) = self.panes.iter().position(|p| p.id() == id) else {
            return false;
        };
        self.panes.remove(pos);

        // Pick the neighbour that visually replaces the closed tab — prefer
        // the one to the right (now at the same index), falling back to the
        // last remaining tab when we removed the rightmost one.
        let neighbour = self.panes.get(pos)
            .or_else(|| self.panes.last())
            .map(|p| p.id())
            .expect("panes is non-empty after the early return above");

        if self.active_terminal == id {
            self.active_terminal = neighbour;
        }
        if self.center_tab == CenterTab::Claude(id) {
            self.center_tab = CenterTab::Claude(neighbour);
        }
        if self.prompt_source_terminal == Some(id) {
            self.pending_prompt         = None;
            self.prompt_source_terminal = None;
            self.last_handled_prompt    = None;
            self.prompt_lost_at         = None;
        }
        true
    }

    #[allow(dead_code)]
    pub fn terminal(&self, id: TerminalId) -> Option<&TerminalPane> {
        self.panes.iter().find(|p| p.id() == id).and_then(|p| p.as_terminal())
    }
}
