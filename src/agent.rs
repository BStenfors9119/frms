//! Agent panes — the per-tab unit shown in the center pane's agent strip.
//!
//! A pane is one of three modes, all Claude-backed:
//!   - `Build`    — agentic Claude Code running in a PTY (`TerminalPane`)
//!   - `Research` — non-agentic Claude chat (Opus 4.8), headless `claude -p`
//!   - `Chat`     — non-agentic Claude chat (Sonnet 4.6), headless `claude -p`
//!
//! `Build` panes run the `claude` CLI in a PTY ([`AgentPane::Terminal`]).
//! `Research`/`Chat` panes are native [`ChatPane`]s that drive the same `claude`
//! CLI in headless mode for streamed Q&A (see [`crate::chat_api`]) — so the
//! whole app authenticates through one path, the Claude Code login.

use crate::terminal::{TerminalId, TerminalPane};

/// Which kind of agent a pane hosts. Drives the command/transport (PTY vs
/// native API), the model, and — later — the prompt-detection strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AgentKind {
    /// Agentic Claude Code, run as `claude` in a PTY.
    #[default]
    Build,
    /// Non-agentic Claude chat for research/analysis (Opus 4.8).
    Research,
    /// Non-agentic Claude chat for quick conversation (Sonnet 4.6).
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

    /// Whether this mode runs the agentic Claude Code harness (PTY) rather than
    /// a plain chat against the API.
    pub fn is_agentic(self) -> bool {
        matches!(self, AgentKind::Build)
    }

    /// Model ID for the non-agentic chat modes. `Build` returns `None` — it
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

// ── chat panes ──────────────────────────────────────────────────────────────

/// Who authored a chat turn — drives how the message is rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

/// One completed turn in a chat transcript.
#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

/// A native (non-agentic) Claude chat pane — `Research` or `Chat`. Holds the
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
    /// The text the user is composing but hasn't sent yet.
    pub input: String,
    /// True while a reply is being streamed from the API.
    pub streaming: bool,
    /// Assistant text accumulated so far for the in-flight reply. Promoted to
    /// a `ChatMessage` once the stream ends.
    pub pending: String,
    /// Last error from a failed request, shown as a banner until the next send.
    pub error: Option<String>,
    /// `claude` headless session id, captured from the first reply and passed as
    /// `--resume` on later turns so the CLI keeps the conversation going. `None`
    /// until the first turn completes.
    pub session_id: Option<String>,
}

impl ChatPane {
    pub fn new(id: TerminalId, kind: AgentKind) -> Self {
        Self {
            id,
            kind,
            name:      None,
            messages:  Vec::new(),
            input:     String::new(),
            streaming:  false,
            pending:    String::new(),
            error:      None,
            session_id: None,
        }
    }

    /// The model this pane sends to. Falls back to Opus for the (unexpected)
    /// `Build` case so callers always get a usable id.
    pub fn model(&self) -> &'static str {
        self.kind.model().unwrap_or("claude-opus-4-8")
    }
}

// ── AgentPane ───────────────────────────────────────────────────────────────

/// One agent tab: either a `Build` PTY pane or a native chat pane.
pub enum AgentPane {
    Terminal(TerminalPane),
    Chat(ChatPane),
}

impl AgentPane {
    pub fn id(&self) -> TerminalId {
        match self {
            AgentPane::Terminal(t) => t.id,
            AgentPane::Chat(c)     => c.id,
        }
    }

    pub fn kind(&self) -> AgentKind {
        match self {
            AgentPane::Terminal(_) => AgentKind::Build,
            AgentPane::Chat(c)     => c.kind,
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            AgentPane::Terminal(t) => t.name.as_deref(),
            AgentPane::Chat(c)     => c.name.as_deref(),
        }
    }

    pub fn set_name(&mut self, name: Option<String>) {
        match self {
            AgentPane::Terminal(t) => t.name = name,
            AgentPane::Chat(c)     => c.name = name,
        }
    }

    /// True when this pane is stopped awaiting the user. Only Build panes (with
    /// their menu detection) can be waiting; chat panes never are.
    pub fn waiting_on_user(&self) -> bool {
        match self {
            AgentPane::Terminal(t) => t.waiting_on_user,
            AgentPane::Chat(_)     => false,
        }
    }

    /// The underlying PTY terminal, when this is a Build pane.
    pub fn as_terminal(&self) -> Option<&TerminalPane> {
        match self {
            AgentPane::Terminal(t) => Some(t),
            AgentPane::Chat(_)     => None,
        }
    }

    pub fn as_terminal_mut(&mut self) -> Option<&mut TerminalPane> {
        match self {
            AgentPane::Terminal(t) => Some(t),
            AgentPane::Chat(_)     => None,
        }
    }

    /// The chat state, when this is a Research/Chat pane.
    pub fn as_chat(&self) -> Option<&ChatPane> {
        match self {
            AgentPane::Chat(c)     => Some(c),
            AgentPane::Terminal(_) => None,
        }
    }

    pub fn as_chat_mut(&mut self) -> Option<&mut ChatPane> {
        match self {
            AgentPane::Chat(c)     => Some(c),
            AgentPane::Terminal(_) => None,
        }
    }
}
