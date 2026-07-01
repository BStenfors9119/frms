use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::widget::{column, container, pane_grid, text_editor, text_input};
use iced::widget::pane_grid::Configuration;
use iced::{Element, Length, Subscription, Task, Theme, window};
use iced::keyboard::{self, key};

use crate::agent::{AgentKind, AgentPane, ChatMessage, ChatPane, ChatRole};
use crate::cdp;
use crate::chat_api;
use crate::claude_prompt;
use crate::db::{self, DbClient, DbEngine, DbProvider, ForeignKey, QueryResult, RoutineRef, StatementOutcome, TableRef};
use crate::db_panel::{ColRef, Conj, ConnState, Filter, FilterOp};
use crate::file_browser::{DirEdit, FileBrowserState};
use crate::notes::{NoteFormat, NoteViewMode, NotesState};
use crate::plugin_panel::{PluginSlot, PluginTab};
use crate::prefs::Prefs;
use crate::stats::{self, ClaudeStats, ProcStats, RefreshInterval};
use crate::theme::{self as theme_mod, FontScale, Mode, Palette, TerminalFontScale, ThemeColors};
use crate::pane::PaneKind;
use crate::session::{Session, SessionKind};
use crate::terminal::{self, TerminalId};
use crate::ui;

/// How long the surfaced permission prompt must stay undetected before the
/// response pane is torn down. Long enough to ride over the mid-repaint frame
/// that the pane's own resize provokes (which would otherwise flicker), short
/// enough that the pane disappears promptly once the prompt is truly answered.
const PROMPT_CLEAR_DELAY: Duration = Duration::from_millis(400);

/// Text-input widget id for a chat pane's message box, so the app can move
/// keyboard focus into it when the pane is created or selected.
pub fn chat_input_id(id: TerminalId) -> text_input::Id {
    text_input::Id::new(format!("chat-input-{id}"))
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

/// Full-screen file picker for choosing a local file to copy to a receiver.
/// Carries its own directory browser plus the receiver the chosen file is
/// destined for (captured at open time so it can't drift).
pub struct FilePicker {
    pub browser:  FileBrowserState,
    pub receiver: u64,
}

pub struct Frms {
    pub sessions:                Vec<Session>,
    pub active_session:          usize,
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
    pub agent_menu_open:         bool,
    next_terminal_id:            u64,
    next_session_id:             usize,
    pub claude_stats:            ClaudeStats,
    pub claude_procs:            ProcStats,
    pub stats_refresh_interval:  RefreshInterval,
    // Preferences
    pub prefs:                   Prefs,
    pub showing_prefs:           bool,
    pub prefs_dev_dir_input:     String,
    pub prefs_file_browser:      FileBrowserState,
    /// Whether the `claude` CLI is on `PATH`, shown in the Profile tab's Claude
    /// Code status. Probed once at startup (an external install/sign-in via
    /// `frms-setup-claude` is picked up on the next launch).
    pub claude_installed:        bool,
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
    /// its tab (Build = Claude Code PTY, Research/Chat = native chat).
    AgentAdded(AgentKind),
    /// Toggle the "+ Agent" dropdown menu open/closed.
    AgentMenuToggled,
    /// Edited the draft input of a chat (Research/Chat) pane.
    ChatInputEdited(TerminalId, String),
    /// Submitted the draft of a chat pane — sends the transcript to the API.
    ChatSubmitted(TerminalId),
    /// A streamed text fragment arrived for a chat pane's in-flight reply.
    ChatDelta(TerminalId, String),
    /// A chat pane's reply stream finished — `Ok(session_id)` (to resume on the
    /// next turn) or `Err(message)` on failure.
    ChatEnded(TerminalId, Result<String, String>),
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
    // Stats
    StatsRefresh,
    StatsLoaded(ClaudeStats),
    ProcsLoaded(ProcStats),
    StatsRefreshIntervalChanged(RefreshInterval),
    // Plugin panel (DB / Profile / Notes / Terminals)
    /// A tab was clicked in the given slot — switch that slot to the tab.
    PluginTabClicked(PluginSlot, PluginTab),
    /// Hide a plugin from the tab bar via its per-tab `✕`.
    PluginTabHidden(PluginTab),
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
    // Notes
    NotesNew,
    NotesSelected(u64),
    NotesDelete(u64),
    NotesTitleEdited(String),
    NotesContentAction(text_editor::Action),
    /// Apply a markdown formatting action (toolbar button or Ctrl+B/I/E/K)
    /// to the note body editor.
    NotesFormat(NoteFormat),
    /// Switch the note body between Edit / Split / Preview display.
    NotesViewMode(NoteViewMode),
    /// Send the highlighted selection (or the whole note if nothing is
    /// highlighted) to the active session's active Claude terminal.
    NotesSendToClaude,
    /// Toggle a note between pinned-to-this-project and global.
    NotesPinToggled(u64),
    /// Move keyboard focus from the note Title field into the body editor —
    /// emitted by Tab or Enter while naming.
    NotesFocusBody,
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
    /// Add a receiver under the given container.
    ReceiverAdd(u64),
    ReceiverSelect(u64),
    ReceiverDelete(u64),
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
    /// Open the file picker to choose a local file to copy to the selected receiver.
    ReceiverCopyPick,
    /// Navigate the file picker to a directory.
    ReceiverPickerBrowse(std::path::PathBuf),
    /// A file was chosen in the picker — copy it to the picker's receiver.
    ReceiverPickerChoose(std::path::PathBuf),
    /// Dismiss the file picker without copying.
    ReceiverPickerCancel,
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
    DbConnect,
    DbConnected(usize, Result<(DbClient, Vec<TableRef>, Vec<ForeignKey>, Vec<RoutineRef>), String>),
    DbDisconnect,
    DbTableToggled(String),
    /// Open a stored procedure/function's source for viewing (Objects list).
    DbRoutineSelected(RoutineRef),
    /// The fetched source of the selected routine (or an error).
    DbRoutineLoaded(usize, Result<String, String>),
    /// Dismiss the routine source view and return to query results.
    DbRoutineClosed,
    DbColumnsLoaded(usize, String, Result<Vec<String>, String>),
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
    DbQueryResult(usize, Result<QueryResult, String>),
    DbResetTables,
    /// Switch the builder row between the visual builder and the free-hand SQL
    /// editor (false = builder, true = raw SQL).
    DbSetSqlMode(bool),
    /// Edit the free-hand SQL buffer.
    DbSqlAction(text_editor::Action),
    /// Execute the free-hand SQL verbatim against the connection.
    DbRunSql,
    /// Outcome of a free-hand statement — a result set or an affected-row count.
    DbStatementResult(usize, Result<StatementOutcome, String>),
    /// Filter the Tables/Objects list in Col 1 by name.
    DbObjectFilterChanged(String),
    /// Collapse/expand the Tables and Objects groups in Col 1.
    DbTablesToggleCollapsed,
    DbObjectsToggleCollapsed,
    /// Collapse/expand the builder row to give the results table more room.
    DbRow1ToggleCollapsed,
    /// Copy a single results-table cell value (e.g. an ID) to the clipboard.
    DbCopyValue(String),
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
    /// Profile-tab toggle for anonymous usage telemetry.
    PrefsTelemetryToggled(bool),
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
    state.update(message)
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
            split_view:               None,
            pending_close:            None,
            creating_session:         true,
            new_session_name:         String::new(),
            new_session_dir:          start_dir_str,
            new_session_file_browser: new_session_browser,
            renaming_session:         None,
            renaming_claude:          None,
            agent_menu_open:          false,
            next_terminal_id:         0,
            next_session_id:          1,
            claude_stats:             ClaudeStats::default(),
            claude_procs:             ProcStats::default(),
            stats_refresh_interval:   RefreshInterval::default(),
            prefs,
            showing_prefs:            false,
            prefs_dev_dir_input,
            prefs_file_browser:       prefs_browser,
            claude_installed:         crate::session::claude_on_path(),
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
        // Telemetry only starts after the user has seen the first-run notice —
        // no events (not even launch) are sent before consent. Once acked, it
        // honors the on/off pref. The first post-consent launch is recorded by
        // the notice handler.
        if app.prefs.telemetry_notice_ack {
            app.telemetry = crate::telemetry::Telemetry::start(app.prefs.telemetry);
            app.telemetry.record(crate::telemetry::Event::Launch);
        }
        let init_stats = Task::perform(
            stats::load(),
            |s| Message::StatsLoaded(s.unwrap_or_default()),
        );
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
            );
            // Restored sessions were only written because they were pinned,
            // so keep them pinned (otherwise they'd vanish on next save).
            session.pinned = ps.pinned;
            // `Session::new` always makes pane[0] a Build/Claude PTY. If this
            // session's first tab was actually a chat pane, swap it (killing the
            // just-spawned `claude` child so it doesn't linger orphaned).
            if let Some(k0) = ps.claude_kinds.first().copied() {
                if !k0.is_agentic() {
                    if let Some(t) = session.panes[0].as_terminal() {
                        let _ = t.child.lock().map(|mut c| c.kill());
                    }
                    session.panes[0] = AgentPane::Chat(ChatPane::new(first_term, k0));
                }
            }
            // Recreate any additional tabs the session had open, each with its
            // persisted kind (defaulting to Build for pre-kinds save files).
            for i in 1..ps.claude_count.max(1) {
                let tid = self.next_terminal_id;
                self.next_terminal_id += 1;
                let kind = ps.claude_kinds.get(i).copied().unwrap_or_default();
                session.add_agent(tid, kind);
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
            // Respawn the session's pinned plugin shell terminals as fresh
            // shells, preserving the names the user gave them.
            for name in &ps.plugin_terminals {
                let tid = self.next_terminal_id;
                self.next_terminal_id += 1;
                session.terminals_plugin.create_with_name(
                    tid, &ps.working_dir, name.clone(), true,
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
                plugin_terminals: s.terminals_plugin.terminals.iter()
                    .filter(|t| t.pinned)
                    .map(|t| t.name.clone())
                    .collect(),
                pinned:       s.pinned,
                plugin_panel: Some(s.plugin_panel.clone()),
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
                s.file_browser_collapsed = !s.file_browser_collapsed;
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
                self.sessions[self.active_session].editor.content.perform(action);
            }
            Message::EditorTogglePreview => {
                let s = &mut self.sessions[self.active_session];
                let sid = s.id;
                let e = &mut s.editor;
                e.preview_active = !e.preview_active;
                if e.preview_active && e.preview_kind() == Some(crate::editor::PreviewKind::Html) {
                    if let Some(path) = e.path.clone() {
                        let needs_render = e.preview_image.is_none()
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
                            e.preview_image      = Some(bytes);
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
                        t.waiting_on_user = t.grid.lock()
                            .map(|g| claude_prompt::is_waiting_on_user(&g))
                            .unwrap_or(false);
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
            Message::AgentMenuToggled => {
                self.agent_menu_open = !self.agent_menu_open;
            }
            Message::AgentAdded(kind) => {
                self.agent_menu_open = false;
                let new_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                let s = &mut self.sessions[self.active_session];
                s.add_agent(new_id, kind);
                s.active_terminal = new_id;
                s.center_tab      = crate::session::CenterTab::Claude(new_id);
                self.telemetry.record(crate::telemetry::Event::AgentCreated { kind: kind.as_str() });
                self.save_persisted();
                if !kind.is_agentic() {
                    // Drop the keyboard cursor into the new chat's input box so
                    // the user can start typing immediately.
                    return text_input::focus(chat_input_id(new_id));
                }
            }
            Message::ChatInputEdited(id, value) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.input = value;
                    }
                }
            }
            Message::ChatSubmitted(id) => {
                let Some(s) = self.session_for_terminal_mut(id) else { return Task::none(); };
                let Some(c) = s.chat_mut(id) else { return Task::none(); };
                let text = c.input.trim().to_string();
                if text.is_empty() || c.streaming {
                    return Task::none();
                }
                c.input.clear();
                c.error = None;
                c.pending.clear();
                c.messages.push(ChatMessage { role: ChatRole::User, text: text.clone() });
                c.streaming = true;
                let model  = c.model();
                let resume = c.session_id.clone();
                return Task::run(
                    chat_api::stream_completion(
                        id, model, text, resume,
                        Message::ChatDelta,
                        Message::ChatEnded,
                    ),
                    |m| m,
                );
            }
            Message::ChatDelta(id, chunk) => {
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.pending.push_str(&chunk);
                    }
                }
            }
            Message::ChatEnded(id, result) => {
                // Outcome captured here, recorded after the session borrow ends.
                let mut tel: Option<crate::telemetry::Event> = None;
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(c) = s.chat_mut(id) {
                        c.streaming = false;
                        // Commit whatever text streamed in (a failed request may
                        // still have produced a partial reply worth keeping).
                        if !c.pending.is_empty() {
                            let text = std::mem::take(&mut c.pending);
                            c.messages.push(ChatMessage { role: ChatRole::Assistant, text });
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
                        "\r" | "\n" => self.confirm_pending_close(),
                        "\x1b"      => self.pending_close = None,
                        _           => {}
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
                let delta = lines.round() as i32 * 3;
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(t) = s.terminal_mut(id) {
                        // Wheel "up" yields positive y in iced — scroll into history.
                        if let Ok(mut g) = t.grid.lock() {
                            g.scroll_by(delta);
                        }
                    }
                } else if let Some(s) = self.session_for_plugin_terminal_mut(id) {
                    if let Some(t) = s.terminals_plugin.terminal_mut(id) {
                        if let Ok(mut g) = t.pane.grid.lock() {
                            g.scroll_by(delta);
                        }
                    }
                } else if let Some(t) = self.receivers.ssh_mut(id) {
                    if let Ok(mut g) = t.pane.grid.lock() {
                        g.scroll_by(delta);
                    }
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
                    let s = &mut self.sessions[idx];
                    s.active_terminal          = id;
                    s.center_tab               = crate::session::CenterTab::Claude(id);
                    s.db_panel.focus_active    = false;
                    s.terminals_plugin.focused = false;
                    self.receivers.unfocus_ssh();
                    eprintln!("[copydbg] MouseDown claude id={id} ({row},{col})");
                    if let Some(t) = s.terminal_mut(id) {
                        t.selection = Some(crate::terminal::Selection::new(row, col));
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
                        t.pane.selection = Some(crate::terminal::Selection::new(row, col));
                    }
                } else if self.receivers.has_ssh(id) {
                    // Focus the clicked receiver SSH terminal and begin a
                    // selection, dropping focus from any other SSH session.
                    self.sessions[self.active_session].terminals_plugin.focused = false;
                    self.sessions[self.active_session].db_panel.focus_active    = false;
                    self.receivers.unfocus_ssh();
                    if let Some(t) = self.receivers.ssh_mut(id) {
                        t.focused   = true;
                        t.pane.selection = Some(crate::terminal::Selection::new(row, col));
                    }
                }
            }
            Message::TerminalMouseMove(id, row, col) => {
                eprintln!("[copydbg] MouseMove id={id} ({row},{col})");
                if let Some(s) = self.session_for_terminal_mut(id) {
                    if let Some(t) = s.terminal_mut(id) {
                        if let Some(sel) = t.selection.as_mut() {
                            if sel.active { sel.head = (row, col); }
                        }
                    }
                } else if let Some(s) = self.session_for_plugin_terminal_mut(id) {
                    if let Some(t) = s.terminals_plugin.terminal_mut(id) {
                        if let Some(sel) = t.pane.selection.as_mut() {
                            if sel.active { sel.head = (row, col); }
                        }
                    }
                } else if let Some(t) = self.receivers.ssh_mut(id) {
                    if let Some(sel) = t.pane.selection.as_mut() {
                        if sel.active { sel.head = (row, col); }
                    }
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
                return iced::exit();
            }

            // ── stats ─────────────────────────────────────────────────────────
            Message::StatsRefresh => {
                return Task::batch([
                    Task::perform(
                        stats::load(),
                        |s| Message::StatsLoaded(s.unwrap_or_default()),
                    ),
                    Task::perform(stats::load_procs(), Message::ProcsLoaded),
                ]);
            }
            Message::StatsLoaded(s) => {
                self.claude_stats = s;
            }
            Message::ProcsLoaded(p) => {
                self.claude_procs = p;
            }
            Message::StatsRefreshIntervalChanged(i) => {
                self.stats_refresh_interval = i;
            }

            // ── plugin panel ──────────────────────────────────────────────────
            Message::PluginTabClicked(slot, tab) => {
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
                self.save_persisted();
            }
            Message::PluginTabHidden(tab) => {
                let panel = &mut self.sessions[self.active_session].plugin_panel;
                panel.hide_tab(tab);
                self.save_persisted();
            }
            Message::PluginTabShown(tab) => {
                let panel = &mut self.sessions[self.active_session].plugin_panel;
                panel.show_tab(tab);
                // Surface the just-added plugin and close the picker.
                panel.active_tab = tab;
                panel.adding = false;
                panel.visible = true;
                self.save_persisted();
            }
            Message::PluginAddPickerToggled => {
                let panel = &mut self.sessions[self.active_session].plugin_panel;
                panel.adding = !panel.adding;
            }
            Message::PluginPanelDockToggled => {
                self.sessions[self.active_session].plugin_panel.toggle_dock();
                self.save_persisted();
            }
            Message::PluginPanelSplitToggled => {
                self.sessions[self.active_session].plugin_panel.toggle_split();
                self.save_persisted();
            }
            Message::PluginPanelHide => {
                let s = &mut self.sessions[self.active_session];
                s.plugin_panel.visible = false;
                self.save_persisted();
            }
            Message::PluginPanelToggled => {
                let s = &mut self.sessions[self.active_session];
                s.plugin_panel.visible = !s.plugin_panel.visible;
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

            // ── notes ─────────────────────────────────────────────────────────
            Message::NotesNew => {
                // New notes are pinned to the active session's project; the
                // 🖈 button in the editor header makes them global.
                let project = self.active_project();
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
            Message::TerminalPluginNew => {
                let new_id = self.next_terminal_id;
                self.next_terminal_id += 1;
                let s = &mut self.sessions[self.active_session];
                let dir = s.working_dir.clone();
                s.terminals_plugin.create(new_id, &dir);
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
                let Some(r) = self.receivers.receiver_by_id(id).cloned() else {
                    return Task::none();
                };
                if r.host.trim().is_empty() || r.username.trim().is_empty() {
                    self.receivers.status =
                        Some(Err("set a host and username before connecting".into()));
                    return Task::none();
                }
                let tid = self.next_terminal_id;
                self.next_terminal_id += 1;
                let (prog, args, envs) = r.ssh_command();
                match crate::terminal::TerminalPane::spawn_args(tid, &prog, &args, &envs, None) {
                    Ok(pane) => {
                        // Other terminals lose keyboard focus to the new SSH pane.
                        self.sessions[self.active_session].terminals_plugin.focused = false;
                        // Replaces any prior session to *this* receiver; sessions
                        // to other receivers keep running so their panes survive.
                        self.receivers.set_ssh(crate::receivers::SshTerminal {
                            id: tid,
                            receiver: id,
                            name: r.name.clone(),
                            pane,
                            focused: true,
                            exited: false,
                        });
                        self.receivers.status =
                            Some(Ok(format!("Connected to {}", r.destination())));
                    }
                    Err(e) => {
                        self.receivers.status = Some(Err(format!("ssh failed: {e}")));
                    }
                }
            }
            Message::ReceiverSshDisconnect(receiver) => {
                self.receivers.remove_ssh_for(receiver);
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
                self.file_picker = Some(FilePicker { browser, receiver });
            }
            Message::ReceiverPickerBrowse(path) => {
                if let Some(fp) = self.file_picker.as_mut() {
                    fp.browser.navigate(path);
                }
            }
            Message::ReceiverPickerChoose(path) => {
                let receiver = self.file_picker.as_ref().map(|fp| fp.receiver);
                self.file_picker = None;
                if let Some(receiver) = receiver {
                    return self.copy_files_to(receiver, vec![path]);
                }
            }
            Message::ReceiverPickerCancel => {
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
            Message::DbConnect => {
                let s   = &mut self.sessions[self.active_session];
                let sid = s.id;
                s.db_panel.commit_history();
                s.center_tab = crate::session::CenterTab::Database;
                let cfg = s.db_panel.config.clone();
                s.db_panel.conn_state   = ConnState::Connecting;
                s.db_panel.client       = None;
                s.db_panel.tables.clear();
                s.db_panel.foreign_keys.clear();
                s.db_panel.selected_tables.clear();
                s.db_panel.columns_by_table.clear();
                s.db_panel.selected_cols.clear();
                s.db_panel.joins.clear();
                s.db_panel.dragging_field = None;
                s.db_panel.query_result = None;
                s.db_panel.query_error  = None;
                return Task::perform(
                    db::connect_and_list(cfg),
                    move |r| Message::DbConnected(sid, r),
                );
            }
            Message::DbConnected(session_id, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
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
                let p = &mut self.sessions[self.active_session].db_panel;
                p.client = None;
                p.conn_state = ConnState::Disconnected;
                p.tables.clear();
                p.foreign_keys.clear();
                p.routines.clear();
                p.selected_routine = None;
                p.routine_code     = None;
                p.routine_loading  = false;
                p.selected_tables.clear();
                p.columns_by_table.clear();
                p.selected_cols.clear();
                p.joins.clear();
                p.dragging_field = None;
                p.loading_columns.clear();
                p.query_result = None;
                p.query_error  = None;
                // Reopen the form so the user can reconnect.
                p.form_collapsed = false;
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
                            p.loading_columns.insert(key.clone());
                            return Task::perform(
                                db::list_columns(client, table),
                                move |r| Message::DbColumnsLoaded(sid, key.clone(), r),
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
                p.selected_routine = Some(routine.clone());
                p.routine_code     = None;
                p.routine_loading  = true;
                // Make sure the source view is visible even if the builder had
                // the results row hidden behind a collapsed Row 1.
                return Task::perform(
                    db::routine_definition(client, routine),
                    move |r| Message::DbRoutineLoaded(sid, r),
                );
            }
            Message::DbRoutineLoaded(session_id, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    let p = &mut s.db_panel;
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
            Message::DbColumnsLoaded(session_id, key, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
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
                p.query            = sql.clone();
                p.query_running    = true;
                p.query_error      = None;
                p.statement_status = None;
                return Task::perform(
                    db::run_query(client, sql),
                    move |r| Message::DbQueryResult(sid, r),
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
                p.query_running    = true;
                p.query_error      = None;
                p.statement_status = None;
                return Task::perform(
                    db::run_query(client, sql),
                    move |r| Message::DbQueryResult(sid, r),
                );
            }
            Message::DbQueryResult(session_id, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    s.db_panel.query_running = false;
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
                p.query            = sql.clone();
                p.query_running    = true;
                p.query_error      = None;
                p.statement_status = None;
                return Task::perform(
                    db::run_statement(client, sql),
                    move |r| Message::DbStatementResult(sid, r),
                );
            }
            Message::DbStatementResult(session_id, result) => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == session_id) {
                    let p = &mut s.db_panel;
                    p.query_running = false;
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
            }
            Message::DbRow1ToggleCollapsed => {
                let p = &mut self.sessions[self.active_session].db_panel;
                p.row1_collapsed = !p.row1_collapsed;
            }
            Message::DbCopyValue(value) => {
                return Task::perform(crate::clipboard::write(value), |_| Message::ClipboardWritten);
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
            Message::PrefsTelemetryToggled(on) => {
                self.prefs.telemetry = on;
                self.prefs.save();
                // Apply immediately: start a fresh sender, or drop to a no-op.
                self.telemetry = if on {
                    crate::telemetry::Telemetry::start(true)
                } else {
                    crate::telemetry::Telemetry::disabled()
                };
            }
            Message::NdaChoice(accepted) => {
                if accepted {
                    self.prefs.nda_accepted = true;
                    self.prefs.save();
                } else {
                    // Declined — close the window, which exits the app.
                    return iced::window::get_latest().and_then(iced::window::close);
                }
            }
            Message::TelemetryNoticeChoice(keep_on) => {
                self.prefs.telemetry           = keep_on;
                self.prefs.telemetry_notice_ack = true;
                self.prefs.save();
                // Consent given: start telemetry now (if kept on) and record the
                // first launch — the one we deliberately held back in `new`.
                if keep_on {
                    self.telemetry = crate::telemetry::Telemetry::start(true);
                    self.telemetry.record(crate::telemetry::Event::Launch);
                } else {
                    self.telemetry = crate::telemetry::Telemetry::disabled();
                }
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
            let receiver_name = self.receivers.receiver_by_id(fp.receiver)
                .map(|r| r.name.as_str())
                .unwrap_or("receiver");
            return ui::file_picker::view(&fp.browser, receiver_name, colors);
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
                    PaneKind::FileBrowser => ui::file_browser::view(file_browser),
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
                session.working_dir.to_str(),
                &session.terminals_plugin,
                &self.receivers,
                session.db_panel.client.is_some(),
                &session.db_panel.tables,
                self.claude_installed,
                self.prefs.telemetry,
            );
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

        Subscription::batch(subs)
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
            std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
        });
        let session = Session::new(id, name, kind, first_term, dir);
        self.sessions.push(session);
        self.active_session = self.sessions.len() - 1;
        self.telemetry.record(crate::telemetry::Event::SessionCreated {
            kind: match kind {
                SessionKind::Terminal => "terminal",
                SessionKind::Browser  => "browser",
            },
        });
    }

    fn session_for_terminal_mut(&mut self, id: TerminalId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.panes.iter().any(|p| p.id() == id))
    }

    fn session_for_plugin_terminal_mut(&mut self, id: TerminalId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.terminals_plugin.has_terminal(id))
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
        let Some(text) = text.filter(|t| !t.trim().is_empty()) else { return };

        // The note lands in the active session's Notes tab, so pin it to
        // that session's project.
        let project = self.active_project();
        self.notes.create_from(&text, project);

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
        let detected = s.terminal(source)
            .and_then(|t| t.grid.lock().ok().and_then(|g| claude_prompt::detect(&g)));

        match detected {
            Some(prompt) => {
                // Prompt is on screen — cancel any pending tear-down.
                s.prompt_lost_at = None;
                let fp = prompt.fingerprint();
                // Don't re-surface a prompt the user already answered.
                if s.last_handled_prompt.as_deref() == Some(fp.as_str()) {
                    return;
                }
                if s.pending_prompt.as_ref() != Some(&prompt) {
                    s.pending_prompt         = Some(prompt);
                    s.prompt_source_terminal = Some(source);
                }
            }
            None if s.prompt_source_terminal == Some(source) && s.pending_prompt.is_some() => {
                // The prompt vanished from a surfaced source. This is usually a
                // transient mid-repaint frame caused by the response pane
                // resizing the terminal — tearing the pane down now would
                // resize back and flicker forever. Debounce: only drop the
                // prompt once it's stayed gone past PROMPT_CLEAR_DELAY.
                let now = std::time::Instant::now();
                match s.prompt_lost_at {
                    None => s.prompt_lost_at = Some(now),
                    Some(t0) if now.duration_since(t0) >= PROMPT_CLEAR_DELAY => {
                        s.pending_prompt         = None;
                        s.prompt_source_terminal = None;
                        s.prompt_lost_at         = None;
                        s.last_handled_prompt    = None;
                    }
                    _ => {}
                }
            }
            None => {
                // No prompt and none surfaced for this source — clear stale
                // bookkeeping so the next (different) prompt can re-surface.
                s.prompt_lost_at      = None;
                s.last_handled_prompt = None;
            }
        }
    }

    /// Write a response (e.g. b"1\r" or b"3\r") to the Claude terminal that
    /// triggered the active prompt. The next `TerminalData` from Claude
    /// will clear `last_handled_prompt`.
    fn respond_to_prompt(&mut self, bytes: &[u8]) {
        let s = &mut self.sessions[self.active_session];
        let Some(target) = s.prompt_source_terminal else { return };
        if let Some(prompt) = &s.pending_prompt {
            s.last_handled_prompt = Some(prompt.fingerprint());
        }
        if let Some(t) = s.terminal_mut(target) {
            t.write_input(bytes);
            // Drop the attention highlight immediately — the next PTY data
            // chunk would clear it anyway, but this avoids a stale flash.
            t.waiting_on_user = false;
        }
        s.pending_prompt         = None;
        s.prompt_source_terminal = None;
        s.prompt_lost_at         = None;
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
    // TEMP DEBUG (FRMS_KEYLOG=1): log every key press + its capture status so we
    // can see whether ArrowRight is arriving Captured (eaten by a widget) or
    // Ignored (so the swallow is downstream). Remove once the arrow bug is found.
    if std::env::var("FRMS_KEYLOG").is_ok() {
        if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) = &event {
            eprintln!("[keylog] key={key:?} status={status:?}");
        }
    }
    let is_tab = matches!(
        &event,
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
            ..
        })
    );
    if !is_tab && status == iced::event::Status::Captured {
        return None;
    }
    if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, text, .. }) = event {
        handle_key(key, modifiers, text.map(|t| t.to_string()))
    } else {
        None
    }
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

/// Top-level keyboard handler — returns app messages or terminal input.
fn handle_key(
    key: keyboard::Key,
    modifiers: keyboard::Modifiers,
    text: Option<String>,
) -> Option<Message> {
    if modifiers.control() {
        if let keyboard::Key::Character(s) = &key {
            // Ctrl+Shift+C / Ctrl+Shift+V → terminal copy / paste. The shift
            // disambiguates from the existing Ctrl+C → SIGINT and frees the
            // unshifted Ctrl+V for any future terminal use. Ctrl+Shift+N
            // turns the selection into a new note.
            if modifiers.shift() {
                match s.to_ascii_lowercase().as_str() {
                    "c" => return Some(Message::TerminalCopy),
                    "v" => return Some(Message::TerminalPaste),
                    "n" => return Some(Message::TerminalSendToNote),
                    _   => {}
                }
            }
            // General Ctrl+<key> → ASCII control byte, sent to the focused
            // terminal. Ctrl+A…Ctrl+Z map to 0x01…0x1A and Ctrl+[ \ ] ^ _ @
            // to 0x1B…0x1F / 0x00 — the standard caret-notation control codes —
            // so readline chords (Ctrl+A, Ctrl+E, Ctrl+R, Ctrl+U, Ctrl+W, …)
            // reach the shell, not just the old c/d/z/l subset.
            let ch = s.chars().next()?;
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
    let home = std::env::var("HOME").unwrap_or_else(|_| "/var/tmp".to_string());
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
    for binary in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"] {
        try_cmd!(binary, &chrome_args, binary);
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
