pub mod grid;

use std::io::{Read, Write};
use iced::futures::SinkExt;
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use vte::Parser;

pub use grid::{Grid, COLS, ROWS};

pub type TerminalId = u64;

// ── selection ────────────────────────────────────────────────────────────────

/// Mouse-drag selection over the terminal viewport. Coordinates are in
/// **viewport** row/col space (post-scroll), so when the user scrolls the
/// highlight moves with the viewport — the selected *region* is fixed in
/// pixel terms, the *content* reflects whatever is currently rendered there.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    /// Where the drag started.
    pub anchor: (usize, usize),
    /// Where the cursor is now (or where it was on release).
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

    /// Whether `(row, col)` falls inside the selected region. Selection
    /// wraps full lines between the first and last rows, like ordinary text.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        let ((sr, sc), (er, ec)) = self.range();
        if row < sr || row > er { return false; }
        if sr == er { col >= sc && col <= ec }
        else if row == sr { col >= sc }
        else if row == er { col <= ec }
        else { true }
    }

    /// Extract the selected text from a grid, joining rows with `\n`.
    /// Trailing whitespace on each row is trimmed so spurious padding
    /// doesn't leak into the clipboard.
    pub fn text(&self, grid: &Grid) -> String {
        let ((sr, sc), (er, ec)) = self.range();
        let last_col = grid.cols.saturating_sub(1);
        let mut out = String::new();
        for r in sr..=er {
            if r >= grid.rows { break; }
            let (c0, c1) = if sr == er { (sc, ec) }
                           else if r == sr { (sc, last_col) }
                           else if r == er { (0,  ec) }
                           else            { (0,  last_col) };
            let mut row_text = String::new();
            for c in c0..=c1.min(last_col) {
                row_text.push(grid.view_cell(r, c).c);
            }
            out.push_str(row_text.trim_end());
            if r < er { out.push('\n'); }
        }
        out
    }
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
    /// Active or just-finished mouse selection over the viewport.
    pub selection: Option<Selection>,
    /// True while a Claude question menu is on the visible screen — i.e. the
    /// session has stopped and is waiting on the user. Drives the attention
    /// highlight on the Claude tab and its parent session tab. Re-evaluated
    /// on every PTY data chunk (see `app::update` / `claude_prompt`).
    pub waiting_on_user: bool,
    parser:     Parser,
    writer:     Arc<Mutex<Box<dyn Write + Send>>>,
    /// Master PTY handle, kept so the canvas can resize the kernel pty window
    /// (`TIOCSWINSZ`) to match the visible pane — letting the child reflow its
    /// output to the real width instead of the fixed 80-column default.
    master:     Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Kept alive so the slave PTY end stays open for the child process.
    _slave:     Box<dyn portable_pty::SlavePty + Send>,
    /// Shared with the subscription; wrapped so it outlives the PTY setup.
    pub reader: Arc<Mutex<Box<dyn Read + Send>>>,
}

impl TerminalPane {
    /// Spawn `command` in a new PTY, optionally with a working directory.
    pub fn spawn(
        id:          TerminalId,
        command:     &str,
        working_dir: Option<&std::path::Path>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows: ROWS as u16,
            cols: COLS as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(command);
        // CommandBuilder inherits the parent env verbatim and sets no TERM of
        // its own. When the IDE is launched from a desktop entry (rather than
        // a shell), TERM is unset — children then assume a dumb terminal: no
        // color anywhere, and terminfo users like `clear` fail outright. So
        // advertise the same xterm flavor children saw when the IDE was run
        // from a terminal during development.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(dir) = working_dir {
            cmd.cwd(dir);
        }
        let _child = pair.slave.spawn_command(cmd)?;

        let writer = pair.master.take_writer()?;
        let reader = pair.master.try_clone_reader()?;

        Ok(Self {
            id,
            name:      None,
            grid:      Arc::new(Mutex::new(Grid::new())),
            selection: None,
            waiting_on_user: false,
            parser:    Parser::new(),
            writer:    Arc::new(Mutex::new(writer)),
            master:    Arc::new(Mutex::new(pair.master)),
            _slave:    pair.slave,
            reader:    Arc::new(Mutex::new(reader)),
        })
    }

    /// A clonable handle to the master PTY, handed to the canvas so it can
    /// resize the kernel pty window to the visible pane. See [`resize_pty`].
    pub fn master(&self) -> Arc<Mutex<Box<dyn MasterPty + Send>>> {
        self.master.clone()
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
/// master and emits `on_data(id, bytes)` messages.
pub fn pty_subscription<Message>(
    id: TerminalId,
    reader: Arc<Mutex<Box<dyn Read + Send + 'static>>>,
    on_data: impl Fn(TerminalId, Vec<u8>) -> Message + Send + Sync + 'static,
) -> iced::Subscription<Message>
where
    Message: Send + 'static,
{
    use std::time::Duration;

    iced::Subscription::run_with_id(id, iced::stream::channel(256, move |mut tx| {
        let reader = reader.clone();
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
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        }
    }))
}
