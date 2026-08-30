use std::path::PathBuf;

use crate::agent::{AgentKind, AgentPane, AiTool, PendingPane, ToolModel};
use crate::claude_prompt::PendingPrompt;
use crate::db_panel::DbPanel;
use crate::docs::DocsState;
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

/// Whether the `claude` CLI is installed (on `PATH`). Both Build panes and the
/// native chat panes drive it, so the Profile tab surfaces this to the user.
pub fn claude_on_path() -> bool {
    crate::port::shell::on_path("claude")
}

/// Spawn a Build agent pane in a PTY, running `tool`'s CLI (`claude`, `devin`, …).
///
/// When the tool's binary is on `PATH`, run it directly. When it isn't — common
/// right after an `.rpm`/`.deb` install, since these CLIs ship via npm/curl, not
/// as distro packages — fall back to an interactive shell that first prints
/// install guidance for that specific tool. That keeps a fresh machine from
/// crashing at launch (the previous code `panic!`ed on the failed spawn) and
/// tells the user exactly how to get the tool, while leaving them a usable shell
/// in the pane. Re-running an agent pane after installing picks the CLI up.
fn spawn_agent_pane(
    id:    TerminalId,
    dir:   Option<&std::path::Path>,
    tool:  AiTool,
    model: ToolModel,
) -> TerminalPane {
    if tool.on_path() {
        // Pin the model with `--model <id>` when the user picked one; the
        // default entry has `cli_arg: None` and omits the flag, letting the CLI
        // choose. `cli_arg` is tool-native, so this works for every tool.
        let args: Vec<String> = match model.cli_arg {
            Some(id) => vec!["--model".to_string(), id.to_string()],
            None     => Vec::new(),
        };
        if let Ok(mut pane) = TerminalPane::spawn_args(id, tool.binary(), &args, &[], dir) {
            pane.agent_tool  = tool;
            pane.agent_model = model;
            pane.agent_cli_ready = true;
            return pane;
        }
    }

    let bin  = tool.binary();
    let lines_owned = [
        format!("{} CLI ('{}') was not found on your PATH.", tool.label(), bin),
        "Agent panes use it to run the AI CLI. To install it:".to_string(),
        format!("    {}", tool.install_command()),
        "Then restart frms or open a new agent pane.".to_string(),
        String::new(),
    ];
    let lines: Vec<&str> = lines_owned.iter().map(String::as_str).collect();
    let (prog, args) = crate::port::shell::guidance_shell(&lines);
    let mut pane = TerminalPane::spawn_args(id, &prog, &args, &[], dir)
        .unwrap_or_else(|e| panic!("Failed to spawn fallback shell for agent pane {id}: {e}"));
    // Remember the user's tool/model choice so it persists and is applied once
    // the CLI becomes available on a later respawn.
    pane.agent_tool  = tool;
    pane.agent_model = model;
    pane
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
    /// Search index over this project's `_specs` / `_docs` folders, backing the
    /// Docs toolbox tab. Per-session because the folders are project-relative.
    /// Populated lazily — the scan runs when the tab is opened, not at startup,
    /// so sessions the user never asks for docs on never touch the disk.
    pub docs:             DocsState,
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
    /// `PROMPT_CLEAR_DELAY` — or past the much longer `PROMPT_STALE_MAX` while
    /// the terminal still shows *a* menu, since then the parse failed on a
    /// half-drawn frame rather than the question going away.
    pub prompt_lost_at:        Option<std::time::Instant>,
    /// A different reading of the prompt than the one on screen, and when we
    /// first read it. Most such readings are half-drawn repaint frames, so the
    /// panel only adopts one that has held still (`PROMPT_SWAP_SETTLE`) rather
    /// than redrawing its diff on every chunk.
    pub prompt_swap:           Option<(String, std::time::Instant)>,
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
    /// Create a session. Its first agent pane runs `pane0_tool` at that tool's
    /// default model — new sessions pass the user's `default_tool` so the first
    /// pane honors their choice; the restore path passes the default (Claude)
    /// and respawns pane[0] itself from the persisted tool/model.
    pub fn new(
        id:          usize,
        name:        Option<String>,
        kind:        SessionKind,
        first_term:  TerminalId,
        working_dir: PathBuf,
        pane0_tool:  AiTool,
    ) -> Self {
        let layout = match kind {
            SessionKind::Terminal => Layout::new_terminal(),
            SessionKind::Browser  => Layout::new_browser(),
        };
        // Unspawned: the CLI starts the first time the user opens this tab.
        let panes = vec![
            AgentPane::Pending(PendingPane {
                id:    first_term,
                tool:  pane0_tool,
                model: *pane0_tool.default_model(),
                name:  None,
            }),
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
            docs:             DocsState::default(),
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
            prompt_swap:            None,
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

    /// Add an agent pane of the given `kind` and return its id. `Build` records
    /// the AI CLI (`tool`) to run, which [`Self::spawn_pending`] launches in a
    /// PTY once the tab is displayed; `Research`/`Chat` create a native chat
    /// pane that drives the `claude` CLI headless. Neither starts a process
    /// here. `tool` is ignored for the chat kinds, which are Claude-only; they
    /// keep `model` as an override on the model their kind implies.
    pub fn add_agent(&mut self, id: TerminalId, kind: AgentKind, tool: AiTool, model: ToolModel) -> TerminalId {
        if kind.is_agentic() {
            self.panes.push(AgentPane::Pending(PendingPane { id, tool, model, name: None }));
        } else {
            let mut chat = crate::agent::ChatPane::new(id, kind);
            // Chat panes are Claude-only, so a Claude model carries over as an
            // override; anything else leaves them on their kind's own model.
            chat.model = model;
            self.panes.push(AgentPane::Chat(chat));
        }
        id
    }

    /// Launch the CLI for the `Pending` pane `id`, in place — the lazy half of
    /// [`add_agent`](Self::add_agent). Returns true if a process was started.
    ///
    /// Unlike [`respawn_agent`](Self::respawn_agent), this keeps the pane's
    /// existing `TerminalId`: a pending pane has no PTY reader subscribed to
    /// that id yet, so there is no stale reader to strand and nothing pointing
    /// at the pane (active tab, center tab) needs re-pointing. `iced` picks the
    /// new PTY up on the next `subscription()` rebuild.
    ///
    /// Already-live and chat panes are a no-op, so callers can fire this at any
    /// pane that has come on screen without checking what it is first.
    pub fn spawn_pending(&mut self, id: TerminalId) -> bool {
        let Some(pos) = self.panes.iter().position(|p| p.id() == id) else {
            return false;
        };
        let AgentPane::Pending(p) = &self.panes[pos] else {
            return false;
        };
        let (tool, model, name) = (p.tool, p.model, p.name.clone());

        let dir = Some(self.working_dir.as_path());
        let mut pane = AgentPane::Terminal(spawn_agent_pane(id, dir, tool, model));
        pane.set_name(name);
        self.panes[pos] = pane;
        true
    }

    /// Restart the `Build` pane `id` on `tool` at `model`, keeping its tab
    /// position and name. Used when the tool can't switch models in place (see
    /// [`AiTool::model_command`]) — the running CLI is killed and relaunched
    /// with the new `--model`, so its conversation is lost.
    ///
    /// The replacement takes a *new* `TerminalId` on purpose: the PTY reader is
    /// an `iced` subscription keyed by that id, so reusing it would leave the
    /// old (now dead) reader attached and never start one for the new child.
    /// Every reference to the old id — active tab, center tab, pending prompt —
    /// is re-pointed here. Returns false if `id` isn't a Build pane.
    pub fn respawn_agent(
        &mut self,
        id:     TerminalId,
        new_id: TerminalId,
        tool:   AiTool,
        model:  ToolModel,
    ) -> bool {
        let Some(pos) = self.panes.iter().position(|p| p.id() == id) else {
            return false;
        };
        if self.panes[pos].as_terminal().is_none() {
            return false;
        }
        let name = self.panes[pos].name().map(str::to_owned);
        if let Some(t) = self.panes[pos].as_terminal() {
            let _ = t.child.lock().map(|mut c| c.kill());
        }

        let dir = Some(self.working_dir.as_path());
        let mut pane = AgentPane::Terminal(spawn_agent_pane(new_id, dir, tool, model));
        pane.set_name(name);
        self.panes[pos] = pane;

        if self.active_terminal == id {
            self.active_terminal = new_id;
        }
        if self.center_tab == CenterTab::Claude(id) {
            self.center_tab = CenterTab::Claude(new_id);
        }
        // The question that was on screen belonged to the process we just
        // killed, so drop it rather than re-pointing it at the fresh one.
        if self.prompt_source_terminal == Some(id) {
            self.clear_prompt();
        }
        true
    }

    /// Forget the surfaced permission prompt and all of its bookkeeping.
    pub fn clear_prompt(&mut self) {
        self.pending_prompt         = None;
        self.prompt_source_terminal = None;
        self.last_handled_prompt    = None;
        self.prompt_lost_at         = None;
        self.prompt_swap            = None;
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
        // Stopping the CLI is the point of closing a tab — the user wants the
        // resources back. Dropping the pane closes the PTY master, which
        // normally SIGHUPs the child, but "normally" isn't good enough for a
        // process someone explicitly asked to end.
        if let Some(t) = self.panes[pos].as_terminal_mut() {
            if let Ok(mut c) = t.child.lock() {
                let _ = c.kill();
            }
        }
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
            self.clear_prompt();
        }
        true
    }

    #[allow(dead_code)]
    pub fn terminal(&self, id: TerminalId) -> Option<&TerminalPane> {
        self.panes.iter().find(|p| p.id() == id).and_then(|p| p.as_terminal())
    }

    /// The chat state for `id`, when that pane is a Research/Chat pane.
    pub fn chat(&self, id: TerminalId) -> Option<&crate::agent::ChatPane> {
        self.panes.iter().find(|p| p.id() == id).and_then(|p| p.as_chat())
    }
}
