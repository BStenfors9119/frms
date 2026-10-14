/// Iced canvas widget that renders a terminal Grid and handles mouse focus,
/// scroll, and drag-selection.

use std::sync::{Arc, Mutex};

use iced::widget::canvas::{self, Canvas, Frame, Text};
use iced::{alignment, mouse, Color, Element, Length, Pixels, Point, Rectangle, Size};
use portable_pty::MasterPty;

use crate::app::Message;
use crate::fonts::{MONO_ADVANCE_RATIO, MONO_FONT};
use crate::terminal::{FindHighlights, Grid, Selection, TerminalId, COLS, ROWS};
use crate::theme::{from_hsl, to_hsl, TerminalFontScale};

/// Line-box height at `TerminalFontScale::Medium`. The font scale multiplies
/// this; the number of whole cells that fit the pane then *is* the terminal
/// size, so the child always wraps to exactly what's on screen.
const BASE_CELL_H: f32 = 17.0;
/// Font point size as a fraction of the line-box height — the remainder is
/// inter-line spacing. Kept < 1.0 so a glyph's em box always fits inside its
/// row and lines never overlap vertically.
const FONT_RATIO: f32 = 0.80;
/// Floor on cell height so the smallest scale stays legible.
const MIN_CELL_H: f32 = 8.0;
/// Accent for rows the user typed: the active theme's primary lifted most of
/// the way to white. The saturated violet it replaced sat too close in
/// brightness to the CLI's own output to catch the eye, and agent panes always
/// paint on a dark background (see `draw`), so a pale tint reads at a glance
/// whichever palette and mode the profile picked.
pub fn user_accent(theme: &iced::Theme) -> Color {
    lighten(theme_accent(theme), 0.72)
}

/// Accent for rows the agent produced — also drawn from the theme, but pushed
/// across the wheel from [`user_accent`] so the two sides can never be confused
/// for one another. The profile palettes are monochrome (three shades of one
/// hue), so hue alone can't separate the speakers: this takes the theme's hue
/// about halfway round and saturates it, leaving "pale wash = me, colored bar =
/// the agent". A near-neutral palette (Grays) has no hue worth rotating, so
/// there the two separate by brightness alone.
pub fn agent_accent(theme: &iced::Theme) -> Color {
    let (h, s, _) = to_hsl(theme_accent(theme));
    if s < 0.10 {
        from_hsl(h, s, 0.55)
    } else {
        // Saturation stays modest on purpose — the palettes are muted, and a
        // vivid complement would read as an error color rather than a speaker.
        from_hsl((h + 160.0) % 360.0, (s * 1.6).clamp(0.28, 0.45), 0.60)
    }
}

/// The palette color both speaker accents are derived from.
fn theme_accent(theme: &iced::Theme) -> Color {
    theme.extended_palette().primary.base.color
}

/// `t` of the way from `c` to white.
fn lighten(c: Color, t: f32) -> Color {
    Color {
        r: c.r + (1.0 - c.r) * t,
        g: c.g + (1.0 - c.g) * t,
        b: c.b + (1.0 - c.b) * t,
        a: 1.0,
    }
}

/// Opacity of the full-width speaker wash. Deliberately faint: it has to sit
/// under a CLI's own colored output without muddying it. The user's rows get
/// the heavier of the two — a pale wash carries less weight than a saturated
/// one, and the user's own turns are what they scan back for.
const SPEAKER_WASH_ALPHA:      f32 = 0.10;
const USER_SPEAKER_WASH_ALPHA: f32 = 0.16;
/// Width of the solid speaker bar at the left edge, in logical px.
const SPEAKER_BAR_W: f32 = 3.0;

/// Find highlights: every hit washed amber, the current one solid.
const FIND_MATCH:   Color = Color { r: 0.85, g: 0.65, b: 0.20, a: 0.30 };
const FIND_CURRENT: Color = Color { r: 0.95, g: 0.72, b: 0.20, a: 0.60 };

/// Label of the floating button shown next to a finished selection — clicking
/// it turns the selected text into a new note.
const NOTE_BTN_LABEL: &str = "+ Note";
/// Padding inside the floating note button, in logical px per side.
const NOTE_BTN_PAD: f32 = 6.0;

// ── public view function ──────────────────────────────────────────────────────

/// Everything one terminal canvas renders from. A struct rather than a long
/// positional argument list — the pane kinds differ only in a couple of these.
pub struct View {
    pub id:         TerminalId,
    pub grid:       Arc<Mutex<Grid>>,
    pub master:     Arc<Mutex<Box<dyn MasterPty + Send>>>,
    pub focused:    bool,
    pub is_shell:   bool,
    pub font_scale: TerminalFontScale,
    pub selection:  Option<Selection>,
    /// Tint the two sides of the conversation apart. On for Build panes, which
    /// run an AI CLI; off for plain shells and SSH sessions, where the
    /// heuristics in [`crate::transcript`] have nothing to find.
    pub transcript: bool,
    /// Hits to paint for an open find bar.
    pub find:       Option<FindHighlights>,
}

pub fn view(v: View) -> Element<'static, Message> {
    Canvas::new(TerminalCanvas {
        id:         v.id,
        grid:       v.grid,
        master:     v.master,
        focused:    v.focused,
        is_shell:   v.is_shell,
        font_scale: v.font_scale,
        selection:  v.selection,
        transcript: v.transcript,
        find:       v.find,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The accent for one side of the conversation, and the opacity of its wash.
pub fn speaker_accent(speaker: crate::transcript::Speaker, theme: &iced::Theme) -> (Color, f32) {
    match speaker {
        crate::transcript::Speaker::User  => (user_accent(theme),  USER_SPEAKER_WASH_ALPHA),
        crate::transcript::Speaker::Agent => (agent_accent(theme), SPEAKER_WASH_ALPHA),
    }
}

// ── canvas program ────────────────────────────────────────────────────────────

struct TerminalCanvas {
    id:         TerminalId,
    grid:       Arc<Mutex<Grid>>,
    master:     Arc<Mutex<Box<dyn MasterPty + Send>>>,
    focused:    bool,
    is_shell:   bool,
    font_scale: TerminalFontScale,
    selection:  Option<Selection>,
    transcript: bool,
    find:       Option<FindHighlights>,
}

impl TerminalCanvas {
    /// Layout numbers shared by `update()` and `draw()`. Cell size is fixed by
    /// the font scale (not the pane), and the pane size decides how many cells
    /// fit — which we push down to the grid + PTY so output reflows to fit.
    fn layout(&self, bounds: Rectangle) -> Layout {
        let scale  = self.font_scale.factor();
        let cell_h = (BASE_CELL_H * scale).max(MIN_CELL_H);
        // Cell width *is* the font's glyph advance, so each character occupies
        // exactly one cell: no overlap (glyphs merging) and no slack (text
        // floating apart). cell_w == font_size × advance-ratio by construction.
        let cell_w = cell_h * FONT_RATIO * MONO_ADVANCE_RATIO;
        let cols = ((bounds.width  / cell_w).floor() as usize).max(1);
        let rows = ((bounds.height / cell_h).floor() as usize).max(1);
        // Reconcile the grid + kernel pty to the visible size. Because `rows`
        // equals the visible row count, the live cursor row is always on
        // screen, so no vertical offset is needed. Skip degenerate bounds
        // (collapsed/initial layout) so we never flap the pty down to 1×1.
        if bounds.width >= cell_w && bounds.height >= cell_h {
            // Remember this as the size the *next* freshly spawned pane's pty
            // should open at, so its child reads the real dimensions at startup
            // rather than the 80×24 default. See `terminal::spawn_size`.
            crate::terminal::remember_spawn_size(cols, rows);
            self.ensure_size(cols, rows);
        }
        Layout { cell_w, cell_h, x_offset: 0.0, y_offset: 0.0, rows }
    }

    /// Resize the grid and PTY to `cols`×`rows` when (and only when) they
    /// differ from the current size. The grid lock is released before the PTY
    /// resize so the two locks never nest.
    fn ensure_size(&self, cols: usize, rows: usize) {
        let changed = match self.grid.lock() {
            Ok(mut g) if g.cols != cols || g.rows != rows => {
                g.resize(cols, rows);
                true
            }
            _ => false,
        };
        if changed {
            crate::terminal::resize_pty(&self.master, cols, rows);
        }
    }

    /// Project a canvas-relative pixel into the viewport cell it falls on.
    /// Coordinates are clamped to the grid so drags off the edge still snap
    /// to the nearest valid row/column.
    fn pixel_to_cell(&self, p: Point, bounds: Rectangle) -> (usize, usize) {
        let Layout { cell_w, cell_h, x_offset, y_offset, .. } = self.layout(bounds);
        let (rows, cols) = self
            .grid
            .lock()
            .map(|g| (g.rows, g.cols))
            .unwrap_or((ROWS, COLS));
        let col_f = (p.x - x_offset) / cell_w;
        let row_f = (p.y - y_offset) / cell_h;
        let col = col_f.floor().clamp(0.0, (cols.max(1) - 1) as f32) as usize;
        let row = row_f.floor().clamp(0.0, (rows.max(1) - 1) as f32) as usize;
        (row, col)
    }

    /// Absolute row index of the topmost visible row — the offset that turns a
    /// viewport row into the [`Selection`]'s absolute coordinate space.
    fn abs_top(&self) -> usize {
        self.grid.lock().map(|g| g.abs_top()).unwrap_or(0)
    }

    /// Bounds (canvas-relative) of the floating "+ Note" button, shown only
    /// while a finished, non-empty selection is on screen. Placed just below
    /// the selection's last row, flipping above its first row when the pane
    /// bottom is too close, and clamped horizontally so it never overflows.
    /// Hidden entirely once the selection has scrolled out of view.
    ///
    /// Takes the precomputed `layout` and `abs_top` rather than calling
    /// [`Self::layout`] / [`Self::abs_top`] itself: both lock the grid mutex,
    /// and this is called from `draw()` *while the grid lock is already held* —
    /// re-locking there would deadlock the render thread (the app froze the
    /// first frame after any drag-selection finished).
    fn note_button_rect(
        &self,
        bounds:  Rectangle,
        layout:  &Layout,
        abs_top: usize,
    ) -> Option<Rectangle> {
        let sel = self.selection.filter(|s| !s.active && !s.is_empty())?;
        let &Layout { cell_w, cell_h, x_offset, y_offset, .. } = layout;
        let font_size = (cell_h * FONT_RATIO).max(6.0);

        let w = NOTE_BTN_LABEL.chars().count() as f32 * font_size * MONO_ADVANCE_RATIO
            + 2.0 * NOTE_BTN_PAD;
        let h = cell_h + 2.0 * NOTE_BTN_PAD;

        // Absolute rows → viewport rows, clamped to the visible window so a
        // selection that only partly overlaps the viewport still anchors the
        // button to the part the user can see.
        let rows = layout.rows;
        let ((abs_sr, _), (abs_er, ec)) = sel.range();
        if abs_er < abs_top || abs_sr >= abs_top + rows { return None; }
        let sr = abs_sr.saturating_sub(abs_top);
        let er = (abs_er - abs_top).min(rows.saturating_sub(1));

        let mut y = y_offset + (er + 1) as f32 * cell_h + 4.0;
        if y + h > bounds.height {
            y = (y_offset + sr as f32 * cell_h - h - 4.0).max(0.0);
        }
        let x = (x_offset + (ec + 1) as f32 * cell_w + 4.0)
            .min(bounds.width - w - 4.0)
            .max(0.0);
        Some(Rectangle::new(Point::new(x, y), Size::new(w, h)))
    }
}

#[derive(Clone, Copy)]
struct Layout {
    cell_w:   f32,
    cell_h:   f32,
    x_offset: f32,
    y_offset: f32,
    /// Visible row count — also the grid's row count, since the layout pushes
    /// this size down to the grid and PTY.
    rows:     usize,
}

impl canvas::Program<Message> for TerminalCanvas {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use iced::event::Status;
        let inside = cursor.position_in(bounds);
        let drag_active = matches!(self.selection, Some(s) if s.active);

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(p) = inside {
                    // The floating "+ Note" button eats the click before it
                    // can become a new selection drag (which would clear the
                    // very selection the button is about to capture).
                    let layout = self.layout(bounds);
                    let abs_top = self.abs_top();
                    if matches!(self.note_button_rect(bounds, &layout, abs_top), Some(btn) if btn.contains(p)) {
                        return (Status::Captured, Some(Message::TerminalSendToNoteFrom(self.id)));
                    }
                    let (row, col) = self.pixel_to_cell(p, bounds);
                    return (Status::Captured, Some(Message::TerminalMouseDown(self.id, row, col)));
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if drag_active => {
                if let Some(p) = inside {
                    let (row, col) = self.pixel_to_cell(p, bounds);
                    return (Status::Captured, Some(Message::TerminalMouseMove(self.id, row, col)));
                }
                // Dragging past the top or bottom edge scrolls the viewport a
                // row at a time and takes the selection head with it, so the
                // user can reach content that isn't on screen yet.
                if let Some(p) = cursor.position() {
                    let rel = Point::new(p.x - bounds.x, p.y - bounds.y);
                    let lines = if rel.y < 0.0 { 1.0 }
                                else if rel.y > bounds.height { -1.0 }
                                else { 0.0 };
                    if lines != 0.0 {
                        let clamped = Point::new(rel.x, rel.y.clamp(0.0, bounds.height - 1.0));
                        let (row, col) = self.pixel_to_cell(clamped, bounds);
                        return (
                            Status::Captured,
                            Some(Message::TerminalScrollDrag(self.id, lines, row, col)),
                        );
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if drag_active => {
                return (Status::Captured, Some(Message::TerminalMouseUp(self.id)));
            }
            // Right-click is the one-button copy/paste gesture from Linux
            // terminals: copy when there's a highlighted selection, otherwise
            // paste the clipboard. Middle-click always pastes.
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if inside.is_some() =>
            {
                let has_selection = matches!(self.selection, Some(s) if !s.is_empty());
                let msg = if has_selection {
                    Message::TerminalCopyFrom(self.id)
                } else {
                    Message::TerminalPasteInto(self.id)
                };
                return (Status::Captured, Some(msg));
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle))
                if inside.is_some() =>
            {
                return (Status::Captured, Some(Message::TerminalPasteInto(self.id)));
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) if inside.is_some() => {
                let lines = match delta {
                    mouse::ScrollDelta::Lines  { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 16.0,
                };
                // Scrolling mid-drag also drags: the selection head follows the
                // content that scrolls under the (stationary) mouse, so the
                // highlight extends past the visible window instead of being
                // capped at one screenful.
                let msg = match inside.filter(|_| drag_active) {
                    Some(p) => {
                        let (row, col) = self.pixel_to_cell(p, bounds);
                        Message::TerminalScrollDrag(self.id, lines, row, col)
                    }
                    None => Message::TerminalScroll(self.id, lines),
                };
                return (Status::Captured, Some(msg));
            }
            _ => {}
        }
        (Status::Ignored, None)
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match cursor.position_in(bounds) {
            Some(p) => {
                let layout = self.layout(bounds);
                let abs_top = self.abs_top();
                if matches!(self.note_button_rect(bounds, &layout, abs_top), Some(btn) if btn.contains(p)) {
                    mouse::Interaction::Pointer
                } else {
                    mouse::Interaction::Text
                }
            }
            None => mouse::Interaction::default(),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let layout = self.layout(bounds);
        let Layout { cell_w, cell_h, x_offset, y_offset, .. } = layout;
        let font_size = (cell_h * FONT_RATIO).max(6.0);

        let mut frame = Frame::new(renderer, bounds.size());
        let grid = self.grid.lock().unwrap();

        // Dark background
        frame.fill_rectangle(
            Point::ORIGIN,
            bounds.size(),
            Color::from_rgb8(28, 28, 28),
        );

        let selection_color = Color::from_rgba8(80, 130, 220, 0.35);
        // Selections live in absolute row space, so each viewport row is
        // tested against the buffer line it is currently showing. A selection
        // taller than the pane simply draws the slice that's on screen.
        let abs_top = grid.abs_top();

        // Who said what, for the visible rows only — an unrecognised row (and
        // every row of a non-agent pane) simply gets no tint.
        let speakers = if self.transcript {
            crate::transcript::speakers(&grid, abs_top, grid.rows)
        } else {
            Vec::new()
        };

        for row in 0..grid.rows {
            let abs_row = abs_top + row;
            let y = y_offset + row as f32 * cell_h;
            if y + cell_h <= 0.0 || y >= bounds.height { continue; }

            // Speaker band: a full-width wash plus a solid bar at the left
            // edge. The wash is faint enough to sit under the CLI's own colored
            // backgrounds (drawn per-cell below); the bar is what actually
            // reads from across the room.
            if let Some(Some(speaker)) = speakers.get(row) {
                let (accent, wash) = speaker_accent(*speaker, theme);
                frame.fill_rectangle(
                    Point::new(x_offset, y),
                    Size::new(bounds.width - x_offset, cell_h),
                    Color { a: wash, ..accent },
                );
                frame.fill_rectangle(
                    Point::new(x_offset, y),
                    Size::new(SPEAKER_BAR_W, cell_h),
                    accent,
                );
            }

            // Find hits on this row, resolved once so the per-cell test below
            // is a walk over a handful of entries rather than the whole list.
            let row_hits = self.find.as_ref().map(|f| f.on_row(abs_row));

            for col in 0..grid.cols {
                let cell = grid.view_cell(row, col);
                let x = x_offset + col as f32 * cell_w;
                if x + cell_w <= 0.0 || x >= bounds.width { continue; }

                // Per-cell background (only when non-default)
                use crate::terminal::grid::DEFAULT_BG;
                if cell.bg != DEFAULT_BG {
                    frame.fill_rectangle(
                        Point::new(x, y),
                        Size::new(cell_w, cell_h),
                        cell.bg.to_iced(),
                    );
                }

                // Find hit — under the selection so an explicit highlight still
                // wins, but over the cell background so it reads on colored
                // output. The current hit is the strong one.
                if let (Some(hits), Some(find)) = (row_hits.as_ref(), self.find.as_ref()) {
                    if let Some(is_current) = crate::terminal::hit_at(hits, col, find.width) {
                        frame.fill_rectangle(
                            Point::new(x, y),
                            Size::new(cell_w, cell_h),
                            if is_current { FIND_CURRENT } else { FIND_MATCH },
                        );
                    }
                }

                // Selection highlight — drawn under the glyph so the text
                // stays legible.
                if let Some(sel) = self.selection {
                    if !sel.is_empty() && sel.contains(abs_row, col) {
                        frame.fill_rectangle(
                            Point::new(x, y),
                            Size::new(cell_w, cell_h),
                            selection_color,
                        );
                    }
                }

                // Character (skip spaces to save draw calls)
                if cell.c != ' ' {
                    frame.fill_text(Text {
                        content: cell.c.to_string(),
                        position: Point::new(x, y),
                        size: Pixels(font_size),
                        color: cell.fg.to_iced(),
                        font: MONO_FONT,
                        horizontal_alignment: alignment::Horizontal::Left,
                        vertical_alignment: alignment::Vertical::Top,
                        line_height: iced::widget::text::LineHeight::Absolute(Pixels(cell_h)),
                        shaping: iced::widget::text::Shaping::Basic,
                    });
                }
            }
        }

        // Cursor block — sized to the visible glyph (not the whole cell) so a
        // tall cell driven by pane height doesn't produce a giant cursor that
        // dwarfs the text. Top-aligned to match the text rendering above.
        if let Some((cr, cc)) = grid.cursor_view_pos() {
            let cx = x_offset + cc as f32 * cell_w;
            let cy = y_offset + cr as f32 * cell_h;
            let cursor_h = (font_size * 1.1).min(cell_h);
            let cursor_color = if self.focused {
                Color::from_rgba8(200, 200, 200, 0.85)
            } else {
                Color::from_rgba8(120, 120, 120, 0.45)
            };
            frame.fill_rectangle(Point::new(cx, cy), Size::new(cell_w, cursor_h), cursor_color);
        }

        // Floating "+ Note" button next to a finished selection — click to
        // capture the selected text into a new note. Uses the layout computed
        // above — must not call `self.layout()` here, the grid lock is held.
        if let Some(btn) = self.note_button_rect(bounds, &layout, grid.abs_top()) {
            let path = canvas::Path::rounded_rectangle(
                btn.position(),
                btn.size(),
                4.0.into(),
            );
            frame.fill(&path, Color::from_rgb8(45, 55, 75));
            frame.stroke(
                &path,
                canvas::Stroke::default()
                    .with_color(Color::from_rgb8(80, 130, 220))
                    .with_width(1.0),
            );
            frame.fill_text(Text {
                content: NOTE_BTN_LABEL.into(),
                position: Point::new(btn.center_x(), btn.center_y()),
                size: Pixels(font_size),
                color: Color::from_rgb8(210, 220, 240),
                font: MONO_FONT,
                horizontal_alignment: alignment::Horizontal::Center,
                vertical_alignment: alignment::Vertical::Center,
                line_height: iced::widget::text::LineHeight::Absolute(Pixels(cell_h)),
                shaping: iced::widget::text::Shaping::Basic,
            });
        }

        // Tiny "scrolled-up" indicator stripe at the right edge.
        if grid.scroll_offset > 0 {
            let total = grid.scrollback.len() as f32 + grid.rows as f32;
            let visible = grid.rows as f32;
            let bar_h = (bounds.height * (visible / total)).max(20.0);
            let max_off = (grid.scrollback.len() as f32).max(1.0);
            let bar_y = (bounds.height - bar_h)
                * (1.0 - grid.scroll_offset as f32 / max_off);
            frame.fill_rectangle(
                Point::new(bounds.width - 4.0, bar_y),
                Size::new(3.0, bar_h),
                Color::from_rgba8(180, 180, 200, 0.55),
            );
        }

        // Focus border — blue for Claude panes, green for shell pane
        if self.focused {
            let border_color = if self.is_shell {
                Color::from_rgb8(80, 200, 120)
            } else {
                Color::from_rgb8(80, 130, 220)
            };
            frame.stroke(
                &canvas::Path::rectangle(Point::ORIGIN, bounds.size()),
                canvas::Stroke::default()
                    .with_color(border_color)
                    .with_width(2.0),
            );
        }

        vec![frame.into_geometry()]
    }
}
