//! Agent panes — the per-tab unit shown in the center pane's agent strip.
//!
//! A pane is one of three modes, all Claude-backed:
//!   - `Build`    — Claude Code running in a PTY (`TerminalPane`)
//!   - `Research` — Claude chat (Opus 4.8), headless `claude -p`
//!   - `Chat`     — Claude chat (Sonnet 4.6), headless `claude -p`
//!
//! `Build` panes run the `claude` CLI in a PTY ([`AgentPane::Terminal`]).
//! `Research`/`Chat` panes are native [`ChatPane`]s that drive the same `claude`
//! CLI in headless mode for streamed Q&A (see [`crate::chat_api`]) — so the
//! whole app authenticates through one path, the Claude Code login.
//!
//! The split is the *interface*, not the capability: a chat pane runs in the
//! session's project directory with real tools, so it can look through the
//! tree and make changes just like a Build pane — it just talks instead of
//! taking over a terminal. How far its tools reach is the user's choice (see
//! [`crate::chat_api::ChatAccess`]).

use iced::widget::text_editor;

use crate::terminal::{TerminalId, TerminalPane};

/// Which kind of agent a pane hosts. Drives the command/transport (PTY vs
/// native API), the model, and — later — the prompt-detection strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentKind {
    /// Agentic Claude Code, run as `claude` in a PTY.
    #[default]
    Build,
    /// Claude chat for research/analysis (Opus 4.8), tools included.
    Research,
    /// Claude chat for quick conversation (Sonnet 4.6), tools included.
    Chat,
}

impl AgentKind {
    pub const ALL: &'static [AgentKind] =
        &[AgentKind::Build, AgentKind::Research, AgentKind::Chat];

    /// Positional tab-label prefix ("Build 1", "Research 2", …).
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Build    => "Build",
            AgentKind::Research => "Research",
            AgentKind::Chat     => "Chat",
        }
    }

    /// Stable token for persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Build    => "build",
            AgentKind::Research => "research",
            AgentKind::Chat     => "chat",
        }
    }

    /// Longer label for the "+ Agent" picker, describing the mode.
    pub fn menu_label(self) -> &'static str {
        match self {
            AgentKind::Build    => "Build (Claude Code)",
            AgentKind::Research => "Research (Opus)",
            AgentKind::Chat     => "Chat (Sonnet)",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "build"    => Some(AgentKind::Build),
            "research" => Some(AgentKind::Research),
            "chat"     => Some(AgentKind::Chat),
            _          => None,
        }
    }

    /// Whether this mode drives Claude Code through a PTY rather than a native
    /// chat pane. Both reach the project's files; this is about the interface
    /// (a terminal vs a transcript), not about what the model may do.
    pub fn is_agentic(self) -> bool {
        matches!(self, AgentKind::Build)
    }

    /// Model ID for the chat modes. `Build` returns `None` — it
    /// runs the `claude` CLI, which selects its own model.
    pub fn model(self) -> Option<&'static str> {
        match self {
            AgentKind::Build    => None,
            AgentKind::Research => Some("claude-opus-4-8"),
            AgentKind::Chat     => Some("claude-sonnet-4-6"),
        }
    }
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.menu_label())
    }
}

// ── AI tool selection (Build panes) ──────────────────────────────────────────

/// One selectable model for an [`AiTool`]. A static table entry: the label
/// shown in the picker, a stable `token` for persistence, and the `--model`
/// argument to pass (`None` omits the flag and lets the tool pick its default).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolModel {
    pub label:   &'static str,
    pub token:   &'static str,
    /// Value for the tool's `--model` flag, or `None` to omit it entirely.
    pub cli_arg: Option<&'static str>,
}

impl Default for ToolModel {
    /// The tool-agnostic "let the CLI choose" model: token `"default"`, no
    /// `--model` flag. Matches the first entry of every tool's model list, so
    /// plain shells and pre-tool save files land on a sensible model.
    fn default() -> Self {
        ToolModel { label: "Default", token: "default", cli_arg: None }
    }
}

/// Claude Code's model list — the source of truth for the picker, the spawn
/// path, and persistence (each entry's `token` is the stable persisted id).
const CLAUDE_MODELS: &[ToolModel] = &[
    ToolModel { label: "Default",   token: "default",   cli_arg: None },
    ToolModel { label: "Fable 5",   token: "fable-5",   cli_arg: Some("claude-fable-5") },
    ToolModel { label: "Opus 4.8",  token: "opus-4-8",  cli_arg: Some("claude-opus-4-8") },
    ToolModel { label: "Sonnet 5",  token: "sonnet-5",  cli_arg: Some("claude-sonnet-5") },
    ToolModel { label: "Haiku 4.5", token: "haiku-4-5", cli_arg: Some("claude-haiku-4-5") },
];

/// Windsurf ("Devin for Terminal") model list. `devin --model <id>` selects the
/// model; the public docs don't enumerate exact ids yet, so this seeds the set
/// from the names in the launch materials (the `opus` alias plus Windsurf's own
/// SWE model and a GPT option) and `Default` to omit the flag. Refine once
/// `devin` confirms the values it accepts.
const WINDSURF_MODELS: &[ToolModel] = &[
    ToolModel { label: "Default", token: "default", cli_arg: None },
    ToolModel { label: "Opus",    token: "opus",    cli_arg: Some("opus") },
    ToolModel { label: "GPT-5.5", token: "gpt-5-5", cli_arg: Some("gpt-5.5") },
    ToolModel { label: "SWE-1.6", token: "swe-1-6", cli_arg: Some("swe-1.6") },
];

/// Which AI CLI drives a `Build` (PTY) agent pane. Every tool is launched as its
/// own binary in a [`TerminalPane`], exactly like Claude Code — the only
/// per-tool differences are the binary name, the model list, and how a model
/// becomes a `--model` argument (both current tools happen to share that flag).
///
/// Adding a tool = add a variant plus its `match` arms below; the "+ Agent"
/// picker and the Profile tab are generated from [`AiTool::ALL`], so they pick
/// it up automatically. Only agentic (PTY) tools live here; the native
/// Research/Chat panes remain Claude-only (see [`AgentKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AiTool {
    /// Anthropic's Claude Code (`claude`).
    #[default]
    Claude,
    /// Windsurf's terminal agent — "Devin for Terminal" (`devin`). Same login
    /// model as Claude Code: `devin auth login` signs in with the user's
    /// existing Windsurf subscription, so frms never handles credentials.
    Windsurf,
}

impl AiTool {
    pub const ALL: &'static [AiTool] = &[AiTool::Claude, AiTool::Windsurf];

    /// Human-readable name for the picker and Profile tab.
    pub fn label(self) -> &'static str {
        match self {
            AiTool::Claude   => "Claude Code",
            AiTool::Windsurf => "Windsurf",
        }
    }

    /// The executable frms spawns in the PTY.
    pub fn binary(self) -> &'static str {
        match self {
            AiTool::Claude   => "claude",
            AiTool::Windsurf => "devin",
        }
    }

    /// Stable token for persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            AiTool::Claude   => "claude",
            AiTool::Windsurf => "windsurf",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "claude"   => Some(AiTool::Claude),
            "windsurf" => Some(AiTool::Windsurf),
            _          => None,
        }
    }

    /// The tool's selectable models, most-default first.
    pub fn models(self) -> &'static [ToolModel] {
        match self {
            AiTool::Claude   => CLAUDE_MODELS,
            AiTool::Windsurf => WINDSURF_MODELS,
        }
    }

    /// The default model (first entry — omits `--model`).
    pub fn default_model(self) -> &'static ToolModel {
        &self.models()[0]
    }

    /// Look up one of the tool's models by its persisted token.
    pub fn model_by_token(self, token: &str) -> Option<&'static ToolModel> {
        self.models().iter().find(|m| m.token == token)
    }

    /// The line to type into a running session to switch it to `model` without
    /// restarting the CLI, or `None` when the tool has no such command — those
    /// panes have to be respawned with a new `--model` (losing their context),
    /// so prefer this whenever a tool offers it.
    ///
    /// Claude Code takes `/model <name>` in its REPL and accepts both the
    /// literal model ids we pass as `--model` and the alias `default`.
    /// Windsurf's `devin` has no documented equivalent yet, so it respawns.
    pub fn model_command(self, model: ToolModel) -> Option<String> {
        match self {
            AiTool::Claude   => Some(format!("/model {}", model.cli_arg.unwrap_or("default"))),
            AiTool::Windsurf => None,
        }
    }

    /// Whether switching model on a live pane of this tool keeps the session
    /// (see [`AiTool::model_command`]). Drives the wording in the model dialog.
    pub fn switches_model_in_place(self) -> bool {
        self.model_command(*self.default_model()).is_some()
    }

    /// Whether the tool's binary is on `PATH`.
    pub fn on_path(self) -> bool {
        crate::port::shell::on_path(self.binary())
    }

    /// One-shot shell command that installs the tool. Run by the Profile tab's
    /// "Install" button (see [`crate::port::shell::install_shell`]).
    pub fn install_command(self) -> &'static str {
        match self {
            AiTool::Claude   => "npm install -g @anthropic-ai/claude-code",
            AiTool::Windsurf => "curl -fsSL https://cli.devin.ai/install.sh | bash",
        }
    }

    /// One-line reminder of how to sign in once the tool is installed. Both CLIs
    /// own their own auth against the user's subscription — no API key.
    pub fn login_hint(self) -> &'static str {
        match self {
            AiTool::Claude =>
                "run 'claude' once to sign in (a Claude Pro/Max subscription, no API key).",
            AiTool::Windsurf =>
                "run 'devin auth login' once to sign in with your existing Windsurf subscription.",
        }
    }
}

impl std::fmt::Display for AiTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ── chat panes ──────────────────────────────────────────────────────────────

/// Who authored a chat turn — drives how the message is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// One completed turn in a chat transcript. The body is held as a
/// `text_editor::Content` (not a plain `String`) so the transcript can render
/// it as a read-only, *selectable* editor — letting the user highlight a span
/// and turn it into a note, the same affordance the Build panes have. The raw
/// text is still available via `content.text()`.
#[derive(Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: text_editor::Content,
}

/// A native Claude chat pane — `Research` or `Chat`. Holds the
/// transcript, the in-progress input draft, and the streaming state for the
/// reply currently being generated. The actual API call lives in
/// [`crate::chat_api`]; this struct is just the view/model state.
pub struct ChatPane {
    pub id:   TerminalId,
    pub kind: AgentKind,
    /// User-assigned tab name; `None` falls back to the positional label.
    pub name: Option<String>,
    /// Completed turns, oldest first.
    pub messages: Vec<ChatMessage>,
    /// The text the user is composing but hasn't sent yet. A multi-line
    /// editor so long messages wrap and scroll instead of running off one row.
    pub input: text_editor::Content,
    /// True while a reply is being streamed from the API.
    pub streaming: bool,
    /// Assistant text accumulated so far for the in-flight reply. Promoted to
    /// a `ChatMessage` once the stream ends.
    pub pending: String,
    /// Tools the model has reached for during the in-flight reply, oldest
    /// first ("Read  src/app.rs", "Bash  cargo test"). A turn spent working
    /// through the project produces no text for a long stretch; these lines are
    /// what keep the pane from looking hung. Cleared at the start of each turn.
    pub activity: Vec<String>,
    /// Last error from a failed request, shown as a banner until the next send.
    pub error: Option<String>,
    /// `claude` headless session id, captured from the first reply and passed as
    /// `--resume` on later turns so the CLI keeps the conversation going. `None`
    /// until the first turn completes.
    pub session_id: Option<String>,
    /// A reply landed while this pane wasn't on screen. Lights the tab/session
    /// attention highlight — the chat equivalent of a Build pane's
    /// `waiting_on_user`. Cleared as soon as the pane is shown.
    pub unread: bool,
    /// Keep the transcript pinned to its newest text. Set while the view is at
    /// (or near) the bottom, cleared once the user scrolls up to read back, so
    /// streaming text doesn't yank them away from what they're reading.
    pub follow_tail: bool,
    /// Model override picked in the "Change model" dialog, chosen from Claude
    /// Code's model list (chat panes are Claude-only). The `Default` entry
    /// carries no `cli_arg` and leaves the pane on its kind's model — Opus for
    /// `Research`, Sonnet for `Chat`. Changing it takes effect on the next
    /// turn; `--resume` carries the conversation over, so nothing is lost.
    pub model: ToolModel,
    /// Open find-in-conversation, or `None` when the find bar is closed.
    pub find: Option<ChatFind>,
}

/// Find-in-conversation over a chat transcript. Hits are whole *messages*
/// rather than character offsets: a transcript bubble is a read-only text
/// editor with no way to paint a span, so the match is shown by picking the
/// bubble out with an accent border and scrolling to it.
#[derive(Debug, Clone, Default)]
pub struct ChatFind {
    pub query:   String,
    /// Indices into `messages` that contain the query, oldest first.
    pub hits:    Vec<usize>,
    /// Index into `hits` of the current one.
    pub current: usize,
}

impl ChatFind {
    /// How this message should be marked: `None` if it isn't a hit, otherwise
    /// whether it's the *current* hit.
    pub fn mark(&self, message: usize) -> Option<bool> {
        let at = self.hits.iter().position(|h| *h == message)?;
        Some(at == self.current)
    }
}

impl ChatPane {
    /// Open the find bar, refreshing hits for whatever query it last held.
    pub fn find_open(&mut self) {
        let query = self.find.take().map(|f| f.query).unwrap_or_default();
        self.find_query(query);
    }

    pub fn find_close(&mut self) {
        self.find = None;
    }

    /// Re-run the search for `query`, landing on the newest hit — in a
    /// conversation, "where did that come up" usually means the last time.
    pub fn find_query(&mut self, query: String) {
        let needle = query.to_lowercase();
        let hits: Vec<usize> = if needle.is_empty() {
            Vec::new()
        } else {
            self.messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m.content.text().to_lowercase().contains(&needle))
                .map(|(i, _)| i)
                .collect()
        };
        let current = hits.len().saturating_sub(1);
        self.find = Some(ChatFind { query, hits, current });
    }

    /// Step to the next / previous hit, wrapping at either end.
    pub fn find_step(&mut self, forward: bool) {
        let Some(prev) = self.find.take() else { return };
        // Refresh first — replies may have landed since the query was typed —
        // then step from wherever the hit we were on ended up.
        let anchor = prev.hits.get(prev.current).copied();
        self.find_query(prev.query);
        let Some(find) = self.find.as_mut() else { return };
        let n = find.hits.len();
        if n == 0 {
            return;
        }
        let at = anchor
            .and_then(|m| find.hits.iter().position(|h| *h == m))
            .unwrap_or(n - 1);
        find.current = if forward { (at + 1) % n } else { (at + n - 1) % n };
    }

    pub fn new(id: TerminalId, kind: AgentKind) -> Self {
        Self {
            id,
            kind,
            name:      None,
            messages:  Vec::new(),
            input:     text_editor::Content::new(),
            streaming:  false,
            pending:    String::new(),
            activity:   Vec::new(),
            error:      None,
            session_id: None,
            unread:      false,
            follow_tail: true,
            model:       ToolModel::default(),
            find:        None,
        }
    }

    /// The model this pane sends to: the user's override when they picked one,
    /// otherwise the kind's own model. Falls back to Opus for the (unexpected)
    /// `Build` case so callers always get a usable id.
    pub fn model(&self) -> &'static str {
        self.model.cli_arg
            .or_else(|| self.kind.model())
            .unwrap_or("claude-opus-4-8")
    }

    /// Append a completed turn, wrapping its body in a `text_editor::Content`
    /// so the transcript can render it as a selectable, read-only editor.
    pub fn push_message(&mut self, role: ChatRole, text: &str) {
        self.messages.push(ChatMessage {
            role,
            content: text_editor::Content::with_text(text),
        });
    }
}

// ── AgentPane ───────────────────────────────────────────────────────────────

/// A `Build` pane whose CLI hasn't been launched yet.
///
/// Build panes are created in this state and only spawn their PTY once the
/// pane is actually displayed (see [`crate::session::Session::spawn_pending`]).
/// Loading frms with several restored sessions would otherwise start one
/// `claude` per agent tab before the user has looked at any of them.
///
/// It carries everything the pane needs to describe itself in the meantime —
/// the tab strip, the model picker, and persistence all read a pending pane
/// exactly like a live one, so a session that is never opened still saves and
/// restores with the tool, model, and name the user chose.
pub struct PendingPane {
    pub id:    TerminalId,
    pub tool:  AiTool,
    pub model: ToolModel,
    pub name:  Option<String>,
}

/// One agent tab: a `Build` PTY pane, a `Build` pane not yet spawned, or a
/// native chat pane.
pub enum AgentPane {
    Terminal(TerminalPane),
    Pending(PendingPane),
    Chat(ChatPane),
}

impl AgentPane {
    pub fn id(&self) -> TerminalId {
        match self {
            AgentPane::Terminal(t) => t.id,
            AgentPane::Pending(p)  => p.id,
            AgentPane::Chat(c)     => c.id,
        }
    }

    pub fn kind(&self) -> AgentKind {
        match self {
            AgentPane::Terminal(_) | AgentPane::Pending(_) => AgentKind::Build,
            AgentPane::Chat(c)     => c.kind,
        }
    }

    /// True while this is a `Build` pane whose CLI has yet to be launched.
    pub fn is_pending(&self) -> bool {
        matches!(self, AgentPane::Pending(_))
    }

    /// Which AI CLI a `Build` pane runs. Chat panes are Claude-only, so they
    /// report [`AiTool::Claude`].
    pub fn tool(&self) -> AiTool {
        match self {
            AgentPane::Terminal(t) => t.agent_tool,
            AgentPane::Pending(p)  => p.tool,
            AgentPane::Chat(_)     => AiTool::Claude,
        }
    }

    /// The model this pane is set to: what a `Build` pane's CLI was launched
    /// with (or last switched to), or a chat pane's override. Both persist as
    /// the entry's `token`.
    pub fn model_choice(&self) -> ToolModel {
        match self {
            AgentPane::Terminal(t) => t.agent_model,
            AgentPane::Pending(p)  => p.model,
            AgentPane::Chat(c)     => c.model,
        }
    }

    /// How this pane's current model reads in the UI. `Default` spells out
    /// what that resolves to for a chat pane (whose kind fixes the model);
    /// a Build pane's default is the CLI's own choice, so it stays "Default".
    pub fn model_label(&self) -> String {
        let choice = self.model_choice();
        match self {
            AgentPane::Chat(c) if choice.cli_arg.is_none() => {
                let implied = match c.kind {
                    AgentKind::Research => "Opus 4.8",
                    AgentKind::Chat     => "Sonnet 4.6",
                    AgentKind::Build    => "Default",
                };
                format!("Default ({implied})")
            }
            _ => choice.label.to_string(),
        }
    }

    /// Whether a model from `tool`'s list can be applied to this pane. A Build
    /// pane only takes its own tool's models (a `devin` model id means nothing
    /// to `claude`); chat panes are Claude-only.
    pub fn accepts_models_from(&self, tool: AiTool) -> bool {
        match self {
            AgentPane::Terminal(t) => t.agent_tool == tool,
            AgentPane::Pending(p)  => p.tool == tool,
            AgentPane::Chat(_)     => tool == AiTool::Claude,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            AgentPane::Terminal(t) => t.name.as_deref(),
            AgentPane::Pending(p)  => p.name.as_deref(),
            AgentPane::Chat(c)     => c.name.as_deref(),
        }
    }

    pub fn set_name(&mut self, name: Option<String>) {
        match self {
            AgentPane::Terminal(t) => t.name = name,
            AgentPane::Pending(p)  => p.name = name,
            AgentPane::Chat(c)     => c.name = name,
        }
    }

    /// True when this pane has something the user hasn't seen: a Build pane
    /// stopped on a question, or a chat pane whose reply arrived off-screen.
    /// Drives the tab/session attention highlight and Ctrl+Shift+↑/↓ cycling.
    /// A pane that was never opened has nothing to report — its CLI hasn't run.
    pub fn needs_attention(&self) -> bool {
        match self {
            AgentPane::Terminal(t) => t.waiting_on_user,
            AgentPane::Pending(_)  => false,
            AgentPane::Chat(c)     => c.unread,
        }
    }

    /// The underlying PTY terminal, when this is a Build pane that has spawned.
    /// A `Pending` pane returns `None`, which is what keeps its (non-existent)
    /// PTY out of `subscription()` until it starts.
    pub fn as_terminal(&self) -> Option<&TerminalPane> {
        match self {
            AgentPane::Terminal(t) => Some(t),
            AgentPane::Pending(_) | AgentPane::Chat(_) => None,
        }
    }

    pub fn as_terminal_mut(&mut self) -> Option<&mut TerminalPane> {
        match self {
            AgentPane::Terminal(t) => Some(t),
            AgentPane::Pending(_) | AgentPane::Chat(_) => None,
        }
    }

    /// The chat state, when this is a Research/Chat pane.
    pub fn as_chat(&self) -> Option<&ChatPane> {
        match self {
            AgentPane::Chat(c) => Some(c),
            AgentPane::Terminal(_) | AgentPane::Pending(_) => None,
        }
    }

    pub fn as_chat_mut(&mut self) -> Option<&mut ChatPane> {
        match self {
            AgentPane::Chat(c) => Some(c),
            AgentPane::Terminal(_) | AgentPane::Pending(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_pane_alerts_only_while_unread() {
        let mut pane = AgentPane::Chat(ChatPane::new(1, AgentKind::Research));
        assert!(!pane.needs_attention(), "a fresh chat pane has nothing to show");

        // A reply that landed off-screen lights the tab…
        pane.as_chat_mut().unwrap().unread = true;
        assert!(pane.needs_attention());

        // …and showing the pane clears it.
        pane.as_chat_mut().unwrap().unread = false;
        assert!(!pane.needs_attention());
    }

    #[test]
    fn chat_find_lands_on_the_newest_hit_and_walks_them() {
        let mut pane = ChatPane::new(7, AgentKind::Chat);
        pane.push_message(ChatRole::User,      "check the PTY resize path");
        pane.push_message(ChatRole::Assistant, "nothing to do there");
        pane.push_message(ChatRole::User,      "and the resize on Windows?");

        pane.find_query("RESIZE".into());
        let f = pane.find.as_ref().unwrap();
        assert_eq!(f.hits, vec![0, 2], "case-insensitive, oldest first");
        assert_eq!(f.current, 1, "starts on the most recent hit");
        assert_eq!(f.mark(2), Some(true));
        assert_eq!(f.mark(0), Some(false));
        assert_eq!(f.mark(1), None);

        // Forward from the last hit wraps to the first…
        pane.find_step(true);
        assert_eq!(pane.find.as_ref().unwrap().current, 0);
        // …and back again.
        pane.find_step(false);
        assert_eq!(pane.find.as_ref().unwrap().current, 1);
    }

    #[test]
    fn chat_find_steps_from_the_same_hit_after_a_new_reply_lands() {
        let mut pane = ChatPane::new(8, AgentKind::Chat);
        pane.push_message(ChatRole::User,      "resize one");
        pane.push_message(ChatRole::Assistant, "no resize needed");
        pane.find_query("resize".into());
        pane.find_step(false); // park on message 0

        // A reply arrives that also matches: the refreshed search must not lose
        // where the user was.
        pane.push_message(ChatRole::Assistant, "resize again");
        pane.find_step(true);
        let f = pane.find.as_ref().unwrap();
        assert_eq!(f.hits, vec![0, 1, 2]);
        assert_eq!(f.current, 1, "stepped forward from message 0, not from the end");
    }

    #[test]
    fn chat_pane_model_override_beats_its_kind_default() {
        let mut pane = ChatPane::new(3, AgentKind::Chat);
        // No override → the kind's own model.
        assert_eq!(pane.model(), "claude-sonnet-4-6");

        // A picked model wins…
        pane.model = *AiTool::Claude.model_by_token("opus-4-8").unwrap();
        assert_eq!(pane.model(), "claude-opus-4-8");

        // …and going back to Default restores the kind's model.
        pane.model = ToolModel::default();
        assert_eq!(pane.model(), "claude-sonnet-4-6");
    }

    #[test]
    fn claude_switches_model_in_place_others_respawn() {
        let opus = *AiTool::Claude.model_by_token("opus-4-8").unwrap();
        assert_eq!(
            AiTool::Claude.model_command(opus).as_deref(),
            Some("/model claude-opus-4-8"),
        );
        // The Default entry has no `--model` id; `/model default` is the alias.
        assert_eq!(
            AiTool::Claude.model_command(ToolModel::default()).as_deref(),
            Some("/model default"),
        );
        assert!(AiTool::Claude.switches_model_in_place());

        // Windsurf has no in-session command, so its panes have to respawn.
        assert!(AiTool::Windsurf.model_command(ToolModel::default()).is_none());
        assert!(!AiTool::Windsurf.switches_model_in_place());
    }

    #[test]
    fn chat_pane_default_model_label_names_the_implied_model() {
        let pane = AgentPane::Chat(ChatPane::new(1, AgentKind::Research));
        assert_eq!(pane.model_label(), "Default (Opus 4.8)");
    }

    #[test]
    fn chat_pane_follows_its_tail_by_default() {
        let pane = ChatPane::new(7, AgentKind::Chat);
        assert!(pane.follow_tail);
        assert!(!pane.unread);
    }

    /// A pending pane must stay invisible to the PTY plumbing — `as_terminal`
    /// returning `None` is what keeps it out of `subscription()`, so no reader
    /// is started for a tab the user never opened.
    #[test]
    fn pending_pane_has_no_terminal_and_raises_no_alert() {
        let pane = AgentPane::Pending(PendingPane {
            id: 4, tool: AiTool::Claude, model: ToolModel::default(), name: None,
        });
        assert!(pane.is_pending());
        assert!(pane.as_terminal().is_none());
        assert!(pane.as_chat().is_none());
        assert!(!pane.needs_attention(), "a CLI that never ran can't be waiting");
        assert_eq!(pane.kind(), AgentKind::Build);
    }

    /// Persistence reads panes through these accessors, so a session left
    /// unopened for the whole run still saves the tool, model, and name the
    /// user picked — and comes back on them next launch.
    #[test]
    fn pending_pane_reports_its_tool_model_and_name() {
        let opus = *AiTool::Claude.model_by_token("opus-4-8").unwrap();
        let mut pane = AgentPane::Pending(PendingPane {
            id: 9, tool: AiTool::Windsurf, model: opus, name: None,
        });
        pane.set_name(Some("planning".to_string()));

        assert_eq!(pane.tool(), AiTool::Windsurf);
        assert_eq!(pane.model_choice().token, "opus-4-8");
        assert_eq!(pane.name(), Some("planning"));
        assert_eq!(pane.model_label(), "Opus 4.8");
        // Model lists are per-tool: a Claude model means nothing to `devin`.
        assert!(pane.accepts_models_from(AiTool::Windsurf));
        assert!(!pane.accepts_models_from(AiTool::Claude));
    }
}
