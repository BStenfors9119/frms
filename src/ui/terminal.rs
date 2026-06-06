/// Iced canvas widget that renders a terminal Grid and handles mouse focus,
/// scroll, and drag-selection.

use std::sync::{Arc, Mutex};

use iced::widget::canvas::{self, Canvas, Frame, Text};
use iced::{alignment, mouse, Color, Element, Length, Pixels, Point, Rectangle, Size};
use portable_pty::MasterPty;

use crate::app::Message;
use crate::fonts::{MONO_ADVANCE_RATIO, MONO_FONT};
use crate::terminal::{Grid, Selection, TerminalId, COLS, ROWS};
use crate::theme::TerminalFontScale;

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
/// Label of the floating button shown next to a finished selection — clicking
/// it turns the selected text into a new note.
const NOTE_BTN_LABEL: &str = "+ Note";
/// Padding inside the floating note button, in logical px per side.
const NOTE_BTN_PAD: f32 = 6.0;

// ── public view function ──────────────────────────────────────────────────────

pub fn view(
    id:           TerminalId,
    grid:         Arc<Mutex<Grid>>,
    master:       Arc<Mutex<Box<dyn MasterPty + Send>>>,
    focused:      bool,
    is_shell:     bool,
    font_scale:   TerminalFontScale,
    selection:    Option<Selection>,
) -> Element<'static, Message> {
    Canvas::new(TerminalCanvas { id, grid, master, focused, is_shell, font_scale, selection })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
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
            self.ensure_size(cols, rows);
        }
        Layout { cell_w, cell_h, x_offset: 0.0, y_offset: 0.0 }
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
        let Layout { cell_w, cell_h, x_offset, y_offset } = self.layout(bounds);
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

    /// Bounds (canvas-relative) of the floating "+ Note" button, shown only
    /// while a finished, non-empty selection is on screen. Placed just below
    /// the selection's last row, flipping above its first row when the pane
    /// bottom is too close, and clamped horizontally so it never overflows.
    ///
    /// Takes the precomputed `layout` rather than calling [`Self::layout`]
    /// itself: `layout()` locks the grid mutex (via `ensure_size`), and this
    /// is called from `draw()` *while the grid lock is already held* —
    /// re-locking there would deadlock the render thread (the app froze the
    /// first frame after any drag-selection finished).
    fn note_button_rect(&self, bounds: Rectangle, layout: &Layout) -> Option<Rectangle> {
        let sel = self.selection.filter(|s| !s.active && !s.is_empty())?;
        let &Layout { cell_w, cell_h, x_offset, y_offset } = layout;
        let font_size = (cell_h * FONT_RATIO).max(6.0);

        let w = NOTE_BTN_LABEL.chars().count() as f32 * font_size * MONO_ADVANCE_RATIO
            + 2.0 * NOTE_BTN_PAD;
        let h = cell_h + 2.0 * NOTE_BTN_PAD;

        let ((sr, _), (er, ec)) = sel.range();
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
                    if matches!(self.note_button_rect(bounds, &layout), Some(btn) if btn.contains(p)) {
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
                return (Status::Captured, Some(Message::TerminalScroll(self.id, lines)));
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
                if matches!(self.note_button_rect(bounds, &layout), Some(btn) if btn.contains(p)) {
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
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let layout = self.layout(bounds);
        let Layout { cell_w, cell_h, x_offset, y_offset } = layout;
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

        for row in 0..grid.rows {
            let y = y_offset + row as f32 * cell_h;
            if y + cell_h <= 0.0 || y >= bounds.height { continue; }
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

                // Selection highlight — drawn under the glyph so the text
                // stays legible.
                if let Some(sel) = self.selection {
                    if !sel.is_empty() && sel.contains(row, col) {
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
        if let Some(btn) = self.note_button_rect(bounds, &layout) {
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
