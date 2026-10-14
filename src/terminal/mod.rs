pub mod grid;

use std::io::{Read, Write};
use iced::futures::SinkExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use vte::Parser;

pub use grid::{Container, Grid, COLS, ROWS};

pub type TerminalId = u64;

/// The most recent on-screen size (cols, rows) a terminal canvas computed from
/// its pane bounds — see [`spawn_size`]. The canvas records it on every draw
/// with real bounds; a freshly spawned PTY then opens at this size instead of
/// the 80×24 default.
///
/// Why it matters: a CLI like Claude reads the pty window size (`TIOCGWINSZ`)
/// the instant it starts and lays its whole UI — logo, input box, cursor —
/// against it. Spawning at 80×24 and only reconciling to the real (usually
/// taller) pane on the first post-spawn `draw()` meant Claude painted that first
/// frame too small and didn't re-lay-out until our resize reached it a few
/// frames later, so the cursor sat pinned to the top, behind the logo, until it
/// settled. Opening the pty at the pane's actual size closes that race: the
/// child reads the right dimensions up front and never paints the cramped frame.
static LAST_COLS: AtomicUsize = AtomicUsize::new(COLS);
static LAST_ROWS: AtomicUsize = AtomicUsize::new(ROWS);

/// Record the on-screen size of a terminal pane. Called by the canvas each time
/// it lays out against real bounds.
pub fn remember_spawn_size(cols: usize, rows: usize) {
    LAST_COLS.store(cols.max(1), Ordering::Relaxed);
    LAST_ROWS.store(rows.max(1), Ordering::Relaxed);
}

/// The size a freshly spawned pty should open at — the last on-screen terminal
/// size, or the 80×24 default before any pane has been drawn.
pub fn spawn_size() -> (usize, usize) {
    (LAST_COLS.load(Ordering::Relaxed), LAST_ROWS.load(Ordering::Relaxed))
}

/// How long a question menu must stay off the screen before the attention
/// flag drops. Long enough to outlast a CLI's repaint (which briefly erases
/// the menu), short enough that answering in the terminal clears the badge
/// while the user is still looking at it. See
/// [`TerminalPane::note_menu_on_screen`].
const ATTENTION_CLEAR_DELAY: std::time::Duration = std::time::Duration::from_millis(600);

/// The state machine behind [`TerminalPane::note_menu_on_screen`], split out so
/// it can be driven with synthetic clocks in tests. Returns the next
/// `(waiting_on_user, menu_gone_since)`.
fn next_attention(
    waiting:    bool,
    gone_since: Option<std::time::Instant>,
    on_screen:  bool,
    now:        std::time::Instant,
) -> (bool, Option<std::time::Instant>) {
    if on_screen {
        return (true, None);
    }
    if !waiting {
        return (false, None);
    }
    let since = gone_since.unwrap_or(now);
    if now.duration_since(since) >= ATTENTION_CLEAR_DELAY {
        (false, None)
    } else {
        (true, Some(since))
    }
}

// ── selection ────────────────────────────────────────────────────────────────

/// Mouse-drag selection over the terminal buffer. Coordinates are in
/// **absolute** row/col space (see [`Grid::abs_top`]), so the highlight stays
/// attached to the text it was drawn over: scrolling moves it with the
/// content, and scrolling mid-drag extends it into scrollback rather than
/// re-selecting whatever has slid under the mouse. The selected region can
/// therefore be taller than the viewport — only the on-screen part is drawn,
/// but the whole of it copies.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    /// Where the drag started, in absolute row/col.
    pub anchor: (usize, usize),
    /// Where the cursor is now (or where it was on release), in absolute row/col.
    pub head:   (usize, usize),
    /// True while the mouse button is still held.
    pub active: bool,
}

impl Selection {
    pub fn new(row: usize, col: usize) -> Self {
        Self { anchor: (row, col), head: (row, col), active: true }
    }

    /// `(start, end)` in row-major order — `start <= end`.
    pub fn range(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head { (self.anchor, self.head) }
        else                        { (self.head, self.anchor) }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Whether absolute `(row, col)` falls inside the selected region.
    /// Selection wraps full lines between the first and last rows, like
    /// ordinary text.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let ((sr, sc), (er, ec)) = self.range();
        if row < sr || row > er { return false; }
        if sr == er { col >= sc && col <= ec }
        else if row == sr { col >= sc }
        else if row == er { col <= ec }
        else { true }
    }

    /// Extract the selected text from a grid, joining rows with `\n`.
    /// Rows outside the buffer (scrolled past the trim limit) are skipped, so
    /// a selection reaching beyond the viewport still copies in full.
    /// Trailing whitespace on each row is trimmed so spurious padding
    /// doesn't leak into the clipboard.
    pub fn text(&self, grid: &Grid) -> String {
        let ((sr, sc), (er, ec)) = self.range();
        let last_col = grid.cols.saturating_sub(1);
        let mut out = String::new();
        for r in sr.max(grid.abs_first())..=er.min(grid.abs_end().saturating_sub(1)) {
            let (c0, c1) = if sr == er { (sc, ec) }
                           else if r == sr { (sc, last_col) }
                           else if r == er { (0,  ec) }
                           else            { (0,  last_col) };
            let mut row_text = String::new();
            for c in c0..=c1.min(last_col) {
                row_text.push(grid.abs_cell(r, c).c);
            }
            out.push_str(row_text.trim_end());
            if r < er { out.push('\n'); }
        }
        out
    }
}

// ── find ─────────────────────────────────────────────────────────────────────

/// How many hits one query keeps. A one-letter query against a full 5000-row
/// scrollback can match tens of thousands of times, and neither the highlight
/// pass nor a "1 of 40000" counter is useful at that point — so the search
/// stops here and the UI says the count is capped.
pub const FIND_HIT_LIMIT: usize = 2000;

/// Rows of context kept above a revealed hit, so the user reads into it rather
/// than landing on it cold.
const FIND_REVEAL_MARGIN: usize = 4;

/// Find-in-conversation state for one pane: the query, where it matches in the
/// buffer, and which of those the viewport is parked on.
#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub query:   String,
    /// Absolute `(row, col)` of every hit, oldest first — capped at
    /// [`FIND_HIT_LIMIT`].
    pub hits:    Vec<(usize, usize)>,
    /// True when the search stopped at the cap and more hits exist.
    pub capped:  bool,
    /// Index into `hits` of the current one.
    pub current: usize,
}

impl FindState {
    /// Match width in cells — the canvas paints this many columns per hit.
    pub fn width(&self) -> usize {
        self.query.chars().count()
    }

    /// The snapshot the canvas needs to paint. Cloned per frame, which is why
    /// the hit list is capped.
    pub fn highlights(&self) -> FindHighlights {
        FindHighlights {
            hits:    self.hits.clone(),
            width:   self.width(),
            current: self.current,
        }
    }
}

/// What the terminal canvas draws for an open find: every hit washed amber,
/// with the current one picked out more strongly.
#[derive(Debug, Clone, Default)]
pub struct FindHighlights {
    pub hits:    Vec<(usize, usize)>,
    pub width:   usize,
    pub current: usize,
}

impl FindHighlights {
    /// Hits that fall on absolute `row`, as `(start column, is current)`.
    /// Called once per visible row per frame — cheap enough that the per-cell
    /// test is then a walk over a handful of entries.
    pub fn on_row(&self, row: usize) -> Vec<(usize, bool)> {
        if self.width == 0 {
            return Vec::new();
        }
        self.hits
            .iter()
            .enumerate()
            .filter(|(_, (r, _))| *r == row)
            .map(|(i, (_, c))| (*c, i == self.current))
            .collect()
    }
}

/// Whether `col` falls inside one of a row's hits, and if so whether that hit
/// is the current one. Pairs with [`FindHighlights::on_row`].
pub fn hit_at(row_hits: &[(usize, bool)], col: usize, width: usize) -> Option<bool> {
    let mut current = false;
    let mut found   = false;
    for (start, is_current) in row_hits {
        if col >= *start && col < start + width {
            found = true;
            current |= *is_current;
        }
    }
    found.then_some(current)
}

// ── TerminalPane ──────────────────────────────────────────────────────────────

/// One terminal instance: owns a PTY, a VT100 grid, and an input writer.
pub struct TerminalPane {
    pub id:     TerminalId,
    /// User-assigned name. `None` means use the caller's positional fallback
    /// (e.g., "Claude 1" / "Terminal 1") so the label still reads sensibly
    /// before the user has renamed anything.
    pub name:   Option<String>,
    /// Shared with the canvas widget for lock-free snapshot reads.
    pub grid:   Arc<Mutex<Grid>>,
    /// Active or just-finished mouse selection over the buffer, in absolute
    /// row/col — so it can span more than the visible viewport.
    pub selection: Option<Selection>,
    /// Open find-in-conversation, or `None` when the find bar is closed.
    pub find: Option<FindState>,
    /// True while a Claude question menu is on the visible screen — i.e. the
    /// session has stopped and is waiting on the user. Drives the attention
    /// highlight on the Claude tab and its parent session tab. Re-evaluated
    /// on every PTY data chunk via [`Self::note_menu_on_screen`], which is the
    /// only thing that should write it.
    pub waiting_on_user: bool,
    /// When the question menu first went missing while `waiting_on_user` was
    /// still set. See [`Self::note_menu_on_screen`].
    menu_gone_since: Option<std::time::Instant>,
    parser:     Parser,
    writer:     Arc<Mutex<Box<dyn Write + Send>>>,
    /// Master PTY handle, kept so the canvas can resize the kernel pty window
    /// (`TIOCSWINSZ`) to match the visible pane — letting the child reflow its
    /// output to the real width instead of the fixed 80-column default.
    master:     Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// The spawned child process. The subscription polls this to learn when
    /// the shell has exited (e.g. the user typed `exit`) so the terminal can
    /// be torn down. Shared so the subscription's poll thread can reach it.
    pub child:  Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    /// Shared with the subscription; wrapped so it outlives the PTY setup.
    pub reader: Arc<Mutex<Box<dyn Read + Send>>>,
    /// For `Build` agent panes, which AI CLI this PTY is running (`claude`,
    /// `devin`, …). `Claude` for plain shells, receivers, and other non-agent
    /// terminals — they never read it, so the default is harmless.
    pub agent_tool: crate::agent::AiTool,
    /// For `Build` agent panes, the model this PTY's CLI was launched with
    /// (drives `--model` and persistence). `Default` for plain shells,
    /// receivers, and other non-agent terminals.
    pub agent_model: crate::agent::ToolModel,
    /// True when this pane really is running `agent_tool`'s CLI, rather than
    /// the install-guidance shell it falls back to when the binary is missing
    /// (see `crate::session::spawn_agent_pane`). A model switch can only be
    /// typed into the CLI itself, so the fallback shell has to respawn instead.
    /// False for plain shells, receivers, and other non-agent terminals.
    pub agent_cli_ready: bool,
}

impl TerminalPane {
    /// Spawn `command` in a new PTY, optionally with a working directory.
    /// Thin wrapper over [`spawn_args`](Self::spawn_args) for arg-less commands;
    /// used by tests (agent panes go through `spawn_args` to pass `--model`).
    #[allow(dead_code)]
    pub fn spawn(
        id:          TerminalId,
        command:     &str,
        working_dir: Option<&std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::spawn_args(id, command, &[], &[], working_dir)
    }

    /// Spawn `command args…` in a new PTY with extra environment variables,
    /// optionally rooted at `working_dir`. Used to launch `sshpass ssh …` for
    /// receiver sessions, where the password rides in `SSHPASS` rather than the
    /// argument list.
    pub fn spawn_args(
        id:          TerminalId,
        command:     &str,
        args:        &[String],
        envs:        &[(String, String)],
        working_dir: Option<&std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pty_system = native_pty_system();

        // Open the pty at the last on-screen pane size, not the 80×24 default,
        // so the child (e.g. Claude) reads realistic dimensions at startup and
        // lays its UI out correctly on the very first frame. See [`spawn_size`].
        let (cols, rows) = spawn_size();
        let pair = pty_system.openpty(PtySize {
            rows: rows as u16,
            cols: cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        // CommandBuilder inherits the parent env verbatim. Layer on the
        // platform's baseline terminal env (TERM/COLORTERM on unix, so children
        // launched from a desktop entry still emit colour; nothing on Windows).
        for (k, v) in crate::port::shell::terminal_env() {
            cmd.env(k, v);
        }
        for (k, v) in envs {
            cmd.env(k, v);
        }
        if let Some(dir) = working_dir {
            cmd.cwd(dir);
        }
        let child = pair.slave.spawn_command(cmd)?;
        // Drop our copy of the slave now that the child owns its own pty fds.
        // Holding it open would keep the master from ever returning EOF/EIO
        // when the child exits, so the subscription could never tell that the
        // shell had closed.
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;

        Ok(Self {
            id,
            name:      None,
            grid:      Arc::new(Mutex::new(Grid::with_size(cols, rows))),
            selection: None,
            find:      None,
            waiting_on_user: false,
            menu_gone_since: None,
            parser:    Parser::new(),
            writer:    Arc::new(Mutex::new(writer)),
            master:    Arc::new(Mutex::new(pair.master)),
            child:     Arc::new(Mutex::new(child)),
            reader:    Arc::new(Mutex::new(reader)),
            agent_tool:  crate::agent::AiTool::default(),
            agent_model: crate::agent::ToolModel::default(),
            agent_cli_ready: false,
        })
    }

    /// A clonable handle to the master PTY, handed to the canvas so it can
    /// resize the kernel pty window to the visible pane. See [`resize_pty`].
    pub fn master(&self) -> Arc<Mutex<Box<dyn MasterPty + Send>>> {
        self.master.clone()
    }

    /// Record whether a question menu is on the visible screen right now, and
    /// update [`Self::waiting_on_user`] from it.
    ///
    /// The rising edge is immediate — an alert the user is waiting on should
    /// never lag. The falling edge is damped, because the menu "disappears"
    /// constantly for reasons that have nothing to do with the user answering
    /// it: the CLI repaints its frame by erasing lines and rewriting them, and
    /// a PTY read can land anywhere in that sequence, so a fraction of chunks
    /// see a half-drawn screen with no menu on it.
    ///
    /// Left undamped, that flapping is not merely a blinking badge. The badge
    /// is hashed into `Frms::agent_strip_signature`, a changed signature drops
    /// the agent-tab strip's scrollbar gutter, and losing the gutter hands the
    /// terminal below an extra row — which resizes the pty, which makes the CLI
    /// repaint, which flaps the badge again. Damping the falling edge breaks
    /// that loop at its source.
    pub fn note_menu_on_screen(&mut self, on_screen: bool) {
        let (waiting, gone_since) = next_attention(
            self.waiting_on_user,
            self.menu_gone_since,
            on_screen,
            std::time::Instant::now(),
        );
        self.waiting_on_user = waiting;
        self.menu_gone_since = gone_since;
    }

    /// Drop the attention flag immediately — the user just answered, so there
    /// is nothing to damp.
    pub fn clear_waiting(&mut self) {
        self.waiting_on_user = false;
        self.menu_gone_since = None;
    }

    /// The working directory the child shell is currently sitting in, if the
    /// platform can read it. `None` once the shell has exited or where the OS
    /// doesn't expose it. Used to persist a pinned terminal's location so it
    /// reopens where the user left it rather than back at the project root.
    /// The container this terminal's shell is sitting in right now — the
    /// innermost toolbox/distrobox it has entered — or `None` on the host.
    /// Tracked by the grid from OSC 777; see [`Grid::osc_dispatch`].
    pub fn container(&self) -> Option<Container> {
        self.grid.lock().ok()?.container().cloned()
    }

    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        let pid = self.child.lock().ok()?.process_id()?;
        crate::port::proc::cwd(pid)
    }

    /// Start a drag-selection at a **viewport** cell, translating it to the
    /// absolute row the cell currently shows.
    pub fn begin_selection(&mut self, row: usize, col: usize) {
        let abs = self.grid.lock().map(|g| g.abs_top() + row).unwrap_or(row);
        self.selection = Some(Selection::new(abs, col));
    }

    /// Move the head of an in-progress drag-selection to a **viewport** cell.
    /// No-op once the drag has finished, so a later scroll can't stretch a
    /// finished selection.
    pub fn drag_selection(&mut self, row: usize, col: usize) {
        if !matches!(self.selection, Some(s) if s.active) { return; }
        let abs = self.grid.lock().map(|g| g.abs_top() + row).unwrap_or(row);
        if let Some(sel) = self.selection.as_mut() {
            sel.head = (abs, col);
        }
    }

    /// Scroll the viewport by `delta` rows, extending an in-progress
    /// drag-selection to whatever now sits under the mouse (`row`/`col` in
    /// viewport space). This is what lets a selection grow past the visible
    /// window: hold the button, wheel up, and the highlight keeps reaching
    /// back into scrollback.
    pub fn scroll_drag(&mut self, delta: i32, row: usize, col: usize) {
        if let Ok(mut g) = self.grid.lock() {
            g.scroll_by(delta);
        }
        self.drag_selection(row, col);
    }

    /// Open the find bar (keeping any previous query, so reopening resumes the
    /// last search) and refresh its hits against the buffer as it stands now.
    pub fn find_open(&mut self) {
        let mut find = self.find.take().unwrap_or_default();
        self.run_find(&mut find, true);
        self.find = Some(find);
    }

    /// Close the find bar, dropping its highlights.
    pub fn find_close(&mut self) {
        self.find = None;
    }

    /// Re-run the search for a new query, jumping to the last (most recent)
    /// hit — searching a conversation usually means "where did that just come
    /// up", and the newest hit is the best first answer.
    pub fn find_query(&mut self, query: String) {
        let mut find = self.find.take().unwrap_or_default();
        find.query = query;
        self.run_find(&mut find, true);
        self.find = Some(find);
    }

    /// Step to the next / previous hit, wrapping at either end, and scroll it
    /// into view.
    pub fn find_step(&mut self, forward: bool) {
        let Some(mut find) = self.find.take() else { return };
        // Refresh first: the agent has probably said more since the query was
        // typed, and stepping is where the user asks "and after that?". Hold on
        // to the hit we were parked on so the step continues from *it* — rows
        // trimmed off the top of the scrollback shift every index down.
        let anchor = find.hits.get(find.current).copied();
        self.run_find(&mut find, false);
        let n = find.hits.len();
        if n > 0 {
            let at = anchor
                .and_then(|h| find.hits.iter().position(|x| *x == h))
                .unwrap_or(n - 1);
            find.current = if forward { (at + 1) % n } else { (at + n - 1) % n };
            self.reveal_hit(&find);
        }
        self.find = Some(find);
    }

    /// Recompute `find.hits` against the current buffer. `jump` parks the
    /// viewport on the newest hit; without it the caller keeps its own idea of
    /// which hit is current (clamped, since the buffer may have moved on).
    fn run_find(&mut self, find: &mut FindState, jump: bool) {
        let Ok(grid) = self.grid.lock() else { return };
        find.hits = grid.search(&find.query);
        find.capped = find.hits.len() > FIND_HIT_LIMIT;
        find.hits.truncate(FIND_HIT_LIMIT);
        find.current = if jump {
            find.hits.len().saturating_sub(1)
        } else {
            find.current.min(find.hits.len().saturating_sub(1))
        };
        drop(grid);
        self.reveal_hit(find);
    }

    /// Scroll the viewport to the current hit, if there is one.
    fn reveal_hit(&mut self, find: &FindState) {
        let Some((row, _)) = find.hits.get(find.current) else { return };
        if let Ok(mut g) = self.grid.lock() {
            g.scroll_to_abs(*row, FIND_REVEAL_MARGIN);
        }
    }

    /// Feed raw PTY output bytes through the VT100 parser into the grid.
    /// Called from `app::update()` on `Message::TerminalData`.
    pub fn process(&mut self, bytes: &[u8]) {
        let mut grid = self.grid.lock().unwrap();
        for &b in bytes {
            self.parser.advance(&mut *grid, b);
        }
    }

    /// Write raw bytes to the PTY (keyboard input).
    pub fn write_input(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
        }
    }
}

/// Resize the kernel pty window for `master` to `cols`×`rows`. Cheap and
/// idempotent (a `TIOCSWINSZ` ioctl); the canvas calls it only when the
/// computed terminal size actually changes.
pub fn resize_pty(master: &Arc<Mutex<Box<dyn MasterPty + Send>>>, cols: usize, rows: usize) {
    if let Ok(m) = master.lock() {
        let _ = m.resize(PtySize {
            rows:         rows.max(1) as u16,
            cols:         cols.max(1) as u16,
            pixel_width:  0,
            pixel_height: 0,
        });
    }
}

// ── subscription ──────────────────────────────────────────────────────────────

/// Returns an iced Subscription that continuously reads bytes from the PTY
/// master and emits `on_data(id, bytes)` messages. When the child process
/// exits (e.g. the user typed `exit`), it emits `on_exit(id)` once and ends.
pub fn pty_subscription<Message>(
    id: TerminalId,
    reader: Arc<Mutex<Box<dyn Read + Send + 'static>>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync + 'static>>>,
    on_data: impl Fn(TerminalId, Vec<u8>) -> Message + Send + Sync + 'static,
    on_exit: impl Fn(TerminalId) -> Message + Send + Sync + 'static,
) -> iced::Subscription<Message>
where
    Message: Send + 'static,
{
    use std::time::Duration;

    iced::Subscription::run_with_id(id, iced::stream::channel(256, move |mut tx| {
        let reader = reader.clone();
        let child  = child.clone();
        async move {
            loop {
                // Drive blocking reads off the async executor thread.
                let reader = reader.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let mut r = reader.lock().unwrap();
                    let mut buf = vec![0u8; 4096];
                    match r.read(&mut buf) {
                        Ok(0) | Err(_) => None,
                        Ok(n) => { buf.truncate(n); Some(buf) }
                    }
                }).await;

                match result {
                    Ok(Some(bytes)) => {
                        let _ = tx.send(on_data(id, bytes)).await;
                    }
                    _ => {
                        // No data — either a transient read hiccup or the child
                        // has gone. Confirm via the child handle: once it has
                        // exited, tell the app to tear this terminal down and
                        // stop the read loop.
                        let child = child.clone();
                        let exited = tokio::task::spawn_blocking(move || {
                            child.lock().ok()
                                .and_then(|mut c| c.try_wait().ok().flatten())
                                .is_some()
                        }).await.unwrap_or(false);
                        if exited {
                            let _ = tx.send(on_exit(id)).await;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use vte::Parser;

    /// Five lines through a 3-row grid: two rows in scrollback, three live.
    fn scrolled_grid() -> Grid {
        let mut grid = Grid::with_size(10, 3);
        let mut parser = Parser::new();
        for &b in b"aaa\r\nbbb\r\nccc\r\nddd\r\neee" {
            parser.advance(&mut grid, b);
        }
        grid
    }

    /// An alert the user is waiting on shows the instant the menu appears.
    #[test]
    fn attention_rises_immediately() {
        let now = Instant::now();
        assert_eq!(next_attention(false, None, true, now), (true, None));
    }

    /// The CLI erases its menu on every repaint, so a chunk that lands mid-frame
    /// reads "no menu" while the question is very much still up. One such frame
    /// must not clear the badge — that flap is what drove the tab strip's
    /// gutter on and off and resized the pty underneath it.
    #[test]
    fn attention_survives_a_mid_repaint_frame() {
        let now = Instant::now();
        let (waiting, gone) = next_attention(true, None, false, now);
        assert!(waiting, "one menu-less frame must not clear the badge");
        assert_eq!(gone, Some(now), "but it starts the clock");

        // A few more frames arrive over the next handful of milliseconds.
        let later = now + Duration::from_millis(50);
        assert_eq!(next_attention(true, gone, false, later), (true, Some(now)));

        // …and then the menu is back, so the clock resets.
        assert_eq!(next_attention(true, gone, true, later), (true, None));
    }

    /// Once the menu has genuinely stayed gone, the badge drops.
    #[test]
    fn attention_clears_after_the_delay() {
        let now  = Instant::now();
        let gone = Some(now - ATTENTION_CLEAR_DELAY);
        assert_eq!(next_attention(true, gone, false, now), (false, None));
    }

    /// Nothing to damp when the badge was never lit.
    #[test]
    fn attention_stays_off_without_a_menu() {
        let now = Instant::now();
        assert_eq!(next_attention(false, None, false, now), (false, None));
    }

    #[test]
    fn selection_reaching_past_the_viewport_copies_in_full() {
        let grid = scrolled_grid();
        // Absolute rows 0..=4 — the first two of which have scrolled off the
        // 3-row viewport entirely.
        let sel = Selection { anchor: (0, 0), head: (4, 2), active: false };
        assert_eq!(sel.text(&grid), "aaa\nbbb\nccc\nddd\neee");
    }

    #[test]
    fn selection_stays_on_its_text_while_the_viewport_scrolls() {
        let mut grid = scrolled_grid();
        // "aaa" — the oldest line, off screen at the live bottom.
        let sel = Selection { anchor: (0, 0), head: (0, 2), active: false };
        assert_eq!(sel.text(&grid), "aaa");
        assert!(!sel.contains(grid.abs_top(), 0));
        // Scroll it back into view: same selection, now on viewport row 0.
        grid.scroll_by(2);
        assert_eq!(sel.text(&grid), "aaa");
        assert!(sel.contains(grid.abs_top(), 0));
    }

    #[test]
    fn selection_survives_rows_falling_out_of_scrollback() {
        let mut grid = scrolled_grid();
        // Simulate the oldest two rows being trimmed at SCROLLBACK_LIMIT.
        grid.scrollback.remove(0);
        grid.scrollback.remove(0);
        grid.trimmed = 2;
        // Rows 0..=1 are gone; the rest of the selection still copies.
        let sel = Selection { anchor: (0, 0), head: (4, 2), active: false };
        assert_eq!(sel.text(&grid), "ccc\nddd\neee");
    }
}
