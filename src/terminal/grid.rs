/// VT100 terminal grid — implements vte::Perform so the parser can drive it directly.

use vte::{Params, Perform};

/// Default grid dimensions used before the canvas reports the pane's real
/// size. Both are per-instance (`Grid::cols`/`rows`) once the terminal is
/// laid out — these are only the starting point for a freshly spawned PTY.
pub const COLS: usize = 80;
pub const ROWS: usize = 24;

/// Maximum number of historical rows kept for scrollback.
pub const SCROLLBACK_LIMIT: usize = 5000;

// ── color ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
    pub fn to_iced(self) -> iced::Color {
        iced::Color::from_rgb8(self.r, self.g, self.b)
    }
}

pub const DEFAULT_FG: Rgb = Rgb::new(204, 204, 204);
pub const DEFAULT_BG: Rgb = Rgb::new(28, 28, 28);

// ANSI 16 colours (normal + bright)
const PALETTE: [Rgb; 16] = [
    Rgb::new(0,   0,   0),   // 0  black
    Rgb::new(170, 0,   0),   // 1  red
    Rgb::new(0,   170, 0),   // 2  green
    Rgb::new(170, 170, 0),   // 3  yellow
    // ANSI "blue" is what `ls` paints directories with (SGR 34 / `di=01;34`).
    // The true-blue (0,0,170) is unreadable on the dark background, so we map
    // it to a readable green. This recolors all SGR-blue text, not just dirs —
    // a raw VT stream has no filesystem concept to target more narrowly.
    Rgb::new(110, 210, 120), // 4  blue → readable green
    Rgb::new(170, 0,   170), // 5  magenta
    Rgb::new(0,   170, 170), // 6  cyan
    Rgb::new(170, 170, 170), // 7  white
    Rgb::new(85,  85,  85),  // 8  bright black
    Rgb::new(255, 85,  85),  // 9  bright red
    Rgb::new(85,  255, 85),  // 10 bright green
    Rgb::new(255, 255, 85),  // 11 bright yellow
    Rgb::new(85,  85,  255), // 12 bright blue
    Rgb::new(255, 85,  255), // 13 bright magenta
    Rgb::new(85,  255, 255), // 14 bright cyan
    Rgb::new(255, 255, 255), // 15 bright white
];

fn palette_256(idx: u16) -> Rgb {
    match idx {
        0..=15  => PALETTE[idx as usize],
        16..=231 => {
            let i = idx - 16;
            Rgb::new(
                (i / 36 % 6 * 51) as u8,
                (i / 6  % 6 * 51) as u8,
                (i      % 6 * 51) as u8,
            )
        }
        _ => { let v = ((idx - 232) * 10 + 8) as u8; Rgb::new(v, v, v) }
    }
}

// ── cell ──────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Cell {
    pub c:    char,
    pub fg:   Rgb,
    pub bg:   Rgb,
    pub bold: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self { c: ' ', fg: DEFAULT_FG, bg: DEFAULT_BG, bold: false }
    }
}

// ── grid ──────────────────────────────────────────────────────────────────────

pub struct Grid {
    /// Current live grid dimensions. Updated by [`Grid::resize`] whenever the
    /// canvas reports a new pane size so the program wraps to what fits.
    pub cols:          usize,
    pub rows:          usize,
    pub cells:         Vec<Vec<Cell>>,
    /// Rows that have scrolled off the top, oldest first. Capped at
    /// `SCROLLBACK_LIMIT`. Rendering walks scrollback + `cells` together.
    pub scrollback:    Vec<Vec<Cell>>,
    /// How many rows above the live bottom the user is currently viewing.
    /// 0 = at the bottom (live), 1..=scrollback.len() = looking into history.
    pub scroll_offset: usize,
    pub cursor_row:    usize,
    pub cursor_col:    usize,
    /// Saved cursor position for DECSC/DECRC (`ESC 7` / `ESC 8`) and the
    /// `CSI s` / `CSI u` pair — TUIs stash the cursor before drawing a
    /// transient region and restore it afterward.
    saved_cursor:      (usize, usize),
    /// DECSET 2004 — when the running program has opted in to bracketed
    /// paste mode, pasted text must be wrapped with `\e[200~` / `\e[201~`
    /// so it isn't interpreted as typed input. Toggled by `CSI ?2004 h/l`.
    pub bracketed_paste: bool,
    // Current SGR attributes applied to new characters
    cur_fg:   Rgb,
    cur_bg:   Rgb,
    cur_bold: bool,
}

impl Grid {
    pub fn new() -> Self {
        Self::with_size(COLS, ROWS)
    }

    pub fn with_size(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cells:         vec![vec![Cell::default(); cols]; rows],
            scrollback:    Vec::new(),
            scroll_offset: 0,
            cursor_row:    0,
            cursor_col:    0,
            saved_cursor:  (0, 0),
            bracketed_paste: false,
            cur_fg:        DEFAULT_FG,
            cur_bg:        DEFAULT_BG,
            cur_bold:      false,
        }
    }

    /// Resize the live grid to `cols`×`rows`. Each live row is padded or
    /// truncated to the new width; growing adds blank rows at the bottom while
    /// shrinking pushes the topmost rows into scrollback (and walks the cursor
    /// up with them) so the most recent output stays on screen. Historical
    /// scrollback rows keep their original width — [`Grid::view_cell`] reads
    /// every row defensively, so a width mismatch just renders blanks.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }

        // Width first: every live row matches the new column count.
        for row in &mut self.cells {
            row.resize(cols, Cell::default());
        }

        // Height: spill the top into scrollback when shrinking, pad the bottom
        // when growing.
        if rows < self.cells.len() {
            let excess = self.cells.len() - rows;
            for _ in 0..excess {
                let row = self.cells.remove(0);
                self.scrollback.push(row);
                self.cursor_row = self.cursor_row.saturating_sub(1);
            }
            while self.scrollback.len() > SCROLLBACK_LIMIT {
                self.scrollback.remove(0);
            }
        } else {
            for _ in 0..(rows - self.cells.len()) {
                self.cells.push(vec![Cell::default(); cols]);
            }
        }

        self.cols = cols;
        self.rows = rows;
        self.cursor_row = self.cursor_row.min(rows - 1);
        self.cursor_col = self.cursor_col.min(cols - 1);
        self.scroll_offset = self.scroll_offset.min(self.scrollback.len());
    }

    /// Adjust the user's viewport. Positive `delta` scrolls into history;
    /// negative scrolls back toward the live bottom.
    pub fn scroll_by(&mut self, delta: i32) {
        if delta > 0 {
            self.scroll_offset =
                (self.scroll_offset + delta as usize).min(self.scrollback.len());
        } else if delta < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub((-delta) as usize);
        }
    }

    /// Snap the viewport back to the live bottom — call when the user types
    /// so input doesn't appear to disappear into history.
    pub fn snap_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Cell at viewport row/column, accounting for scrollback offset. Reads
    /// defensively — out-of-range rows/columns (e.g. a narrower historical
    /// scrollback row after a resize) yield a blank default cell rather than
    /// panicking.
    pub fn view_cell(&self, r: usize, c: usize) -> Cell {
        let history = self.scrollback.len();
        // Top of the visible window in virtual-row coordinates.
        let top = history.saturating_sub(self.scroll_offset);
        let virtual_row = top + r;
        let row = if virtual_row < history {
            self.scrollback.get(virtual_row)
        } else {
            self.cells.get(virtual_row - history)
        };
        row.and_then(|row| row.get(c)).cloned().unwrap_or_default()
    }

    /// Where the cursor sits in viewport coordinates. `None` when scrolled
    /// far enough that the cursor's row is below the visible window.
    pub fn cursor_view_pos(&self) -> Option<(usize, usize)> {
        let r = self.scroll_offset + self.cursor_row;
        (r < self.rows).then_some((r, self.cursor_col))
    }

    fn put_char(&mut self, c: char) {
        // Wrap to next line if needed
        if self.cursor_col >= self.cols {
            self.cursor_col = 0;
            self.newline();
        }
        // Scroll if at bottom
        if self.cursor_row >= self.rows {
            self.scroll_up(1);
            self.cursor_row = self.rows - 1;
        }
        self.cells[self.cursor_row][self.cursor_col] = Cell {
            c,
            fg: self.cur_fg,
            bg: self.cur_bg,
            bold: self.cur_bold,
        };
        self.cursor_col += 1;
    }

    fn newline(&mut self) {
        self.cursor_row += 1;
        if self.cursor_row >= self.rows {
            self.scroll_up(1);
            self.cursor_row = self.rows - 1;
        }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            let removed = self.cells.remove(0);
            self.scrollback.push(removed);
            if self.scrollback.len() > SCROLLBACK_LIMIT {
                self.scrollback.remove(0);
            }
            self.cells.push(vec![Cell::default(); self.cols]);
        }
        // If the user is currently looking at history, keep them anchored to
        // the same content rather than yanking them back to the live bottom.
        if self.scroll_offset > 0 {
            self.scroll_offset =
                (self.scroll_offset + n).min(self.scrollback.len());
        }
    }

    fn clear_row(&mut self, row: usize) {
        self.cells[row] = vec![Cell::default(); self.cols];
    }

    /// ICH — insert `n` blank cells at the cursor, shifting the rest of the
    /// line right; cells pushed past the right edge fall off.
    fn insert_blanks(&mut self, n: usize) {
        let row = &mut self.cells[self.cursor_row];
        let at = self.cursor_col.min(row.len());
        let n = n.min(row.len() - at);
        for _ in 0..n {
            row.insert(at, Cell::default());
            row.pop();
        }
    }

    /// DCH — delete `n` cells at the cursor, shifting the rest of the line
    /// left and backfilling the right edge with blanks.
    fn delete_chars(&mut self, n: usize) {
        let row = &mut self.cells[self.cursor_row];
        let at = self.cursor_col.min(row.len());
        let n = n.min(row.len() - at);
        for _ in 0..n {
            row.remove(at);
            row.push(Cell::default());
        }
    }

    /// ECH — erase `n` cells at the cursor in place (no shifting).
    fn erase_chars(&mut self, n: usize) {
        let row = &mut self.cells[self.cursor_row];
        let end = (self.cursor_col + n).min(row.len());
        for c in self.cursor_col..end {
            row[c] = Cell::default();
        }
    }

    /// IL — insert `n` blank lines at the cursor row, scrolling the lines
    /// below it down; lines pushed past the bottom are discarded.
    fn insert_lines(&mut self, n: usize) {
        let top = self.cursor_row;
        let n = n.min(self.rows - top);
        for _ in 0..n {
            self.cells.insert(top, vec![Cell::default(); self.cols]);
            self.cells.remove(self.rows);
        }
    }

    /// DL — delete `n` lines at the cursor row, scrolling the lines below it
    /// up and backfilling the bottom with blanks.
    fn delete_lines(&mut self, n: usize) {
        let top = self.cursor_row;
        let n = n.min(self.rows - top);
        for _ in 0..n {
            self.cells.remove(top);
            self.cells.insert(self.rows - 1, vec![Cell::default(); self.cols]);
        }
    }

    // ── SGR (Select Graphic Rendition) ────────────────────────────────────────
    fn handle_sgr(&mut self, params: &Params) {
        let mut iter = params.iter().peekable();
        loop {
            let Some(sub) = iter.next() else { break };
            let n = sub.first().copied().unwrap_or(0);
            match n {
                0 => {
                    self.cur_fg = DEFAULT_FG;
                    self.cur_bg = DEFAULT_BG;
                    self.cur_bold = false;
                }
                1       => self.cur_bold = true,
                22      => self.cur_bold = false,
                30..=37 => self.cur_fg = PALETTE[(n - 30) as usize],
                38 => {
                    match iter.next().and_then(|p| p.first().copied()) {
                        Some(5) => {
                            if let Some(i) = iter.next().and_then(|p| p.first().copied()) {
                                self.cur_fg = palette_256(i);
                            }
                        }
                        Some(2) => {
                            let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                            let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                            let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                            self.cur_fg = Rgb::new(r as u8, g as u8, b as u8);
                        }
                        _ => {}
                    }
                }
                39      => self.cur_fg = DEFAULT_FG,
                40..=47 => self.cur_bg = PALETTE[(n - 40) as usize],
                48 => {
                    match iter.next().and_then(|p| p.first().copied()) {
                        Some(5) => {
                            if let Some(i) = iter.next().and_then(|p| p.first().copied()) {
                                self.cur_bg = palette_256(i);
                            }
                        }
                        Some(2) => {
                            let r = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                            let g = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                            let b = iter.next().and_then(|p| p.first().copied()).unwrap_or(0);
                            self.cur_bg = Rgb::new(r as u8, g as u8, b as u8);
                        }
                        _ => {}
                    }
                }
                49       => self.cur_bg = DEFAULT_BG,
                90..=97  => self.cur_fg = PALETTE[(n - 90 + 8) as usize],
                100..=107 => self.cur_bg = PALETTE[(n - 100 + 8) as usize],
                _ => {}
            }
        }
    }

    fn p0(params: &Params) -> u16 {
        params.iter().next().and_then(|s| s.first().copied()).unwrap_or(0)
    }
    fn p1(params: &Params, default: u16) -> u16 {
        params.iter().next().and_then(|s| s.first().copied()).unwrap_or(default)
    }
    fn p2(params: &Params, default: u16) -> u16 {
        params.iter().nth(1).and_then(|s| s.first().copied()).unwrap_or(default)
    }
}

// ── vte::Perform ──────────────────────────────────────────────────────────────

impl Perform for Grid {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n'  => self.newline(),
            b'\r'  => self.cursor_col = 0,
            b'\x08' => { if self.cursor_col > 0 { self.cursor_col -= 1; } }
            b'\x07' => {} // bell — ignore
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, c: char) {
        match c {
            // Cursor movement
            'A' => self.cursor_row = self.cursor_row.saturating_sub(Self::p1(params, 1) as usize),
            'B' => self.cursor_row = (self.cursor_row + Self::p1(params, 1) as usize).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + Self::p1(params, 1) as usize).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(Self::p1(params, 1) as usize),
            // Cursor position (1-based)
            'H' | 'f' => {
                self.cursor_row = (Self::p1(params, 1).saturating_sub(1) as usize).min(self.rows - 1);
                self.cursor_col = (Self::p2(params, 1).saturating_sub(1) as usize).min(self.cols - 1);
            }
            // CHA / HPA — cursor to an absolute column (1-based). TUIs use this
            // to position each styled segment instead of emitting the spaces
            // between them; without it, segments collapse together.
            'G' | '`' => self.cursor_col = (Self::p1(params, 1).saturating_sub(1) as usize).min(self.cols - 1),
            // VPA — cursor to an absolute row (1-based).
            'd' => self.cursor_row = (Self::p1(params, 1).saturating_sub(1) as usize).min(self.rows - 1),
            // ICH / DCH / ECH — insert / delete / erase characters in the line.
            '@' => self.insert_blanks(Self::p1(params, 1) as usize),
            'P' => self.delete_chars(Self::p1(params, 1) as usize),
            'X' => self.erase_chars(Self::p1(params, 1) as usize),
            // IL / DL — insert / delete whole lines at the cursor row.
            'L' => self.insert_lines(Self::p1(params, 1) as usize),
            'M' => self.delete_lines(Self::p1(params, 1) as usize),
            // DECSC / DECRC via CSI — save / restore the cursor position.
            's' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            'u' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
            }
            // Erase in display
            'J' => match Self::p0(params) {
                0 => {
                    for c in self.cursor_col..self.cols { self.cells[self.cursor_row][c] = Cell::default(); }
                    for r in (self.cursor_row + 1)..self.rows { self.clear_row(r); }
                }
                1 => {
                    for r in 0..self.cursor_row { self.clear_row(r); }
                    for c in 0..=self.cursor_col { self.cells[self.cursor_row][c] = Cell::default(); }
                }
                2 | 3 => {
                    for r in 0..self.rows { self.clear_row(r); }
                    self.cursor_row = 0;
                    self.cursor_col = 0;
                }
                _ => {}
            },
            // Erase in line
            'K' => match Self::p0(params) {
                0 => for c in self.cursor_col..self.cols { self.cells[self.cursor_row][c] = Cell::default(); },
                1 => for c in 0..=self.cursor_col  { self.cells[self.cursor_row][c] = Cell::default(); },
                2 => self.clear_row(self.cursor_row),
                _ => {}
            },
            // Scroll up / down
            'S' => self.scroll_up(Self::p1(params, 1) as usize),
            'T' => {
                let n = Self::p1(params, 1) as usize;
                for _ in 0..n {
                    self.cells.pop();
                    self.cells.insert(0, vec![Cell::default(); self.cols]);
                }
            }
            // SGR
            'm' => self.handle_sgr(params),
            // DEC private mode set/reset (intermediate '?'). Only 2004
            // (bracketed paste) is tracked; other modes are ignored.
            'h' | 'l' if intermediates.first() == Some(&b'?') => {
                let enable = c == 'h';
                for sub in params.iter() {
                    if sub.first().copied() == Some(2004) {
                        self.bracketed_paste = enable;
                    }
                }
            }
            // Cursor show/hide and other non-private h/l — ignore for now
            'h' | 'l' => {}
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // RIS — full reset, but keep the current dimensions so we don't snap
            // back to the 80×24 default and force a re-resize on the next frame.
            b'c' => *self = Grid::with_size(self.cols, self.rows),
            // DECSC / DECRC — save / restore cursor position.
            b'7' => self.saved_cursor = (self.cursor_row, self.cursor_col),
            b'8' => {
                self.cursor_row = self.saved_cursor.0.min(self.rows - 1);
                self.cursor_col = self.saved_cursor.1.min(self.cols - 1);
            }
            _ => {}
        }
    }

    // Required stubs
    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use vte::Parser;

    /// Feed raw bytes through a real vte parser into a grid and read row 0 back
    /// as a trimmed string.
    fn render(bytes: &[u8]) -> String {
        let mut grid = Grid::with_size(80, 24);
        let mut parser = Parser::new();
        for &b in bytes {
            parser.advance(&mut grid, b);
        }
        let row: String = grid.cells[0].iter().map(|c| c.c).collect();
        row.trim_end().to_string()
    }

    #[test]
    fn cha_positions_segments_so_words_dont_collapse() {
        // A TUI writing "foo bar" by placing each word at an absolute column
        // (CHA) rather than emitting the literal space between them. Column 5
        // is 1-based, so "bar" lands at index 4 → one space gap.
        let out = render(b"foo\x1b[5Gbar");
        assert_eq!(out, "foo bar");
    }

    #[test]
    fn ich_dch_ech_edit_in_place() {
        // ICH: "abc", home, insert 2 blanks → "  abc"
        assert_eq!(render(b"abc\x1b[3D\x1b[2@"), "  abc");
        // DCH: "abcdef", home, delete 3 → "def"
        assert_eq!(render(b"abcdef\x1b[6D\x1b[3P"), "def");
        // ECH: "abcdef", home, erase 3 in place → "   def"
        assert_eq!(render(b"abcdef\x1b[6D\x1b[3X"), "   def");
    }

    #[test]
    fn cursor_save_restore_roundtrips() {
        // Write "ab", save, move away and scribble, restore, overwrite "X".
        // The 'X' must land where the cursor was saved (column 2).
        let out = render(b"ab\x1b7\x1b[10Gzzz\x1b8X");
        assert_eq!(&out[..3], "abX");
    }
}
