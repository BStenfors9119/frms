use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::{column, container, image, pane_grid, scrollable, text_editor, text_input};
use iced::widget::pane_grid::Configuration;
use iced::{Element, Length, Subscription, Task, Theme, window};
use iced::keyboard::{self, key};

use crate::agent::{AgentKind, AgentPane, ChatPane, ChatRole};
use crate::cdp;
use crate::chat_api;
use crate::claude_prompt;
use crate::cloud_sql;
use crate::db::{self, DbClient, DbEngine, DbProvider, ForeignKey, QueryResult, RoutineRef, StatementOutcome, TableRef};
use crate::db_panel::{ColRef, Conj, ConnState, Filter, FilterOp};
use crate::file_browser::{DirEdit, FileBrowserState};
use crate::notes::{CategoryChoice, NoteFormat, NoteViewMode, NotesState};
use crate::plugin_panel::{PluginSlot, PluginTab};
use crate::prefs::Prefs;
use crate::stats::{self, ClaudeStats, ProcStats, RefreshInterval};
use crate::theme::{self as theme_mod, FontScale, Mode, Palette, TerminalFontScale, ThemeColors};
use crate::pane::PaneKind;
use crate::session::{Session, SessionKind};
use crate::terminal::{self, TerminalId, TerminalPane};
use crate::ui;

/// How long the surfaced permission prompt must stay undetected before the
/// response pane is torn down. Long enough to ride over the mid-repaint frame
/// that the pane's own resize provokes (which would otherwise flicker), short
/// enough that the pane disappears promptly once the prompt is truly answered.
const PROMPT_CLEAR_DELAY: Duration = Duration::from_millis(400);

/// The same debounce, but for the case where the terminal still shows *a*
/// question menu and only our parse of it came up empty. That's a half-drawn
/// repaint, not an answered question, so the panel holds — bounded, so a
/// question we genuinely can't parse can't strand the previous one's diff.
const PROMPT_STALE_MAX: Duration = Duration::from_secs(4);

/// How long a *different* reading of the prompt must hold still before the
/// panel adopts it. Keeps the diff from redrawing on every mid-repaint frame.
const PROMPT_SWAP_SETTLE: Duration = Duration::from_millis(250);

/// Scrollable widget id for a chat pane's transcript, so the app can snap it
/// to the bottom as replies stream in (auto-scroll to the latest answer).
pub fn chat_scroll_id(id: TerminalId) -> scrollable::Id {
    scrollable::Id::new(format!("chat-scroll-{id}"))
}

// ── splash animation state ────────────────────────────────────────────────────

/// Drives the splash sequence:
///   Video → FadeOut → FadeIn → Text
///
/// The `Video`/`FadeOut`/`FadeIn` arms are currently unused — the splash
/// initialises directly to `Text`. Kept around so the intro video can be
/// switched back on without rebuilding the state machine.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum SplashAnim {
    /// Video is playing; no overlay.
    Video,
    /// Fading the last video frame to black.  `t` goes 0.0 → 1.0.
    FadeOut(f32),
    /// Black screen fading in the tagline text.  `t` goes 0.0 → 1.0.
    FadeIn(f32),
    /// Text fully visible; waiting for the user to click "Get Started".
    Text,
}

// ── app state ─────────────────────────────────────────────────────────────────

/// Which half of the two-up split the user interacted with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitSlot {
    Top,
    Bottom,
}

/// Two-up viewing of independent sessions stacked vertically. The outer
/// pane_grid holds two panes — each tagged with the `session.id` it should
/// display — and is rebuilt whenever the user toggles the split.
pub struct SplitView {
    pub top_id:    usize,
    pub bottom_id: usize,
    pub panes:     pane_grid::State<usize>,
}

/// A tab close (session or Claude) awaiting user confirmation. The inline ✕
/// sits right next to the other tab icons, so a click on it opens a dialog
/// instead of closing immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingClose {
    /// Close the session at this index in `sessions`.
    Session(usize),
    /// Close the Claude terminal tab with this id.
    Claude(TerminalId),
}

/// Full-screen file picker for choosing a local file. Carries its own
/// directory browser plus what the choice is for — captured at open time so it
/// can't drift while the overlay is up.
pub struct FilePicker {
    pub browser: FileBrowserState,
    pub purpose: PickerPurpose,
}

/// Which flow a picked file belongs to. One overlay serves them all; this is
/// what tells `FilePickerChoose` where to send the path.
#[derive(Debug, Clone, Copy)]
pub enum PickerPurpose {
    /// Copy the chosen file to this receiver.
    ReceiverCopy(u64),
    /// Use the chosen file as the Cloud SQL Proxy's service-account key.
    CloudSqlCredentials,
}

impl PickerPurpose {
    /// Heading shown at the top of the picker overlay.
    fn title(self, receiver_name: &str) -> String {
        match self {
            PickerPurpose::ReceiverCopy(_) =>
                format!("Choose a file to copy to “{receiver_name}”"),
            PickerPurpose::CloudSqlCredentials =>
                String::from("Choose the Cloud SQL service-account key"),
        }
    }

    /// One-line explanation under the listing.
    fn hint(self) -> &'static str {
        match self {
            PickerPurpose::ReceiverCopy(_) =>
                "Click a folder to open it; click a file to copy it.",
            PickerPurpose::CloudSqlCredentials =>
                "Click a folder to open it; click your auth.json to use it.",
        }
    }
}

/// State of the per-model usage breakdown card, opened by clicking one of the
/// header usage bars. The window is captured at open time so a stats refresh
/// behind the card can't repoint it at the other bar.
pub struct UsageBreakdown {
    pub window: crate::stats::UsageWindow,
    /// Whether the scan follows the limit window (matching the bar) or a plain
    /// trailing span. Early in a weekly window the two differ enormously, so
    /// the card lets the reader switch.
    pub scope:  crate::stats::UsageScope,
    /// Unix-second start of the span that was scanned — shown so the user can
    /// see exactly what the card covers.
    pub since:  i64,
    /// `None` while the transcript scan is still running.
    pub data:   Option<crate::stats::ModelBreakdown>,
}

/// State of the "Change model" dialog — retarget one or many of a project's
/// running AI sessions at a different model without closing and recreating
/// them. Opened from the agent strip's "Model" button (or Ctrl+Shift+M).
pub struct AgentsDialog {
    /// Session whose agent panes are being retargeted, captured when the
    /// dialog opened so switching projects behind it can't move the goalposts.
    pub session_id: usize,
    /// Agent panes ticked for the change, by terminal id. Panes that have gone
    /// away since are ignored on apply.
    pub selected:   Vec<TerminalId>,
    /// The model the user picked, with the tool whose list it came from.
    /// `None` until they choose one — Apply stays disabled meanwhile.
    pub choice:     Option<(crate::agent::AiTool, crate::agent::ToolModel)>,
    /// Result of the last Apply, shown in the dialog: how many panes switched
    /// in place, how many were restarted, and how many were skipped.
    pub outcome:    Option<ModelChangeOutcome>,
    /// The pane whose ✕ has been pressed and is awaiting confirmation. Closing
    /// ends a running CLI, so it asks first — inline in the row rather than as
    /// a second overlay stacked on this one.
    pub confirming: Option<TerminalId>,
}

/// Tally of what an Apply actually did, per delivery method.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelChangeOutcome {
    /// Panes switched without losing their session (`/model` into a live CLI,
    /// or a chat pane's next-turn override).
    pub in_place:  usize,
    /// Build panes whose CLI had to be relaunched with a new `--model`.
    pub respawned: usize,
    /// Selected panes the chosen model doesn't apply to (a different tool).
    pub skipped:   usize,
}

impl ModelChangeOutcome {
    pub fn changed(&self) -> usize {
        self.in_place + self.respawned
    }

    /// One-line summary for the dialog's status row.
    pub fn summary(&self) -> String {
        let plural = |n: usize| if n == 1 { "agent" } else { "agents" };
        let mut parts = Vec::new();
        if self.in_place > 0 {
            parts.push(format!("{} {} switched", self.in_place, plural(self.in_place)));
        }
        if self.respawned > 0 {
            parts.push(format!("{} restarted", self.respawned));
        }
        if self.skipped > 0 {
            parts.push(format!("{} skipped (different tool)", self.skipped));
        }
        if parts.is_empty() {
            "Nothing to change.".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

pub struct Frms {
    pub sessions:                Vec<Session>,
    pub active_session:          usize,
    /// Ctrl+Tab pane cycling treats the toolbox as one more stop; this is true
    /// while that stop is the current focus so the next cycle steps off it.
    pub pane_focus_toolbox:      bool,
    pub split_view:              Option<SplitView>,
    /// `Some` while the receiver file-copy picker overlay is on screen.
    pub file_picker:             Option<FilePicker>,
    /// `Some` while the close-confirmation dialog is on screen.
    pub pending_close:           Option<PendingClose>,
    pub creating_session:        bool,
    pub new_session_name:        String,
    pub new_session_dir:         String,
    pub new_session_file_browser: FileBrowserState,
    pub renaming_session:        Option<(usize, String)>,
    /// Inline rename for a Claude terminal tab — `(terminal_id, draft)`.
    pub renaming_claude:         Option<(TerminalId, String)>,
    /// `true` while the "+ Agent" button's dropdown menu is open.
    /// `true` while the agent-tab strip is wider than the space it has, i.e.
    /// while a horizontal scrollbar is actually drawn over it. The strip only
    /// reserves the blank gutter that keeps the bar off the tabs when this is
    /// set — see `ui::editor::AGENT_SCROLLBAR_GUTTER`. Measured by the strip
    /// itself (`Message::AgentStripViewport`); cleared optimistically whenever
    /// the strip's contents or the layout change, so it shrinks back.
    pub agent_strip_overflow:    bool,
    /// Fingerprint of what the strip last rendered, so `update` can tell when
    /// the tabs changed and the overflow measurement needs redoing.
    agent_strip_sig:             u64,
    /// `Some` while the "Change model" dialog is on screen.
    pub agents_dialog:            Option<AgentsDialog>,
    next_terminal_id:            u64,
    next_session_id:             usize,
    pub claude_stats:            ClaudeStats,
    /// True when the most recent usage fetch failed (rate limit, offline,
    /// expired token) and `claude_stats` is therefore an older reading — or no
    /// reading at all. The header says so rather than letting a stale or empty
    /// bar be read as "0% used".
    pub stats_stale:             bool,
    pub claude_procs:            ProcStats,
    pub stats_refresh_interval:  RefreshInterval,
    /// `Some` while the per-model usage breakdown is on screen — opened by
    /// clicking either header usage bar. `data` is `None` until the transcript
    /// scan finishes, so the card can show a "reading…" line instead of
    /// blocking the UI thread on a few hundred MB of JSONL.
    pub usage_breakdown:         Option<UsageBreakdown>,
    // Preferences
    pub prefs:                   Prefs,
    pub showing_prefs:           bool,
    pub prefs_dev_dir_input:     String,
    pub prefs_file_browser:      FileBrowserState,
    /// Whether the `claude` CLI is on `PATH`, shown in the Profile tab's Claude
    /// Code status. Probed once at startup (an external install/sign-in via
    /// `frms-setup-claude` is picked up on the next launch).
    pub claude_installed:        bool,
    /// True once the "Claude Code required" startup prompt has been dismissed
    /// this session (shown whenever `claude` is missing; not persisted).
    pub claude_notice_dismissed: bool,
    /// Anonymous usage telemetry sender (no-op when the user has opted out).
    pub telemetry:               crate::telemetry::Telemetry,
    pub notes:                   NotesState,
    /// Global TSR receiver inventory shown in the Receivers plugin tab.
    pub receivers:               crate::receivers::ReceiversState,
    /// Rows parsed from a CSV import, held between loading the file and the
    /// user confirming the column mapping. Empty when no CSV import is pending.
    pub csv_import_rows:         Vec<Vec<String>>,
    /// Active theme-color crossfade. `None` when the theme is settled.
    pub theme_anim:              Option<ThemeAnim>,
    /// When true the top header collapses to a slim bar with just an expand toggle.
    pub header_collapsed:        bool,
    /// Active plugin-panel resize drag, if any. The first `Move` event after
    /// `Start` initialises `last_x` so width updates are purely delta-based —
    /// no need to know the window width.
    pub plugin_panel_drag:       Option<PluginPanelDrag>,
    /// Active plugin-panel split-divider drag, if any. Tracks the last cursor
    /// y so the top-slot height updates are purely delta-based.
    pub plugin_split_drag:       Option<PluginSplitDrag>,
    // Splash
    pub showing_splash:          bool,
    pub splash_yt_dlp:           Option<PathBuf>,  // set once yt-dlp is located
    pub splash_frame:            Option<Vec<u8>>,  // latest JPEG frame (raw JPEG bytes)
    pub splash_anim:             SplashAnim,       // animation state machine
    splash_audio_children:       Vec<std::process::Child>, // yt-dlp + ffmpeg audio procs
}

// ── plugin panel resize ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct PluginPanelDrag {
    /// Most recent cursor x. `None` until the first move event arrives.
    pub last_x: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct PluginSplitDrag {
    /// Most recent cursor y. `None` until the first move event arrives.
    pub last_y: Option<f32>,
}

// ── theme crossfade ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThemeAnim {
    from:    ThemeColors,
    to:      ThemeColors,
    started: Instant,
    duration: Duration,
}

impl ThemeAnim {
    fn progress(&self) -> f32 {
        let elapsed = self.started.elapsed().as_secs_f32();
        let total   = self.duration.as_secs_f32();
        (elapsed / total).clamp(0.0, 1.0)
    }
    fn finished(&self) -> bool { self.progress() >= 1.0 }
    fn current(&self) -> ThemeColors {
        ThemeColors::lerp(self.from, self.to, theme_mod::ease_in_out(self.progress()))
    }
}

// ── messages ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Message {
    // Sessions
    SessionSelected(usize),
    NewSessionRequested,
    NewSessionNameEdited(String),
    NewSessionDirEdited(String),
    NewSessionDirBrowsed(PathBuf),
    NewSessionCreated(SessionKind),
    NewSessionCancelled,
    SessionCloseRequested(usize),
    /// Confirm the pending tab close (session or Claude) from the dialog.
    ClosePendingConfirmed,
    /// Dismiss the close-confirmation dialog without closing anything.
    ClosePendingCancelled,
    SessionRenameStarted(usize),
    SessionRenameEdited(String),
    SessionRenameConfirmed,
    SessionRenameCancelled,
    /// Toggle a session as the bottom row of the two-up split view.
    /// Carries the target `session.id` (stable across reorder/close).
    SessionSplitToggled(usize),
    /// Toggle whether a session is pinned (persisted across restarts).
    /// Carries the target `session.id`.
    SessionPinToggled(usize),
    /// Tab clicked inside one slot of the split view — swap that slot to
    /// display the chosen session instead. Carries `(slot, session_index)`.
    SplitSlotSessionSelected(SplitSlot, usize),
    // Layout
    /// Resize event from a session's inner pane grid. The first field is the
    /// `session.id` so split view doesn't confuse top vs. bottom split IDs.
    PaneResized(usize, pane_grid::ResizeEvent),
    /// Resize event from the outer top/bottom split grid.
    SplitResized(pane_grid::ResizeEvent),
    // File browser (active session)
    BrowseDir(PathBuf),
    OpenFile(PathBuf),
    /// Copy the file browser's current directory path to the clipboard.
    CopyPath(String),
    /// Hide/show the active session's file-browser pane.
    FileBrowserToggleCollapsed,
    /// Begin an inline create/rename/delete on a directory.
    DirEditStart(DirEdit),
    /// Text typed into the inline folder-name input.
    DirEditDraftChanged(String),
    /// Apply the in-progress directory edit.
    DirEditConfirm,
    /// Abandon the in-progress directory edit.
    DirEditCancel,
    // Editor (active session)
    EditorAction(text_editor::Action),
    /// Write the active editor buffer back to its file (Ctrl+S / Save button).
    EditorSave,
    EditorTogglePreview,
    EditorRefreshPreview,
    EditorPreviewReady(usize, PathBuf, Result<Vec<u8>, String>),
    // Terminals
    TerminalData(TerminalId, Vec<u8>),
    /// The child process behind a terminal exited (e.g. the user typed
    /// `exit`). Plugin shell terminals are removed in response.
    TerminalExited(TerminalId),
    TerminalInput(String),
    TerminalScroll(TerminalId, f32),
    /// Scrolled while a drag-selection is in progress: scroll by the given
    /// lines *and* move the selection head to the viewport cell the mouse is
    /// over afterwards, so the highlight extends into the newly revealed rows
    /// instead of being capped at one screenful.
    TerminalScrollDrag(TerminalId, f32, usize, usize),
    /// Left mouse pressed on a terminal canvas at the given viewport cell —
    /// focuses the terminal and starts a drag-selection anchored on the cell.
    TerminalMouseDown(TerminalId, usize, usize),
    /// Cursor moved while the selection drag is in progress.
    TerminalMouseMove(TerminalId, usize, usize),
    /// Mouse released; finalize the selection (or clear it if the drag was a click).
    TerminalMouseUp(TerminalId),
    /// Copy the active terminal's current selection to the clipboard.
    TerminalCopy,
    /// Copy the given terminal's current selection to the clipboard, then clear
    /// it. Emitted by a right-click on a terminal canvas that has a selection —
    /// the copy half of the one-button copy/paste gesture.
    TerminalCopyFrom(TerminalId),
    /// Turn the active terminal's selection into a new note (Ctrl+Shift+N).
    TerminalSendToNote,
    /// Turn the given terminal's selection into a new note, then clear it.
    /// Emitted by the floating "+ Note" button drawn next to a finished
    /// selection on a terminal canvas.
    TerminalSendToNoteFrom(TerminalId),
    /// Read the clipboard, then dispatch the contents to the active terminal.
    TerminalPaste,
    /// Focus the given terminal, then paste the clipboard into it. Emitted by
    /// a right- or middle-click on a terminal canvas.
    TerminalPasteInto(TerminalId),
    /// Clipboard read completed — write its contents to the active terminal,
    /// wrapping with bracketed-paste markers when the program has opted in.
    TerminalPasteReady(Option<String>),
    /// A background clipboard *write* finished. No-op; exists only so the
    /// shell-out `Task` has a message to resolve to.
    ClipboardWritten,
    TabPressed { shift: bool },
    /// Add a new agent pane of the given kind to the active session and select
    /// its tab (Build = an AI CLI in a PTY, Research/Chat = native Claude chat).
    /// The `AiTool` chooses which CLI a `Build` pane runs; the `ToolModel` picks
    /// its model (`--model`). Chat panes are Claude-only, so they ignore the
    /// tool and take the model as an override on their kind's default.
    AgentAdded(AgentKind, crate::agent::AiTool, crate::agent::ToolModel),
    /// Add a Build pane using the user's default tool at its default model
    /// (the Ctrl+Shift+T quick shortcut). Resolved to [`Message::AgentAdded`] in
    /// `update`, where the prefs are reachable.
    NewDefaultAgent,
    /// Toggle the "+ Agent" dropdown menu open/closed.
    /// Nudge the agent-tab strip one tab's worth left/right — the ❮ ❯ buttons
    /// beside it, for reaching tabs that have scrolled out of the strip.
    AgentTabsNudged { forward: bool },
    /// The agent-tab strip reporting its own geometry. Iced only publishes this
    /// while the strip overflows (see `notify_viewport` in `iced_widget`), which
    /// is exactly the question `agent_strip_overflow` answers.
    AgentStripViewport(iced::widget::scrollable::Viewport),
    /// The window changed size — panes (and so the agent-tab strip) got
    /// re-laid-out. Nothing to update beyond re-measuring what now fits.
    WindowResized,
    /// Toggle the find bar on whichever agent pane is in front (Ctrl+Shift+L).
    /// Resolved to [`Message::FindOpened`] in `update`, where the active
    /// session's front tab is known.
    FindToggled,
    /// Toggle the find bar on an agent pane, focusing its input when it opens.
    /// Re-opening keeps the last query, so the search resumes where it left off.
    FindOpened(TerminalId),
    /// The find query changed — re-run the search and jump to its newest hit.
    FindQueryChanged(TerminalId, String),
    /// Step to the next (`forward`) or previous hit, wrapping at the ends.
    FindStepped(TerminalId, bool),
    /// Close the find bar and drop its highlights.
    FindClosed(TerminalId),
    /// Open the "Change model" dialog for the active session, pre-ticking the
    /// agent pane that's currently in front.
    AgentsDialogOpened,
    /// Dismiss the "Change model" dialog without applying anything.
    AgentsDialogClosed,
    /// Tick / untick one agent pane in the "Change model" dialog.
    AgentsDialogPaneToggled(TerminalId),
    /// Tick every eligible pane (`true`) or none (`false`).
    AgentsDialogSelectAll(bool),
    /// Pick the model to switch the ticked panes to, from `AiTool`'s list.
    AgentsDialogModelPicked(crate::agent::AiTool, crate::agent::ToolModel),
    /// Apply the picked model to every ticked pane.
    AgentsDialogApplied,
    /// ✕ on an agent row — arm the inline "close this one?" confirmation.
    AgentsDialogPaneCloseRequested(TerminalId),
    /// Back out of that confirmation.
    AgentsDialogPaneCloseCancelled,
    /// Confirmed — close the pane and stop its CLI.
    AgentsDialogPaneClosed(TerminalId),
    /// Edited the draft input of a chat (Research/Chat) pane.
    ChatInputAction(TerminalId, text_editor::Action),
    /// Interaction with a completed transcript bubble (the `usize` indexes
    /// `ChatPane::transcript`). The bubble editors are read-only, so only
    /// cursor/selection actions are applied — edits are dropped — letting the
    /// user highlight a span of a past reply.
    ChatBubbleAction(TerminalId, usize, text_editor::Action),
    /// Ctrl/Cmd+C over a transcript bubble — copy that bubble's selection.
    ChatBubbleCopy(TerminalId, usize),
    /// "+ Note" on a transcript bubble — turn the highlighted span into a note.
    ChatSendToNote(TerminalId, usize),
    /// Submitted the draft of a chat pane — sends the transcript to the API.
    ChatSubmitted(TerminalId),
    /// A chat pane's reply reached for a tool ("Read  src/app.rs"). Shown while
    /// the reply streams so a turn spent working the project shows progress.
    ChatActivity(TerminalId, String),
    /// Ctrl/Cmd+C in a chat pane's input — copy the selection to the clipboard.
    ChatCopy(TerminalId),
    /// Ctrl/Cmd+X in a chat pane's input — cut the selection to the clipboard.
    ChatCut(TerminalId),
    /// Ctrl/Cmd+V in a chat pane's input — kicks off an async clipboard read.
    ChatPaste(TerminalId),
    /// The clipboard read for a chat-pane paste finished; insert its contents.
    ChatPasteReady(TerminalId, Option<String>),
    /// A streamed text fragment arrived for a chat pane's in-flight reply.
    ChatDelta(TerminalId, String),
    /// A chat pane's reply stream finished — `Ok(session_id)` (to resume on the
    /// next turn) or `Err(message)` on failure.
    ChatEnded(TerminalId, Result<String, String>),
    /// The user scrolled a chat transcript. Tracks whether the view is still at
    /// the bottom, which decides if streaming text keeps auto-scrolling.
    ChatScrolled(TerminalId, scrollable::Viewport),
    /// Close a Claude terminal tab from a session (only when more than one exists).
    ClaudeSessionClosed(TerminalId),
    /// Begin inline rename of the named Claude terminal tab.
    ClaudeRenameStarted(TerminalId),
    ClaudeRenameEdited(String),
    ClaudeRenameConfirmed,
    ClaudeRenameCancelled,
    // Browser pane
    BrowserUrlEdited(String),
    BrowserUrlSubmitted,
    BrowserRefresh,
    BrowserScreenshotReady(usize, Vec<u8>),
    BrowserScreenshotFailed(usize, String),
    // CDP interactive browser
    CdpReady(usize),
    CdpLaunchFailed(usize, String),
    BrowserMouseMoved(f32, f32),
    BrowserMousePressed,
    // Prompt response (Claude permission prompts)
    PromptAccepted,
    PromptRejected,
    // Splash screen
    SplashDismissed,
    #[allow(dead_code)]
    SplashYtDlpReady(Result<PathBuf, String>),
    SplashVideoFrame(Vec<u8>),
    SplashVideoEnded,
    SplashAnimTick,
    // Window
    Quit,
    // Keyboard navigation (see `handle_key` and the Profile shortcuts callout)
    /// Cycle keyboard focus across the active project's panes (and the toolbox).
    CyclePane { forward: bool },
    /// Cycle keyboard focus through only the active project's agent panes,
    /// skipping the toolbox (Ctrl+Shift+A).
    CycleAgentPane { forward: bool },
    /// Jump to the next/previous pane that's waiting on the user (a surfaced
    /// question or a review), across all sessions.
    CycleAttention { forward: bool },
    /// Switch to the next/previous project session (the top tabs).
    CycleSession { forward: bool },
    /// Cycle the toolbox (plugin panel) to the next/previous tool tab.
    CyclePluginTab { forward: bool },
    /// Close the currently focused agent pane (same confirm flow as its ✕).
    CloseActiveAgent,
    // Claude-Code-required startup prompt
    /// Continue without the `claude` CLI for this session.
    ClaudeNoticeDismissed,
    /// Re-probe the PATH for `claude` (after installing it in a terminal).
    ClaudeRecheck,
    // Stats
    StatsRefresh,
    /// A usage fetch finished. `None` means it failed — the handler keeps the
    /// previous reading rather than clobbering it with zeros.
    StatsLoaded(Option<ClaudeStats>),
    ProcsLoaded(ProcStats),
    StatsRefreshIntervalChanged(RefreshInterval),
    /// A usage bar was clicked — open the per-model breakdown for its window.
    UsageBreakdownOpened(crate::stats::UsageWindow),
    /// The transcript scan finished; fill in the open card.
    UsageBreakdownLoaded(crate::stats::ModelBreakdown),
    /// Re-run the scan for the window the card is already showing.
    UsageBreakdownRefreshed,
    /// Switch the open card between the limit window and a trailing span.
    UsageBreakdownScopeChanged(crate::stats::UsageScope),
    UsageBreakdownClosed,
    // Plugin panel (DB / Profile / Notes / Terminals)
    /// A tab was clicked in the given slot — switch that slot to the tab.
    PluginTabClicked(PluginSlot, PluginTab),
    /// Hide a plugin from the tab bar via its per-tab `✕`.
    PluginTabHidden(PluginTab),
    /// Ctrl+Shift+1…9 — focus agent pane N of the active session directly,
    /// without starting the panes in between (see the update arm).
    AgentPaneSelected(usize),
    /// Re-show a hidden plugin (and make it active) from the `+` picker.
    PluginTabShown(PluginTab),
    /// Toggle the `+` add-plugin picker that lists hidden plugins.
    PluginAddPickerToggled,
    PluginPanelHide,
    /// Toggle plugin panel visibility without changing the active tab.
    /// Bound to the consolidated header button.
    PluginPanelToggled,
    /// Move the panel to the opposite window edge (left ⇄ right).
    PluginPanelDockToggled,
    /// Split the panel into two stacked slots, or merge back to one.
    PluginPanelSplitToggled,
    /// Expand the panel to full width (hiding the main working area), or
    /// restore the main area. The mirror of hiding the panel entirely.
    PluginPanelExpandToggled,
    /// Open/close the `⋯` overflow menu holding the panel-wide controls
    /// (dock / split / expand / close) so they stay usable at any panel width.
    PluginPanelControlsToggled,
    /// User pressed the drag handle on the inner edge of the plugin panel.
    PluginPanelResizeStart,
    /// Latest cursor x while a width-resize drag is active (logical px from window left).
    PluginPanelResizeMove(f32),
    /// Mouse released — end the width drag.
    PluginPanelResizeEnd,
    /// User pressed the divider between the two split slots.
    PluginSplitResizeStart,
    /// Latest cursor y while a split-resize drag is active (logical px from window top).
    PluginSplitResizeMove(f32),
    /// Mouse released — end the split drag.
    PluginSplitResizeEnd,
    // Docs (search over the project's `_specs` / `_docs` folders)
    /// Search text changed — filters the index by name / relative path.
    DocsQueryChanged(String),
    /// Narrow the list to documents modified within a given window.
    DocsDateChanged(crate::docs::DateFilter),
    /// Reorder results by name or by most-recently-modified.
    DocsSortChanged(crate::docs::SortBy),
    /// Re-read `_specs` / `_docs` from disk (the ↻ button).
    DocsRefresh,
    /// Reset the search text and date filter, keeping the sort order.
    DocsClearFilters,
    /// Ctrl+Shift+S — surface the Docs tab, rescan, and focus its search box.
    DocsOpen,
    // Notes
    NotesNew,
    NotesSelected(u64),
    /// Filter the notes list by title / body / category text.
    NotesSearchChanged(String),
    /// Clear the notes-list search filter (the ✕ beside the field).
    NotesSearchCleared,
    /// Ctrl+Shift+G — surface the Notes tab and focus its search box.
    NotesSearchOpen,
    NotesDelete(u64),
    NotesTitleEdited(String),
    NotesContentAction(text_editor::Action),
    /// Apply a markdown formatting action (toolbar button or Ctrl+B/I/E/K)
    /// to the note body editor.
    NotesFormat(NoteFormat),
    /// Switch the note body between Edit / Split / Preview display.
    NotesViewMode(NoteViewMode),
    /// Tab in the note body editor — indent the selected lines, or insert one
    /// indent step at the cursor.
    NotesIndent,
    /// Shift+Tab in the note body editor — strip one indent step.
    NotesOutdent,
    /// Send the highlighted selection (or the whole note if nothing is
    /// highlighted) to the active session's active Claude terminal.
    NotesSendToClaude,
    /// Toggle a note between pinned-to-this-project and global.
    NotesPinToggled(u64),
    /// Move keyboard focus from the note Title field into the body editor —
    /// emitted by Tab or Enter while naming.
    NotesFocusBody,
    /// A category was picked for the selected note from the dropdown — an
    /// existing one, "General" (uncategorized), or "+ New category…".
    NotesCategorySelected(CategoryChoice),
    /// Keystroke in the new-category name field.
    NotesCategoryInput(String),
    /// Commit the typed new-category name to the selected note (Enter / Add).
    NotesCategoryCommit,
    /// Abandon the new-category name field without changing the category.
    NotesCategoryCancel,
    /// Collapse/expand a category group in the notes list. Empty string is the
    /// uncategorized ("General") group.
    NotesCategoryToggleCollapsed(String),
    /// Install an AI CLI tool from the Profile tab: opens a Terminals-plugin
    /// shell running the tool's one-shot install command, then drops the user
    /// into a shell to sign in.
    InstallTool(crate::agent::AiTool),
    // Terminals plugin (per-session shell terminals)
    TerminalPluginNew,
    TerminalPluginSelected(TerminalId),
    TerminalPluginDelete(TerminalId),
    /// Toggle whether a plugin shell terminal is pinned. Pinned terminals
    /// have their name persisted and are respawned (as fresh shells) on the
    /// next launch.
    TerminalPluginPinToggled(TerminalId),
    TerminalPluginNameEdited(String),
    /// Move keyboard focus out of the Terminals-plugin Name field and into the
    /// selected terminal — emitted by Tab or Enter while naming.
    TerminalPluginFocusTerminal,
    // Receivers plugin (global TSR receiver inventory)
    /// Add a container under `Some(parent)` or as a new root (`None`).
    ReceiverContainerAdd(Option<u64>),
    ReceiverContainerSelect(u64),
    ReceiverContainerRenamed(u64, String),
    ReceiverContainerDelete(u64),
    /// Expand/collapse a container row in the tree.
    ReceiverContainerToggle(u64),
    /// Reparent a container under another (`None` = top level).
    ReceiverContainerMoveTo(u64, Option<u64>),
    /// Move every receiver held directly by the first container into the second.
    ReceiverMoveAll(u64, u64),
    /// Add a receiver under the given container.
    ReceiverAdd(u64),
    ReceiverSelect(u64),
    ReceiverDelete(u64),
    /// Move a single receiver into another container.
    ReceiverMoveTo(u64, u64),
    // Edit form for the selected receiver.
    ReceiverNameEdited(String),
    ReceiverHostEdited(String),
    ReceiverUserEdited(String),
    ReceiverPasswordEdited(String),
    ReceiverPortEdited(String),
    /// Open an interactive SSH session to the given receiver.
    ReceiverConnect(u64),
    /// Tear down the live SSH session for the given receiver.
    ReceiverSshDisconnect(u64),
    /// Open an SSH session to every receiver under the given container.
    ReceiverConnectAll(u64),
    /// Tear down every SSH session under the given container.
    ReceiverDisconnectAll(u64),
    /// Collapse/expand the credential form so the SSH terminal can grow.
    ReceiverToggleConnectionForm,
    /// Open the file picker to choose a local file to copy to the selected receiver.
    ReceiverCopyPick,
    /// Navigate the file picker to a directory.
    FilePickerBrowse(std::path::PathBuf),
    /// A file was chosen — routed by the picker's `PickerPurpose`.
    FilePickerChoose(std::path::PathBuf),
    /// Dismiss the file picker without choosing.
    FilePickerCancel,
    /// Copy a specific file (e.g. a file-browser row action) to the selected receiver.
    ReceiverCopyFile(std::path::PathBuf),
    /// Result of an scp copy.
    ReceiverCopyDone(Result<String, String>),
    // Import flow (DB / CSV).
    /// Begin a database import for the selected container (shows the table picker).
    ReceiverImportDbStart,
    /// A table was chosen — fetch its columns.
    ReceiverImportTablePicked(String),
    /// Columns fetched for the chosen DB table (or an error).
    ReceiverImportColumnsLoaded(Result<Vec<String>, String>),
    /// Begin a CSV import for the selected container.
    ReceiverImportCsvStart,
    ReceiverImportCsvPathEdited(String),
    /// Parse the CSV at the entered path and populate the column mapping.
    ReceiverImportCsvLoad,
    /// Map one receiver field onto a source column (or clear it).
    ReceiverImportFieldMapped(crate::receivers::MapField, Option<String>),
    /// Set a literal username/password/port to apply when that column is unmapped.
    ReceiverImportLiteralEdited(crate::receivers::MapField, String),
    /// Set the optional import row-filter column (or clear it).
    ReceiverImportFilterColumn(Option<String>),
    ReceiverImportFilterOp(crate::receivers::ImportFilterOp),
    ReceiverImportFilterValue(String),
    /// Confirm the import — fetch rows (DB) or reuse parsed rows (CSV) and create receivers.
    ReceiverImportConfirm,
    /// Rows fetched for a DB import (or an error).
    ReceiverImportRowsLoaded(Result<Vec<Vec<String>>, String>),
    ReceiverImportCancel,
    // Database panel (active session)
    DbEngineChanged(DbEngine),
    DbProviderChanged(DbProvider),
    DbFormToggleCollapsed,
    DbHostEdited(String),
    DbPortEdited(String),
    DbUserEdited(String),
    DbPasswordEdited(String),
    DbDatabaseEdited(String),
    DbFieldSubmitted(&'static str),
    // ── Cloud SQL Auth Proxy (GCP provider only) ──────────────────────────
    /// Instance connection name (`project:region:instance`) edited.
    DbInstanceEdited(String),
    /// Open the file picker to choose the proxy's service-account key.
    DbProxyCredentialsPick,
    /// Forget the chosen key — the proxy falls back to gcloud ADC.
    DbProxyCredentialsCleared,
    /// Collapse/expand the Cloud SQL section of the connection form.
    DbProxyToggleCollapsed,
    /// Start the proxy. `then_connect` is set when this was triggered by
    /// Connect, so a successful start chains straight into the connection.
    DbProxyStart { then_connect: bool },
    /// Outcome of a start attempt, tagged with the session it belongs to.
    DbProxyStarted(usize, bool, Result<crate::cloud_sql::Proxy, String>),
    /// Stop the managed proxy.
    DbProxyStop,
    /// Install the Cloud SQL Proxy — runs the packaged setup helper in a
    /// Terminals-plugin pane, like the Profile tab's tool installs.
    DbProxyInstall,
    DbConnect,
    /// Outcome of a connect attempt, tagged with the session and the panel epoch
    /// it was issued under (see `DbPanel::epoch`).
    DbConnected(usize, u64, Result<(DbClient, Vec<TableRef>, Vec<ForeignKey>, Vec<RoutineRef>), String>),
    DbDisconnect,
    /// Force the panel back to a clean disconnected state — the way out when the
    /// connection has died and the tool is wedged waiting on it.
    DbReset,
    DbTableToggled(String),
    /// Open a stored procedure/function's source for viewing (Objects list).
    DbRoutineSelected(RoutineRef),
    /// The fetched source of the selected routine (or an error).
    DbRoutineLoaded(usize, u64, Result<String, String>),
    /// Dismiss the routine source view and return to query results.
    DbRoutineClosed,
    DbColumnsLoaded(usize, u64, String, Result<Vec<String>, String>),
    DbColumnToggled(String, String),
    /// Check/uncheck "All fields" — select or clear every available column.
    DbFieldsSelectAll(bool),
    /// Mouse-drag reordering of the picked result columns (Col 3): begin a drag
    /// on a field, drag over another to move it there, release to finish.
    DbFieldDragStart(usize),
    DbFieldDragOver(usize),
    DbFieldDragEnd,
    /// Remove a picked result column from the query by its position.
    DbFieldRemoved(usize),
    /// WHERE-clause editing in the Filters column: add a new condition,
    /// change a condition's column / operator / value, flip its AND/OR
    /// connector, or remove it by position.
    DbFilterAdd,
    DbFilterColumnChanged(usize, ColRef),
    DbFilterOpChanged(usize, FilterOp),
    DbFilterValueChanged(usize, String),
    DbFilterConjToggled(usize),
    DbFilterRemoved(usize),
    /// Row-cap ("Limit") field edited; empty = no LIMIT (all rows).
    DbLimitChanged(String),
    /// Toggle a (table, column) into/out of the GROUP BY clause.
    DbGroupToggled(String, String),
    /// Manual JOIN-condition edits, keyed by the joined table. The first arg is
    /// the joined table key; the second the chosen left table / column / column.
    DbJoinLeftTableChanged(String, String),
    DbJoinLeftColChanged(String, String),
    DbJoinRightColChanged(String, String),
    DbRunQuery,
    /// Re-run the last query so the results list reflects current data.
    DbRefreshResults,
    DbQueryResult(usize, u64, Result<QueryResult, String>),
    DbResetTables,
    /// Switch the builder row between the visual builder and the free-hand SQL
    /// editor (false = builder, true = raw SQL).
    DbSetSqlMode(bool),
    /// Edit the free-hand SQL buffer.
    DbSqlAction(text_editor::Action),
    /// Ctrl/Cmd+C in the SQL editor — copy the selection to the clipboard.
    DbSqlCopy,
    /// Ctrl/Cmd+X in the SQL editor — cut the selection to the clipboard.
    DbSqlCut,
    /// Ctrl/Cmd+V in the SQL editor — kicks off an async clipboard read.
    DbSqlPaste,
    /// The clipboard read for a SQL-editor paste finished; insert its contents.
    DbSqlPasteReady(Option<String>),
    /// Execute the free-hand SQL verbatim against the connection.
    DbRunSql,
    /// Outcome of a free-hand statement — a result set or an affected-row count.
    DbStatementResult(usize, u64, Result<StatementOutcome, String>),
    /// Filter the Tables/Objects list in Col 1 by name.
    DbObjectFilterChanged(String),
    /// Collapse/expand the Tables and Objects groups in Col 1.
    DbTablesToggleCollapsed,
    DbObjectsToggleCollapsed,
    /// Collapse/expand the builder row to give the results table more room.
    DbRow1ToggleCollapsed,
    /// Copy a single results-table cell value (e.g. an ID) to the clipboard.
    DbCopyValue(String),
    /// Pick the text format used when copying results (CSV/TSV/Markdown/JSON).
    DbCopyFormatChanged(crate::db::CopyFormat),
    /// Copy the whole result set — header row and all rows — to the clipboard.
    DbCopyResults,
    /// Copy one record (row index) from the result set to the clipboard.
    DbCopyRow(usize),
    /// The results body scrolled — keep the frozen header aligned horizontally.
    DbResultsScrolled(scrollable::Viewport),
    // Center-pane tab switcher
    CenterTabSelected(crate::session::CenterTab),
    // Preferences
    PrefsOpened,
    PrefsCancelled,
    PrefsSaved,
    PrefsDevDirEdited(String),
    PrefsDevDirBrowsed(PathBuf),
    PrefsPaletteChanged(Palette),
    PrefsModeChanged(Mode),
    PrefsFontScaleChanged(FontScale),
    PrefsTerminalFontScaleChanged(TerminalFontScale),
    /// Profile-tab picker for the file-browser submodule highlight color.
    PrefsSubmoduleColorChanged(iced::Color),
    /// Profile-tab toggle for anonymous usage telemetry.
    PrefsTelemetryToggled(bool),
    /// Profile-tab picker for the default AI tool new Build panes launch.
    PrefsDefaultToolChanged(crate::agent::AiTool),
    /// Profile-tab toggle: hide tools not on `PATH` from the "+ Agent" picker.
    PrefsFilterToolsToggled(bool),
    /// Profile tab: how much of the project the native chat panes may touch.
    PrefsChatAccessChanged(crate::chat_api::ChatAccess),
    /// Profile tab: switch one `ls` short option on/off for new shell terminals.
    PrefsLsFlagToggled(char, bool),
    /// Profile tab: this machine runs an immutable OS, so shell terminals track
    /// and re-enter the toolbox/distrobox they were left in.
    PrefsImmutableSystemToggled(bool),
    /// First-run telemetry notice dismissed — `true` keeps telemetry on, `false`
    /// turns it off. Either way the notice is acknowledged and won't reappear.
    TelemetryNoticeChoice(bool),
    /// First-run NDA gate — `true` accepts (and proceeds), `false` declines and
    /// exits the app.
    NdaChoice(bool),
    /// Tick from the per-frame theme-crossfade subscription.
    ThemeAnimTick,
    /// Collapse / expand the top header panel.
    HeaderToggleCollapse,
}

// ── application ───────────────────────────────────────────────────────────────

pub fn initialize() -> (Frms, Task<Message>) {
    Frms::new()
}

pub fn title(state: &Frms) -> String {
    state.title()
}

pub fn update(state: &mut Frms, message: Message) -> Task<Message> {
    // A message that rearranges the layout invalidates the agent strip's
    // overflow measurement the same way a changed set of tabs does — the strip
    // keeps the same tabs but gets a different width to fit them in.
    let relaid_out = matches!(
        message,
        Message::PaneResized(..)
            | Message::SplitResized(..)
            | Message::WindowResized
            | Message::PluginPanelResizeMove(_)
            | Message::PluginPanelToggled,
    );
    let task = state.update(message);
    // Agent panes spawn lazily; whatever this message did to the layout, start
    // the CLI for any pane it just brought on screen (see `spawn_visible_panes`).
    state.spawn_visible_panes();
    state.refresh_agent_strip_overflow(relaid_out);
    task
}

pub fn view(state: &Frms) -> Element<'_, Message> {
    state.view()
}

pub fn theme(state: &Frms) -> Theme {
    state.theme()
}

pub fn subscription(state: &Frms) -> Subscription<Message> {
    state.subscription()
}

pub fn scale_factor(state: &Frms) -> f64 {
    state.scale_factor()
}

impl Frms {
    pub fn new() -> (Self, Task<Message>) {
        let prefs = Prefs::load();
        let start_dir = prefs.new_session_start_dir();
        let start_dir_str = start_dir.to_string_lossy().into_owned();

        let mut new_session_browser = FileBrowserState::new();
        new_session_browser.navigate(start_dir.clone());

        let mut prefs_browser = FileBrowserState::new();
        prefs_browser.navigate(start_dir);

        let prefs_dev_dir_input = prefs
            .dev_dir
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut app = Self {
            sessions:                 Vec::new(),
            active_session:           0,
            pane_focus_toolbox:       false,
            split_view:               None,
            pending_close:            None,
            creating_session:         true,
            new_session_name:         String::new(),
            new_session_dir:          start_dir_str,
            new_session_file_browser: new_session_browser,
            renaming_session:         None,
            renaming_claude:          None,
            agent_strip_overflow:     false,
            agent_strip_sig:          0,
            agents_dialog:             None,
            usage_breakdown:          None,
            next_terminal_id:         0,
            next_session_id:          1,
            claude_stats:             ClaudeStats::default(),
            stats_stale:              false,
            claude_procs:             ProcStats::default(),
            stats_refresh_interval:   RefreshInterval::default(),
            prefs,
            showing_prefs:            false,
            prefs_dev_dir_input,
            prefs_file_browser:       prefs_browser,
            claude_installed:         crate::session::claude_on_path(),
            claude_notice_dismissed:  false,
            telemetry:                crate::telemetry::Telemetry::disabled(),
            notes:                    NotesState::load(),
            receivers:                crate::receivers::ReceiversState::load(),
            csv_import_rows:          Vec::new(),
            file_picker:              None,
            theme_anim:               None,
            header_collapsed:         false,
            plugin_panel_drag:        None,
            plugin_split_drag:        None,
            showing_splash:           true,
            splash_yt_dlp:            None,
            splash_frame:             None,
            // Intro video is disabled for now — jump straight to the welcome
            // text phase. The video pipeline is left intact so flipping this
            // back to `SplashAnim::Video` (and re-adding the yt-dlp Command
            // below) is the only change needed to bring it back.
            splash_anim:              SplashAnim::Text,
            splash_audio_children:    Vec::new(),
        };
        app.restore_persisted();
        // Telemetry starts here, every launch. The essential tier (one `install`
        // per install, one `launch` per open) is always reported — the first-run
        // notice discloses it and the Profile toggle doesn't cover it. The
        // toggle governs the optional tier only, and applies from this point:
        // no feature-usage or diagnostic event can fire before `new` returns.
        // A restored session parked on an agent tab is displayed as soon as the
        // window paints, so start that one pane before the first frame rather
        // than waiting for whatever message happens to arrive next.
        app.spawn_visible_panes();
        app.telemetry = crate::telemetry::Telemetry::start(app.prefs.telemetry);
        app.telemetry.record(crate::telemetry::Event::Launch);
        let init_stats = Task::perform(stats::load(), Message::StatsLoaded);
        let init_procs = Task::perform(stats::load_procs(), Message::ProcsLoaded);
        // Start maximized: grab the freshly-opened window and maximize it.
        let maximize = window::get_latest()
            .and_then(|id| window::maximize(id, true));
        (app, Task::batch([init_stats, init_procs, maximize]))
    }

    /// Re-create sessions from `~/.frms/sessions.json` (written on every
    /// session mutation). If any sessions are restored we skip the
    /// new-session dialog so the user lands directly back in their IDE.
    fn restore_persisted(&mut self) {
        let state = crate::persisted::PersistedState::load();
        if state.sessions.is_empty() {
            return;
        }

        let mut max_session_id = 0_usize;
        for ps in &state.sessions {
            // Rebuild the live Session — fresh PTYs but same name, dir,
            // and (for Browser sessions) the URL they last navigated to.
            let first_term = self.next_terminal_id;
            self.next_terminal_id += 1;

            let mut session = Session::new(
                ps.id, Some(ps.name.clone()), ps.kind,
                first_term, ps.working_dir.clone(),
                // Restore respawns pane[0] from the persisted tool below, so the
                // baseline here is the default (Claude) that logic compares to.
                crate::agent::AiTool::default(),
            );
            // Restored sessions were only written because they were pinned,
            // so keep them pinned (otherwise they'd vanish on next save).
            session.pinned = ps.pinned;
            // Resolve the persisted (tool, model) for tab `i`. Missing entries
            // default to Claude at its default model — matching how pre-tool
            // save files (which only ran `claude`) should come back.
            let resolve_tool_model = |i: usize| -> (crate::agent::AiTool, crate::agent::ToolModel) {
                let tool = ps.claude_tools.get(i).copied().unwrap_or_default();
                let model = ps.claude_models.get(i)
                    .and_then(|tok| tool.model_by_token(tok))
                    .copied()
                    .unwrap_or_else(|| *tool.default_model());
                (tool, model)
            };

            // `Session::new` always makes pane[0] a pending Build/Claude pane at
            // the default model. If this session's first tab was actually a chat
            // pane, swap it; if it was a different tool or a pinned model, point
            // it at those instead. Nothing has been spawned yet, so both cases
            // are a plain replacement — no child to kill.
            if let Some(k0) = ps.claude_kinds.first().copied() {
                let (tool, model) = resolve_tool_model(0);
                if !k0.is_agentic() {
                    let mut chat = ChatPane::new(first_term, k0);
                    // Carry over a model the user picked for this chat pane.
                    chat.model = model;
                    session.panes[0] = AgentPane::Chat(chat);
                } else {
                    session.panes[0] = AgentPane::Pending(crate::agent::PendingPane {
                        id: first_term, tool, model, name: None,
                    });
                }
            }
            // Recreate any additional tabs the session had open, each with its
            // persisted kind/tool/model (defaulting to Build/Claude for pre-kinds
            // save files).
            for i in 1..ps.claude_count.max(1) {
                let tid = self.next_terminal_id;
                self.next_terminal_id += 1;
                let kind  = ps.claude_kinds.get(i).copied().unwrap_or_default();
                let (tool, model) = resolve_tool_model(i);
                session.add_agent(tid, kind, tool, model);
            }
            // Restore per-tab user-assigned names.
            for (i, p) in session.panes.iter_mut().enumerate() {
                if let Some(name) = ps.claude_names.get(i).and_then(|n| n.clone()) {
                    if !name.trim().is_empty() {
                        p.set_name(Some(name));
                    }
                }
            }
            session.center_tab = match ps.center_tab {
                crate::persisted::PersistedCenterTab::Editor   => crate::session::CenterTab::Editor,
                crate::persisted::PersistedCenterTab::Database => crate::session::CenterTab::Database,
                crate::persisted::PersistedCenterTab::Claude(i) => {
                    session.panes.get(i)
                        .map(|p| crate::session::CenterTab::Claude(p.id()))
                        .unwrap_or(crate::session::CenterTab::Editor)
                }
            };
            if let Some(u) = &ps.url_input   { session.url_input   = u.clone(); }
            if let Some(u) = &ps.browser_url { session.browser_url = u.clone(); }
            if let Some(p) = &ps.open_file   { session.editor.open(p.clone()); }
            // Restore the plugin panel layout — dock side, split, tabs,
            // sizes, visibility — exactly as the user last arranged it.
            if let Some(panel) = &ps.plugin_panel {
                session.plugin_panel = panel.clone();
            }
            session.file_browser_collapsed = ps.file_browser_collapsed;
            // Respawn the session's pinned plugin shell terminals as fresh
            // shells, preserving the names the user gave them and reopening each
            // in the directory it was last left in (falling back to the session
            // root if that directory is gone or was never captured) and back
            // inside the toolbox it was in.
            for pt in &ps.plugin_terminals {
                let tid = self.next_terminal_id;
                self.next_terminal_id += 1;
                let start_dir = pt.cwd.as_deref()
                    .filter(|d| d.is_dir())
                    .unwrap_or(&ps.working_dir);
                let init = crate::terminals::ShellInit {
                    // Only re-enter containers when the user runs an immutable
                    // system; off, a saved container is remembered but ignored,
                    // so turning the toggle back on restores the behaviour.
                    container: self.prefs.immutable_system
                        .then(|| pt.container.clone())
                        .flatten(),
                    ls_flags:  self.prefs.ls_flags.clone(),
                };
                session.terminals_plugin.create_with_name(
                    tid, start_dir, pt.name.clone(), true, &init,
                );
            }
            // `create_with_name` drops each respawned terminal into naming mode;
            // on restore there's no user typing a name, so clear that so the
            // first keystroke goes to the shell, not the Name field.
            session.terminals_plugin.naming = false;
            self.sessions.push(session);

            max_session_id = max_session_id.max(ps.id);
        }
        self.next_session_id = max_session_id.saturating_add(1);

        // Pick the previously active session, falling back to the first.
        if let Some(active_id) = state.active_session_id {
            if let Some(idx) = self.sessions.iter().position(|s| s.id == active_id) {
                self.active_session = idx;
            }
        }

        // Restore the two-up split if both referenced sessions came back.
        if let Some(sv) = state.split_view {
            let have_top    = self.sessions.iter().any(|s| s.id == sv.top_id);
            let have_bottom = self.sessions.iter().any(|s| s.id == sv.bottom_id);
            if have_top && have_bottom && sv.top_id != sv.bottom_id {
                let panes = pane_grid::State::with_configuration(Configuration::Split {
                    axis:  pane_grid::Axis::Horizontal,
                    ratio: 0.5,
                    a: Box::new(Configuration::Pane(sv.top_id)),
                    b: Box::new(Configuration::Pane(sv.bottom_id)),
                });
                self.split_view = Some(SplitView {
                    top_id:    sv.top_id,
                    bottom_id: sv.bottom_id,
                    panes,
                });
            }
        }

        self.creating_session = false;
    }

    /// Snapshot the current session list and split state to disk. Called
    /// after every mutation that affects persisted fields.
    /// The file browser that inline directory edits (create/rename/delete)
    /// should act on: the new-project dialog's picker while that dialog is
    /// open, otherwise the active session's file browser.
    fn dir_edit_browser(&mut self) -> &mut FileBrowserState {
        if self.creating_session {
            &mut self.new_session_file_browser
        } else {
            &mut self.sessions[self.active_session].file_browser
        }
    }

    fn save_persisted(&self) {
        // A session is written if the user pinned it, or if it owns at least
        // one pinned plugin terminal (so a pinned shell survives even inside
        // an otherwise-ephemeral session). Everything else is left out.
        let sessions = self.sessions.iter()
            .filter(|s| s.pinned || s.terminals_plugin.terminals.iter().any(|t| t.pinned))
            .map(|s| {
            let center_tab = match s.center_tab {
                crate::session::CenterTab::Editor   => crate::persisted::PersistedCenterTab::Editor,
                crate::session::CenterTab::Database => crate::persisted::PersistedCenterTab::Database,
                crate::session::CenterTab::Claude(tid) => {
                    let idx = s.panes.iter().position(|p| p.id() == tid).unwrap_or(0);
                    crate::persisted::PersistedCenterTab::Claude(idx)
                }
            };
            crate::persisted::PersistedSession {
                id:           s.id,
                kind:         s.kind,
                name:         s.name.clone(),
                working_dir:  s.working_dir.clone(),
                url_input:    Some(s.url_input.clone()),
                browser_url:  Some(s.browser_url.clone()),
                open_file:    s.editor.path.clone(),
                center_tab,
                claude_count: s.panes.len(),
                claude_names: s.panes.iter().map(|p| p.name().map(str::to_owned)).collect(),
                claude_kinds: s.panes.iter().map(|p| p.kind()).collect(),
                claude_tools: s.panes.iter().map(|p| p.tool()).collect(),
                claude_models: s.panes.iter().map(|p| p.model_choice().token.to_string()).collect(),
                plugin_terminals: s.terminals_plugin.terminals.iter()
                    .filter(|t| t.pinned)
                    .map(|t| crate::persisted::PersistedTerminal {
                        name: t.name.clone(),
                        // Capture where the shell is now so it reopens there.
                        cwd:  t.pane.cwd(),
                        // …and which toolbox it's in, so it reopens inside it.
                        container: t.pane.container(),
                    })
                    .collect(),
                pinned:       s.pinned,
                plugin_panel: Some(s.plugin_panel.clone()),
                file_browser_collapsed: s.file_browser_collapsed,
            }
        }).collect();

        let active_session_id = self.sessions
            .get(self.active_session)
            .map(|s| s.id);

        let split_view = self.split_view.as_ref().map(|sv| {
            crate::persisted::PersistedSplitView {
                top_id:    sv.top_id,
                bottom_id: sv.bottom_id,
            }
        });

        let state = crate::persisted::PersistedState {
            sessions, active_session_id, split_view,
        };
        state.save();
    }

    /// Point the agent panes in `targets` (of session `session_id`) at `model`
    /// from `tool`'s list, and report what that took.
    ///
    /// Each pane is switched the least disruptive way it supports:
    ///   - chat panes just record the override — the next turn resumes the same
    ///     conversation on the new model;
    ///   - a Build pane whose CLI takes an in-session command gets it typed into
    ///     its PTY (`/model …` for Claude Code), so the session keeps running;
    ///   - any other Build pane is relaunched with a new `--model`.
    ///
    /// Panes the model doesn't apply to — a `devin` pane when a Claude model
    /// was picked, say — are left alone and counted as skipped.
    ///
    /// A relaunched pane comes back under a new terminal id, so `targets` is
    /// rewritten in place: the caller's selection keeps following the same
    /// agents rather than pointing at ids that no longer exist.
    fn apply_model_change(
        &mut self,
        session_id: usize,
        targets:    &mut [TerminalId],
        tool:       crate::agent::AiTool,
        model:      crate::agent::ToolModel,
    ) -> ModelChangeOutcome {
        let mut outcome = ModelChangeOutcome::default();
        let Some(idx) = self.sessions.iter().position(|s| s.id == session_id) else {
            return outcome;
        };

        // Panes that need a relaunch are collected first: each one needs a
        // fresh terminal id, and handing those out borrows `self`.
        let mut respawn: Vec<usize> = Vec::new();
        {
            let session = &mut self.sessions[idx];
            for (slot, &tid) in targets.iter().enumerate() {
                let Some(pane) = session.panes.iter_mut().find(|p| p.id() == tid) else {
                    continue;
                };
                if !pane.accepts_models_from(tool) {
                    outcome.skipped += 1;
                    continue;
                }
                match pane {
                    AgentPane::Chat(c) => {
                        c.model = model;
                        outcome.in_place += 1;
                    }
                    // Nothing has launched yet, so the choice just replaces the
                    // model the pane will spawn with — no relaunch needed.
                    AgentPane::Pending(p) => {
                        p.model = model;
                        outcome.in_place += 1;
                    }
                    // A pane still sitting in the install-guidance shell has no
                    // CLI to type into, so it respawns — which also picks the
                    // CLI up if it has been installed in the meantime.
                    AgentPane::Terminal(t) => match tool.model_command(model)
                        .filter(|_| t.agent_cli_ready)
                    {
                        Some(cmd) => {
                            // Typed at the CLI exactly as the user would, so a
                            // busy session queues it like any other input.
                            t.write_input(format!("{cmd}\r").as_bytes());
                            t.agent_model = model;
                            outcome.in_place += 1;
                        }
                        None => respawn.push(slot),
                    },
                }
            }
        }

        for slot in respawn {
            let new_id = self.next_terminal_id;
            self.next_terminal_id += 1;
            if self.sessions[idx].respawn_agent(targets[slot], new_id, tool, model) {
                targets[slot] = new_id;
                outcome.respawned += 1;
            } else {
                // The pane vanished between the two passes — give the id back.
                self.next_terminal_id -= 1;
            }
        }
        outcome
    }

    pub fn title(&self) -> String {
        let Some(session) = self.sessions.get(self.active_session) else {
            return "frms".to_string();
        };
        match &session.editor.path {
            Some(p) => format!(
                "{} — {} — frms",
                p.file_name().unwrap_or_default().to_string_lossy(),
                session.name,
            ),
            None => format!("{} — frms", session.name),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            // ── splash screen ─────────────────────────────────────────────────
            Message::SplashDismissed => {
                self.showing_splash = false;
                self.splash_yt_dlp  = None;  // drops the video subscription
                self.splash_frame   = None;
                for mut child in self.splash_audio_children.drain(..) {
                    let _ = child.kill();
                }
            }
            Message::SplashYtDlpReady(Ok(yt_dlp_path)) => {
                const YT_URL: &str = "https://youtu.be/UisAg3bhp-g";
                // Spawn audio pipeline immediately (yt-dlp → ffmpeg → sound device).
                self.splash_audio_children =
                    crate::splash_video::spawn_audio(&yt_dlp_path, YT_URL);
                // Store the path — the subscription() method uses it to start
                // the video frame pipeline.
                self.splash_yt_dlp = Some(yt_dlp_path);
            }
            Message::SplashYtDlpReady(Err(e)) => {
                eprintln!("[splash] yt-dlp unavailable: {e}");
                // Splash continues without video — static fallback.
            }
            Message::SplashVideoFrame(jpeg) => {
                self.splash_frame = Some(jpeg);
            }
            Message::SplashVideoEnded => {
                eprintln!("[splash] video stream ended — starting fade");
                self.splash_anim = SplashAnim::FadeOut(0.0);
            }
            Message::SplashAnimTick => {
                // ~40 ticks per fade phase at 16 ms = ~640 ms per fade.
                const STEP: f32 = 0.025;
                match self.splash_anim {
                    SplashAnim::FadeOut(t) => {
                        let next = t + STEP;
                        if next >= 1.0 {
                            self.splash_anim = SplashAnim::FadeIn(0.0);
                        } else {
                            self.splash_anim = SplashAnim::FadeOut(next);
                        }
                    }
                    SplashAnim::FadeIn(t) => {
                        let next = t + STEP;
                        if next >= 1.0 {
                            self.splash_anim = SplashAnim::Text;
                        } else {
                            self.splash_anim = SplashAnim::FadeIn(next);
                        }
                    }
                    _ => {}
                }
            }

            // ── session management ────────────────────────────────────────────
            Message::SessionSelected(idx) => {
                if idx < self.sessions.len() {
                    self.active_session = idx;
                    self.save_persisted();
                    return self.reveal_chat_panes();
                }
            }
            Message::AgentPaneSelected(index) => {
                // Straight to pane N. This matters beyond convenience: a Build
                // pane starts its CLI the first time it comes on screen, so
                // *cycling* from pane 1 to pane 9 launches all nine. Jumping
                // puts only the target on screen, and only the target spawns.
                let idx = self.active_session;
                if let Some(tid) = self.sessions[idx].panes.get(index).map(|p| p.id()) {
                    self.focus_pane(idx, tid);
                    self.save_persisted();
                    return self.reveal_chat_panes();
                }
            }
            Message::NewSessionRequested => {
                let start = self.prefs.new_session_start_dir();
                self.new_session_dir  = start.to_string_lossy().into_owned();
                self.new_session_name = String::new();
                self.new_session_file_browser.navigate(start);
                self.creating_session = true;
            }
            Message::NewSessionDirEdited(s) => {
                // If a valid directory is typed, navigate the picker to it
                let p = PathBuf::from(s.trim());
                if p.is_dir() {
                    self.new_session_file_browser.navigate(p);
                }
                self.new_session_dir = s;
            }
            Message::NewSessionDirBrowsed(path) => {
                // Navigate picker and update the text input
                self.new_session_file_browser.navigate(path.clone());
                self.new_session_dir = path.to_string_lossy().into_owned();
            }
            Message::NewSessionNameEdited(s) => {
                self.new_session_name = s;
            }
            Message::NewSessionCreated(kind) => {
                self.creating_session = false;
                let dir = PathBuf::from(self.new_session_dir.trim());
                let dir = if dir.is_dir() { Some(dir) } else { None };
                let name = {
                    let trimmed = self.new_session_name.trim();
                    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
                };
                self.new_session_name.clear();
                self.push_session(kind, name, dir);
                self.save_persisted();
            }
            Message::NewSessionCancelled => {
                if !self.sessions.is_empty() {
                    self.creating_session = false;
                }
            }
            Message::SessionCloseRequested(idx) => {
                // The ✕ is a fat-finger magnet next to the pin/split icons —
                // ask before closing instead of acting immediately.
                if self.sessions.len() > 1 {
                    self.pending_close = Some(PendingClose::Session(idx));
                }
            }
            Message::ClosePendingConfirmed => {
                self.confirm_pending_close();
                // Closing a tab/session hands the center area to a neighbour,
                // which may be a chat pane.
                return self.reveal_chat_panes();
            }
            Message::ClosePendingCancelled => {
                self.pending_close = None;
            }
            Message::SessionRenameStarted(idx) => {
                let draft = self.sessions[idx].name.clone();
                self.renaming_session = Some((idx, draft));
                return text_input::focus(text_input::Id::new("session-rename"));
            }
            Message::SessionRenameEdited(s) => {
                if let Some((_, ref mut draft)) = self.renaming_session {
                    *draft = s;
                }
            }
            Message::SessionRenameConfirmed => {
                if let Some((idx, name)) = self.renaming_session.take() {
                    if !name.trim().is_empty() {
                        self.sessions[idx].name = name;
                        self.save_persisted();
                    }
                }
            }
            Message::SessionRenameCancelled => {
                self.renaming_session = None;
            }
            Message::SessionSplitToggled(target_id) => {
                let current_id = self.sessions[self.active_session].id;
                let already_in_split = matches!(
                    &self.split_view,
                    Some(sv) if sv.top_id == target_id || sv.bottom_id == target_id
                );
                if already_in_split {
                    // Clicking the split icon on either of the two currently-
                    // shown sessions unsplits the view.
                    self.split_view = None;
                } else if current_id != target_id
                    && self.sessions.iter().any(|s| s.id == target_id)
                {
                    let panes = pane_grid::State::with_configuration(Configuration::Split {
                        axis:  pane_grid::Axis::Horizontal,
                        ratio: 0.5,
                        a: Box::new(Configuration::Pane(current_id)),
                        b: Box::new(Configuration::Pane(target_id)),
                    });
                    self.split_view = Some(SplitView {
                        top_id:    current_id,
                        bottom_id: target_id,
                        panes,
                    });
                }
                self.save_persisted();
                // Splitting/unsplitting changes which sessions are rendered.
                return self.reveal_chat_panes();
            }

            Message::SessionPinToggled(target_id) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == target_id) {
                    s.pinned = !s.pinned;
                }
                self.save_persisted();
            }

            Message::SplitSlotSessionSelected(slot, idx) => {
                if let Some(sv) = self.split_view.as_mut() {
                    if let Some(new_id) = self.sessions.get(idx).map(|s| s.id) {
                        let (slot_id, other_id) = match slot {
                            SplitSlot::Top    => (sv.top_id, sv.bottom_id),
                            SplitSlot::Bottom => (sv.bottom_id, sv.top_id),
                        };
                        if new_id != slot_id && new_id != other_id {
                            // Re-tag the corresponding pane in the outer grid
                            // so its contents flip to the chosen session.
                            for (_p, sid) in sv.panes.iter_mut() {
                                if *sid == slot_id { *sid = new_id; }
                            }
                            match slot {
                                SplitSlot::Top    => sv.top_id    = new_id,
                                SplitSlot::Bottom => sv.bottom_id = new_id,
                            }
                            self.active_session = idx;
                            self.save_persisted();
                            return self.reveal_chat_panes();
                        }
                    }
                }
            }

            // ── layout ────────────────────────────────────────────────────────
            Message::PaneResized(session_id, pane_grid::ResizeEvent { split, ratio }) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.layout.panes.resize(split, ratio);
                }
            }
            Message::SplitResized(pane_grid::ResizeEvent { split, ratio }) => {
                if let Some(sv) = self.split_view.as_mut() {
                    sv.panes.resize(split, ratio);
                }
            }

            // ── file browser ──────────────────────────────────────────────────
            Message::BrowseDir(path) => {
                self.sessions[self.active_session].file_browser.navigate(path);
            }
            Message::OpenFile(path) => {
                let s = &mut self.sessions[self.active_session];
                s.editor.open(path);
                // Selecting a file means the user wants to see it — flip the
                // center pane to the Editor tab even if a Claude/Database tab
                // was showing.
                s.center_tab = crate::session::CenterTab::Editor;
                self.save_persisted();
            }
            Message::FileBrowserToggleCollapsed => {
                let s = &mut self.sessions[self.active_session];
                // Browser sessions have no file-browser pane, so the chord is a
                // no-op there rather than flipping a flag the user can't see —
                // which would otherwise surprise them on the next Terminal
                // session they switch to.
                if matches!(s.kind, crate::session::SessionKind::Terminal) {
                    s.file_browser_collapsed = !s.file_browser_collapsed;
                    self.save_persisted();
                }
            }
            Message::CopyPath(path) => {
                return Task::perform(crate::clipboard::write(path), |_| Message::ClipboardWritten);
            }
            Message::DirEditStart(edit) => {
                let fb = self.dir_edit_browser();
                fb.edit = Some(edit);
                fb.error = None;
            }
            Message::DirEditDraftChanged(s) => {
                if let Some(DirEdit::Create { draft } | DirEdit::Rename { draft, .. }) =
                    &mut self.dir_edit_browser().edit
                {
                    *draft = s;
                }
            }
            Message::DirEditConfirm => {
                self.dir_edit_browser().confirm_edit();
            }
            Message::DirEditCancel => {
                let fb = self.dir_edit_browser();
                fb.edit = None;
                fb.error = None;
            }

            // ── editor ────────────────────────────────────────────────────────
            Message::EditorAction(action) => {
                let e = &mut self.sessions[self.active_session].editor;
                if action.is_edit() {
                    e.dirty = true;
                }
                e.content.perform(action);
            }
            Message::EditorSave => {
                self.sessions[self.active_session].editor.save();
            }
            Message::EditorTogglePreview => {
                let s = &mut self.sessions[self.active_session];
                let sid = s.id;
                let e = &mut s.editor;
                e.preview_active = !e.preview_active;
                if e.preview_active && e.preview_kind() == Some(crate::editor::PreviewKind::Html) {
                    if let Some(path) = e.path.clone() {
                        let needs_render = e.preview_handle.is_none()
                            || e.preview_image_path.as_deref() != Some(path.as_path());
                        if needs_render {
                            e.preview_loading = true;
                            e.preview_error   = None;
                            return Task::perform(
                                render_html_preview(sid, path),
                                |(sid, path, r)| Message::EditorPreviewReady(sid, path, r),
                            );
                        }
                    }
                }
            }
            Message::EditorRefreshPreview => {
                let s = &mut self.sessions[self.active_session];
                let sid = s.id;
                let e = &mut s.editor;
                if let Some(path) = e.path.clone() {
                    if e.preview_kind() == Some(crate::editor::PreviewKind::Html) {
                        e.preview_loading = true;
                        e.preview_error   = None;
                        return Task::perform(
                            render_html_preview(sid, path),
                            |(sid, path, r)| Message::EditorPreviewReady(sid, path, r),
                        );
                    }
                }
            }
            Message::EditorPreviewReady(session_id, path, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    let e = &mut s.editor;
                    // Drop stale results from a previously-open file.
                    if e.path.as_deref() != Some(path.as_path()) {
                        return Task::none();
                    }
                    e.preview_loading = false;
                    match result {
                        Ok(bytes) => {
                            // Built here, once, rather than per-frame in view().
                            e.preview_handle     = Some(image::Handle::from_bytes(bytes));
                            e.preview_image_path = Some(path);
                            e.preview_error      = None;
                        }
                        Err(err) => {
                            e.preview_error = Some(err);
                        }
                    }
                }
            }

            // ── terminals (any session) ───────────────────────────────────────
            Message::TerminalData(id, bytes) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(t) = s.terminal_mut(id) {
                        t.process(&bytes);
                        // Re-check whether Claude is stopped on a question so
                        // the tab/session attention highlight tracks reality.
                        // Runs for background sessions too — that's the point:
                        // alert the user about sessions they aren't watching.
                        let menu_up = t.grid.lock()
                            .map(|g| claude_prompt::is_waiting_on_user(&g))
                            .unwrap_or(false);
                        t.note_menu_on_screen(menu_up);
                    }
                    Self::reconcile_prompt(s, id);
                } else if let Some(s) = self.session_for_plugin_terminal_mut(id) {
                    if let Some(t) = s.terminals_plugin.terminal_mut(id) {
                        t.pane.process(&bytes);
                    }
                } else if let Some(t) = self.receivers.ssh_mut(id) {
                    t.pane.process(&bytes);
                }
            }
            Message::TerminalExited(id) => {
                // The shell in this terminal exited (e.g. the user typed
                // `exit`). Remove plugin shell terminals so a dead session
                // doesn't linger. Claude panes are managed through their own
                // session/tab lifecycle, so they're left untouched here.
                // A receiver SSH session that exited (auth failure, dropped
                // connection, or the user typed `exit`) keeps its pane so the
                // final output stays readable — mark it exited and drop focus.
                if let Some(t) = self.receivers.ssh_mut(id) {
                    t.exited  = true;
                    t.focused = false;
                }
                let removed = if let Some(s) = self.session_for_plugin_terminal_mut(id) {
                    s.terminals_plugin.delete(id);
                    true
                } else {
                    false
                };
                if removed {
                    // A pinned terminal that exited shouldn't be respawned on
                    // the next launch — re-snapshot the persisted session.
                    self.save_persisted();
                }
            }
            Message::FindToggled => {
                // The pane in front, or — when the center is showing the
                // editor / database instead — the session's active agent.
                let s = &self.sessions[self.active_session];
                let id = match s.center_tab {
                    crate::session::CenterTab::Claude(tid) => tid,
                    _ => s.active_terminal,
                };
                return self.update(Message::FindOpened(id));
            }
            Message::FindOpened(id) => {
                // Toggle: the same chord that opens the bar closes it, since
                // Escape belongs to the CLI running in the pane.
                let open = self.find_is_open(id);
                if open {
                    self.find_close(id);
                } else {
                    self.find_open(id);
                    return text_input::focus(crate::ui::editor::find_input_id(id));
                }
            }
            Message::FindQueryChanged(id, q) => {
                if let Some(t) = self.terminal_pane_mut(id) {
                    t.find_query(q);
                } else if let Some(c) = self.chat_pane_mut(id) {
                    c.find_query(q);
                    return self.reveal_chat_hit(id);
                }
            }
            Message::FindStepped(id, forward) => {
                if let Some(t) = self.terminal_pane_mut(id) {
                    t.find_step(forward);
                } else if let Some(c) = self.chat_pane_mut(id) {
                    c.find_step(forward);
                    return self.reveal_chat_hit(id);
                }
            }
            Message::FindClosed(id) => {
                self.find_close(id);
            }
            Message::AgentTabsNudged { forward } => {
                let dx = if forward {
                    crate::ui::editor::AGENT_TAB_NUDGE
                } else {
                    -crate::ui::editor::AGENT_TAB_NUDGE
                };
                // Clamped by the widget, so clicking at either end is a no-op.
                return scrollable::scroll_by(
                    crate::ui::editor::agent_strip_id(),
                    scrollable::AbsoluteOffset { x: dx, y: 0.0 },
                );
            }
            Message::AgentStripViewport(v) => {
                self.agent_strip_overflow =
                    v.content_bounds().width > v.bounds().width;
            }
            // Handled entirely by `refresh_agent_strip_overflow` in the `update`
            // wrapper — the window's new size is iced's business, not ours.
            Message::WindowResized => {}
            Message::AgentsDialogOpened => {
                let s = &self.sessions[self.active_session];
                // Start on the agent that's in front: the common case is
                // "switch this one", and "All" is one click away for the rest.
                let front = match s.center_tab {
                    crate::session::CenterTab::Claude(tid) => tid,
                    _ => s.active_terminal,
                };
                let selected = s.panes.iter()
                    .map(|p| p.id())
                    .filter(|id| *id == front)
                    .collect();
                self.agents_dialog = Some(AgentsDialog {
                    session_id: s.id,
                    selected,
                    choice:     None,
                    outcome:    None,
                    confirming: None,
                });
            }
            Message::AgentsDialogClosed => {
                self.agents_dialog = None;
            }
            Message::AgentsDialogPaneToggled(tid) => {
                if let Some(d) = &mut self.agents_dialog {
                    match d.selected.iter().position(|id| *id == tid) {
                        Some(i) => { d.selected.remove(i); }
                        None    => d.selected.push(tid),
                    }
                    // The tally described the previous selection.
                    d.outcome = None;
                }
            }
            Message::AgentsDialogSelectAll(all) => {
                let ids: Vec<TerminalId> = if all {
                    self.agents_dialog.as_ref()
                        .and_then(|d| self.sessions.iter().find(|s| s.id == d.session_id))
                        .map(|s| s.panes.iter().map(|p| p.id()).collect())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                if let Some(d) = &mut self.agents_dialog {
                    d.selected = ids;
                    d.outcome  = None;
                }
            }
            Message::AgentsDialogModelPicked(tool, model) => {
                if let Some(d) = &mut self.agents_dialog {
                    d.choice  = Some((tool, model));
                    d.outcome = None;
                }
            }
            Message::AgentsDialogApplied => {
                let Some(d) = &self.agents_dialog else { return Task::none(); };
                let Some((tool, model)) = d.choice else { return Task::none(); };
                let session_id  = d.session_id;
                let mut targets = d.selected.clone();
                let outcome     = self.apply_model_change(session_id, &mut targets, tool, model);
                if outcome.changed() > 0 {
                    self.telemetry.record(crate::telemetry::Event::AgentModelChanged {
                        tool:  tool.as_str(),
                        count: outcome.changed(),
                    });
                    self.save_persisted();
                }
                if let Some(d) = &mut self.agents_dialog {
                    // Relaunched panes came back under new ids; keep them ticked.
                    d.selected = targets;
                    d.outcome  = Some(outcome);
                }
            }
            Message::AgentsDialogPaneCloseRequested(tid) => {
                if let Some(d) = &mut self.agents_dialog {
                    d.confirming = Some(tid);
                }
            }
            Message::AgentsDialogPaneCloseCancelled => {
                if let Some(d) = &mut self.agents_dialog {
                    d.confirming = None;
                }
            }
            Message::AgentsDialogPaneClosed(tid) => {
                // Act on the session the dialog captured at open time, so a
                // session switch behind the overlay can't redirect the close.
                let sid = self.agents_dialog.as_ref().map(|d| d.session_id);
                if let Some(s) = sid.and_then(|sid| self.sessions.iter_mut().find(|s| s.id == sid)) {
                    if s.close_claude_terminal(tid) {
                        self.save_persisted();
                    }
                }
                if let Some(d) = &mut self.agents_dialog {
                    d.confirming = None;
                    d.selected.retain(|id| *id != tid);
                    // The tally described a selection that no longer exists.
                    d.outcome = None;
                }
            }
            Message::NewDefaultAgent => {
                let tool = self.prefs.default_tool;
                let model = *tool.default_model();
                return self.update(Message::AgentAdded(AgentKind::Build, tool, model));
            }
            Message::AgentAdded(kind, tool, model) => {
                // Adding from the Agents dialog focuses the new pane, so the
                // dialog has served its purpose — get out of the way of it.
                self.agents_dialog = None;
                let new_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                self.pane_focus_toolbox = false;
                let s = &mut self.sessions[self.active_session];
                s.add_agent(new_id, kind, tool, model);
                s.active_terminal = new_id;
                s.center_tab      = crate::session::CenterTab::Claude(new_id);
                self.telemetry.record(crate::telemetry::Event::AgentCreated { kind: kind.as_str() });
                self.save_persisted();
                if !kind.is_agentic() {
                    // Drop keyboard focus into the new chat's composer so the
                    // user can type immediately. The composer is a `text_editor`,
                    // which carries no focusable id (unlike `text_input`), so we
                    // step focus onto it with `focus_next` — the same approach the
                    // Notes body editor uses. A freshly created chat has an empty
                    // transcript, so its composer is the only focusable rendered
                    // in the center pane and focus lands on it.
                    return iced::widget::focus_next();
                }
            }
            Message::ChatInputAction(id, action) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.input.perform(action);
                    }
                }
            }
            Message::ChatBubbleAction(id, index, action) => {
                // Transcript bubbles are read-only: apply cursor/selection
                // movement so the user can highlight, but drop edits so the
                // past reply can't be mutated.
                if matches!(action, text_editor::Action::Edit(_)) {
                    return Task::none();
                }
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        if let Some(m) = c.messages.get_mut(index) {
                            m.content.perform(action);
                        }
                    }
                }
            }
            Message::ChatBubbleCopy(id, index) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        if let Some(sel) = c.messages.get(index)
                            .and_then(|m| m.content.selection())
                            .filter(|s| !s.is_empty())
                        {
                            return Task::perform(crate::clipboard::write(sel), |_| Message::ClipboardWritten);
                        }
                    }
                }
            }
            Message::ChatSendToNote(id, index) => {
                let text = self.session_for_terminal_mut(id)
                    .and_then(|s| s.chat_mut(id))
                    .and_then(|c| c.messages.get(index))
                    .and_then(|m| m.content.selection())
                    .unwrap_or_default();
                self.text_to_note(&text);
            }
            Message::ChatCopy(id) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        if let Some(sel) = c.input.selection().filter(|s| !s.is_empty()) {
                            return Task::perform(crate::clipboard::write(sel), |_| Message::ClipboardWritten);
                        }
                    }
                }
            }
            Message::ChatCut(id) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        if let Some(sel) = c.input.selection().filter(|s| !s.is_empty()) {
                            // Match iced's built-in Cut: copy the selection out, then delete it.
                            c.input.perform(text_editor::Action::Edit(text_editor::Edit::Delete));
                            return Task::perform(crate::clipboard::write(sel), |_| Message::ClipboardWritten);
                        }
                    }
                }
            }
            Message::ChatPaste(id) => {
                return Task::perform(crate::clipboard::read(), move |c| Message::ChatPasteReady(id, c));
            }
            Message::ChatPasteReady(id, contents) => {
                let Some(text) = contents.filter(|s| !s.is_empty()) else { return Task::none(); };
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.input.perform(text_editor::Action::Edit(
                            text_editor::Edit::Paste(std::sync::Arc::new(text)),
                        ));
                    }
                }
            }
            Message::ChatSubmitted(id) => {
                // The pane's tools work in its own project directory, so the
                // run is rooted there (captured before the mutable borrow).
                let access = self.prefs.chat_access;
                let Some(s) = self.session_for_terminal_mut(id) else { return Task::none(); };
                let cwd = s.working_dir.clone();
                let Some(c) = s.chat_mut(id) else { return Task::none(); };
                let text = c.input.text().trim().to_string();
                if text.is_empty() || c.streaming {
                    return Task::none();
                }
                c.input = text_editor::Content::new();
                c.error = None;
                c.pending.clear();
                c.activity.clear();
                c.push_message(ChatRole::User, &text);
                c.streaming = true;
                let model  = c.model();
                let resume = c.session_id.clone();
                return Task::run(
                    chat_api::stream_completion(
                        id, model, text, resume, Some(cwd), access,
                        Message::ChatDelta,
                        Message::ChatActivity,
                        Message::ChatEnded,
                    ),
                    |m| m,
                );
            }
            Message::ChatActivity(id, line) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        // Collapse an immediate repeat (Claude re-reading the
                        // same file, say) so the list stays scannable.
                        if c.activity.last().map(String::as_str) != Some(line.as_str()) {
                            c.activity.push(line);
                        }
                    }
                }
                return self.pin_chat_tail(id);
            }
            Message::ChatDelta(id, chunk) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.pending.push_str(&chunk);
                    }
                }
                // Keep the transcript pinned to the newest text as it streams in,
                // so the answer stays visible without manual scrolling.
                return self.pin_chat_tail(id);
            }
            Message::ChatScrolled(id, viewport) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        // Treat "within a line of the bottom" as still at the
                        // bottom: auto-scroll survives rounding and the tiny lag
                        // between a chunk arriving and the snap landing, but a
                        // deliberate scroll back through the transcript sticks.
                        let off = viewport.absolute_offset().y;
                        let max = (viewport.content_bounds().height
                            - viewport.bounds().height).max(0.0);
                        c.follow_tail = off >= max - 20.0;
                    }
                }
            }
            Message::ChatEnded(id, result) => {
                // Outcome captured here, recorded after the session borrow ends.
                let mut tel: Option<crate::telemetry::Event> = None;
                // A reply that lands while the pane is off-screen lights its tab
                // (and its session tab), the same alert Build panes get when
                // Claude stops on a question.
                let offscreen = !self.pane_on_screen(id);
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.streaming = false;
                        c.unread    = offscreen;
                        // Commit whatever text streamed in (a failed request may
                        // still have produced a partial reply worth keeping).
                        if !c.pending.is_empty() {
                            let text = std::mem::take(&mut c.pending);
                            c.push_message(ChatRole::Assistant, &text);
                        }
                        match result {
                            // Remember the session so the next turn resumes it.
                            Ok(session) => {
                                if !session.is_empty() {
                                    c.session_id = Some(session);
                                }
                                tel = Some(crate::telemetry::Event::ChatCompleted { model: c.model() });
                            }
                            Err(e) => {
                                tel = Some(crate::telemetry::Event::Error {
                                    kind: "chat", detail: e.clone(),
                                });
                                c.error = Some(e);
                            }
                        }
                    }
                }
                if let Some(event) = tel {
                    self.telemetry.record(event);
                }
                // Final snap so the completed answer is fully in view.
                return self.pin_chat_tail(id);
            }
            Message::ClaudeSessionClosed(tid) => {
                // Same accidental-click guard as session tabs: confirm first.
                self.pending_close = Some(PendingClose::Claude(tid));
            }
            Message::ClaudeRenameStarted(tid) => {
                let sess = &self.sessions[self.active_session];
                let draft = sess.panes.iter()
                    .find(|p| p.id() == tid)
                    .and_then(|p| p.name().map(str::to_owned))
                    .unwrap_or_else(|| {
                        // Fall back to the displayed positional label so the
                        // user starts editing what they actually see.
                        let idx = sess.panes.iter().position(|p| p.id() == tid).unwrap_or(0);
                        let kind = sess.panes.get(idx).map(|p| p.kind()).unwrap_or_default();
                        format!("{} {}", kind.label(), idx + 1)
                    });
                self.renaming_claude = Some((tid, draft));
                return text_input::focus(text_input::Id::new("claude-rename"));
            }
            Message::ClaudeRenameEdited(s) => {
                if let Some((_, ref mut draft)) = self.renaming_claude {
                    *draft = s;
                }
            }
            Message::ClaudeRenameConfirmed => {
                if let Some((tid, name)) = self.renaming_claude.take() {
                    let trimmed = name.trim().to_string();
                    if let Some(p) = self.sessions
                        .iter_mut()
                        .flat_map(|s| s.panes.iter_mut())
                        .find(|p| p.id() == tid)
                    {
                        p.set_name(if trimmed.is_empty() { None } else { Some(trimmed) });
                    }
                    self.save_persisted();
                }
            }
            Message::ClaudeRenameCancelled => {
                self.renaming_claude = None;
            }
            Message::TerminalInput(s) => {
                // A full-screen picker overlay swallows keystrokes so they
                // don't leak into the terminal hidden behind it.
                if self.file_picker.is_some() {
                    return Task::none();
                }
                // While the close-confirmation dialog is up, keys must not
                // leak into the terminal behind it: Enter confirms, Esc
                // cancels, everything else is swallowed.
                if self.pending_close.is_some() {
                    match s.as_str() {
                        "\r" | "\n" => {
                            self.confirm_pending_close();
                            return self.reveal_chat_panes();
                        }
                        "\x1b" => self.pending_close = None,
                        _      => {}
                    }
                    return Task::none();
                }
                // A focused receiver SSH terminal (panel-global) wins over the
                // active session's terminals.
                if let Some(t) = self.receivers.focused_ssh_mut() {
                    if let Ok(mut g) = t.pane.grid.lock() { g.snap_to_bottom(); }
                    t.pane.write_input(s.as_bytes());
                    return Task::none();
                }
                let sess = &mut self.sessions[self.active_session];
                if sess.terminals_plugin.focused {
                    if let Some(id) = sess.terminals_plugin.selected {
                        if let Some(t) = sess.terminals_plugin.terminal_mut(id) {
                            if let Ok(mut g) = t.pane.grid.lock() {
                                g.snap_to_bottom();
                            }
                            t.pane.write_input(s.as_bytes());
                        }
                    }
                } else {
                    let active_term = sess.active_terminal;
                    // Keystrokes can be routed at the active tab before it has
                    // been displayed, so launch it here rather than dropping the
                    // input into a pane with no PTY. No-op once it's live.
                    sess.spawn_pending(active_term);
                    if let Some(t) = sess.terminal_mut(active_term) {
                        // Snap the terminal back to the live bottom so the keystroke
                        // doesn't appear to vanish into history.
                        if let Ok(mut g) = t.grid.lock() {
                            g.snap_to_bottom();
                        }
                        t.write_input(s.as_bytes());
                    }
                }
            }
            Message::TerminalScroll(id, lines) => {
                // Wheel "up" yields positive y in iced — scroll into history.
                let delta = lines.round() as i32 * 3;
                if let Some(t) = self.terminal_pane_mut(id) {
                    if let Ok(mut g) = t.grid.lock() {
                        g.scroll_by(delta);
                    }
                }
            }
            Message::TerminalScrollDrag(id, lines, row, col) => {
                let delta = lines.round() as i32 * 3;
                if let Some(t) = self.terminal_pane_mut(id) {
                    t.scroll_drag(delta, row, col);
                }
            }
            Message::TerminalMouseDown(id, row, col) => {
                // Re-uses the same focus logic as `TerminalFocused` so a click
                // on either kind of terminal pane (Claude or shell) becomes the
                // input target before the selection drag begins.
                if let Some(idx) = self.sessions.iter()
                    .position(|s| s.panes.iter().any(|p| p.id() == id))
                {
                    self.active_session = idx;
                    self.pane_focus_toolbox = false;
                    let s = &mut self.sessions[idx];
                    s.active_terminal          = id;
                    s.center_tab               = crate::session::CenterTab::Claude(id);
                    s.db_panel.focus_active    = false;
                    s.terminals_plugin.focused = false;
                    self.receivers.unfocus_ssh();
                    eprintln!("[copydbg] MouseDown claude id={id} ({row},{col})");
                    if let Some(t) = s.terminal_mut(id) {
                        t.begin_selection(row, col);
                    }
                } else if let Some(idx) = self.sessions.iter()
                    .position(|s| s.terminals_plugin.has_terminal(id))
                {
                    self.active_session = idx;
                    let s = &mut self.sessions[idx];
                    s.terminals_plugin.select(id);
                    s.terminals_plugin.focused = true;
                    s.db_panel.focus_active    = false;
                    self.receivers.unfocus_ssh();
                    if let Some(t) = s.terminals_plugin.terminal_mut(id) {
                        t.pane.begin_selection(row, col);
                    }
                } else if self.receivers.has_ssh(id) {
                    // Focus the clicked receiver SSH terminal and begin a
                    // selection, dropping focus from any other SSH session.
                    self.sessions[self.active_session].terminals_plugin.focused = false;
                    self.sessions[self.active_session].db_panel.focus_active    = false;
                    self.receivers.unfocus_ssh();
                    if let Some(t) = self.receivers.ssh_mut(id) {
                        t.focused   = true;
                        t.pane.begin_selection(row, col);
                    }
                }
            }
            Message::TerminalMouseMove(id, row, col) => {
                eprintln!("[copydbg] MouseMove id={id} ({row},{col})");
                if let Some(t) = self.terminal_pane_mut(id) {
                    t.drag_selection(row, col);
                }
            }
            Message::TerminalMouseUp(id) => {
                // A click without drag — anchor == head — leaves nothing to
                // copy, so clear it. Real drags stay around as an inactive
                // selection until the next click or copy.
                let clear_if_empty = |sel: &mut Option<crate::terminal::Selection>| {
                    if let Some(s) = sel.as_mut() {
                        s.active = false;
                        if s.is_empty() { *sel = None; }
                    }
                };
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(t) = s.terminal_mut(id) { clear_if_empty(&mut t.selection); }
                } else if let Some(s) = self.session_for_plugin_terminal_mut(id) {
                    if let Some(t) = s.terminals_plugin.terminal_mut(id) {
                        clear_if_empty(&mut t.pane.selection);
                    }
                } else if let Some(t) = self.receivers.ssh_mut(id) {
                    clear_if_empty(&mut t.pane.selection);
                }
            }
            Message::TerminalCopy => {
                let sess = &self.sessions[self.active_session];
                let text = if sess.terminals_plugin.focused {
                    sess.terminals_plugin.selected
                        .and_then(|id| sess.terminals_plugin.terminal(id))
                        .and_then(|t| {
                            let sel = t.pane.selection?;
                            let g = t.pane.grid.lock().ok()?;
                            Some(sel.text(&g))
                        })
                } else {
                    sess.terminal(sess.active_terminal)
                        .and_then(|t| {
                            let sel = t.selection?;
                            let g = t.grid.lock().ok()?;
                            Some(sel.text(&g))
                        })
                };
                if let Some(t) = text.filter(|s| !s.is_empty()) {
                    return Task::perform(crate::clipboard::write(t), |_| Message::ClipboardWritten);
                }
            }
            Message::TerminalCopyFrom(id) => {
                eprintln!("[copydbg] TerminalCopyFrom({id})");
                // Pull the selection text and clear it in one pass (`take`),
                // working for both Claude panes and shell-plugin terminals.
                let text = if let Some(s) = self.session_for_terminal_mut(id) {
                    s.terminal_mut(id).and_then(|t| {
                        let sel = t.selection.take()?;
                        let g = t.grid.lock().ok()?;
                        Some(sel.text(&g))
                    })
                } else if let Some(s) = self.session_for_plugin_terminal_mut(id) {
                    s.terminals_plugin.terminal_mut(id).and_then(|t| {
                        let sel = t.pane.selection.take()?;
                        let g = t.pane.grid.lock().ok()?;
                        Some(sel.text(&g))
                    })
                } else if let Some(t) = self.receivers.ssh_mut(id) {
                    t.pane.selection.take().and_then(|sel| {
                        let g = t.pane.grid.lock().ok()?;
                        Some(sel.text(&g))
                    })
                } else {
                    None
                };
                eprintln!("[copydbg] TerminalCopyFrom text = {:?}", text);
                if let Some(t) = text.filter(|s| !s.is_empty()) {
                    eprintln!("[copydbg] writing {} bytes to clipboard", t.len());
                    return Task::perform(crate::clipboard::write(t), |_| Message::ClipboardWritten);
                }
            }
            Message::TerminalSendToNote => {
                // Resolve the focused terminal the same way TerminalCopy does:
                // a focused shell-plugin terminal wins, otherwise the active
                // Claude pane.
                let sess = &self.sessions[self.active_session];
                let id = if sess.terminals_plugin.focused {
                    sess.terminals_plugin.selected
                } else {
                    Some(sess.active_terminal)
                };
                if let Some(id) = id {
                    self.selection_to_note(id);
                }
            }
            Message::TerminalSendToNoteFrom(id) => {
                self.selection_to_note(id);
            }
            Message::TerminalPasteInto(id) => {
                eprintln!("[copydbg] TerminalPasteInto({id}) — (no selection, so paste)");
                // Focus the clicked terminal so the subsequent paste lands in
                // it, mirroring the focus logic in `TerminalMouseDown`.
                if let Some(idx) = self.sessions.iter()
                    .position(|s| s.panes.iter().any(|p| p.id() == id))
                {
                    self.active_session = idx;
                    self.pane_focus_toolbox = false;
                    let s = &mut self.sessions[idx];
                    s.active_terminal          = id;
                    s.center_tab               = crate::session::CenterTab::Claude(id);
                    s.db_panel.focus_active    = false;
                    s.terminals_plugin.focused = false;
                    self.receivers.unfocus_ssh();
                } else if let Some(idx) = self.sessions.iter()
                    .position(|s| s.terminals_plugin.has_terminal(id))
                {
                    self.active_session = idx;
                    let s = &mut self.sessions[idx];
                    s.terminals_plugin.select(id);
                    s.terminals_plugin.focused = true;
                    s.db_panel.focus_active    = false;
                    self.receivers.unfocus_ssh();
                } else if self.receivers.has_ssh(id) {
                    self.sessions[self.active_session].terminals_plugin.focused = false;
                    self.receivers.unfocus_ssh();
                    if let Some(t) = self.receivers.ssh_mut(id) { t.focused = true; }
                }
                return Task::perform(crate::clipboard::read(), Message::TerminalPasteReady);
            }
            Message::TerminalPaste => {
                return Task::perform(crate::clipboard::read(), Message::TerminalPasteReady);
            }
            Message::TerminalPasteReady(contents) => {
                let Some(text) = contents.filter(|s| !s.is_empty()) else {
                    return Task::none();
                };
                if let Some(t) = self.receivers.focused_ssh_mut() {
                    let bracketed = t.pane.grid.lock()
                        .map(|g| g.bracketed_paste)
                        .unwrap_or(false);
                    if let Ok(mut g) = t.pane.grid.lock() { g.snap_to_bottom(); }
                    t.pane.write_input(paste_payload(&text, bracketed).as_bytes());
                    return Task::none();
                }
                let sess = &mut self.sessions[self.active_session];
                if sess.terminals_plugin.focused {
                    if let Some(id) = sess.terminals_plugin.selected {
                        if let Some(t) = sess.terminals_plugin.terminal_mut(id) {
                            let bracketed = t.pane.grid.lock()
                                .map(|g| g.bracketed_paste)
                                .unwrap_or(false);
                            if let Ok(mut g) = t.pane.grid.lock() { g.snap_to_bottom(); }
                            t.pane.write_input(paste_payload(&text, bracketed).as_bytes());
                        }
                    }
                } else {
                    let active_term = sess.active_terminal;
                    sess.spawn_pending(active_term);
                    if let Some(t) = sess.terminal_mut(active_term) {
                        let bracketed = t.grid.lock()
                            .map(|g| g.bracketed_paste)
                            .unwrap_or(false);
                        if let Ok(mut g) = t.grid.lock() { g.snap_to_bottom(); }
                        t.write_input(paste_payload(&text, bracketed).as_bytes());
                    }
                }
            }
            Message::ClipboardWritten => {}
            Message::TabPressed { shift } => {
                // Swallow Tab while the close-confirmation dialog is up.
                if self.pending_close.is_some() {
                    return Task::none();
                }
                // The note body editor already indented on this press (its key
                // binding ran first, during widget event handling). Tab is
                // exempt from the `Status::Captured` gate in
                // `main_key_listener`, so we get the press anyway — drop it.
                if std::mem::take(&mut self.notes.tab_in_body) {
                    return Task::none();
                }
                let s = &self.sessions[self.active_session];
                let db_form_focused = s.center_tab == crate::session::CenterTab::Database
                    && !s.db_panel.form_collapsed
                    && s.db_panel.focus_active;
                if db_form_focused {
                    return if shift {
                        iced::widget::focus_previous()
                    } else {
                        iced::widget::focus_next()
                    };
                }
                // Tab out of the Terminals-plugin Name field into the shell.
                let naming_terminal = s.plugin_panel.visible
                    && s.plugin_panel.active_tab == PluginTab::Terminals
                    && s.terminals_plugin.naming;
                if naming_terminal {
                    let s = &mut self.sessions[self.active_session];
                    s.terminals_plugin.focus_terminal();
                    return text_input::focus(
                        text_input::Id::new("terminal-plugin-blur"),
                    );
                }
                // Tab out of the Notes-plugin Title field into the body
                // editor (which registers no widget Id — step to it).
                let naming_note = s.plugin_panel.visible
                    && (s.plugin_panel.active_tab == PluginTab::Notes
                        || (s.plugin_panel.split && s.plugin_panel.bottom_tab == PluginTab::Notes))
                    && self.notes.naming;
                if naming_note {
                    self.notes.naming = false;
                    return iced::widget::focus_next();
                }
                // The Receivers (SSH) panel shows credential/import forms whose
                // plain text_inputs don't auto-advance on Tab. When one of those
                // forms is up — and no SSH terminal has grabbed focus — walk
                // between its fields instead of emitting a tab byte.
                let receiver_form = s.plugin_panel.visible
                    && (s.plugin_panel.active_tab == PluginTab::Receivers
                        || (s.plugin_panel.split
                            && s.plugin_panel.bottom_tab == PluginTab::Receivers))
                    && (self.receivers.selected_receiver.is_some()
                        || self.receivers.selected_container.is_some());
                // Otherwise treat as a normal terminal Tab/Shift-Tab.
                let bytes: &[u8] = if shift { b"\x1b[Z" } else { b"\t" };
                // A focused receiver SSH terminal wins over the active session's
                // terminals — same precedence as ordinary keystrokes — so shell
                // Tab-completion works while connected to a receiver. (Clicking
                // the terminal focuses it; clicking back into a form field does
                // not, so the form-nav case below only runs when no SSH terminal
                // holds focus.)
                if let Some(t) = self.receivers.focused_ssh_mut() {
                    if let Ok(mut g) = t.pane.grid.lock() { g.snap_to_bottom(); }
                    t.pane.write_input(bytes);
                    return Task::none();
                }
                if receiver_form {
                    return if shift {
                        iced::widget::focus_previous()
                    } else {
                        iced::widget::focus_next()
                    };
                }
                let sess = &mut self.sessions[self.active_session];
                if sess.terminals_plugin.focused {
                    if let Some(id) = sess.terminals_plugin.selected {
                        if let Some(t) = sess.terminals_plugin.terminal_mut(id) {
                            t.pane.write_input(bytes);
                        }
                    }
                } else {
                    let active_term = sess.active_terminal;
                    // A chat/research pane has no PTY, so Tab can't be a terminal
                    // byte — cycle focus instead, letting the user step into the
                    // composer (an id-less text_editor reached via focus_next).
                    let active_is_chat = sess.panes.iter()
                        .find(|p| p.id() == active_term)
                        .map_or(false, |p| p.as_chat().is_some());
                    if active_is_chat {
                        return if shift {
                            iced::widget::focus_previous()
                        } else {
                            iced::widget::focus_next()
                        };
                    }
                    sess.spawn_pending(active_term);
                    if let Some(t) = sess.terminal_mut(active_term) {
                        t.write_input(bytes);
                    }
                }
            }

            // ── browser ───────────────────────────────────────────────────────
            Message::BrowserUrlEdited(s) => {
                self.sessions[self.active_session].url_input = s;
            }
            Message::BrowserUrlSubmitted => {
                eprintln!("[browser] BrowserUrlSubmitted fired");
                let session = &mut self.sessions[self.active_session];
                let url = normalize_url(session.url_input.trim());
                eprintln!("[browser] normalized url={url:?}");
                if !url.is_empty() {
                    session.browser_url      = url.clone();
                    session.screenshot_bytes = None;
                    session.browser_loading  = true;
                    session.browser_error    = None;
                    let sid  = session.id;
                    let port = session.cdp_port;
                    self.save_persisted();
                    let session = &self.sessions[self.active_session];
                    if session.cdp_active {
                        // CDP already running — navigate directly
                        return Task::perform(
                            cdp::navigate_and_screenshot(port, url),
                            move |r| match r {
                                Ok(b)  => Message::BrowserScreenshotReady(sid, b),
                                Err(e) => Message::BrowserScreenshotFailed(sid, e),
                            },
                        );
                    } else {
                        // Launch Chromium first, then navigate in CdpReady handler
                        return Task::perform(
                            cdp::launch(port),
                            move |r| match r {
                                Ok(()) => Message::CdpReady(sid),
                                Err(e) => Message::CdpLaunchFailed(sid, e),
                            },
                        );
                    }
                }
            }
            Message::BrowserRefresh => {
                let session = &mut self.sessions[self.active_session];
                let url = session.browser_url.clone();
                if !url.is_empty() && !session.browser_loading {
                    session.browser_loading = true;
                    session.browser_error   = None;
                    let sid  = session.id;
                    let port = session.cdp_port;
                    if session.cdp_active {
                        return Task::perform(
                            cdp::screenshot(port),
                            move |r| match r {
                                Ok(b)  => Message::BrowserScreenshotReady(sid, b),
                                Err(e) => Message::BrowserScreenshotFailed(sid, e),
                            },
                        );
                    } else {
                        return Task::perform(
                            take_screenshot(sid, url),
                            |(s, r)| match r {
                                Ok(b)  => Message::BrowserScreenshotReady(s, b),
                                Err(e) => Message::BrowserScreenshotFailed(s, e),
                            },
                        );
                    }
                }
            }
            Message::BrowserScreenshotReady(session_id, bytes) => {
                eprintln!("[browser] screenshot ready: session={session_id} bytes={}", bytes.len());
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.screenshot_bytes = Some(bytes);
                    s.browser_loading  = false;
                    s.browser_error    = None;
                } else {
                    eprintln!("[browser] WARNING: no session found for id={session_id}");
                }
            }
            Message::BrowserScreenshotFailed(session_id, err) => {
                eprintln!("[browser] screenshot failed: session={session_id} err={err}");
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.browser_loading = false;
                    s.browser_error   = Some(err);
                }
            }

            // ── CDP ────────────────────────────────────────────────────────────
            Message::CdpReady(sid) => {
                eprintln!("[browser] CDP ready: session={sid}");
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.cdp_active = true;
                    let port = s.cdp_port;
                    let url  = s.browser_url.clone();
                    return Task::perform(
                        cdp::navigate_and_screenshot(port, url),
                        move |r| match r {
                            Ok(b)  => Message::BrowserScreenshotReady(sid, b),
                            Err(e) => Message::BrowserScreenshotFailed(sid, e),
                        },
                    );
                }
            }
            Message::CdpLaunchFailed(sid, err) => {
                eprintln!("[browser] CDP launch failed: session={sid} err={err}");
                // CDP unavailable — fall back to a headless static screenshot
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    let url = s.browser_url.clone();
                    eprintln!("[browser] falling back to take_screenshot for url={url}");
                    return Task::perform(
                        take_screenshot(sid, url),
                        |(s, r)| match r {
                            Ok(b)  => Message::BrowserScreenshotReady(s, b),
                            Err(e) => Message::BrowserScreenshotFailed(s, e),
                        },
                    );
                }
            }
            Message::BrowserMouseMoved(x, y) => {
                self.sessions[self.active_session].browser_mouse_pos = Some((x, y));
            }
            Message::BrowserMousePressed => {
                let session = &mut self.sessions[self.active_session];
                if session.cdp_active && !session.browser_loading {
                    if let Some((mx, my)) = session.browser_mouse_pos {
                        session.browser_loading = true;
                        session.browser_error   = None;
                        let sid  = session.id;
                        let port = session.cdp_port;
                        return Task::perform(
                            cdp::click_and_screenshot(port, mx as f64, my as f64),
                            move |r| match r {
                                Ok(b)  => Message::BrowserScreenshotReady(sid, b),
                                Err(e) => Message::BrowserScreenshotFailed(sid, e),
                            },
                        );
                    }
                }
            }

            // ── prompt response ───────────────────────────────────────────────
            Message::PromptAccepted => {
                // Claude pre-highlights "❯ 1. Yes", so a bare Enter accepts the
                // focused option. Sending the literal "1" risked it landing in
                // the text input (if the menu wasn't focused at that instant)
                // and being submitted as a stray "1" message to Claude.
                self.respond_to_prompt(b"\r");
            }
            Message::PromptRejected => {
                // The Accept/Reject pane only renders for the 3-option edit
                // menu (Yes / Yes-don't-ask / No), with option 1 pre-focused.
                // Arrow down twice to reach "3. No, and tell Claude…" then
                // Enter — same idea as Accept: no literal digit that could leak
                // into the text input as a stray "3" message.
                self.respond_to_prompt(b"\x1b[B\x1b[B\r");
            }

            // ── window ────────────────────────────────────────────────────────
            Message::Quit => {
                self.shutdown();
            }

            // ── keyboard navigation ─────────────────────────────────────────────
            Message::CyclePane { forward } => {
                self.cycle_pane(forward);
                return self.reveal_chat_panes();
            }
            Message::CycleAgentPane { forward } => {
                self.cycle_agent_pane(forward);
                return self.reveal_chat_panes();
            }
            Message::CycleAttention { forward } => {
                self.cycle_attention(forward);
                return self.reveal_chat_panes();
            }
            Message::CycleSession { forward } => {
                let n = self.sessions.len();
                if n > 1 {
                    self.active_session = if forward {
                        (self.active_session + 1) % n
                    } else {
                        (self.active_session + n - 1) % n
                    };
                    self.pane_focus_toolbox = false;
                    self.save_persisted();
                    return self.reveal_chat_panes();
                }
            }
            Message::CyclePluginTab { forward } => {
                // Cycle the toolbox to its next/previous tool. Surface the panel
                // (and mark it the focused stop) so the switch is visible.
                let s = &mut self.sessions[self.active_session];
                s.plugin_panel.visible = true;
                s.plugin_panel.cycle_tab(forward);
                self.pane_focus_toolbox = true;
                self.save_persisted();
            }
            Message::CloseActiveAgent => {
                // Same confirm flow as the agent tab's ✕.
                let tid = self.sessions[self.active_session].active_terminal;
                self.pending_close = Some(PendingClose::Claude(tid));
            }
            Message::ClaudeNoticeDismissed => {
                self.claude_notice_dismissed = true;
            }
            Message::ClaudeRecheck => {
                // Re-probe; if `claude` is now installed the notice's view gate
                // stops firing on its own. Otherwise the prompt stays up.
                self.claude_installed = crate::session::claude_on_path();
            }

            // ── stats ─────────────────────────────────────────────────────────
            Message::StatsRefresh => {
                return Task::batch([
                    Task::perform(stats::load(), Message::StatsLoaded),
                    Task::perform(stats::load_procs(), Message::ProcsLoaded),
                ]);
            }
            Message::StatsLoaded(s) => match s {
                Some(s) => {
                    self.claude_stats = s;
                    self.stats_stale  = false;
                }
                // A failed fetch is not "you used nothing". The endpoint rate-limits
                // (HTTP 429) under a tight poll interval, and the token expires on
                // its own schedule; either way, zeroing the bars would state a
                // utilization we don't actually know. Hold the last reading and let
                // the header mark it stale.
                None => self.stats_stale = true,
            },
            Message::ProcsLoaded(p) => {
                self.claude_procs = p;
            }
            Message::StatsRefreshIntervalChanged(i) => {
                self.stats_refresh_interval = i;
            }
            Message::UsageBreakdownOpened(window) => {
                let scope = stats::UsageScope::default();
                let since = self.claude_stats.scan_start(window, scope);
                self.usage_breakdown = Some(UsageBreakdown { window, scope, since, data: None });
                return Task::perform(
                    stats::load_model_breakdown(since),
                    Message::UsageBreakdownLoaded,
                );
            }
            Message::UsageBreakdownRefreshed => {
                let Some(card) = &mut self.usage_breakdown else { return Task::none() };
                // Re-anchor on the latest stats: the window may have rolled over
                // since the card was opened.
                let since = self.claude_stats.scan_start(card.window, card.scope);
                card.since = since;
                card.data  = None;
                return Task::perform(
                    stats::load_model_breakdown(since),
                    Message::UsageBreakdownLoaded,
                );
            }
            Message::UsageBreakdownScopeChanged(scope) => {
                let Some(card) = &mut self.usage_breakdown else { return Task::none() };
                if card.scope == scope { return Task::none(); }
                let since = self.claude_stats.scan_start(card.window, scope);
                card.scope = scope;
                card.since = since;
                card.data  = None;
                return Task::perform(
                    stats::load_model_breakdown(since),
                    Message::UsageBreakdownLoaded,
                );
            }
            Message::UsageBreakdownLoaded(data) => {
                // Dropped on the floor if the card was closed mid-scan.
                if let Some(card) = &mut self.usage_breakdown {
                    card.data = Some(data);
                }
            }
            Message::UsageBreakdownClosed => {
                self.usage_breakdown = None;
            }

            // ── plugin panel ──────────────────────────────────────────────────
            Message::PluginTabClicked(slot, tab) => {
                let revealed = {
                    let s = &mut self.sessions[self.active_session];
                    let panel = &mut s.plugin_panel;
                    // Clicking the already-active tab of a single (non-split) panel
                    // toggles it closed — preserving the original muscle memory.
                    // In a split panel the tab bar only switches that slot's plugin.
                    let toggle_hide = !panel.split
                        && slot == PluginSlot::Top
                        && panel.visible
                        && panel.active_tab == tab;
                    if toggle_hide {
                        panel.visible = false;
                    } else {
                        panel.visible = true;
                        panel.set_tab(slot, tab);
                    }
                    !toggle_hide
                };
                // The Docs index is a snapshot, so refresh it whenever the tab
                // is brought up — otherwise a doc added since the last look
                // would be missing with no hint as to why.
                if revealed && tab == PluginTab::Docs {
                    self.rescan_docs();
                }
                self.save_persisted();
            }
            Message::PluginTabHidden(tab) => {
                let panel = &mut self.sessions[self.active_session].plugin_panel;
                panel.hide_tab(tab);
                self.save_persisted();
            }
            Message::PluginTabShown(tab) => {
                {
                    let panel = &mut self.sessions[self.active_session].plugin_panel;
                    panel.show_tab(tab);
                    // Surface the just-added plugin and close the picker.
                    panel.active_tab = tab;
                    panel.adding = false;
                    panel.visible = true;
                }
                if tab == PluginTab::Docs {
                    self.rescan_docs();
                }
                self.save_persisted();
            }
            Message::PluginAddPickerToggled => {
                let panel = &mut self.sessions[self.active_session].plugin_panel;
                panel.adding = !panel.adding;
            }
            Message::PluginPanelDockToggled => {
                let p = &mut self.sessions[self.active_session].plugin_panel;
                p.toggle_dock();
                p.controls_menu = false;
                self.save_persisted();
            }
            Message::PluginPanelSplitToggled => {
                let p = &mut self.sessions[self.active_session].plugin_panel;
                p.toggle_split();
                p.controls_menu = false;
                self.save_persisted();
            }
            Message::PluginPanelExpandToggled => {
                let p = &mut self.sessions[self.active_session].plugin_panel;
                p.toggle_expanded();
                p.controls_menu = false;
                self.save_persisted();
            }
            Message::PluginPanelControlsToggled => {
                let p = &mut self.sessions[self.active_session].plugin_panel;
                p.controls_menu = !p.controls_menu;
            }
            Message::PluginPanelHide => {
                let s = &mut self.sessions[self.active_session];
                s.plugin_panel.visible = false;
                s.plugin_panel.expanded = false;
                s.plugin_panel.controls_menu = false;
                self.save_persisted();
            }
            Message::PluginPanelToggled => {
                let s = &mut self.sessions[self.active_session];
                s.plugin_panel.visible = !s.plugin_panel.visible;
                if !s.plugin_panel.visible {
                    s.plugin_panel.expanded = false;
                    s.plugin_panel.controls_menu = false;
                }
                self.save_persisted();
            }
            Message::PluginPanelResizeStart => {
                self.plugin_panel_drag = Some(PluginPanelDrag { last_x: None });
            }
            Message::PluginPanelResizeMove(x) => {
                if let Some(drag) = self.plugin_panel_drag.as_mut() {
                    match drag.last_x {
                        None => drag.last_x = Some(x),
                        Some(prev) => {
                            let s = &mut self.sessions[self.active_session];
                            // The handle sits on the panel's inner edge, so the
                            // grow direction flips with the dock side: docked
                            // right, dragging left grows; docked left, dragging
                            // right grows.
                            let delta = match s.plugin_panel.dock {
                                crate::plugin_panel::DockSide::Right => prev - x,
                                crate::plugin_panel::DockSide::Left  => x - prev,
                            };
                            drag.last_x = Some(x);
                            if delta.abs() > 0.0 {
                                let new_w = s.plugin_panel.width + delta;
                                s.plugin_panel.set_width(new_w);
                            }
                        }
                    }
                }
            }
            Message::PluginPanelResizeEnd => {
                if self.plugin_panel_drag.take().is_some() {
                    // Persist the final width once per drag, not per move.
                    self.save_persisted();
                }
            }
            Message::PluginSplitResizeStart => {
                self.plugin_split_drag = Some(PluginSplitDrag { last_y: None });
            }
            Message::PluginSplitResizeMove(y) => {
                if let Some(drag) = self.plugin_split_drag.as_mut() {
                    match drag.last_y {
                        None => drag.last_y = Some(y),
                        Some(prev) => {
                            // Dragging down (y increasing) grows the top slot.
                            let delta = y - prev;
                            drag.last_y = Some(y);
                            if delta.abs() > 0.0 {
                                let p = &mut self.sessions[self.active_session].plugin_panel;
                                let new_h = p.top_height + delta;
                                p.set_top_height(new_h);
                            }
                        }
                    }
                }
            }
            Message::PluginSplitResizeEnd => {
                if self.plugin_split_drag.take().is_some() {
                    // Persist the final slot height once per drag, not per move.
                    self.save_persisted();
                }
            }

            // ── docs ──────────────────────────────────────────────────────────
            Message::DocsQueryChanged(q) => {
                // Filtering runs over the in-memory index, so this never
                // touches the disk — typing stays instant on a big doc tree.
                self.sessions[self.active_session].docs.query = q;
            }
            Message::DocsDateChanged(d) => {
                self.sessions[self.active_session].docs.date = d;
            }
            Message::DocsSortChanged(s) => {
                self.sessions[self.active_session].docs.sort = s;
            }
            Message::DocsClearFilters => {
                self.sessions[self.active_session].docs.clear();
            }
            Message::DocsRefresh => {
                self.rescan_docs();
            }
            Message::DocsOpen => {
                return self.open_plugin_tab(PluginTab::Docs, crate::ui::docs_panel::DOCS_SEARCH_INPUT);
            }

            // ── notes ─────────────────────────────────────────────────────────
            Message::NotesSearchChanged(q) => {
                self.notes.search = q;
            }
            Message::NotesSearchCleared => {
                self.notes.search.clear();
            }
            Message::NotesSearchOpen => {
                return self.open_plugin_tab(
                    PluginTab::Notes,
                    crate::ui::plugin_panel::NOTES_SEARCH_INPUT,
                );
            }
            Message::NotesNew => {
                // New notes are pinned to the active session's project; the
                // 🖈 button in the editor header makes them global.
                let project = self.active_project();
                // A fresh note is empty, so a standing search would filter it
                // straight back out of the list it was just added to.
                self.notes.search.clear();
                self.notes.create(project);
                // Drop the keyboard cursor into the Title field so the user
                // can name the new note, then Tab/Enter into the body.
                return text_input::focus(
                    text_input::Id::new(crate::ui::plugin_panel::NOTES_TITLE_INPUT),
                );
            }
            Message::NotesSelected(id) => {
                self.notes.select(id);
            }
            Message::NotesDelete(id) => {
                let project = self.active_project();
                self.notes.delete(id, project.as_deref());
            }
            Message::NotesPinToggled(id) => {
                let project = self.active_project();
                self.notes.toggle_pin(id, project);
            }
            Message::NotesFocusBody => {
                self.notes.naming = false;
                // The body text_editor is the next focusable widget after the
                // Title field — it registers no Id, so step rather than jump.
                return iced::widget::focus_next();
            }
            Message::NotesTitleEdited(s) => {
                self.notes.edit_title(s);
            }
            Message::NotesContentAction(action) => {
                // Any body interaction ends naming mode — the user got there
                // by clicking instead of Tab and the flag must not hijack a
                // later Tab press.
                self.notes.naming = false;
                self.notes.body_content.perform(action);
                self.notes.commit_body();
            }
            Message::NotesFormat(fmt) => {
                self.notes.apply_format(fmt);
                self.notes.commit_body();
            }
            Message::NotesViewMode(mode) => {
                self.notes.view_mode = mode;
            }
            // Tab / Shift+Tab arrived through the body editor's key binding,
            // so the press also lands on `Message::TabPressed` (Tab bypasses
            // the capture gate) — `tab_in_body` makes that handler stand down.
            Message::NotesIndent => {
                self.notes.naming = false;
                self.notes.tab_in_body = true;
                self.notes.indent();
                self.notes.commit_body();
            }
            Message::NotesOutdent => {
                self.notes.naming = false;
                self.notes.tab_in_body = true;
                self.notes.outdent();
                self.notes.commit_body();
            }
            Message::NotesCategorySelected(choice) => {
                let Some(id) = self.notes.selected else { return Task::none(); };
                match choice {
                    CategoryChoice::NewCategory => {
                        self.notes.begin_new_category();
                        // Drop the cursor into the name field so the user can
                        // type the category straight away.
                        return text_input::focus(text_input::Id::new(
                            crate::ui::plugin_panel::NOTES_CATEGORY_INPUT,
                        ));
                    }
                    CategoryChoice::Uncategorized => self.notes.set_category(id, None),
                    CategoryChoice::Named(name)   => self.notes.set_category(id, Some(name)),
                }
            }
            Message::NotesCategoryInput(s) => {
                self.notes.category_input = s;
            }
            Message::NotesCategoryCommit => {
                if let Some(id) = self.notes.selected {
                    let name = self.notes.category_input.trim().to_string();
                    // Empty name = cancel (leave the category untouched).
                    if name.is_empty() {
                        self.notes.adding_category = false;
                        self.notes.category_input.clear();
                    } else {
                        self.notes.set_category(id, Some(name));
                    }
                }
            }
            Message::NotesCategoryCancel => {
                self.notes.adding_category = false;
                self.notes.category_input.clear();
            }
            Message::NotesCategoryToggleCollapsed(key) => {
                self.notes.toggle_category_collapsed(&key);
            }
            Message::NotesSendToClaude => {
                // Prefer the highlighted selection; fall back to the whole note
                // body when nothing is highlighted so the button is never a
                // surprising no-op. The text is pasted (not submitted) into the
                // active Claude terminal so the user can review before sending.
                let text = self.notes.body_content.selection()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| self.notes.body_content.text());
                let text = text.trim_end_matches('\n').to_string();
                if !text.is_empty() {
                    let sess = &mut self.sessions[self.active_session];
                    let active = sess.active_terminal;
                    // The toolbox can send a note to an agent tab the user has
                    // never opened — start it so the paste has somewhere to go.
                    sess.spawn_pending(active);
                    if let Some(t) = sess.terminal_mut(active) {
                        let bracketed = t.grid.lock()
                            .map(|g| g.bracketed_paste)
                            .unwrap_or(false);
                        if let Ok(mut g) = t.grid.lock() { g.snap_to_bottom(); }
                        t.write_input(paste_payload(&text, bracketed).as_bytes());
                    }
                }
            }

            // ── terminals plugin ──────────────────────────────────────────────
            Message::InstallTool(tool) => {
                let new_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                let s = &mut self.sessions[self.active_session];
                let dir = s.working_dir.clone();
                let (prog, args) = crate::port::shell::install_shell(tool.install_command());
                s.terminals_plugin.create_command(
                    new_id, &dir, format!("Install {}", tool.label()), &prog, &args,
                );
                // Reveal the Terminals tab so the running install is visible.
                s.plugin_panel.visible = true;
                s.plugin_panel.show_tab(crate::plugin_panel::PluginTab::Terminals);
                s.plugin_panel.set_tab(
                    crate::plugin_panel::PluginSlot::Top,
                    crate::plugin_panel::PluginTab::Terminals,
                );
            }
            Message::TerminalPluginNew => {
                let new_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                // A brand-new terminal starts on the host; only the user's `ls`
                // flags apply.
                let init = crate::terminals::ShellInit {
                    container: None,
                    ls_flags:  self.prefs.ls_flags.clone(),
                };
                let s = &mut self.sessions[self.active_session];
                let dir = s.working_dir.clone();
                s.terminals_plugin.create(new_id, &dir, &init);
                // Drop the keyboard cursor into the Name field so the user can
                // title the new terminal, then Tab/Enter into the shell.
                return text_input::focus(
                    text_input::Id::new(crate::ui::plugin_panel::TERMINAL_NAME_INPUT),
                );
            }
            Message::TerminalPluginSelected(id) => {
                self.sessions[self.active_session].terminals_plugin.select(id);
            }
            Message::TerminalPluginDelete(id) => {
                self.sessions[self.active_session].terminals_plugin.delete(id);
                // A deleted terminal may have been pinned — re-snapshot so it
                // doesn't come back on the next launch.
                self.save_persisted();
            }
            Message::TerminalPluginPinToggled(id) => {
                self.sessions[self.active_session].terminals_plugin.toggle_pin(id);
                self.save_persisted();
            }
            Message::TerminalPluginNameEdited(s) => {
                let sess = &mut self.sessions[self.active_session];
                sess.terminals_plugin.edit_name(s);
                // Keep the persisted name in sync if the edited terminal is
                // pinned; unpinned ones aren't written, so skip the I/O.
                let edited_pinned = sess.terminals_plugin.selected
                    .and_then(|id| sess.terminals_plugin.terminal(id))
                    .map(|t| t.pinned)
                    .unwrap_or(false);
                if edited_pinned {
                    self.save_persisted();
                }
            }
            Message::TerminalPluginFocusTerminal => {
                // Blur the Name field (focus an id no widget owns) and route
                // keyboard input into the selected terminal.
                let s = &mut self.sessions[self.active_session];
                if s.terminals_plugin.selected.is_some() {
                    s.terminals_plugin.focus_terminal();
                    return text_input::focus(
                        text_input::Id::new("terminal-plugin-blur"),
                    );
                }
            }

            // ── receivers plugin ──────────────────────────────────────────────
            Message::ReceiverContainerAdd(parent) => {
                self.receivers.add_container(parent);
            }
            Message::ReceiverContainerSelect(id) => {
                self.receivers.select_container(id);
            }
            Message::ReceiverContainerRenamed(id, name) => {
                self.receivers.rename_container(id, name);
            }
            Message::ReceiverContainerDelete(id) => {
                self.receivers.delete_container(id);
            }
            Message::ReceiverContainerToggle(id) => {
                self.receivers.toggle_expanded(id);
            }
            Message::ReceiverContainerMoveTo(id, parent) => {
                let name = self.receivers.container_by_id(id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.receivers.status = if self.receivers.move_container(id, parent) {
                    let dest = match parent {
                        Some(p) => self.receivers.container_path(p),
                        None    => "the top level".to_string(),
                    };
                    Some(Ok(format!("Moved \"{name}\" to {dest}.")))
                } else {
                    Some(Err(format!("Can't move \"{name}\" there — pick a location outside it.")))
                };
            }
            Message::ReceiverMoveAll(from, to) => {
                let moved = self.receivers.move_receivers_in(from, to);
                let dest  = self.receivers.container_path(to);
                self.receivers.status = Some(if moved > 0 {
                    Ok(format!(
                        "Moved {moved} receiver{} to {dest}.",
                        if moved == 1 { "" } else { "s" },
                    ))
                } else {
                    Err("No receivers to move.".to_string())
                });
            }
            Message::ReceiverAdd(container) => {
                self.receivers.add_receiver(container);
            }
            Message::ReceiverSelect(id) => {
                self.receivers.select_receiver(id);
                // The detail pane shows this receiver's SSH session (if any), so
                // route keyboard to it and drop focus from other sessions —
                // otherwise a hidden, still-focused session would swallow keys.
                self.receivers.focus_ssh_for(id);
            }
            Message::ReceiverDelete(id) => {
                self.receivers.delete_receiver(id);
            }
            Message::ReceiverMoveTo(id, container) => {
                self.receivers.move_receiver(id, container);
                let name = self.receivers.receiver_by_id(id)
                    .map(|r| r.name.clone())
                    .unwrap_or_default();
                self.receivers.status = Some(Ok(format!(
                    "Moved \"{name}\" to {}.",
                    self.receivers.container_path(container),
                )));
            }
            Message::ReceiverNameEdited(s) => {
                self.receivers.name_input = s;
                self.receivers.commit_edits();
            }
            Message::ReceiverHostEdited(s) => {
                self.receivers.host_input = s;
                self.receivers.commit_edits();
            }
            Message::ReceiverUserEdited(s) => {
                self.receivers.user_input = s;
                self.receivers.commit_edits();
            }
            Message::ReceiverPasswordEdited(s) => {
                self.receivers.password_input = s;
                self.receivers.commit_edits();
            }
            Message::ReceiverPortEdited(s) => {
                // Keep digits only; an empty field is allowed (falls back to 22).
                if s.is_empty() || s.chars().all(|c| c.is_ascii_digit()) {
                    self.receivers.port_input = s;
                    self.receivers.commit_edits();
                }
            }
            Message::ReceiverConnect(id) => {
                self.receivers.status = Some(match self.connect_receiver(id, true) {
                    Ok(dest) => Ok(format!("Connected to {dest}")),
                    Err(e)   => Err(e),
                });
            }
            Message::ReceiverSshDisconnect(receiver) => {
                self.receivers.remove_ssh_for(receiver);
            }
            Message::ReceiverConnectAll(container) => {
                // Already-live sessions are left alone; an exited pane is retried.
                let targets: Vec<u64> = self.receivers.receivers_under(container).iter()
                    .map(|r| r.id)
                    .filter(|id| !self.receivers.is_connected(*id))
                    .collect();
                if targets.is_empty() {
                    self.receivers.status =
                        Some(Err("nothing left to connect here".into()));
                    return Task::none();
                }
                // set_ssh drops focus from every other pane, so restore whatever
                // the user was typing into once the batch is done.
                let was_focused = self.receivers.ssh.iter()
                    .find(|t| t.focused)
                    .map(|t| t.receiver);
                let mut ok = 0usize;
                let mut errs: Vec<String> = Vec::new();
                for id in targets {
                    match self.connect_receiver(id, false) {
                        Ok(_)  => ok += 1,
                        Err(e) => errs.push(e),
                    }
                }
                if let Some(r) = was_focused {
                    self.receivers.focus_ssh_for(r);
                }
                let name = self.receivers.container_by_id(container)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.receivers.status = Some(if errs.is_empty() {
                    Ok(format!("Connected {ok} receiver{} in {name}",
                               if ok == 1 { "" } else { "s" }))
                } else {
                    // One failure is worth naming; a pile of them isn't.
                    let detail = if errs.len() == 1 {
                        errs[0].clone()
                    } else {
                        format!("{} failed (first: {})", errs.len(), errs[0])
                    };
                    Err(format!("Connected {ok}, {detail}"))
                });
            }
            Message::ReceiverDisconnectAll(container) => {
                let targets: Vec<u64> = self.receivers.receivers_under(container).iter()
                    .map(|r| r.id)
                    .collect();
                let gone = self.receivers.remove_ssh_many(&targets);
                let name = self.receivers.container_by_id(container)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.receivers.status = Some(Ok(format!(
                    "Disconnected {gone} session{} in {name}",
                    if gone == 1 { "" } else { "s" },
                )));
            }
            Message::ReceiverToggleConnectionForm => {
                self.receivers.connection_collapsed = !self.receivers.connection_collapsed;
            }
            Message::ReceiverCopyPick => {
                let Some(receiver) = self.receivers.selected_receiver else {
                    self.receivers.status = Some(Err("select a receiver first".into()));
                    return Task::none();
                };
                // Root the picker at the open file's directory when there is
                // one, else the active session's working directory.
                let sess = &self.sessions[self.active_session];
                let start = sess.editor.path.as_ref()
                    .and_then(|p| p.parent())
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| sess.working_dir.clone());
                let mut browser = FileBrowserState::new();
                browser.navigate(start);
                self.file_picker = Some(FilePicker {
                    browser,
                    purpose: PickerPurpose::ReceiverCopy(receiver),
                });
            }
            Message::FilePickerBrowse(path) => {
                if let Some(fp) = self.file_picker.as_mut() {
                    fp.browser.navigate(path);
                }
            }
            Message::FilePickerChoose(path) => {
                let purpose = self.file_picker.as_ref().map(|fp| fp.purpose);
                self.file_picker = None;
                match purpose {
                    Some(PickerPurpose::ReceiverCopy(receiver)) => {
                        return self.copy_files_to(receiver, vec![path]);
                    }
                    Some(PickerPurpose::CloudSqlCredentials) => {
                        self.prefs.cloud_sql_credentials =
                            path.to_string_lossy().into_owned();
                        self.prefs.save();
                    }
                    None => {}
                }
            }
            Message::FilePickerCancel => {
                self.file_picker = None;
            }
            Message::ReceiverCopyFile(path) => {
                return self.start_receiver_copy(vec![path]);
            }
            Message::ReceiverCopyDone(result) => {
                self.receivers.status = Some(result);
            }
            Message::ReceiverImportDbStart => {
                let Some(container) = self.receivers.selected_container else {
                    self.receivers.status = Some(Err("select a container to import into".into()));
                    return Task::none();
                };
                if self.sessions[self.active_session].db_panel.client.is_none() {
                    self.receivers.status =
                        Some(Err("connect a database in the Database tool first".into()));
                    return Task::none();
                }
                self.receivers.begin_db_import(container);
            }
            Message::ReceiverImportTablePicked(key) => {
                self.receivers.set_import_table(key.clone());
                let p = &self.sessions[self.active_session].db_panel;
                let (Some(client), Some(table)) = (p.client.clone(), p.table_by_key(&key).cloned())
                else {
                    self.receivers.set_import_error("table not found".into());
                    return Task::none();
                };
                return Task::perform(
                    crate::db::list_columns(client, table),
                    Message::ReceiverImportColumnsLoaded,
                );
            }
            Message::ReceiverImportColumnsLoaded(result) => {
                match result {
                    Ok(cols) => self.receivers.set_import_columns(cols),
                    Err(e)   => self.receivers.set_import_error(e),
                }
            }
            Message::ReceiverImportCsvStart => {
                let Some(container) = self.receivers.selected_container else {
                    self.receivers.status = Some(Err("select a container to import into".into()));
                    return Task::none();
                };
                self.receivers.begin_csv_import(container);
                // Pre-fill the path with the open file when it's a CSV.
                if let Some(p) = &self.sessions[self.active_session].editor.path {
                    if p.extension().and_then(|e| e.to_str()) == Some("csv") {
                        self.receivers.set_csv_path(p.to_string_lossy().into_owned());
                    }
                }
            }
            Message::ReceiverImportCsvPathEdited(s) => {
                self.receivers.set_csv_path(s);
            }
            Message::ReceiverImportCsvLoad => {
                let path = self.receivers.import.as_ref().map(|d| d.csv_path.clone());
                let Some(path) = path.filter(|p| !p.trim().is_empty()) else {
                    self.receivers.set_import_error("enter a CSV file path".into());
                    return Task::none();
                };
                match std::fs::read_to_string(&path) {
                    Ok(text) => match crate::receivers::parse_csv(&text) {
                        Some((header, rows)) => {
                            self.receivers.set_import_columns(header);
                            // Stash parsed rows on the draft via origin reuse is
                            // awkward; instead create immediately is wrong — keep
                            // rows in a field. Store on the state.
                            self.csv_import_rows = rows;
                        }
                        None => self.receivers.set_import_error("CSV is empty".into()),
                    },
                    Err(e) => self.receivers.set_import_error(format!("read failed: {e}")),
                }
            }
            Message::ReceiverImportFieldMapped(field, column) => {
                self.receivers.set_import_field(field, column);
            }
            Message::ReceiverImportLiteralEdited(field, value) => {
                self.receivers.set_import_literal(field, value);
            }
            Message::ReceiverImportFilterColumn(column) => {
                self.receivers.set_import_filter_column(column);
            }
            Message::ReceiverImportFilterOp(op) => {
                self.receivers.set_import_filter_op(op);
            }
            Message::ReceiverImportFilterValue(value) => {
                self.receivers.set_import_filter_value(value);
            }
            Message::ReceiverImportConfirm => {
                let Some(draft) = self.receivers.import.clone() else { return Task::none(); };
                if !draft.is_ready() {
                    self.receivers.set_import_error("map a host column first".into());
                    return Task::none();
                }
                match draft.source {
                    crate::receivers::ImportSource::Csv => {
                        let rows = std::mem::take(&mut self.csv_import_rows);
                        let n = self.receivers.create_from_rows(rows);
                        self.receivers.status = Some(Ok(format!("Imported {n} receiver(s) from CSV")));
                    }
                    crate::receivers::ImportSource::Database => {
                        let p = &self.sessions[self.active_session].db_panel;
                        let Some(client) = p.client.clone() else {
                            self.receivers.set_import_error("database disconnected".into());
                            return Task::none();
                        };
                        let Some(table) = p.table_by_key(&draft.origin).cloned() else {
                            self.receivers.set_import_error("table not found".into());
                            return Task::none();
                        };
                        let engine = p.engine();
                        // Select the mapped columns in `draft.columns` order so the
                        // returned rows line up with `create_from_rows`.
                        let cols = draft.columns.iter()
                            .map(|c| engine.quote_ident(c))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let sql = format!(
                            "SELECT {cols} FROM {}{}",
                            table.quoted(engine),
                            draft.db_where(engine),
                        );
                        return Task::perform(
                            crate::db::run_query(client, sql),
                            |r| Message::ReceiverImportRowsLoaded(r.map(|qr| qr.rows)),
                        );
                    }
                }
            }
            Message::ReceiverImportRowsLoaded(result) => {
                match result {
                    Ok(rows) => {
                        let n = self.receivers.create_from_rows(rows);
                        self.receivers.status =
                            Some(Ok(format!("Imported {n} receiver(s) from database")));
                    }
                    Err(e) => self.receivers.set_import_error(e),
                }
            }
            Message::ReceiverImportCancel => {
                self.receivers.cancel_import();
                self.csv_import_rows.clear();
            }

            // ── database panel ────────────────────────────────────────────────
            Message::DbEngineChanged(engine) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                // Auto-update the port only if it still matches some engine's
                // default — otherwise the user has typed their own value and
                // we leave it alone.
                let was_default = DbEngine::ALL.iter().any(|e| e.default_port() == p.config.port);
                p.config.engine = engine;
                if was_default {
                    p.config.port = engine.default_port();
                    p.port_str    = p.config.port.to_string();
                }
            }
            Message::DbProviderChanged(provider) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.config.provider = provider;
            }
            Message::DbFormToggleCollapsed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.form_collapsed = !p.form_collapsed;
                // A collapsed form has no focusable fields, so Tab should fall
                // back to the terminal rather than cycling hidden inputs.
                if p.form_collapsed {
                    p.focus_active = false;
                }
            }
            Message::DbFieldSubmitted(field) => {
                // Enter advances focus through the form; on the last field,
                // submit triggers Connect.
                use crate::ui::db_panel::{
                    FIELD_DATABASE, FIELD_HOST, FIELD_PASSWORD, FIELD_PORT, FIELD_USER,
                };
                let next = match field {
                    FIELD_HOST     => Some(FIELD_PORT),
                    FIELD_PORT     => Some(FIELD_DATABASE),
                    FIELD_DATABASE => Some(FIELD_USER),
                    FIELD_USER     => Some(FIELD_PASSWORD),
                    FIELD_PASSWORD => None,
                    _              => None,
                };
                return match next {
                    Some(id) => text_input::focus(text_input::Id::new(id)),
                    None     => Task::perform(async {}, |_| Message::DbConnect),
                };
            }
            Message::DbHostEdited(s) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.config.host = s;
                p.focus_active = true;
            }
            Message::DbUserEdited(s) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.config.user = s;
                p.focus_active = true;
            }
            Message::DbPasswordEdited(s) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.config.password = s;
                p.focus_active = true;
            }
            Message::DbDatabaseEdited(s) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.config.database = s;
                p.focus_active = true;
            }
            Message::DbPortEdited(s) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Ok(n) = s.parse::<u16>() {
                    p.config.port = n;
                } else if s.is_empty() {
                    p.config.port = 0;
                }
                p.port_str     = s;
                p.focus_active = true;
            }
            Message::DbInstanceEdited(s) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.config.cloud_sql_instance = s;
                p.focus_active = true;
            }
            Message::DbProxyCredentialsPick => {
                // Root the picker at the key's own folder when one is already
                // chosen, else the session's working directory — an auth.json
                // usually lives with the project it belongs to.
                let sess  = &self.sessions[self.active_session];
                let start = std::path::Path::new(self.prefs.cloud_sql_credentials.trim())
                    .parent()
                    .filter(|p| p.is_dir())
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| sess.working_dir.clone());
                let mut browser = FileBrowserState::new();
                browser.navigate(start);
                self.file_picker = Some(FilePicker {
                    browser,
                    purpose: PickerPurpose::CloudSqlCredentials,
                });
            }
            Message::DbProxyCredentialsCleared => {
                self.prefs.cloud_sql_credentials.clear();
                self.prefs.save();
            }
            Message::DbProxyToggleCollapsed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.proxy_collapsed = !p.proxy_collapsed;
            }
            Message::DbProxyStart { then_connect } => {
                let creds = self.prefs.cloud_sql_credentials.clone();
                let s      = &mut self.sessions[self.active_session];
                let sid    = s.id;
                s.center_tab = crate::session::CenterTab::Database;
                s.db_panel.commit_history();
                // Kill whatever was there first — a stale or dead proxy would
                // otherwise still be holding the port the new one wants.
                s.db_panel.stop_proxy();
                let spec = s.db_panel.proxy_spec(&creds);
                s.db_panel.proxy = cloud_sql::ProxyState::Starting;
                return Task::perform(
                    cloud_sql::start(spec),
                    move |r| Message::DbProxyStarted(sid, then_connect, r),
                );
            }
            Message::DbProxyStarted(session_id, then_connect, result) => {
                let active_id = self.sessions[self.active_session].id;
                let mut connect = false;
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    match result {
                        Ok(proxy) => {
                            // Point the form at the listener the proxy just
                            // opened, so Host/Port show what we actually dial.
                            let (addr, port) = proxy.endpoint();
                            s.db_panel.config.host = addr;
                            s.db_panel.config.port = port;
                            s.db_panel.port_str    = port.to_string();
                            s.db_panel.proxy       = cloud_sql::ProxyState::Running(proxy);
                            // Only chain into Connect for the session the user
                            // is actually looking at — DbConnect always acts on
                            // the active panel.
                            connect = then_connect && session_id == active_id;
                        }
                        Err(err) => {
                            s.db_panel.proxy = cloud_sql::ProxyState::Failed(err.clone());
                            if then_connect {
                                // The Connect that started this can't go ahead;
                                // report the reason where the user is looking.
                                s.db_panel.conn_state = ConnState::Failed(err);
                            }
                        }
                    }
                }
                if connect {
                    return Task::perform(async {}, |_| Message::DbConnect);
                }
            }
            Message::DbProxyStop => {
                self.sessions[self.active_session].db_panel.stop_proxy();
            }
            Message::DbProxyInstall => {
                // Same shape as `InstallTool`: run the one-shot installer in a
                // visible terminal so the user sees the download, the checksum
                // check, and any sudo prompt.
                let new_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                let s   = &mut self.sessions[self.active_session];
                let dir = s.working_dir.clone();
                let (prog, args) =
                    crate::port::shell::install_shell(&cloud_sql::setup_command());
                s.terminals_plugin.create_command(
                    new_id, &dir, String::from("Install Cloud SQL Proxy"), &prog, &args,
                );
                s.plugin_panel.visible = true;
                s.plugin_panel.show_tab(crate::plugin_panel::PluginTab::Terminals);
                s.plugin_panel.set_tab(
                    crate::plugin_panel::PluginSlot::Top,
                    crate::plugin_panel::PluginTab::Terminals,
                );
            }
            Message::DbConnect => {
                // Cloud SQL is reached through the proxy, so it has to be up
                // before there is anything on loopback to dial. Start it first
                // when it's configured and not already listening; the reply
                // chains back into this message.
                {
                    let p = &self.sessions[self.active_session].db_panel;
                    let proxy_up = p.proxy.running().is_some_and(|x| x.is_alive());
                    if p.cloud_sql()
                        && !proxy_up
                        && !p.proxy.is_starting()
                        && !p.config.cloud_sql_instance.trim().is_empty()
                    {
                        return Task::perform(async {}, |_| Message::DbProxyStart {
                            then_connect: true,
                        });
                    }
                }
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                s.db_panel.commit_history();
                s.center_tab = crate::session::CenterTab::Database;
                let cfg = s.db_panel.config.clone();
                // Tear down whatever was there (invalidating in-flight replies)
                // before starting the new attempt.
                s.db_panel.reset_connection();
                let epoch = s.db_panel.epoch;
                s.db_panel.conn_state = ConnState::Connecting;
                return Task::perform(
                    db::connect_and_list(cfg),
                    move |r| Message::DbConnected(sid, epoch, r),
                );
            }
            Message::DbConnected(session_id, epoch, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    // Dropped if the panel was reset (or reconnected) while this
                    // attempt was still in flight.
                    if !s.db_panel.is_current(epoch) {
                        return Task::none();
                    }
                    match result {
                        Ok((client, tables, fks, routines)) => {
                            s.db_panel.conn_state   = ConnState::Connected;
                            s.db_panel.client       = Some(client);
                            s.db_panel.tables       = tables;
                            s.db_panel.foreign_keys = fks;
                            s.db_panel.routines     = routines;
                            // Connected — fold the form away so the builder and
                            // results get the space.
                            s.db_panel.form_collapsed = true;
                            s.db_panel.focus_active   = false;
                        }
                        Err(err) => {
                            s.db_panel.conn_state = ConnState::Failed(err);
                            s.db_panel.client     = None;
                        }
                    }
                }
            }
            Message::DbDisconnect => {
                self.sessions[self.active_session].db_panel.reset_connection();
            }
            Message::DbReset => {
                // Same teardown as Disconnect, but reachable from every state —
                // including a connect that never came back or a query still
                // marked as running against a connection that has since died.
                let p = &mut self.sessions[self.active_session].db_panel;
                p.reset_connection();
                p.focus_active = false;
            }
            Message::DbTableToggled(key) => {
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                let p   = &mut s.db_panel;
                if let Some(pos) = p.selected_tables.iter().position(|t| *t == key) {
                    p.selected_tables.remove(pos);
                    // Drop any picked or grouped columns that belonged to the removed table.
                    p.selected_cols.retain(|c| c.table != key);
                    p.group_by.retain(|c| c.table != key);
                    // Drop this table's own join condition and any condition that
                    // referenced it as the left side.
                    p.joins.remove(&key);
                    p.joins.retain(|_, j| j.left_table != key);
                } else {
                    // Any table can be added; if a foreign key links it to the
                    // current selection, pre-fill the JOIN condition from it,
                    // otherwise the user supplies the ON columns manually.
                    let included = p.selected_tables.clone();
                    if let Some(jc) = p.fk_join_default(&key, &included) {
                        p.joins.insert(key.clone(), jc);
                    }
                    p.selected_tables.push(key.clone());
                    // Lazy-load columns only on first expand.
                    if !p.columns_by_table.contains_key(&key) && !p.loading_columns.contains(&key) {
                        if let (Some(client), Some(table)) = (
                            p.client.clone(),
                            p.tables.iter().find(|t| t.key() == key).cloned(),
                        ) {
                            let epoch = p.epoch;
                            p.loading_columns.insert(key.clone());
                            return Task::perform(
                                db::list_columns(client, table),
                                move |r| Message::DbColumnsLoaded(sid, epoch, key.clone(), r),
                            );
                        }
                    }
                }
            }
            Message::DbRoutineSelected(routine) => {
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                let p   = &mut s.db_panel;
                let Some(client) = p.client.clone() else { return Task::none(); };
                let epoch = p.epoch;
                p.selected_routine = Some(routine.clone());
                p.routine_code     = None;
                p.routine_loading  = true;
                // Make sure the source view is visible even if the builder had
                // the results row hidden behind a collapsed Row 1.
                return Task::perform(
                    db::routine_definition(client, routine),
                    move |r| Message::DbRoutineLoaded(sid, epoch, r),
                );
            }
            Message::DbRoutineLoaded(session_id, epoch, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    let p = &mut s.db_panel;
                    if !p.is_current(epoch) {
                        return Task::none();
                    }
                    p.routine_loading = false;
                    match result {
                        Ok(code) => { p.routine_code = Some(code); }
                        Err(err) => {
                            p.routine_code     = None;
                            p.selected_routine = None;
                            p.query_error      = Some(err);
                        }
                    }
                }
            }
            Message::DbRoutineClosed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.selected_routine = None;
                p.routine_code     = None;
                p.routine_loading  = false;
            }
            Message::DbColumnsLoaded(session_id, epoch, key, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    if !s.db_panel.is_current(epoch) {
                        return Task::none();
                    }
                    s.db_panel.loading_columns.remove(&key);
                    match result {
                        Ok(cols) => { s.db_panel.columns_by_table.insert(key, cols); }
                        Err(err) => { s.db_panel.query_error = Some(err); }
                    }
                }
            }
            Message::DbColumnToggled(table_key, col) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(pos) = p.selected_cols
                    .iter()
                    .position(|c| c.table == table_key && c.column == col)
                {
                    p.selected_cols.remove(pos);
                } else {
                    p.selected_cols.push(ColRef { table: table_key, column: col });
                }
            }
            Message::DbFieldsSelectAll(select) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if select {
                    for c in p.available_cols() {
                        if !p.is_col_selected(&c.table, &c.column) {
                            p.selected_cols.push(c);
                        }
                    }
                } else {
                    p.selected_cols.clear();
                }
            }
            Message::DbFieldDragStart(i) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.dragging_field = (i < p.selected_cols.len()).then_some(i);
            }
            Message::DbFieldDragOver(target) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(from) = p.dragging_field {
                    if target < p.selected_cols.len() && from != target {
                        let item = p.selected_cols.remove(from);
                        p.selected_cols.insert(target, item);
                        p.dragging_field = Some(target);
                    }
                }
            }
            Message::DbFieldDragEnd => {
                self.sessions[self.active_session].db_panel.dragging_field = None;
            }
            Message::DbFieldRemoved(i) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if i < p.selected_cols.len() {
                    p.selected_cols.remove(i);
                }
                p.dragging_field = None;
            }
            Message::DbFilterAdd => {
                let p = &mut self.sessions[self.active_session].db_panel;
                // Seed the new condition with the first available column so it's
                // valid (and previewed in the SQL) the moment a value is typed.
                let (table, column) = p.available_cols()
                    .into_iter()
                    .next()
                    .map(|c| (c.table, c.column))
                    .unwrap_or_default();
                p.filters.push(Filter {
                    table,
                    column,
                    op:    FilterOp::Eq,
                    value: String::new(),
                    conj:  Conj::And,
                });
            }
            Message::DbFilterColumnChanged(i, col) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(f) = p.filters.get_mut(i) {
                    f.table  = col.table;
                    f.column = col.column;
                }
            }
            Message::DbFilterOpChanged(i, op) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(f) = p.filters.get_mut(i) {
                    f.op = op;
                }
            }
            Message::DbFilterValueChanged(i, value) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(f) = p.filters.get_mut(i) {
                    f.value = value;
                }
            }
            Message::DbFilterConjToggled(i) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(f) = p.filters.get_mut(i) {
                    f.conj = f.conj.toggled();
                }
            }
            Message::DbFilterRemoved(i) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if i < p.filters.len() {
                    p.filters.remove(i);
                }
            }
            Message::DbLimitChanged(s) => {
                // Digits only, so the generated LIMIT clause stays valid;
                // empty = no limit (all rows).
                let p = &mut self.sessions[self.active_session].db_panel;
                p.limit_input = s.chars().filter(|c| c.is_ascii_digit()).collect();
            }
            Message::DbGroupToggled(table_key, col) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                if let Some(pos) = p.group_by
                    .iter()
                    .position(|c| c.table == table_key && c.column == col)
                {
                    p.group_by.remove(pos);
                } else {
                    p.group_by.push(ColRef { table: table_key, column: col });
                }
            }
            Message::DbJoinLeftTableChanged(joined, left) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                let jc = p.joins.entry(joined).or_default();
                jc.left_table = left;
                // The previously chosen left column belongs to the old table.
                jc.left_col = String::new();
            }
            Message::DbJoinLeftColChanged(joined, col) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.joins.entry(joined).or_default().left_col = col;
            }
            Message::DbJoinRightColChanged(joined, col) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.joins.entry(joined).or_default().right_col = col;
            }
            Message::DbRunQuery => {
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                let p   = &mut s.db_panel;
                let sql = match p.build_select() {
                    Some(sql) => sql,
                    None      => return Task::none(),
                };
                let client = match p.client.clone() {
                    Some(c) => c,
                    None    => return Task::none(),
                };
                let epoch = p.epoch;
                p.query            = sql.clone();
                p.query_running    = true;
                p.query_error      = None;
                p.statement_status = None;
                return Task::perform(
                    db::run_query(client, sql),
                    move |r| Message::DbQueryResult(sid, epoch, r),
                );
            }
            Message::DbRefreshResults => {
                // Re-run the most recently executed query (data may have changed
                // since); fall back to building from the current builder state if
                // nothing has been run yet.
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                let p   = &mut s.db_panel;
                if p.query_running {
                    return Task::none();
                }
                let sql = if !p.query.is_empty() {
                    p.query.clone()
                } else {
                    match p.build_select() {
                        Some(sql) => { p.query = sql.clone(); sql }
                        None      => return Task::none(),
                    }
                };
                let client = match p.client.clone() {
                    Some(c) => c,
                    None    => return Task::none(),
                };
                let epoch = p.epoch;
                p.query_running    = true;
                p.query_error      = None;
                p.statement_status = None;
                return Task::perform(
                    db::run_query(client, sql),
                    move |r| Message::DbQueryResult(sid, epoch, r),
                );
            }
            Message::DbQueryResult(session_id, epoch, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    if !s.db_panel.is_current(epoch) {
                        return Task::none();
                    }
                    s.db_panel.query_running = false;
                    // The copy confirmation described the previous result set.
                    s.db_panel.copy_notice = None;
                    match result {
                        Ok(r)    => { s.db_panel.query_result = Some(r); s.db_panel.query_error = None; }
                        Err(err) => { s.db_panel.query_error = Some(err); }
                    }
                }
            }
            Message::DbSetSqlMode(on) => {
                self.sessions[self.active_session].db_panel.sql_mode = on;
            }
            Message::DbSqlAction(action) => {
                self.sessions[self.active_session].db_panel.sql_input.perform(action);
            }
            // Route the SQL editor's copy/cut/paste through the app's subprocess
            // clipboard instead of iced's built-in one, whose synchronous read
            // freezes the app on Wayland (see crate::clipboard).
            Message::DbSqlCopy => {
                let input = &self.sessions[self.active_session].db_panel.sql_input;
                if let Some(sel) = input.selection().filter(|s| !s.is_empty()) {
                    return Task::perform(crate::clipboard::write(sel), |_| Message::ClipboardWritten);
                }
            }
            Message::DbSqlCut => {
                let input = &mut self.sessions[self.active_session].db_panel.sql_input;
                if let Some(sel) = input.selection().filter(|s| !s.is_empty()) {
                    // Match iced's built-in Cut: copy the selection out, then delete it.
                    input.perform(text_editor::Action::Edit(text_editor::Edit::Delete));
                    return Task::perform(crate::clipboard::write(sel), |_| Message::ClipboardWritten);
                }
            }
            Message::DbSqlPaste => {
                return Task::perform(crate::clipboard::read(), Message::DbSqlPasteReady);
            }
            Message::DbSqlPasteReady(contents) => {
                let Some(text) = contents.filter(|s| !s.is_empty()) else { return Task::none(); };
                self.sessions[self.active_session].db_panel.sql_input.perform(
                    text_editor::Action::Edit(text_editor::Edit::Paste(std::sync::Arc::new(text))),
                );
            }
            Message::DbRunSql => {
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                let p   = &mut s.db_panel;
                let sql = p.sql_input.text();
                if sql.trim().is_empty() {
                    return Task::none();
                }
                let client = match p.client.clone() {
                    Some(c) => c,
                    None    => return Task::none(),
                };
                let epoch = p.epoch;
                p.query            = sql.clone();
                p.query_running    = true;
                p.query_error      = None;
                p.statement_status = None;
                return Task::perform(
                    db::run_statement(client, sql),
                    move |r| Message::DbStatementResult(sid, epoch, r),
                );
            }
            Message::DbStatementResult(session_id, epoch, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    let p = &mut s.db_panel;
                    if !p.is_current(epoch) {
                        return Task::none();
                    }
                    p.query_running = false;
                    p.copy_notice   = None;
                    match result {
                        Ok(StatementOutcome::Rows(r)) => {
                            p.query_result      = Some(r);
                            p.query_error       = None;
                            p.statement_status  = None;
                        }
                        Ok(StatementOutcome::Affected(n)) => {
                            p.query_result      = None;
                            p.query_error       = None;
                            p.statement_status  =
                                Some(format!("Statement OK — {n} row{} affected", if n == 1 { "" } else { "s" }));
                        }
                        Err(err) => { p.query_error = Some(err); }
                    }
                }
            }
            Message::DbObjectFilterChanged(s) => {
                self.sessions[self.active_session].db_panel.object_filter = s;
            }
            Message::DbTablesToggleCollapsed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.tables_collapsed = !p.tables_collapsed;
            }
            Message::DbObjectsToggleCollapsed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.objects_collapsed = !p.objects_collapsed;
            }
            Message::DbResetTables => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.selected_tables.clear();
                p.selected_cols.clear();
                p.filters.clear();
                p.group_by.clear();
                p.joins.clear();
                p.dragging_field = None;
                p.query_result = None;
                p.query_error  = None;
                p.statement_status = None;
                p.copy_notice  = None;
            }
            Message::DbRow1ToggleCollapsed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.row1_collapsed = !p.row1_collapsed;
            }
            Message::DbCopyValue(value) => {
                return Task::perform(crate::clipboard::write(value), |_| Message::ClipboardWritten);
            }
            Message::DbCopyFormatChanged(format) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.copy_format = format;
                p.copy_notice = None;
            }
            Message::DbCopyResults => {
                let p = &mut self.sessions[self.active_session].db_panel;
                let Some(result) = &p.query_result else { return Task::none() };
                let text = result.to_clipboard(p.copy_format);
                let rows = result.rows.len();
                p.copy_notice = Some(format!(
                    "Copied {rows} {} as {}",
                    if rows == 1 { "row" } else { "rows" },
                    p.copy_format,
                ));
                return Task::perform(crate::clipboard::write(text), |_| Message::ClipboardWritten);
            }
            Message::DbCopyRow(idx) => {
                let p = &mut self.sessions[self.active_session].db_panel;
                let Some(result) = &p.query_result else { return Task::none() };
                let Some(text) = result.row_to_clipboard(idx, p.copy_format) else {
                    return Task::none();
                };
                p.copy_notice = Some(format!("Copied row {} as {}", idx + 1, p.copy_format));
                return Task::perform(crate::clipboard::write(text), |_| Message::ClipboardWritten);
            }
            Message::DbResultsScrolled(viewport) => {
                // Mirror the body's horizontal offset onto the frozen header so
                // the column titles stay lined up with the cells beneath them.
                let x = viewport.absolute_offset().x;
                return scrollable::scroll_to(
                    ui::db_panel::results_header_scroll_id(),
                    scrollable::AbsoluteOffset { x, y: 0.0 },
                );
            }
            Message::CenterTabSelected(tab) => {
                let s = &mut self.sessions[self.active_session];
                s.center_tab = tab;
                // Entering the Database tab with an expanded form puts keyboard
                // focus in the connection fields so Tab cycles them; any other
                // tab releases that focus back to the terminal.
                let focus_db = tab == crate::session::CenterTab::Database
                    && !s.db_panel.form_collapsed;
                s.db_panel.focus_active = focus_db;
                self.save_persisted();
                if focus_db {
                    return text_input::focus(
                        text_input::Id::new(crate::ui::db_panel::FIELD_HOST),
                    );
                }
                // Landing on a chat tab: put its transcript back at the newest
                // message (the scroll offset died when the pane left the view).
                return self.reveal_chat_panes();
            }

            // ── preferences ───────────────────────────────────────────────────
            Message::PrefsOpened => {
                self.prefs_dev_dir_input = self
                    .prefs
                    .dev_dir
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.prefs_file_browser.navigate(self.prefs.new_session_start_dir());
                self.showing_prefs = true;
            }
            Message::PrefsCancelled => {
                self.showing_prefs = false;
            }
            Message::PrefsDevDirEdited(s) => {
                let p = PathBuf::from(s.trim());
                if p.is_dir() {
                    self.prefs_file_browser.navigate(p);
                }
                self.prefs_dev_dir_input = s;
            }
            Message::PrefsDevDirBrowsed(path) => {
                self.prefs_file_browser.navigate(path.clone());
                self.prefs_dev_dir_input = path.to_string_lossy().into_owned();
            }
            Message::PrefsSaved => {
                let trimmed = self.prefs_dev_dir_input.trim();
                self.prefs.dev_dir = if trimmed.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(trimmed))
                };
                self.prefs.save();
                self.showing_prefs = false;
            }
            Message::PrefsPaletteChanged(p) => {
                if self.prefs.palette != p {
                    let from = self.current_colors();
                    self.prefs.palette = p;
                    self.prefs.save();
                    self.start_theme_anim(from);
                }
            }
            Message::PrefsModeChanged(m) => {
                if self.prefs.mode != m {
                    let from = self.current_colors();
                    self.prefs.mode = m;
                    self.prefs.save();
                    self.start_theme_anim(from);
                }
            }
            Message::PrefsFontScaleChanged(f) => {
                self.prefs.font_scale = f;
                self.prefs.save();
            }
            Message::PrefsTerminalFontScaleChanged(f) => {
                self.prefs.terminal_font_scale = f;
                self.prefs.save();
            }
            Message::PrefsSubmoduleColorChanged(c) => {
                if self.prefs.submodule_color != c {
                    self.prefs.submodule_color = c;
                    self.prefs.save();
                }
            }
            Message::PrefsDefaultToolChanged(t) => {
                if self.prefs.default_tool != t {
                    self.prefs.default_tool = t;
                    self.prefs.save();
                }
            }
            Message::PrefsFilterToolsToggled(on) => {
                self.prefs.filter_tools_to_installed = on;
                self.prefs.save();
            }
            Message::PrefsChatAccessChanged(access) => {
                if self.prefs.chat_access != access {
                    self.prefs.chat_access = access;
                    self.prefs.save();
                    // Read on each send, so open chat panes pick this up on
                    // their next turn — nothing to restart.
                }
            }
            Message::PrefsLsFlagToggled(flag, on) => {
                self.prefs.set_ls_flag(flag, on);
                self.prefs.save();
                // Deliberately not pushed into shells that are already open —
                // typing an alias into one would land in whatever program the
                // user has running there. New terminals pick it up.
            }
            Message::PrefsImmutableSystemToggled(on) => {
                self.prefs.immutable_system = on;
                self.prefs.save();
                // Takes effect on the next launch (and immediately for the
                // Terminals-tab container badge). Nothing to restart: which
                // container a shell is in is tracked either way, this only
                // decides whether frms acts on it.
            }
            Message::PrefsTelemetryToggled(on) => {
                self.prefs.telemetry = on;
                self.prefs.save();
                // Applies immediately to the optional tier; the sender keeps
                // running either way so install/launch counts still report.
                self.telemetry.set_optional(on);
            }
            Message::NdaChoice(accepted) => {
                if accepted {
                    self.prefs.nda_accepted = true;
                    self.prefs.save();
                } else {
                    // Declined — terminate the app (hard shutdown; see `shutdown`).
                    self.shutdown();
                }
            }
            Message::TelemetryNoticeChoice(keep_on) => {
                self.prefs.telemetry           = keep_on;
                self.prefs.telemetry_notice_ack = true;
                self.prefs.save();
                // The choice covers the optional tier only — install/launch are
                // disclosed by the notice as always-on, and were already sent
                // by `new`. Same path as the Profile toggle.
                self.telemetry.set_optional(keep_on);
            }
            Message::ThemeAnimTick => {
                if matches!(&self.theme_anim, Some(a) if a.finished()) {
                    self.theme_anim = None;
                }
            }
            Message::HeaderToggleCollapse => {
                self.header_collapsed = !self.header_collapsed;
            }
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, Message> {
        // Splash screen on first launch
        if self.showing_splash {
            return ui::splash::view(self.splash_frame.as_ref(), &self.splash_anim);
        }

        let colors = self.current_colors();

        // First-run NDA gate — must be accepted before the app can be used
        // (declining exits). One acceptance for every install format.
        if !self.prefs.nda_accepted {
            return ui::nda_dialog::view(colors);
        }

        // First-run telemetry notice — shown once (after the NDA), before
        // anything else, so the user sees what's collected (and can opt out)
        // before any data is sent. No telemetry fires until acknowledged.
        if !self.prefs.telemetry_notice_ack {
            return ui::telemetry_notice::view(colors);
        }

        // Claude Code is required to run agents. If the `claude` CLI isn't on
        // the PATH, prompt for it on every launch until it's installed or the
        // user chooses to continue anyway for this session.
        if !self.claude_installed && !self.claude_notice_dismissed {
            return ui::claude_missing::view(colors);
        }

        // Preferences dialog takes precedence — full-screen overlay.
        if self.showing_prefs {
            return ui::prefs_dialog::view(
                &self.prefs_dev_dir_input,
                &self.prefs_file_browser,
                colors,
            );
        }

        // Receiver file picker — full-screen overlay for choosing a file to scp.
        if let Some(fp) = &self.file_picker {
            let receiver_name = match fp.purpose {
                PickerPurpose::ReceiverCopy(id) => self.receivers.receiver_by_id(id)
                    .map(|r| r.name.as_str())
                    .unwrap_or("receiver"),
                PickerPurpose::CloudSqlCredentials => "",
            };
            return ui::file_picker::view(
                &fp.browser,
                fp.purpose.title(receiver_name),
                fp.purpose.hint(),
                colors,
            );
        }

        // If the new-session dialog is open, show it full-screen instead of the IDE
        if self.creating_session {
            return ui::new_session_dialog::view(
                &self.new_session_name,
                &self.new_session_dir,
                &self.new_session_file_browser,
                !self.sessions.is_empty(),
                colors,
            );
        }

        let s             = &self.sessions[self.active_session];
        let plugin_panel  = &s.plugin_panel;
        let header        = ui::header::view(
            plugin_panel.visible,
            &self.claude_stats,
            self.stats_stale,
            &self.claude_procs,
            self.stats_refresh_interval,
            self.header_collapsed,
            colors,
        );
        let split_ids = self
            .split_view
            .as_ref()
            .map(|sv| (sv.top_id, sv.bottom_id));

        let main_area: Element<'_, Message> = match &self.split_view {
            Some(sv) => {
                let top_id    = sv.top_id;
                let bottom_id = sv.bottom_id;
                let outer = pane_grid::PaneGrid::new(&sv.panes, move |_pane, session_id, _max| {
                    let slot = if *session_id == top_id {
                        SplitSlot::Top
                    } else if *session_id == bottom_id {
                        SplitSlot::Bottom
                    } else {
                        // Fallback for stale tags; treat as top so the bar still renders.
                        SplitSlot::Top
                    };
                    let slot_active_idx = self.sessions.iter()
                        .position(|s| s.id == *session_id)
                        .unwrap_or(0);
                    let bar = ui::session_bar::view(
                        &self.sessions,
                        slot_active_idx,
                        &self.renaming_session,
                        split_ids,
                        Some(slot),
                        colors,
                    );
                    let inner = match self.sessions.iter().find(|s| s.id == *session_id) {
                        Some(session) => self.session_inner_view(session, colors),
                        None => iced::widget::container(
                            iced::widget::text("session unavailable"),
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into(),
                    };
                    pane_grid::Content::new(column![bar, inner])
                })
                .on_resize(10, Message::SplitResized)
                .width(Length::Fill)
                .height(Length::Fill);
                outer.into()
            }
            None => {
                let session = &self.sessions[self.active_session];
                let bar = ui::session_bar::view(
                    &self.sessions,
                    self.active_session,
                    &self.renaming_session,
                    split_ids,
                    None,
                    colors,
                );
                let inner = self.session_inner_view(session, colors);
                column![bar, inner].into()
            }
        };

        let bg = colors.background;
        let base: Element<'_, Message> = container(column![header, main_area])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme: &Theme| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg)),
                ..Default::default()
            })
            .into();

        // Per-model usage breakdown, opened from a header bar.
        if let Some(card) = &self.usage_breakdown {
            return iced::widget::stack![base, ui::usage_dialog::overlay(card, colors)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        // "Change model" dialog floats over the IDE, scoped to the session it
        // was opened for (which stays put even if the active session changes).
        if let Some(dialog) = &self.agents_dialog {
            if let Some(session) = self.sessions.iter().find(|s| s.id == dialog.session_id) {
                return iced::widget::stack![
                    base,
                    ui::agents_dialog::overlay(dialog, &session.panes, colors),
                ]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
            }
        }

        // Close-confirmation dialog floats above everything else.
        let Some(pending) = self.pending_close else {
            return base;
        };
        let (title, detail) = match pending {
            PendingClose::Session(idx) => {
                let name = self.sessions.get(idx)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                (
                    "Close session?".to_string(),
                    format!("“{name}” and all of its Claude terminals will be closed."),
                )
            }
            PendingClose::Claude(tid) => {
                let label = self.sessions.iter()
                    .find_map(|s| {
                        let pos = s.panes.iter().position(|p| p.id() == tid)?;
                        Some(s.panes[pos].name().map(str::to_owned)
                            .unwrap_or_else(|| format!("{} {}", s.panes[pos].kind().label(), pos + 1)))
                    })
                    .unwrap_or_else(|| "Agent".to_string());
                (
                    "Close agent tab?".to_string(),
                    format!("“{label}” will be closed."),
                )
            }
        };
        iced::widget::stack![
            base,
            ui::confirm_dialog::overlay(
                title,
                detail,
                "Close",
                Message::ClosePendingConfirmed,
                Message::ClosePendingCancelled,
                colors,
            ),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    /// Render one full session — its pane grid plus the optional database side
    /// panel docked to the right. Used both for the single-session view and as
    /// the body of each outer pane in split view.
    fn session_inner_view<'a>(
        &'a self,
        session: &'a Session,
        colors:  ThemeColors,
    ) -> Element<'a, Message> {
        let pane_border  = colors.border;
        let panes        = &session.layout.panes;
        let file_browser = &session.file_browser;
        let editor       = &session.editor;
        let agent_panes  = &session.panes;
        let active_term  = session.active_terminal;
        let url_input    = &session.url_input;
        let session_id   = session.id;
        let pending      = session.pending_prompt.as_ref();
        let prompt_src   = session.prompt_source_terminal;
        let center_tab   = session.center_tab;
        let submodule_color = self.prefs.submodule_color;
        // The one Cloud SQL setting the user owns: their service-account key.
        // Where the proxy binary lives is frms' problem, not theirs.
        let cloud_sql_creds = &self.prefs.cloud_sql_credentials;

        // File browser hidden (Terminal layout only): skip the pane grid and
        // lay out a thin expand strip beside a full-width editor, so the
        // browser's width is genuinely reclaimed rather than just blanked.
        let collapsed = session.file_browser_collapsed
            && matches!(session.kind, crate::session::SessionKind::Terminal);

        let content: Element<'a, Message> = if collapsed {
            let border_style = move |_theme: &Theme| iced::widget::container::Style {
                border: iced::Border { color: pane_border, width: 1.0, radius: 2.0.into() },
                ..Default::default()
            };
            let editor_el = ui::editor::view(
                editor,
                &session.db_panel,
                center_tab,
                agent_panes,
                active_term,
                pending,
                prompt_src,
                &self.renaming_claude,
                self.agent_menu_open,
                self.prefs.default_tool,
                self.prefs.filter_tools_to_installed,
                self.agent_strip_overflow,
                cloud_sql_creds,
            );
            let editor_bordered = iced::widget::container(editor_el)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(border_style);
            let strip = iced::widget::container(ui::file_browser::collapsed_strip())
                .height(Length::Fill)
                .style(border_style);
            iced::widget::row![strip, editor_bordered]
                .spacing(0)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            pane_grid::PaneGrid::new(panes, move |_pane, kind, _maximized| {
                let content = match kind {
                    PaneKind::FileBrowser => ui::file_browser::view(file_browser, submodule_color),
                    PaneKind::Editor  => {
                        ui::editor::view(
                            editor,
                            &session.db_panel,
                            center_tab,
                            agent_panes,
                            active_term,
                            pending,
                            prompt_src,
                            &self.renaming_claude,
                            self.agent_menu_open,
                            self.prefs.default_tool,
                            self.prefs.filter_tools_to_installed,
                            self.agent_strip_overflow,
                            cloud_sql_creds,
                        )
                    }
                    PaneKind::Browser => {
                        ui::browser::view(
                            url_input,
                            session.screenshot_bytes.as_deref(),
                            session.browser_loading,
                            session.browser_error.as_deref(),
                            session.cdp_active,
                        )
                    }
                };
                let bordered = iced::widget::container(content)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(move |_theme: &Theme| iced::widget::container::Style {
                        border: iced::Border { color: pane_border, width: 1.0, radius: 2.0.into() },
                        ..Default::default()
                    });
                pane_grid::Content::new(bordered)
            })
            .on_resize(10, move |re| Message::PaneResized(session_id, re))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        if session.plugin_panel.visible {
            let panel = ui::plugin_panel::view(
                &session.plugin_panel,
                &self.prefs,
                &self.notes,
                &session.docs,
                session.working_dir.to_str(),
                &session.terminals_plugin,
                &self.receivers,
                session.db_panel.client.is_some(),
                &session.db_panel.tables,
                self.prefs.telemetry,
            );
            // Expanded: the panel swallows the whole width and the main working
            // area is hidden — the mirror of hiding the panel for a full-width
            // main area.
            if session.plugin_panel.expanded {
                return iced::widget::container(panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into();
            }
            // Dock side decides whether the panel sits left of the file browser
            // or right of everything.
            let row = match session.plugin_panel.dock {
                crate::plugin_panel::DockSide::Left  => iced::widget::row![panel, content],
                crate::plugin_panel::DockSide::Right => iced::widget::row![content, panel],
            };
            row.spacing(0).height(Length::Fill).into()
        } else {
            content
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let claude_pty_subs = self.sessions.iter()
            .flat_map(|s| s.panes.iter())
            .filter_map(|p| p.as_terminal())
            .map(|t| terminal::pty_subscription(
                t.id, t.reader.clone(), t.child.clone(),
                Message::TerminalData, Message::TerminalExited));
        let plugin_pty_subs = self.sessions.iter()
            .flat_map(|s| s.terminals_plugin.terminals.iter())
            .map(|t| terminal::pty_subscription(
                t.id, t.pane.reader.clone(), t.pane.child.clone(),
                Message::TerminalData, Message::TerminalExited));
        // Every live receiver SSH session (one per connected receiver). An
        // exited session is skipped — its read loop has already ended, so
        // re-subscribing the dead PTY would just immediately re-fire exit.
        let receiver_pty_sub = self.receivers.ssh.iter()
            .filter(|t| !t.exited)
            .map(|t| terminal::pty_subscription(
                t.id, t.pane.reader.clone(), t.pane.child.clone(),
                Message::TerminalData, Message::TerminalExited));
        let pty_subs = claude_pty_subs.chain(plugin_pty_subs).chain(receiver_pty_sub);

        // Video frames — only while splash is shown and yt-dlp is ready.
        let video_sub: Option<Subscription<Message>> =
            if self.showing_splash {
                self.splash_yt_dlp.clone().map(|yt_dlp| {
                    crate::splash_video::frame_subscription(
                        yt_dlp,
                        "https://youtu.be/UisAg3bhp-g".to_string(),
                        Message::SplashVideoFrame,
                        || Message::SplashVideoEnded,
                    )
                })
            } else {
                None
            };

        // 16 ms animation tick — drives fade-out and fade-in when active.
        let anim_tick: Option<Subscription<Message>> =
            if self.showing_splash && matches!(
                self.splash_anim,
                SplashAnim::FadeOut(_) | SplashAnim::FadeIn(_)
            ) {
                Some(
                    iced::time::every(std::time::Duration::from_millis(16))
                        .map(|_| Message::SplashAnimTick),
                )
            } else {
                None
            };

        // Use listen_with so we can check `Status::Captured` — when a focused
        // text_input has eaten the key, we must NOT also forward it to the
        // active terminal (otherwise typing into the DB form duplicates into
        // the shell pane). Tab is an exception: the `combo_box` widget
        // captures Tab to cycle its own dropdown, but we still want Tab to
        // walk between DB form fields, so we let Tab through regardless and
        // route it through `TabPressed` (which will call `focus_next`).
        //
        // `listen_with` takes a fn pointer (no captures), so when splash is
        // up we swap in a dedicated listener that only forwards Enter →
        // `SplashDismissed` and drops everything else.
        let keyboard = if self.showing_splash {
            iced::event::listen_with(splash_key_listener)
        } else {
            iced::event::listen_with(main_key_listener)
        };

        // Window X is handled by `exit_on_close_request` (default true) — no
        // custom listener needed here.

        // Refresh Claude usage stats on the user-selected interval (off by default).
        let stats_timer: Option<Subscription<Message>> = self
            .stats_refresh_interval
            .as_duration()
            .map(|d| iced::time::every(d).map(|_| Message::StatsRefresh));

        let mut subs: Vec<Subscription<Message>> = pty_subs
            .chain(std::iter::once(keyboard))
            .collect();

        if let Some(st) = stats_timer { subs.push(st); }

        if let Some(vs) = video_sub  { subs.push(vs); }
        if let Some(at) = anim_tick  { subs.push(at); }

        // While the user is dragging the plugin-panel resize handle, listen
        // for global mouse motion and button-release events so we can update
        // the width even when the cursor leaves the 5-px-wide handle strip.
        if self.plugin_panel_drag.is_some() {
            subs.push(iced::event::listen_with(plugin_panel_drag_listener));
        }

        // Likewise while dragging the divider between the two split slots.
        if self.plugin_split_drag.is_some() {
            subs.push(iced::event::listen_with(plugin_split_drag_listener));
        }

        // Per-frame tick while a theme crossfade is in progress.
        if self.theme_anim.is_some() {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(|_| Message::ThemeAnimTick),
            );
        }

        // Route the window-manager close request (title-bar X) through our
        // hard-shutdown path. `exit_on_close_request` is disabled in main.rs, so
        // without this the X would do nothing — and with iced's default it would
        // hang on Windows exactly like the old `Message::Quit` path did.
        subs.push(iced::event::listen_with(window_close_listener));

        Subscription::batch(subs)
    }

    /// Terminate frms immediately.
    ///
    /// We hard-exit instead of returning `iced::exit()` / closing the window
    /// because on Windows the tokio runtime's shutdown joins the blocking PTY
    /// read threads (`spawn_blocking` + a synchronous `read()`), and a read on a
    /// live ConPTY never returns — so a graceful exit hangs forever. Killing the
    /// child processes first closes their PTY ends (tidy on every platform);
    /// `process::exit` then skips the runtime join entirely. State is already
    /// persisted incrementally (on each mutation), so nothing is lost here.
    /// Is agent pane `tid` currently on screen? Only the active session's
    /// center area is rendered — or both slots' while the two-up split is up —
    /// and only the one pane its center tab points at.
    fn pane_on_screen(&self, tid: TerminalId) -> bool {
        let tab = crate::session::CenterTab::Claude(tid);
        match &self.split_view {
            Some(sv) => self.sessions.iter().any(|s| {
                (s.id == sv.top_id || s.id == sv.bottom_id) && s.center_tab == tab
            }),
            None => self.sessions[self.active_session].center_tab == tab,
        }
    }

    /// Launch the CLI for any agent pane that is now on screen but hasn't
    /// started yet.
    ///
    /// Build panes are created unspawned (`AgentPane::Pending`), so opening
    /// frms with several restored sessions doesn't start one `claude` per agent
    /// tab before the user has looked at any of them — a session left untouched
    /// for the whole run never costs a process. This runs after every update,
    /// the one place that sees every path which can change what is displayed
    /// (tab clicks, Ctrl+Tab, session switches, the two-up split, restore), so
    /// no individual navigation handler has to remember to ask.
    fn spawn_visible_panes(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        // Collected first: `pane_on_screen` borrows `self`, and spawning
        // borrows it mutably.
        let pending: Vec<(usize, TerminalId)> = self.sessions.iter().enumerate()
            .flat_map(|(i, s)| s.panes.iter()
                .filter(|p| p.is_pending())
                .map(move |p| (i, p.id())))
            .collect();
        for (i, tid) in pending {
            if self.pane_on_screen(tid) {
                self.sessions[i].spawn_pending(tid);
            }
        }
    }

    /// Snap one chat transcript to its newest message, unless the user has
    /// scrolled back through it.
    ///
    /// Called as replies stream in so the answer scrolls itself into view. A
    /// snap aimed at an off-screen scrollable is a harmless no-op — those panes
    /// are re-pinned by [`Self::reveal_chat_panes`] when they come back.
    fn pin_chat_tail(&mut self, tid: TerminalId) -> Task<Message> {
        let following = self
            .sessions
            .iter()
            .flat_map(|s| s.panes.iter())
            .find(|p| p.id() == tid)
            .and_then(|p| p.as_chat())
            .map_or(false, |c| c.follow_tail);
        if !following {
            return Task::none();
        }
        scrollable::snap_to(chat_scroll_id(tid), scrollable::RelativeOffset::END)
    }

    /// Re-pin the chat panes that are now on screen to the end of their
    /// transcripts, and clear their unread markers.
    ///
    /// iced keeps a scrollable's offset in the widget tree, so a chat pane that
    /// left the view (the user switched center tab or session) loses it and
    /// comes back scrolled to the top — with any reply that arrived meanwhile
    /// below the fold. Every navigation that can change which pane is displayed
    /// runs this, so returning to a conversation lands at its newest message.
    fn reveal_chat_panes(&mut self) -> Task<Message> {
        let visible: Vec<TerminalId> = self
            .sessions
            .iter()
            .flat_map(|s| s.panes.iter().map(|p| p.id()))
            .filter(|&tid| self.pane_on_screen(tid))
            .collect();
        let mut tasks = Vec::new();
        for s in &mut self.sessions {
            for p in &mut s.panes {
                let Some(c) = p.as_chat_mut() else { continue };
                if !visible.contains(&c.id) {
                    continue;
                }
                c.unread      = false;
                // A revealed pane starts at the top regardless of where the user
                // had scrolled to before, so following resumes with it.
                c.follow_tail = true;
                tasks.push(scrollable::snap_to(
                    chat_scroll_id(c.id),
                    scrollable::RelativeOffset::END,
                ));
            }
        }
        Task::batch(tasks)
    }

    /// Move keyboard focus to agent pane `tid` in session `idx`, clearing the
    /// other focus owners — the shared logic behind pane clicks and Ctrl+Tab.
    fn focus_pane(&mut self, idx: usize, tid: TerminalId) {
        self.active_session = idx;
        self.pane_focus_toolbox = false;
        let s = &mut self.sessions[idx];
        s.active_terminal          = tid;
        s.center_tab               = crate::session::CenterTab::Claude(tid);
        s.db_panel.focus_active    = false;
        s.terminals_plugin.focused = false;
        self.receivers.unfocus_ssh();
    }

    /// Ctrl+Tab: cycle focus across the active session's agent panes, with the
    /// toolbox as one extra stop after the last pane.
    fn cycle_pane(&mut self, forward: bool) {
        let idx = self.active_session;
        let ids: Vec<TerminalId> = self.sessions[idx].panes.iter().map(|p| p.id()).collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        let total = n + 1; // agent panes + the toolbox
        let cur = if self.pane_focus_toolbox {
            n
        } else {
            ids.iter()
                .position(|&t| t == self.sessions[idx].active_terminal)
                .unwrap_or(0)
        };
        let next = if forward { (cur + 1) % total } else { (cur + total - 1) % total };
        if next == n {
            // Land on the toolbox: open it and mark it the current stop.
            self.pane_focus_toolbox = true;
            self.sessions[idx].plugin_panel.visible = true;
            self.save_persisted();
        } else {
            self.focus_pane(idx, ids[next]);
        }
    }

    /// Ctrl+Shift+A: cycle focus through only the active session's agent panes,
    /// wrapping around and never stopping on the toolbox.
    fn cycle_agent_pane(&mut self, forward: bool) {
        let idx = self.active_session;
        let ids: Vec<TerminalId> = self.sessions[idx].panes.iter().map(|p| p.id()).collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        let cur = ids
            .iter()
            .position(|&t| t == self.sessions[idx].active_terminal)
            .unwrap_or(0);
        let next = if forward { (cur + 1) % n } else { (cur + n - 1) % n };
        self.focus_pane(idx, ids[next]);
    }

    /// Ctrl+Shift+↑/↓: jump to the next/previous pane that's waiting on the user
    /// (a surfaced question or a review), scanning across every session.
    fn cycle_attention(&mut self, forward: bool) {
        let mut waiting: Vec<(usize, TerminalId)> = Vec::new();
        for (si, s) in self.sessions.iter().enumerate() {
            for p in &s.panes {
                if p.needs_attention() {
                    waiting.push((si, p.id()));
                }
            }
        }
        if waiting.is_empty() {
            return;
        }
        let here = (
            self.active_session,
            self.sessions[self.active_session].active_terminal,
        );
        let len = waiting.len();
        let target = match waiting.iter().position(|&w| w == here) {
            Some(i) => if forward { (i + 1) % len } else { (i + len - 1) % len },
            None    => if forward { 0 } else { len - 1 },
        };
        let (si, tid) = waiting[target];
        self.focus_pane(si, tid);
    }

    fn shutdown(&mut self) -> ! {
        // Snapshot state while the shells are still alive — save_persisted reads
        // each pinned terminal's live cwd from its (about-to-be-killed) child,
        // so it must run before the kill loop below or the location is lost.
        self.save_persisted();
        for session in &mut self.sessions {
            // Managed children that aren't PTYs: the Cloud SQL proxy holds a
            // local port, so leaving one behind would block the next launch.
            session.db_panel.stop_proxy();
            for pane in &mut session.panes {
                if let Some(t) = pane.as_terminal_mut() {
                    if let Ok(mut c) = t.child.lock() {
                        let _ = c.kill();
                    }
                }
            }
            for t in &mut session.terminals_plugin.terminals {
                if let Ok(mut c) = t.pane.child.lock() {
                    let _ = c.kill();
                }
            }
        }
        std::process::exit(0);
    }

    pub fn theme(&self) -> Theme {
        theme_mod::iced_theme_from(self.current_colors())
    }

    pub fn scale_factor(&self) -> f64 {
        self.prefs.font_scale.factor()
    }

    /// Theme colors as currently displayed — returns the interpolated value
    /// while a crossfade is in progress, otherwise the prefs target.
    pub fn current_colors(&self) -> ThemeColors {
        match &self.theme_anim {
            Some(a) => a.current(),
            None    => self.prefs.colors(),
        }
    }

    fn start_theme_anim(&mut self, from: ThemeColors) {
        self.theme_anim = Some(ThemeAnim {
            from,
            to:       self.prefs.colors(),
            started:  Instant::now(),
            duration: Duration::from_millis(900),
        });
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

impl Frms {
    fn push_session(
        &mut self,
        kind:        SessionKind,
        name:        Option<String>,
        working_dir: Option<PathBuf>,
    ) {
        let id         = self.next_session_id;
        let first_term = self.next_terminal_id;
        self.next_session_id  += 1;
        self.next_terminal_id += 1;
        let dir = working_dir.unwrap_or_else(|| {
            crate::port::dirs::home().unwrap_or_else(|| PathBuf::from("."))
        });
        let session = Session::new(id, name, kind, first_term, dir, self.prefs.default_tool);
        self.sessions.push(session);
        self.active_session = self.sessions.len() - 1;
        self.telemetry.record(crate::telemetry::Event::SessionCreated {
            kind: match kind {
                SessionKind::Terminal => "terminal",
                SessionKind::Browser  => "browser",
            },
        });
    }

    /// Spawn an interactive SSH session to `receiver`, replacing any prior
    /// session to that same receiver. `focus` routes the keyboard to the new
    /// pane — false during bulk connects, where nothing is on screen to type
    /// into and stealing focus per host would just thrash it.
    ///
    /// Returns the destination on success, or the reason it couldn't start.
    fn connect_receiver(&mut self, id: u64, focus: bool) -> Result<String, String> {
        let r = self.receivers.receiver_by_id(id).cloned()
            .ok_or_else(|| "receiver not found".to_string())?;
        if r.host.trim().is_empty() || r.username.trim().is_empty() {
            return Err(format!("{}: set a host and username before connecting", r.name));
        }
        let tid = self.next_terminal_id;
        self.next_terminal_id += 1;
        let (prog, args, envs) = r.ssh_command();
        match crate::terminal::TerminalPane::spawn_args(tid, &prog, &args, &envs, None) {
            Ok(pane) => {
                if focus {
                    // Other terminals lose keyboard focus to the new SSH pane.
                    self.sessions[self.active_session].terminals_plugin.focused = false;
                }
                // Replaces any prior session to *this* receiver; sessions to
                // other receivers keep running so their panes survive.
                self.receivers.set_ssh(crate::receivers::SshTerminal {
                    id: tid,
                    receiver: id,
                    name: r.name.clone(),
                    pane,
                    focused: focus,
                    exited: false,
                });
                Ok(r.destination())
            }
            Err(e) => Err(format!("{}: ssh failed: {e}", r.name)),
        }
    }

    fn session_for_terminal_mut(&mut self, id: TerminalId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.panes.iter().any(|p| p.id() == id))
    }

    fn session_for_plugin_terminal_mut(&mut self, id: TerminalId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.terminals_plugin.has_terminal(id))
    }

    /// The chat pane behind `id` (a Research / Chat agent), if that's what it
    /// is. The find handlers use it to fall through from PTY panes to chats.
    fn chat_pane_mut(&mut self, id: TerminalId) -> Option<&mut crate::agent::ChatPane> {
        let i = self.sessions.iter().position(|s| s.panes.iter().any(|p| p.id() == id))?;
        self.sessions[i].chat_mut(id)
    }

    /// Re-measure whether the agent-tab strip overflows, after a message that
    /// could have changed either the tabs or the room they have.
    ///
    /// "Measure" here means: assume they fit. The strip publishes its geometry
    /// (`Message::AgentStripViewport`) on the next redraw *only* while it
    /// overflows, so clearing the flag and letting the strip contradict us is
    /// the whole detection — an unpadded strip that turns out to overflow is
    /// corrected within a frame, and one that fits simply stays unpadded and
    /// keeps the callout at its natural height.
    fn refresh_agent_strip_overflow(&mut self, relaid_out: bool) {
        let sig = self.agent_strip_signature();
        if relaid_out || sig != self.agent_strip_sig {
            self.agent_strip_sig      = sig;
            self.agent_strip_overflow = false;
        }
    }

    /// Fingerprint of everything that decides how wide the agent-tab strip is:
    /// which session is on screen (each project has its own set of tabs), which
    /// panes it holds, what they're called, and any rename box currently open in
    /// their place.
    ///
    /// Deliberately *not* hashed: the panes' attention badges. A badge does
    /// nudge a tab's width, but it also flips several times a second while a
    /// CLI repaints (see [`TerminalPane::note_menu_on_screen`]), and every flip
    /// cleared the overflow flag, dropped the strip's scrollbar gutter, and
    /// handed the terminal below an extra row — resizing the pty, forcing a
    /// repaint, and flipping the badge again. The strip re-reports overflow on
    /// its own the moment it overflows, so all a badge can cost us here is a
    /// gutter that lingers until the next real layout change.
    ///
    /// Only the sessions actually rendering a strip are hashed — the active one
    /// plus, in split view, the two on either side of the divider. Hashing every
    /// session would miss the case that matters most here: switching projects
    /// swaps one set of tabs for another without changing any session's
    /// contents, and a strip that overflowed in the project just left must not
    /// leave its gutter behind in the one just opened.
    fn agent_strip_signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();

        let mut visible: Vec<usize> = Vec::with_capacity(3);
        if let Some(s) = self.sessions.get(self.active_session) {
            visible.push(s.id);
        }
        if let Some(sv) = &self.split_view {
            visible.push(sv.top_id);
            visible.push(sv.bottom_id);
        }

        for id in visible {
            id.hash(&mut h);
            let Some(s) = self.sessions.iter().find(|s| s.id == id) else { continue };
            for p in &s.panes {
                p.id().hash(&mut h);
                p.name().hash(&mut h);
                p.kind().label().hash(&mut h);
            }
        }
        self.renaming_claude.hash(&mut h);
        h.finish()
    }

    /// Whether the find bar is open on `id`, whichever kind of agent pane it is.
    fn find_is_open(&self, id: TerminalId) -> bool {
        let Some(s) = self.sessions.iter().find(|s| s.panes.iter().any(|p| p.id() == id)) else {
            return false;
        };
        s.terminal(id).map_or(false, |t| t.find.is_some())
            || s.chat(id).map_or(false, |c| c.find.is_some())
    }

    /// Open the find bar on `id` and refresh its hits.
    fn find_open(&mut self, id: TerminalId) {
        if let Some(t) = self.terminal_pane_mut(id) {
            t.find_open();
        } else if let Some(c) = self.chat_pane_mut(id) {
            c.find_open();
        }
    }

    /// Close the find bar on `id`.
    fn find_close(&mut self, id: TerminalId) {
        if let Some(t) = self.terminal_pane_mut(id) {
            t.find_close();
        } else if let Some(c) = self.chat_pane_mut(id) {
            c.find_close();
        }
    }

    /// Scroll a chat pane's transcript to its current find hit. Bubbles vary in
    /// height, so this positions by *message index* rather than pixels: it puts
    /// the view in the right neighbourhood, and the hit bubble's accent border
    /// is what pins it down exactly.
    fn reveal_chat_hit(&mut self, id: TerminalId) -> Task<Message> {
        let Some(c) = self.chat_pane_mut(id) else { return Task::none() };
        let total = c.messages.len();
        let Some(hit) = c.find.as_ref().and_then(|f| f.hits.get(f.current).copied()) else {
            return Task::none();
        };
        // Reading a hit means reading what led to it, so aim slightly above.
        let y = if total > 1 {
            (hit.saturating_sub(1) as f32 / (total - 1) as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // The user is now driving the viewport; stop streaming from yanking it.
        c.follow_tail = false;
        scrollable::snap_to(chat_scroll_id(id), scrollable::RelativeOffset { x: 0.0, y })
    }

    /// The terminal pane behind `id`, wherever it lives — an agent/Claude pane,
    /// a plugin shell terminal, or a receiver SSH session. Lets the scroll and
    /// drag-selection handlers treat all three alike.
    fn terminal_pane_mut(&mut self, id: TerminalId) -> Option<&mut TerminalPane> {
        if let Some(i) = self.sessions.iter().position(|s| s.panes.iter().any(|p| p.id() == id)) {
            return self.sessions[i].terminal_mut(id);
        }
        if let Some(i) = self.sessions.iter().position(|s| s.terminals_plugin.has_terminal(id)) {
            return self.sessions[i].terminals_plugin.terminal_mut(id).map(|t| &mut t.pane);
        }
        self.receivers.ssh_mut(id).map(|t| &mut t.pane)
    }

    /// Execute the close the user just confirmed in the dialog — either a
    /// whole session tab or a single Claude tab.
    fn confirm_pending_close(&mut self) {
        match self.pending_close.take() {
            Some(PendingClose::Session(idx)) => {
                // Guard against state having shifted while the dialog was up.
                if self.sessions.len() > 1 && idx < self.sessions.len() {
                    let closed_id = self.sessions[idx].id;
                    // Drop the split if it referenced the session being closed.
                    if matches!(
                        &self.split_view,
                        Some(sv) if sv.top_id == closed_id || sv.bottom_id == closed_id
                    ) {
                        self.split_view = None;
                    }
                    // The Cloud SQL proxy is a child of frms, not of the pane —
                    // closing its session has to take it down too.
                    self.sessions[idx].db_panel.stop_proxy();
                    self.sessions.remove(idx);
                    if self.active_session >= self.sessions.len() {
                        self.active_session = self.sessions.len() - 1;
                    }
                    self.save_persisted();
                }
            }
            Some(PendingClose::Claude(tid)) => {
                if let Some(s) = self.session_for_terminal_mut(tid) {
                    if s.close_claude_terminal(tid) {
                        self.save_persisted();
                    }
                }
            }
            None => {}
        }
    }

    /// The active session's project key for note scoping — its working
    /// directory. `None` (no session / non-UTF-8 path) means global.
    fn active_project(&self) -> Option<String> {
        self.sessions.get(self.active_session)
            .and_then(|s| s.working_dir.to_str())
            .map(String::from)
    }

    /// Re-index the active session's `_specs` / `_docs` folders. Runs whenever
    /// the Docs tab comes on screen (and on the ↻ button) rather than on a
    /// watcher, so the list is current each time it's looked at without frms
    /// holding a watch on every project it has ever opened.
    fn rescan_docs(&mut self) {
        let Some(s) = self.sessions.get_mut(self.active_session) else { return };
        s.docs.rescan(&s.working_dir);
    }

    /// Bring `tab` up in the toolbox's top slot and drop the caret into the
    /// widget `focus_id` — the shared path behind the "go straight to this
    /// search" chords. Un-hides the plugin if the user had closed its tab, so
    /// the chord works from any state rather than silently doing nothing.
    fn open_plugin_tab(&mut self, tab: PluginTab, focus_id: &'static str) -> Task<Message> {
        {
            let panel = &mut self.sessions[self.active_session].plugin_panel;
            panel.show_tab(tab);
            panel.active_tab = tab;
            panel.visible = true;
            panel.adding = false;
            panel.controls_menu = false;
        }
        if tab == PluginTab::Docs {
            self.rescan_docs();
        }
        self.save_persisted();
        text_input::focus(text_input::Id::new(focus_id))
    }

    /// Capture terminal `id`'s selection into a brand-new note (clearing the
    /// selection), then reveal the Notes plugin so the note is on screen and
    /// ready to edit. Works for both Claude panes and shell-plugin terminals.
    /// Copy `files` to the currently-selected receiver. Thin wrapper over
    /// [`copy_files_to`] used by the file-browser row action.
    fn start_receiver_copy(&mut self, files: Vec<std::path::PathBuf>) -> Task<Message> {
        let Some(id) = self.receivers.selected_receiver else {
            self.receivers.status = Some(Err("select a receiver first".into()));
            return Task::none();
        };
        self.copy_files_to(id, files)
    }

    /// Copy `files` to receiver `id`'s home directory over scp, after
    /// validating it has the credentials needed. Records an immediate error in
    /// the status line when prerequisites are missing.
    fn copy_files_to(&mut self, id: u64, files: Vec<std::path::PathBuf>) -> Task<Message> {
        let Some(r) = self.receivers.receiver_by_id(id).cloned() else {
            self.receivers.status = Some(Err("receiver not found".into()));
            return Task::none();
        };
        if r.host.trim().is_empty() || r.username.trim().is_empty() {
            self.receivers.status = Some(Err("receiver is missing a host or username".into()));
            return Task::none();
        }
        self.receivers.status = Some(Ok(format!("Copying to {}…", r.destination())));
        Task::perform(
            crate::scp::copy(r.host, r.username, r.password, r.port, files),
            Message::ReceiverCopyDone,
        )
    }

    fn selection_to_note(&mut self, id: TerminalId) {
        let text = if let Some(s) = self.session_for_terminal_mut(id) {
            s.terminal_mut(id).and_then(|t| {
                let sel = t.selection.take()?;
                let g = t.grid.lock().ok()?;
                Some(sel.text(&g))
            })
        } else if let Some(s) = self.session_for_plugin_terminal_mut(id) {
            s.terminals_plugin.terminal_mut(id).and_then(|t| {
                let sel = t.pane.selection.take()?;
                let g = t.pane.grid.lock().ok()?;
                Some(sel.text(&g))
            })
        } else {
            None
        };
        let Some(text) = text else { return };
        self.text_to_note(&text);
    }

    /// Create a note from `text` (ignored when blank) and reveal the Notes tab
    /// in the active session. Shared by the terminal selection path
    /// ([`Self::selection_to_note`]) and the chat-bubble "+ Note" affordance.
    fn text_to_note(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }

        // The note lands in the active session's Notes tab, so pin it to
        // that session's project.
        let project = self.active_project();
        self.notes.create_from(text, project);

        // Reveal the Notes plugin in the active session. If a split panel
        // already shows Notes in the bottom slot, leave the slots alone;
        // otherwise point the top (or only) slot at Notes.
        let s = &mut self.sessions[self.active_session];
        let panel = &mut s.plugin_panel;
        panel.visible = true;
        if !(panel.split && panel.bottom_tab == PluginTab::Notes) {
            panel.active_tab = PluginTab::Notes;
        }
        s.db_panel.focus_active = false;
    }

    /// Re-evaluate whether a Claude permission prompt is currently on-screen
    /// for `source`'s terminal and update the inline response panel state.
    fn reconcile_prompt(s: &mut Session, source: TerminalId) {
        // Both readings come from one lock: whether the prompt parses, and
        // whether *any* question menu is on screen. They disagree exactly when
        // the CLI is mid-repaint, which is the case the debounce below cares
        // about.
        let (detected, menu_up) = s.terminal(source)
            .and_then(|t| t.grid.lock().ok())
            .map(|g| (claude_prompt::detect(&g), claude_prompt::is_waiting_on_user(&g)))
            .unwrap_or((None, false));

        let surfaced = s.prompt_source_terminal == Some(source) && s.pending_prompt.is_some();

        match detected {
            Some(prompt) => {
                // Prompt is on screen — cancel any pending tear-down.
                s.prompt_lost_at = None;
                let fp = prompt.fingerprint();
                // Don't re-surface a prompt the user already answered.
                if s.last_handled_prompt.as_deref() == Some(fp.as_str()) {
                    return;
                }
                match &s.pending_prompt {
                    // Nothing up yet: surface at once. A question the user is
                    // being kept waiting on shouldn't sit behind a settle timer.
                    None => {
                        s.pending_prompt         = Some(prompt);
                        s.prompt_source_terminal = Some(source);
                        s.prompt_swap            = None;
                    }
                    Some(cur) if *cur == prompt => s.prompt_swap = None,
                    // A *different* reading while a panel is already up. Half of
                    // these are half-drawn frames whose diff is truncated or
                    // empty; adopting each one makes the panel's contents jump.
                    // Only swap once the new reading has held still.
                    Some(_) => match &s.prompt_swap {
                        Some((seen, since)) if *seen == fp => {
                            if since.elapsed() >= PROMPT_SWAP_SETTLE {
                                s.pending_prompt         = Some(prompt);
                                s.prompt_source_terminal = Some(source);
                                s.prompt_swap            = None;
                            }
                        }
                        _ => s.prompt_swap = Some((fp, Instant::now())),
                    },
                }
            }
            None if surfaced => {
                // The prompt stopped parsing on a source whose panel is up.
                // Showing the panel shrinks the terminal, which makes the CLI
                // repaint, and a mid-repaint frame has no parseable prompt on
                // it — tearing the panel down there would resize the terminal
                // back and flicker forever. So debounce, and hold far longer
                // while the terminal still shows a menu of some kind: that says
                // the question is still up and only our parse of it failed.
                // The hold is bounded so a *different*, unparseable question
                // can't leave the previous diff stranded on screen.
                let lost_for = s.prompt_lost_at.get_or_insert_with(Instant::now).elapsed();
                let limit = if menu_up { PROMPT_STALE_MAX } else { PROMPT_CLEAR_DELAY };
                if lost_for >= limit {
                    s.clear_prompt();
                }
            }
            None => {
                // No prompt and none surfaced for this source — clear stale
                // bookkeeping so the next (different) prompt can re-surface.
                s.prompt_lost_at      = None;
                s.last_handled_prompt = None;
                s.prompt_swap         = None;
            }
        }
    }

    /// Write a response (e.g. b"1\r" or b"3\r") to the Claude terminal that
    /// triggered the active prompt. The next `TerminalData` from Claude
    /// will clear `last_handled_prompt`.
    fn respond_to_prompt(&mut self, bytes: &[u8]) {
        let s = &mut self.sessions[self.active_session];
        let Some(target) = s.prompt_source_terminal else { return };
        let answered = s.pending_prompt.as_ref().map(|p| p.fingerprint());
        if let Some(t) = s.terminal_mut(target) {
            t.write_input(bytes);
            // Drop the attention highlight immediately — the next PTY data
            // chunk would clear it anyway, but this avoids a stale flash.
            t.clear_waiting();
        }
        // Clear everything, then re-arm the one piece that has to survive: the
        // fingerprint of what was just answered, so the panel doesn't re-open
        // on the same question in the frames before the CLI redraws without it.
        s.clear_prompt();
        s.last_handled_prompt = answered;
    }
}

// ── keyboard mapping ──────────────────────────────────────────────────────────

/// `listen_with` listener used while the splash is showing: Enter dismisses
/// the splash (same as clicking "Get Started"); every other key is dropped.
fn splash_key_listener(
    event:   iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
        ..
    }) = event {
        return Some(Message::SplashDismissed);
    }
    None
}

/// `listen_with` listener active only while a plugin-panel resize drag is in
/// progress. Forwards cursor motion and mouse-release events to the update
/// loop so width tracking continues outside the drag handle.
fn plugin_panel_drag_listener(
    event:   iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) =>
            Some(Message::PluginPanelResizeMove(position.x)),
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(_)) =>
            Some(Message::PluginPanelResizeEnd),
        _ => None,
    }
}

/// Always-on listener that turns the window-manager close request (the
/// title-bar X) into `Message::Quit`, so it runs through our hard-shutdown
/// path instead of iced's default graceful exit (which hangs on Windows).
/// It also forwards resizes, which change how much room the agent-tab strip has.
fn window_close_listener(
    event:   iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(iced::window::Event::CloseRequested) => Some(Message::Quit),
        iced::Event::Window(iced::window::Event::Resized(_))     => Some(Message::WindowResized),
        _ => None,
    }
}

/// `listen_with` listener active only while the split-divider drag is in
/// progress. Forwards cursor motion and mouse-release events so the top-slot
/// height keeps tracking outside the thin divider strip.
fn plugin_split_drag_listener(
    event:   iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) =>
            Some(Message::PluginSplitResizeMove(position.y)),
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(_)) =>
            Some(Message::PluginSplitResizeEnd),
        _ => None,
    }
}

/// `listen_with` listener used during normal IDE use.
fn main_key_listener(
    event:  iced::Event,
    status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    // FRMS_KEYLOG=1 logs every key press with its physical key, modifiers, and
    // capture status. Kept (not temp): `Captured` vs `Ignored` is the fastest
    // way to tell a key a widget swallowed from one we mis-route ourselves —
    // that's how the Ctrl+Shift+arrow and numpad bugs were pinned down — and
    // it shows what the keypad reports under either NumLock state.
    if std::env::var("FRMS_KEYLOG").is_ok() {
        if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key, physical_key, modifiers, ..
        }) = &event {
            eprintln!("[keylog] key={key:?} physical={physical_key:?} mods={modifiers:?} status={status:?}");
        }
    }
    let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key, modifiers, text, physical_key, ..
    }) = event else {
        return None;
    };

    // App-level chords are resolved first, ahead of the capture gate below —
    // they're global by design, so a focused text widget must not be able to
    // swallow them. See `app_chord` for which keys that covers and what text
    // editing gives up. Two ways the gate used to eat them: Ctrl+Shift+arrow
    // (and PageUp/PageDown, Enter, C/V/X/A) are claimed outright by
    // `text_input`/`text_editor`, and a numpad digit with NumLock off arrives
    // as End / ArrowDown / PageUp / … — navigation keys those widgets consume.
    if let Some(message) = app_chord(&key, modifiers, &physical_key) {
        return Some(message);
    }

    if !matches!(key, iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab))
        && status == iced::event::Status::Captured
    {
        return None;
    }
    handle_key(key, modifiers, text.map(|t| t.to_string()), physical_key)
}

/// Wrap pasted text with bracketed-paste markers if the terminal program
/// has opted in via DECSET 2004. Without bracketing, multi-line pastes get
/// interpreted as typed input — the first newline submits, the rest run as
/// separate commands. The markers are inert when the program hasn't opted
/// in, so we only emit them in that case.
fn paste_payload(text: &str, bracketed: bool) -> String {
    if bracketed {
        format!("\x1b[200~{}\x1b[201~", text)
    } else {
        text.to_string()
    }
}

/// App-level chords, which outrank every widget.
///
/// `main_key_listener` resolves these *before* its `Status::Captured` gate, so
/// they fire even when a focused text widget has already eaten the key. That
/// bypass is the whole point: iced's `text_input` and `text_editor` claim
/// Ctrl+Shift+arrow (select by word), Ctrl+Shift+PageUp/PageDown (select by
/// page), Enter, and Ctrl+Shift+C/V/X/A — none of those arms guard on Shift —
/// so every navigation chord here silently died the moment focus landed in the
/// code editor or any text field. Behind the gate they were unreachable.
///
/// The unshifted forms still reach the widget, so text editing keeps plain
/// Ctrl+C/V/X/A, plain and Ctrl+arrow motion, and Shift+arrow selection. What
/// it gives up is Ctrl+Shift+arrow / Ctrl+Shift+PageUp/PageDown selection —
/// the app owns those.
///
/// Note what running early does *not* do: the listener only observes, so a
/// widget that captured the key still acts on it. Both halves happen. That's
/// fine for navigation (the widget moves a cursor we're about to defocus) but
/// not for the clipboard, so Ctrl+Shift+C/V deliberately stay behind the gate
/// in [`handle_key`] — see there.
fn app_chord(
    key:       &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical:  &keyboard::key::Physical,
) -> Option<Message> {
    if !modifiers.control() {
        return None;
    }
    // Ctrl+Tab / Ctrl+Shift+Tab — cycle panes (incl. the toolbox). Kept as a
    // fallback where the OS leaves it free; the primary binding is
    // Ctrl+Shift+→/← below. Checked first: it isn't a terminal control code,
    // so the shell never wants it.
    if matches!(key, keyboard::Key::Named(key::Named::Tab)) {
        return Some(Message::CyclePane { forward: !modifiers.shift() });
    }

    // Ctrl+1…9 → jump straight to project session N (like browser tabs), from
    // the number row or the numeric keypad.
    if let Some(index) = session_switch_chord(key, modifiers, physical) {
        return Some(Message::SessionSelected(index));
    }

    if modifiers.shift() {
        match key {
            keyboard::Key::Named(key::Named::ArrowDown) =>
                return Some(Message::CycleAttention { forward: true }),
            keyboard::Key::Named(key::Named::ArrowUp) =>
                return Some(Message::CycleAttention { forward: false }),
            // Ctrl+Shift+→/← — cycle panes (incl. the toolbox). Replaces
            // Ctrl+Tab, which the OS reserves; →=next, ←=previous.
            keyboard::Key::Named(key::Named::ArrowRight) =>
                return Some(Message::CyclePane { forward: true }),
            keyboard::Key::Named(key::Named::ArrowLeft) =>
                return Some(Message::CyclePane { forward: false }),
            // Ctrl+Shift+PageDown/PageUp — cycle the toolbox's tool tabs.
            keyboard::Key::Named(key::Named::PageDown) =>
                return Some(Message::CyclePluginTab { forward: true }),
            keyboard::Key::Named(key::Named::PageUp) =>
                return Some(Message::CyclePluginTab { forward: false }),
            keyboard::Key::Named(key::Named::Enter) =>
                return Some(Message::PromptAccepted),
            keyboard::Key::Character(s) => {
                // Session switch. Accept both the base and shifted glyphs
                // ("[" vs "{") since layouts report either for Shift+[.
                match s.as_ref() {
                    "[" | "{" => return Some(Message::CycleSession { forward: false }),
                    "]" | "}" => return Some(Message::CycleSession { forward: true }),
                    _ => {}
                }
                match s.to_ascii_lowercase().as_str() {
                    // Ctrl+Shift+A → cycle focus through the active session's
                    // agent panes only (skips the toolbox, unlike Ctrl+Shift+→).
                    "a" => return Some(Message::CycleAgentPane { forward: true }),
                    // Ctrl+Shift+N → new note from the selection. (C/V are the
                    // clipboard pair and live in `handle_key`, behind the
                    // capture gate.)
                    "n" => return Some(Message::TerminalSendToNote),
                    // Toolbox toggle / new agent / close focused agent.
                    "b" => return Some(Message::PluginPanelToggled),
                    // Ctrl+Shift+F → blow the toolbox up to the full window
                    // width, or restore the main area. Expanding implies
                    // showing it, so this works from a hidden toolbox too.
                    "f" => return Some(Message::PluginPanelExpandToggled),
                    // Ctrl+Shift+E → show/hide the file explorer beside the
                    // editor and agent panes.
                    "e" => return Some(Message::FileBrowserToggleCollapsed),
                    // Ctrl+Shift+D → open this project's Database tool. D for
                    // Database, the obvious key for it; the docs search that
                    // used to hold it moved to S (for _specs).
                    "d" => return Some(Message::CenterTabSelected(
                        crate::session::CenterTab::Database,
                    )),
                    // Ctrl+Shift+S → jump straight to the Docs search over this
                    // project's _specs / _docs, caret already in the field.
                    "s" => return Some(Message::DocsOpen),
                    // Ctrl+Shift+G → the same, for searching notes. G because
                    // D/F/E are taken and Ctrl+Shift+N already makes a note.
                    "g" => return Some(Message::NotesSearchOpen),
                    // Ctrl+Shift+M → change the model on one or many of this
                    // project's agent panes.
                    "m" => return Some(Message::AgentsDialogOpened),
                    // Ctrl+Shift+L → find in the agent conversation that's in
                    // front. Resolved to that pane's id in `update`; L rather
                    // than the conventional F because F expands the toolbox,
                    // and Ctrl+F belongs to the CLI running in the pane.
                    "l" => return Some(Message::FindToggled),
                    // Quick "new agent" — resolved against the user's
                    // default tool in `update` (no prefs access here).
                    "t" => return Some(Message::NewDefaultAgent),
                    "w" => return Some(Message::CloseActiveAgent),
                    // Ctrl+Shift+Q → quit the app (same hard-shutdown path as
                    // the title-bar X and the header Quit button).
                    "q" => return Some(Message::Quit),
                    // Answer the surfaced prompt: Enter accepts, R rejects.
                    "r" => return Some(Message::PromptRejected),
                    _   => {}
                }
            }
            _ => {}
        }
    }

    // Ctrl+Shift+1…9 → jump straight to agent pane N *within* the session.
    // The shifted sibling of Ctrl+1…9, so the pair reads as "Ctrl+digit picks
    // the project, add Shift to pick the session inside it".
    //
    // Deliberately last. A numeric-keypad key with NumLock off reports itself
    // as End / ArrowDown / PageUp, which are Ctrl+Shift chords in their own
    // right — resolving those first keeps Ctrl+Shift+↓ meaning "next pane
    // needing attention" even when it arrives from the keypad.
    if let Some(index) = agent_pane_chord(key, modifiers, physical) {
        return Some(Message::AgentPaneSelected(index));
    }
    None
}

/// Top-level keyboard handler — returns app messages or terminal input.
fn handle_key(
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    text: Option<String>,
    physical: keyboard::key::Physical,
) -> Option<Message> {
    // Global chords win. `main_key_listener` has normally resolved these
    // already (ahead of its capture gate); repeated here so this function is
    // correct on its own, and so they stay ahead of the Ctrl+<letter> →
    // control-byte mapping below rather than reaching the shell.
    if let Some(msg) = app_chord(&key, modifiers, &physical) {
        return Some(msg);
    }

    // Ctrl+Shift+C/V → terminal copy / paste (the shift frees unshifted Ctrl+C
    // → SIGINT and Ctrl+V for the shell). Unlike the chords in `app_chord`,
    // these are held behind the caller's capture gate on purpose: iced's text
    // widgets claim them too, and since a captured key still reaches the
    // widget, running early would copy over the clipboard it just wrote, or
    // paste into the terminal *as well as* the field being typed in. With a
    // text widget focused the widget wins; a focused terminal captures nothing,
    // so the intended case is unaffected.
    if modifiers.control() && modifiers.shift() {
        if let keyboard::Key::Character(s) = &key {
            match s.to_ascii_lowercase().as_str() {
                "c" => return Some(Message::TerminalCopy),
                "v" => return Some(Message::TerminalPaste),
                _   => {}
            }
        }
    }

    if modifiers.control() {
        // General Ctrl+<key> → ASCII control byte, sent to the focused terminal.
        // Ctrl+A…Ctrl+Z map to 0x01…0x1A and Ctrl+[ \ ] ^ _ @ to 0x1B…0x1F / 0x00
        // — the standard caret-notation control codes — so readline chords
        // (Ctrl+A, Ctrl+E, Ctrl+R, Ctrl+U, Ctrl+W, …) reach the shell.
        if let keyboard::Key::Character(s) = &key {
            let ch = s.chars().next()?;
            // Digits already went to `session_switch_chord` above; anything left
            // here (including Ctrl+Shift+<digit>, which arrives as "!", "@", …)
            // falls through to the control-byte mapping or a no-op.
            if ch.is_ascii_alphabetic() || "@[\\]^_".contains(ch) {
                let byte = (ch.to_ascii_uppercase() as u8) & 0x1f;
                return Some(Message::TerminalInput((byte as char).to_string()));
            }
            return None;
        }
    }
    // Route Tab through update() so it can become focus_next/focus_previous
    // when the DB form is in focus, and a literal tab byte otherwise.
    if matches!(key, keyboard::Key::Named(key::Named::Tab)) {
        return Some(Message::TabPressed { shift: modifiers.shift() });
    }
    // Prefer `text` for character keys: it reflects shift/AltGr/IME, so
    // Shift+'-' arrives as "_" rather than the unmodified "-" in `key`.
    // Named keys keep the explicit control-sequence mapping (Backspace → 0x7f, etc.).
    let bytes = match key {
        keyboard::Key::Character(s) => Some(text.unwrap_or_else(|| s.to_string())),
        keyboard::Key::Named(n)     => named_key_bytes(&n),
        keyboard::Key::Unidentified => None,
    }?;
    Some(Message::TerminalInput(bytes))
}

/// Ctrl+1…9 → the 0-based session index to jump to, or `None` if this isn't
/// that chord.
///
/// Two ways in. The number row comes through as a logical `Character("1")`.
/// The numeric keypad has to be matched on its *physical* key instead, because
/// its logical key depends on NumLock: on, the digits arrive as characters;
/// off, they arrive as End / ArrowDown / PageUp / Insert / … — the navigation
/// keys those positions double as. Physical codes are stable across both, and
/// across keyboard layouts.
///
/// Shift is excluded so the Ctrl+Shift+<key> chords keep the field to
/// themselves (Ctrl+Shift+<digit> also arrives as a symbol — "!", "@", … — on
/// the number row, which wouldn't parse as a digit anyway).
fn session_switch_chord(
    key:       &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical:  &keyboard::key::Physical,
) -> Option<usize> {
    if !modifiers.control() || modifiers.shift() {
        return None;
    }
    let digit = match numpad_digit(physical) {
        Some(d) => d,
        None => match key {
            keyboard::Key::Character(s) => s.chars().next()?.to_digit(10)?,
            _ => return None,
        },
    };
    // Sessions are 1-indexed on the keyboard; there's no session 0.
    (1..=9).contains(&digit).then(|| digit as usize - 1)
}

/// Ctrl+Shift+1…9 → the index of the agent pane to focus in the active
/// session, 0-based. The counterpart of [`session_switch_chord`].
///
/// Reads the *physical* key rather than the character, because a shifted
/// number-row digit arrives as its symbol — "!" , "@", "#" on a US layout and
/// something else entirely elsewhere. The symbols are accepted too, as a
/// fallback for platforms that report no usable physical code.
fn agent_pane_chord(
    key:       &keyboard::Key,
    modifiers: keyboard::Modifiers,
    physical:  &keyboard::key::Physical,
) -> Option<usize> {
    if !modifiers.control() || !modifiers.shift() {
        return None;
    }
    // A letter is never a digit chord, whatever physical key it happens to sit
    // on — Ctrl+Shift+<letter> owns those.
    if let keyboard::Key::Character(s) = key {
        if s.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            return None;
        }
    }
    let digit = row_digit(physical)
        .or_else(|| numpad_digit(physical))
        .or_else(|| match key {
            keyboard::Key::Character(s) => shifted_digit(s.chars().next()?),
            _ => None,
        })?;
    // Panes are 1-indexed on the keyboard; there's no pane 0.
    (1..=9).contains(&digit).then(|| digit as usize - 1)
}

/// The digit (1–9) of a number-row key, by physical position — so it is right
/// whatever the layout prints on the keycap and whatever Shift turns it into.
fn row_digit(physical: &keyboard::key::Physical) -> Option<u32> {
    use keyboard::key::{Code, Physical};
    match physical {
        Physical::Code(Code::Digit1) => Some(1),
        Physical::Code(Code::Digit2) => Some(2),
        Physical::Code(Code::Digit3) => Some(3),
        Physical::Code(Code::Digit4) => Some(4),
        Physical::Code(Code::Digit5) => Some(5),
        Physical::Code(Code::Digit6) => Some(6),
        Physical::Code(Code::Digit7) => Some(7),
        Physical::Code(Code::Digit8) => Some(8),
        Physical::Code(Code::Digit9) => Some(9),
        _ => None,
    }
}

/// The digit behind a shifted number-row character on a US layout, or a bare
/// digit where the layout reports one despite Shift. Last-resort fallback for
/// [`agent_pane_chord`].
fn shifted_digit(c: char) -> Option<u32> {
    match c {
        '!' => Some(1), '@' => Some(2), '#' => Some(3),
        '$' => Some(4), '%' => Some(5), '^' => Some(6),
        '&' => Some(7), '*' => Some(8), '(' => Some(9),
        _   => c.to_digit(10),
    }
}

/// The digit (1–9) of a numeric-keypad key, or `None` for anything else.
/// Numpad0 is intentionally excluded — there's no session 0.
fn numpad_digit(physical: &keyboard::key::Physical) -> Option<u32> {
    use keyboard::key::{Code, Physical};
    match physical {
        Physical::Code(Code::Numpad1) => Some(1),
        Physical::Code(Code::Numpad2) => Some(2),
        Physical::Code(Code::Numpad3) => Some(3),
        Physical::Code(Code::Numpad4) => Some(4),
        Physical::Code(Code::Numpad5) => Some(5),
        Physical::Code(Code::Numpad6) => Some(6),
        Physical::Code(Code::Numpad7) => Some(7),
        Physical::Code(Code::Numpad8) => Some(8),
        Physical::Code(Code::Numpad9) => Some(9),
        _ => None,
    }
}

fn named_key_bytes(key: &key::Named) -> Option<String> {
    use key::Named::*;
    Some(match key {
        Space      => " ".into(),
        Enter      => "\r".into(),
        Backspace  => "\x7f".into(),
        Escape     => "\x1b".into(),
        Tab        => "\t".into(),
        ArrowUp    => "\x1b[A".into(),
        ArrowDown  => "\x1b[B".into(),
        ArrowRight => "\x1b[C".into(),
        ArrowLeft  => "\x1b[D".into(),
        Home       => "\x1b[H".into(),
        End        => "\x1b[F".into(),
        PageUp     => "\x1b[5~".into(),
        PageDown   => "\x1b[6~".into(),
        Delete     => "\x1b[3~".into(),
        _          => return None,
    })
}

// ── HTML preview render ───────────────────────────────────────────────────────

/// Render an HTML file via headless chromium/firefox and return the PNG bytes.
/// Reuses the same multi-fallback path as the browser pane's screenshot
/// pipeline so that whatever's installed on the system will work.
async fn render_html_preview(
    session_id: usize,
    path:       PathBuf,
) -> (usize, PathBuf, Result<Vec<u8>, String>) {
    let url = format!("file://{}", path.display());
    let (sid, result) = take_screenshot(session_id, url).await;
    (sid, path, result)
}

// ── URL normalization ─────────────────────────────────────────────────────────

/// Ensure the URL has a scheme. Bare hostnames default to http://.
fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() { return String::new(); }
    if s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("file://")
        || s.starts_with("about:")
    {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

// ── headless browser screenshot ───────────────────────────────────────────────

/// Capture a PNG screenshot of `url`.
/// Tries Chromium variants then Firefox (direct and via flatpak).
/// Returns (session_id, Result<png_bytes, error_string>).
async fn take_screenshot(session_id: usize, url: String) -> (usize, Result<Vec<u8>, String>) {
    use tokio::process::Command;
    use std::fs;

    eprintln!("[take_screenshot] session={session_id} url={url}");

    // Use $HOME — accessible by both the toolbox and Firefox/Chromium Flatpaks on the host
    let home = crate::port::dirs::home()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "/var/tmp".to_string());
    let tmp = std::path::PathBuf::from(format!("{home}/.cache/frms_screenshot_{session_id}.png"));
    // Ensure the directory exists
    let _ = fs::create_dir_all(tmp.parent().unwrap());
    let tmp_str = tmp.display().to_string();

    let mut last_err = String::from("no supported headless browser found");
    let timeout_dur = std::time::Duration::from_secs(20);

    // ── helper: run a command with timeout, check exit, read file ────────────
    macro_rules! try_cmd {
        ($prog:expr, $args:expr, $label:expr) => {{
            let child = Command::new($prog)
                .args($args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn();
            match child {
                Err(_) => {} // binary not found — try next
                Ok(child) => {
                    match tokio::time::timeout(timeout_dur, child.wait_with_output()).await {
                        Err(_) => {
                            last_err = format!("{} timed out", $label);
                        }
                        Ok(Err(_)) => {}
                        Ok(Ok(out)) if out.status.success() => {
                            if let Ok(bytes) = fs::read(&tmp) {
                                let _ = fs::remove_file(&tmp);
                                return (session_id, Ok(bytes));
                            }
                            last_err = format!("{} ran but produced no screenshot file", $label);
                        }
                        Ok(Ok(out)) => {
                            last_err = format!(
                                "{}: exit {:?} — {}",
                                $label,
                                out.status.code(),
                                String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or(""),
                            );
                        }
                    }
                }
            }
        }};
    }

    // ── 1. Chromium — direct ──────────────────────────────────────────────────
    let chrome_screenshot = format!("--screenshot={tmp_str}");
    let chrome_args = vec![
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        "--disable-software-rasterizer",
        "--hide-scrollbars",
        "--window-size=1280,900",
        chrome_screenshot.as_str(),
        url.as_str(),
    ];
    for binary in crate::port::browser::chromium_binaries() {
        try_cmd!(&binary, &chrome_args, binary);
    }

    // ── Firefox helper ────────────────────────────────────────────────────────
    let cache_dir = format!("{home}/.cache");
    let ff_profile = format!("{cache_dir}/frms_ff_profile");
    let _ = fs::create_dir_all(&ff_profile);
    // Use an absolute path so Firefox writes the file regardless of its working
    // directory (important when calling the host binary from inside a container).
    let ff_out_path = format!("{cache_dir}/frms_screenshot_{session_id}.png");

    macro_rules! try_firefox {
        ($prog:expr, $args:expr, $label:expr) => {{
            // Remove profile lock so Firefox doesn't wait for an existing instance
            let _ = fs::remove_file(format!("{ff_profile}/lock"));
            let _ = fs::remove_file(format!("{ff_profile}/.parentlock"));
            eprintln!("[take_screenshot] trying {}", $label);
            let child = Command::new($prog)
                .args($args)
                .current_dir(&cache_dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn();
            match child {
                Err(e) => { eprintln!("[take_screenshot] spawn failed for {}: {e}", $label); }
                Ok(child) => {
                    match tokio::time::timeout(timeout_dur, child.wait_with_output()).await {
                        Err(_) => {
                            eprintln!("[take_screenshot] {} timed out", $label);
                            last_err = format!("{} timed out", $label);
                        }
                        Ok(Err(_)) => {}
                        Ok(Ok(out)) if out.status.success() => {
                            eprintln!("[take_screenshot] {} exited 0, checking for file at {ff_out_path}", $label);
                            // Firefox writes to ff_out_path (absolute) or fallback "screenshot.png"
                            let found = if fs::metadata(&ff_out_path).is_ok() {
                                fs::read(&ff_out_path).ok().map(|b| { let _ = fs::remove_file(&ff_out_path); b })
                            } else {
                                let cwd_png = format!("{cache_dir}/screenshot.png");
                                eprintln!("[take_screenshot] not at ff_out_path, trying {cwd_png}");
                                fs::read(&cwd_png).ok().map(|b| { let _ = fs::remove_file(&cwd_png); b })
                            };
                            if let Some(bytes) = found {
                                eprintln!("[take_screenshot] success: {} bytes", bytes.len());
                                return (session_id, Ok(bytes));
                            }
                            eprintln!("[take_screenshot] {} ran but no screenshot file found", $label);
                            last_err = format!("{} ran but produced no screenshot file", $label);
                        }
                        Ok(Ok(out)) => {
                            let stderr_line = String::from_utf8_lossy(&out.stderr);
                            eprintln!("[take_screenshot] {} failed: exit={:?} stderr={}", $label, out.status.code(), stderr_line.lines().next().unwrap_or(""));
                            last_err = format!(
                                "{}: exit {:?} — {}",
                                $label,
                                out.status.code(),
                                stderr_line.lines().next().unwrap_or(""),
                            );
                        }
                    }
                }
            }
        }};
    }

    // ── 2. Firefox — host path (Silverblue: /run/host mounts host binaries) ──
    let ff_args_host = ["--headless", "--profile", ff_profile.as_str(),
                        "--screenshot", ff_out_path.as_str(), "--window-size=1280,900", url.as_str()];
    try_firefox!("/run/host/usr/bin/firefox", &ff_args_host, "/run/host/usr/bin/firefox");

    // ── 3. Firefox — direct (in PATH) ─────────────────────────────────────────
    try_firefox!(
        "firefox",
        &["--headless", "--profile", ff_profile.as_str(),
          "--screenshot", ff_out_path.as_str(), "--window-size=1280,900", url.as_str()],
        "firefox"
    );

    // ── 4. Chromium Flatpak ───────────────────────────────────────────────────
    {
        let fs_home = format!("--filesystem={home}");
        let mut args = vec!["run", fs_home.as_str(), "org.chromium.Chromium",
            "--headless=new", "--no-sandbox", "--disable-gpu",
            "--disable-dev-shm-usage", "--hide-scrollbars",
            "--window-size=1280,900",
        ];
        args.push(chrome_screenshot.as_str());
        args.push(url.as_str());
        try_cmd!("flatpak", &args, "flatpak org.chromium.Chromium");
    }

    // ── 5. Firefox Flatpak ────────────────────────────────────────────────────
    {
        let fs_home = format!("--filesystem={home}");
        try_firefox!(
            "flatpak",
            &["run", fs_home.as_str(), "org.mozilla.firefox",
              "--headless", "--profile", ff_profile.as_str(),
              "--screenshot", ff_out_path.as_str(),
              "--window-size=1280,900", url.as_str()],
            "flatpak org.mozilla.firefox"
        );
    }

    // ── 6. Via flatpak-spawn --host (toolbox → host) ──────────────────────────
    for binary in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"] {
        let mut args = vec!["--host", binary,
            "--headless=new", "--no-sandbox", "--disable-gpu",
            "--disable-dev-shm-usage", "--hide-scrollbars", "--window-size=1280,900",
        ];
        args.push(chrome_screenshot.as_str());
        args.push(url.as_str());
        try_cmd!("flatpak-spawn", &args, &format!("flatpak-spawn --host {binary}"));
    }
    try_firefox!(
        "flatpak-spawn",
        &["--host", "firefox", "--headless", "--new-instance",
          "--profile", ff_profile.as_str(),
          "--screenshot", ff_out_path.as_str(),
          "--window-size=1280,900", url.as_str()],
        "flatpak-spawn --host firefox"
    );

    (session_id, Err(last_err))
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use iced::keyboard::key::{Code, Physical};
    use iced::keyboard::{Key, Modifiers};

    /// Physical key for a number-row press — anything that isn't a numpad code.
    const ROW: Physical = Physical::Code(Code::Digit1);

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn number_row_ctrl_digit_selects_session() {
        assert_eq!(session_switch_chord(&ch("1"), Modifiers::CTRL, &ROW), Some(0));
        assert_eq!(session_switch_chord(&ch("9"), Modifiers::CTRL, &ROW), Some(8));
    }

    /// NumLock on: the keypad reports a digit, same as the number row.
    #[test]
    fn numpad_with_numlock_on_selects_session() {
        let physical = Physical::Code(Code::Numpad3);
        assert_eq!(session_switch_chord(&ch("3"), Modifiers::CTRL, &physical), Some(2));
    }

    /// NumLock off: the keypad reports navigation keys, so only the physical
    /// code identifies the digit. This is the case that used to fall through to
    /// the capture gate and get eaten by whatever text widget had focus.
    #[test]
    fn numpad_with_numlock_off_selects_session() {
        for (named, physical, expect) in [
            (key::Named::End,       Code::Numpad1, 0),
            (key::Named::ArrowDown, Code::Numpad2, 1),
            (key::Named::PageUp,    Code::Numpad9, 8),
        ] {
            let got = session_switch_chord(
                &Key::Named(named),
                Modifiers::CTRL,
                &Physical::Code(physical),
            );
            assert_eq!(got, Some(expect), "{named:?} on {physical:?}");
        }
    }

    #[test]
    fn zero_is_not_a_session() {
        assert_eq!(session_switch_chord(&ch("0"), Modifiers::CTRL, &ROW), None);
        let numpad0 = Physical::Code(Code::Numpad0);
        assert_eq!(session_switch_chord(&ch("0"), Modifiers::CTRL, &numpad0), None);
    }

    /// Shift belongs to the Ctrl+Shift+<key> chords, and plain digits are just
    /// text — neither may switch sessions.
    #[test]
    fn needs_ctrl_without_shift() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        assert_eq!(session_switch_chord(&ch("1"), both, &ROW), None);
        assert_eq!(
            session_switch_chord(&Key::Named(key::Named::End), both, &Physical::Code(Code::Numpad1)),
            None,
        );
        assert_eq!(session_switch_chord(&ch("1"), Modifiers::empty(), &ROW), None);
    }

    #[test]
    fn non_digits_are_left_alone() {
        assert_eq!(session_switch_chord(&ch("a"), Modifiers::CTRL, &ROW), None);
        assert_eq!(
            session_switch_chord(&Key::Named(key::Named::End), Modifiers::CTRL, &ROW),
            None,
        );
    }

    // ── Ctrl+Shift+digit: agent pane within the session ──────────────────────

    /// The physical key decides, so the chord works on a layout whose Shift+2
    /// is not "@" — and on one where it is.
    #[test]
    fn shifted_digit_selects_an_agent_pane_by_physical_key() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        for (code, expect) in [(Code::Digit1, 0), (Code::Digit6, 5), (Code::Digit9, 8)] {
            let got = agent_pane_chord(&ch("?"), both, &Physical::Code(code));
            assert_eq!(got, Some(expect), "{code:?}");
        }
    }

    /// Fallback path: no usable physical code, so the US shifted glyph is read.
    #[test]
    fn shifted_symbols_fall_back_to_their_digit() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        let none = Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified);
        for (sym, expect) in [("!", 0), ("@", 1), ("^", 5), ("(", 8)] {
            assert_eq!(agent_pane_chord(&ch(sym), both, &none), Some(expect), "{sym}");
        }
        // A layout that reports the bare digit despite Shift works too.
        assert_eq!(agent_pane_chord(&ch("3"), both, &none), Some(2));
    }

    /// The keypad serves both chords — Shift picks which one.
    #[test]
    fn the_two_digit_chords_never_both_fire() {
        let ctrl = Modifiers::CTRL;
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        let numpad4 = Physical::Code(Code::Numpad4);

        // Ctrl+4 → project session 4, and nothing else.
        assert_eq!(session_switch_chord(&ch("4"), ctrl, &numpad4), Some(3));
        assert_eq!(agent_pane_chord(&ch("4"), ctrl, &numpad4), None);

        // Ctrl+Shift+4 → agent pane 4, and nothing else.
        assert_eq!(agent_pane_chord(&ch("$"), both, &numpad4), Some(3));
        assert_eq!(session_switch_chord(&ch("$"), both, &numpad4), None);
    }

    #[test]
    fn an_agent_pane_chord_needs_both_modifiers() {
        assert_eq!(agent_pane_chord(&ch("!"), Modifiers::SHIFT, &ROW), None);
        assert_eq!(agent_pane_chord(&ch("!"), Modifiers::empty(), &ROW), None);
    }

    /// Zero is not a pane, same as it is not a session.
    #[test]
    fn zero_is_not_an_agent_pane() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        let numpad0 = Physical::Code(Code::Numpad0);
        assert_eq!(agent_pane_chord(&ch("0"), both, &numpad0), None);
        assert_eq!(agent_pane_chord(&ch(")"), both, &numpad0), None);
    }

    /// A keypad key with NumLock off reports itself as a navigation key, and
    /// those are Ctrl+Shift chords in their own right. The pane chord resolves
    /// last so it can't shadow them.
    #[test]
    fn navigation_chords_outrank_a_numlock_off_keypad_digit() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        let numpad2 = Physical::Code(Code::Numpad2);
        assert!(matches!(
            app_chord(&Key::Named(key::Named::ArrowDown), both, &numpad2),
            Some(Message::CycleAttention { forward: true }),
        ));
        // With NumLock on the same key is a digit, and then it is pane 2.
        assert!(matches!(
            app_chord(&ch("@"), both, &numpad2),
            Some(Message::AgentPaneSelected(1)),
        ));
    }

    /// Ctrl+Shift+D opens the Database tool from anywhere, including out of a
    /// focused text field (that's why it lives in `app_chord`).
    #[test]
    fn ctrl_shift_d_opens_the_database_tool() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(matches!(
            app_chord(&ch("d"), both, &ROW),
            Some(Message::CenterTabSelected(crate::session::CenterTab::Database)),
        ));
        // Shift is required — plain Ctrl+D is EOF and belongs to the shell.
        assert!(app_chord(&ch("d"), Modifiers::CTRL, &ROW).is_none());
    }
}

#[cfg(test)]
mod chord_tests {
    use super::*;
    use iced::keyboard::key::{Code, Named, Physical};
    use iced::keyboard::{Key, Modifiers};

    /// Stand-in physical key — every chord here is identified by its logical
    /// key, so the physical code only matters for the numpad (covered in
    /// `key_tests`).
    const PHYS: Physical = Physical::Code(Code::KeyA);

    fn chord(key: Key, modifiers: Modifiers) -> Option<Message> {
        app_chord(&key, modifiers, &PHYS)
    }

    fn named(n: Named, modifiers: Modifiers) -> Option<Message> {
        chord(Key::Named(n), modifiers)
    }

    /// The regression this function exists for: iced's text widgets claim
    /// Ctrl+Shift+arrow as select-by-word and mark it Captured, which used to
    /// kill pane cycling outright whenever the editor or any text field had
    /// focus. `app_chord` runs before that gate, so these must resolve.
    #[test]
    fn ctrl_shift_arrows_cycle_panes_and_attention() {
        let m = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(matches!(named(Named::ArrowRight, m), Some(Message::CyclePane { forward: true })));
        assert!(matches!(named(Named::ArrowLeft,  m), Some(Message::CyclePane { forward: false })));
        assert!(matches!(named(Named::ArrowDown,  m), Some(Message::CycleAttention { forward: true })));
        assert!(matches!(named(Named::ArrowUp,    m), Some(Message::CycleAttention { forward: false })));
    }

    /// Same story for the other keys iced's text widgets swallow: PageUp/Down
    /// (select by page), Enter, and Ctrl+Shift+A (their select-all arm doesn't
    /// guard on Shift).
    #[test]
    fn other_captured_keys_still_reach_the_app() {
        let m = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(matches!(named(Named::PageDown, m), Some(Message::CyclePluginTab { forward: true })));
        assert!(matches!(named(Named::PageUp,   m), Some(Message::CyclePluginTab { forward: false })));
        assert!(matches!(named(Named::Enter,    m), Some(Message::PromptAccepted)));
        assert!(matches!(chord(Key::Character("a".into()), m), Some(Message::CycleAgentPane { forward: true })));
        assert!(matches!(chord(Key::Character("m".into()), m), Some(Message::AgentsDialogOpened)));
    }

    /// The clipboard pair is the deliberate exception: it must NOT resolve
    /// pre-gate (a focused field would get pasted into and the terminal too),
    /// but must still work from `handle_key`, which only runs once the gate
    /// has let the key through.
    #[test]
    fn clipboard_chords_stay_behind_the_capture_gate() {
        let m = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(chord(Key::Character("c".into()), m).is_none());
        assert!(chord(Key::Character("v".into()), m).is_none());

        let copy  = handle_key(Key::Character("c".into()), m, None, PHYS);
        let paste = handle_key(Key::Character("v".into()), m, None, PHYS);
        assert!(matches!(copy,  Some(Message::TerminalCopy)));
        assert!(matches!(paste, Some(Message::TerminalPaste)));
    }

    /// Ctrl+Shift+E hides/shows the file explorer. Like the rest of the panel
    /// toggles it has to resolve pre-gate, since the editor usually has focus
    /// when the user wants the extra width.
    #[test]
    fn ctrl_shift_e_toggles_the_file_explorer() {
        let m = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(matches!(
            chord(Key::Character("e".into()), m),
            Some(Message::FileBrowserToggleCollapsed),
        ));
        // Unshifted Ctrl+E is the markdown-formatting binding — leave it alone.
        assert!(chord(Key::Character("e".into()), Modifiers::CTRL).is_none());
    }

    /// Ctrl+Shift+S / Ctrl+Shift+G jump to the Docs and Notes searches. Both
    /// resolve pre-gate like the other toolbox chords, since the user is
    /// usually typing in the editor or a terminal when they go looking.
    #[test]
    fn search_chords_open_their_toolbox_tabs() {
        let m = Modifiers::CTRL | Modifiers::SHIFT;
        assert!(matches!(chord(Key::Character("s".into()), m), Some(Message::DocsOpen)));
        assert!(matches!(chord(Key::Character("g".into()), m), Some(Message::NotesSearchOpen)));
        // Unshifted Ctrl+S is XOFF and Ctrl+G is BEL — both belong to the shell.
        assert!(chord(Key::Character("s".into()), Modifiers::CTRL).is_none());
        assert!(chord(Key::Character("g".into()), Modifiers::CTRL).is_none());
    }

    #[test]
    fn ctrl_tab_cycles_panes_both_ways() {
        assert!(matches!(named(Named::Tab, Modifiers::CTRL), Some(Message::CyclePane { forward: true })));
        assert!(matches!(
            named(Named::Tab, Modifiers::CTRL | Modifiers::SHIFT),
            Some(Message::CyclePane { forward: false }),
        ));
    }

    /// Everything the app doesn't claim must fall through — plain keys go to
    /// the terminal, and the unshifted Ctrl forms stay with the widget (word
    /// motion) or the shell (Ctrl+C → SIGINT).
    #[test]
    fn unclaimed_keys_fall_through() {
        assert!(named(Named::ArrowRight, Modifiers::empty()).is_none());
        assert!(named(Named::ArrowRight, Modifiers::SHIFT).is_none());
        assert!(named(Named::ArrowRight, Modifiers::CTRL).is_none());
        assert!(named(Named::Enter, Modifiers::empty()).is_none());
        assert!(chord(Key::Character("c".into()), Modifiers::CTRL).is_none());
        assert!(chord(Key::Character("z".into()), Modifiers::CTRL | Modifiers::SHIFT).is_none());
    }
}
