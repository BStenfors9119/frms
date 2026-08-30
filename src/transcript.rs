//! Who said what inside an agent PTY pane.
//!
//! A Build pane runs an AI CLI (`claude`, `devin`) whose conversation is plain
//! terminal text: the user's prompts, the agent's replies and its tool traces
//! all land in the same grid with no structure behind them. This module reads
//! the *shape* of the rendered lines to tell the two apart, so the canvas can
//! tint each side differently:
//!
//! ```text
//! > run the tests and fix what breaks     ← user prompt (marker at column 0)
//!   and don't touch the snapshots         ← indented continuation
//!
//! ⏺ Running the suite now.                ← agent reply
//!
//! ⏺ Bash(cargo test)                      ← agent tool call
//!   ⎿  94 passed                          ← and its result
//!
//! ╭──────────────────────────────╮        ← the live input box: neither
//! │ >                            │
//! ╰──────────────────────────────╯
//! ```
//!
//! It is a heuristic and says so: anything it can't place stays untinted,
//! which is the same as the pane looked before. A CLI whose output doesn't
//! follow this shape simply gets no tint at all.

use crate::terminal::Grid;

/// Which side of the conversation a row belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speaker {
    User,
    Agent,
}

/// Rows scanned above the requested range to work out which block its first
/// row is already inside — a long reply can start well above the viewport.
const LOOKBACK: usize = 200;

/// Markers the CLIs put at the head of an agent line: `⏺` for messages and
/// tool calls, `●` in older builds, `⎿` for a tool result.
const AGENT_MARKERS: [char; 3] = ['\u{23FA}', '\u{25CF}', '\u{23BF}'];

/// Box-drawing characters. A row starting with one is chrome — the input box,
/// a diff frame — and belongs to neither speaker.
fn is_box_char(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{257F}')
}

/// Classify `count` absolute rows starting at `first`. The result is exactly
/// `count` long; `None` means "not attributable" (blank rows, chrome, shell
/// output, anything unrecognised).
pub fn speakers(grid: &Grid, first: usize, count: usize) -> Vec<Option<Speaker>> {
    let floor = first.saturating_sub(LOOKBACK).max(grid.abs_first());
    // Walk back only as far as the head of the block `first` sits in — every
    // row above that is irrelevant to it. Usually a row or two; the LOOKBACK
    // floor caps the pathological case (a very long unbroken reply), where the
    // worst outcome is the tail of that block going untinted.
    let mut start = first;
    while start > floor && continues_a_block(&grid.abs_row_text(start - 1)) {
        start -= 1;
    }
    if start > floor {
        start -= 1; // …and include the row that began it.
    }
    let end = (first + count).min(grid.abs_end());

    let mut cur: Option<Speaker> = None;
    let mut out = Vec::with_capacity(count);
    for abs in start..end {
        cur = advance(cur, &grid.abs_row_text(abs));
        if abs >= first {
            out.push(cur);
        }
    }
    // Rows past the end of the buffer (a viewport taller than the content).
    out.resize(count, None);
    out
}

/// Whether a row can only be a continuation of the one above it — indented,
/// non-blank text. Anything else either starts a block or ends one, so the
/// backward walk in [`speakers`] can stop there.
fn continues_a_block(line: &str) -> bool {
    line.starts_with("  ") && !line.trim().is_empty()
}

/// Fold one row into the running block state: which speaker, if any, the row
/// belongs to — and therefore which one the *next* row continues.
fn advance(cur: Option<Speaker>, line: &str) -> Option<Speaker> {
    let trimmed = line.trim_start();
    let Some(head) = trimmed.chars().next() else {
        return None; // blank row ends the block
    };
    if is_box_char(head) {
        return None;
    }
    // A prompt marker only counts at the left edge; indented `>` is quoted
    // text inside somebody's message, not a new turn.
    if head == '>' && !line.starts_with(' ') {
        return Some(Speaker::User);
    }
    if AGENT_MARKERS.contains(&head) {
        return Some(Speaker::Agent);
    }
    // Indented text continues whatever block it's inside; a fresh unindented
    // line that isn't a marker (shell output, a banner) ends it.
    if line.starts_with("  ") { cur } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Grid;
    use vte::Parser;

    fn grid_of(lines: &[&str]) -> Grid {
        let mut grid = Grid::with_size(60, lines.len().max(1));
        let mut parser = Parser::new();
        for (i, l) in lines.iter().enumerate() {
            for &b in l.as_bytes() {
                parser.advance(&mut grid, b);
            }
            if i + 1 < lines.len() {
                for &b in b"\r\n" {
                    parser.advance(&mut grid, b);
                }
            }
        }
        grid
    }

    #[test]
    fn prompts_replies_and_chrome_are_told_apart() {
        let grid = grid_of(&[
            "> run the tests",
            "  and fix what breaks",
            "",
            "\u{23FA} Running them now.",
            "  94 passed",
            "\u{2570}\u{2500}\u{2500}\u{256F}",
        ]);
        let got = speakers(&grid, grid.abs_first(), 6);
        assert_eq!(got, vec![
            Some(Speaker::User),
            Some(Speaker::User),
            None,
            Some(Speaker::Agent),
            Some(Speaker::Agent),
            None,
        ]);
    }

    #[test]
    fn a_block_starting_above_the_range_still_colours_its_tail() {
        let grid = grid_of(&[
            "\u{23FA} A long reply",
            "  that keeps going",
            "  and going",
        ]);
        // Ask only about the last row: the lookback has to find the marker.
        let got = speakers(&grid, grid.abs_first() + 2, 1);
        assert_eq!(got, vec![Some(Speaker::Agent)]);
    }

    #[test]
    fn plain_shell_output_stays_untinted() {
        let grid = grid_of(&["$ ls", "Cargo.toml  src", ""]);
        assert_eq!(speakers(&grid, grid.abs_first(), 3), vec![None, None, None]);
    }
}
