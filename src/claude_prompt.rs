//! Detect Claude Code's interactive confirmation prompts in the terminal grid
//! and surface them as structured `PendingPrompt`s so the UI can render an
//! Accept/Reject pane outside the terminal canvas.
//!
//! Claude's permission prompt looks roughly like:
//!     ╭─ src/foo.rs ─╮
//!     │ 12  old      │   ← rendered with a red background (removed)
//!     │ 12  new      │   ← rendered with a green background (added)
//!     ╰──────────────╯
//!     Do you want to make this edit?
//!     ❯ 1. Yes
//!       2. Yes, and don't ask again this session
//!       3. No, and tell Claude what to do differently
//!
//! Recent Claude Code versions color the diff lines instead of prefixing them
//! with `+`/`-`, so we classify each row by its dominant cell background color
//! (red ⇒ removed/before, green ⇒ added/after) and fall back to `+`/`-` prefix
//! parsing for older output.

use crate::terminal::grid::{Cell, Grid, DEFAULT_BG};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPrompt {
    pub file_path: String,
    pub before:    Vec<String>,
    pub after:     Vec<String>,
}

impl PendingPrompt {
    pub fn fingerprint(&self) -> String {
        let mut s = String::with_capacity(64);
        s.push_str(&self.file_path);
        s.push('\0');
        for l in &self.before { s.push_str(l); s.push('\n'); }
        s.push('\0');
        for l in &self.after  { s.push_str(l); s.push('\n'); }
        s
    }
}

/// Scan the most-recent rows of `grid` for a Claude confirmation prompt.
/// Returns `None` when no prompt is currently on-screen (or it's a non-edit
/// prompt we can't extract structured data from).
pub fn detect(grid: &Grid) -> Option<PendingPrompt> {
    let rows = collect_rows(grid, 60);

    // Anchor on the "❯ 1." selector line that Claude renders for permission
    // menus. Searching from the bottom finds the most-recent prompt.
    let menu_idx = rows.iter().rposition(|r| is_yes_menu_line(&r.text))?;

    // Walk upward collecting diff lines and the first path-like token we see.
    // A row is classified by its background color first (the robust signal);
    // `+`/`-` prefixes are only a fallback for older, uncolored output.
    let mut before: Vec<String> = Vec::new();
    let mut after:  Vec<String> = Vec::new();
    let mut path:   Option<String> = None;

    for r in rows[..menu_idx].iter().rev() {
        let stripped = strip_box_chars(&r.text);

        match r.kind {
            DiffKind::Removed => { before.insert(0, clean_diff_text(&stripped)); continue; }
            DiffKind::Added   => { after.insert(0,  clean_diff_text(&stripped)); continue; }
            DiffKind::Plain   => {}
        }

        if let Some(line) = strip_diff_prefix(&stripped, '+') {
            after.insert(0, line);
            continue;
        }
        if let Some(line) = strip_diff_prefix(&stripped, '-') {
            before.insert(0, line);
            continue;
        }
        if path.is_none() {
            if let Some(p) = extract_path(&stripped) {
                path = Some(p);
            }
        }
    }

    if before.is_empty() && after.is_empty() {
        return None;
    }
    Some(PendingPrompt {
        file_path: path.unwrap_or_else(|| "(file)".to_string()),
        before,
        after,
    })
}

/// True when a numbered selection menu ("❯ 1. …") is on the visible screen —
/// Claude has stopped and is waiting on the user to pick an answer. Broader
/// than [`detect`]: it fires for *any* question menu (plan approval, tool
/// permission, multiple-choice questions), not just edit prompts with
/// extractable diffs. Only the live viewport is scanned (no scrollback) so
/// menus that scrolled into history after being answered don't keep the
/// alert lit.
pub fn is_waiting_on_user(grid: &Grid) -> bool {
    grid.cells.iter().any(|row| {
        let text: String = row.iter().map(|c| c.c).collect();
        is_menu_selector_line(&strip_box_chars(text.trim_end()))
    })
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// "❯ 1." / "❯ 2." … — the focused-option marker of a numbered menu. Requires
/// the digit+dot shape so Claude's plain "❯ " input prompt doesn't match.
fn is_menu_selector_line(r: &str) -> bool {
    let t = r.trim_start();
    let Some(rest) = t.strip_prefix('❯') else { return false };
    let rest = rest.trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest.chars().nth(digits) == Some('.')
}

/// How a single terminal row reads as part of a diff, derived from cell
/// background colors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffKind { Plain, Removed, Added }

/// A collected row: its trimmed text plus the diff classification of its cells.
struct Row {
    text: String,
    kind: DiffKind,
}

fn collect_rows(grid: &Grid, n: usize) -> Vec<Row> {
    let total = grid.scrollback.len() + grid.cells.len();
    let start = total.saturating_sub(n);
    let mut out = Vec::with_capacity(total - start);
    for i in start..total {
        let row = if i < grid.scrollback.len() {
            &grid.scrollback[i]
        } else {
            &grid.cells[i - grid.scrollback.len()]
        };
        let text: String = row.iter().map(|c| c.c).collect();
        out.push(Row {
            text: text.trim_end().to_string(),
            kind: classify(row),
        });
    }
    out
}

/// Classify a row as added / removed / plain from the dominant non-default
/// background color of its cells. Claude tints removed lines red and added
/// lines green; we require a few colored cells so an incidental highlight
/// (e.g. a single selected character) doesn't masquerade as a diff line.
fn classify(row: &[Cell]) -> DiffKind {
    const MIN_CELLS: usize = 3;
    const MARGIN:    i32   = 8; // how much one channel must lead the others
    let mut red   = 0usize;
    let mut green = 0usize;
    for cell in row {
        if cell.bg == DEFAULT_BG {
            continue;
        }
        let (r, g, b) = (cell.bg.r as i32, cell.bg.g as i32, cell.bg.b as i32);
        if r > g + MARGIN && r > b + MARGIN {
            red += 1;
        } else if g > r + MARGIN && g > b + MARGIN {
            green += 1;
        }
    }
    if red >= MIN_CELLS && red >= green {
        DiffKind::Removed
    } else if green >= MIN_CELLS && green > red {
        DiffKind::Added
    } else {
        DiffKind::Plain
    }
}

/// Strip the inline line-number gutter and an optional leading `+`/`-` marker
/// from a color-classified diff line, leaving just the source text.
fn clean_diff_text(line: &str) -> String {
    let t = line.trim_start();
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
    while i < bytes.len() && bytes[i] == b' '          { i += 1; }
    let mut rest = &t[i..];
    if matches!(rest.chars().next(), Some('+') | Some('-')) {
        rest = &rest[1..];
    }
    rest.trim_start().to_string()
}

fn is_yes_menu_line(r: &str) -> bool {
    let t = r.trim_start();
    // Highlighted option: "❯ 1. Yes" — the marker Claude reserves for the
    // currently-focused menu item. Matching just on this avoids false hits
    // on the explanatory "1. Yes" text that scrolls past in chat history.
    t.starts_with("❯ 1.") && t.contains("Yes")
}

fn strip_box_chars(r: &str) -> String {
    r.chars()
        .filter(|c| !matches!(*c,
            '╭' | '╮' | '╰' | '╯' | '│' | '─' |
            '┌' | '┐' | '└' | '┘' | '├' | '┤' |
            '┬' | '┴' | '┼'))
        .collect()
}

/// Strip a leading optional line number then look for `prefix` as the first
/// non-space char. Returns the rest of the line trimmed.
fn strip_diff_prefix(line: &str, prefix: char) -> Option<String> {
    let t = line.trim_start();
    let bytes = t.as_bytes();
    let mut i = 0;
    // Skip optional digits + spaces (Claude shows line numbers inline).
    while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
    while i < bytes.len() && bytes[i] == b' '          { i += 1; }
    let rest = &t[i..];
    let mut chars = rest.chars();
    if chars.next()? != prefix {
        return None;
    }
    // Reject the case where this is part of a longer ASCII prefix like "+++"
    // or "---" which sometimes appear in unified-diff headers.
    if let Some(next) = rest.chars().nth(1) {
        if next == prefix {
            return None;
        }
    }
    Some(chars.as_str().trim_start().to_string())
}

fn extract_path(r: &str) -> Option<String> {
    let t = r.trim();
    if t.is_empty() {
        return None;
    }
    for token in t.split_whitespace() {
        let cleaned: String = token
            .trim_matches(|c: char|
                !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-')
            .to_string();
        if cleaned.contains('/') || has_known_ext(&cleaned) {
            return Some(cleaned);
        }
    }
    None
}

fn has_known_ext(s: &str) -> bool {
    const EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".rb", ".md",
        ".toml", ".json", ".yaml", ".yml", ".sh", ".html", ".css", ".scss",
        ".java", ".kt", ".swift", ".c", ".cc", ".cpp", ".h", ".hpp", ".sql",
    ];
    EXTS.iter().any(|e| s.ends_with(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::grid::Rgb;

    fn row(text: &str, bg: Rgb) -> Vec<Cell> {
        text.chars().map(|c| Cell { c, bg, ..Cell::default() }).collect()
    }

    /// Modern Claude output: diff lines carry no `+`/`-`, only colored
    /// backgrounds. Classification by color must still recover before/after.
    #[test]
    fn detects_color_coded_diff() {
        let red   = Rgb::new(70, 30, 30);
        let green = Rgb::new(30, 60, 30);
        let mut g = Grid::new();
        g.cells = vec![
            row("Edit src/foo.rs",  DEFAULT_BG),
            row("12  let x = 1;",   red),
            row("12  let x = 2;",   green),
            row("❯ 1. Yes",         DEFAULT_BG),
            row("  2. No",          DEFAULT_BG),
        ];

        let p = detect(&g).expect("color-coded prompt should be detected");
        assert_eq!(p.file_path, "src/foo.rs");
        assert_eq!(p.before, vec!["let x = 1;".to_string()]);
        assert_eq!(p.after,  vec!["let x = 2;".to_string()]);
    }

    /// Legacy `+`/`-` prefixed diff (no background color) still parses via the
    /// fallback path.
    #[test]
    fn detects_prefix_diff() {
        let mut g = Grid::new();
        g.cells = vec![
            row("Edit src/foo.rs", DEFAULT_BG),
            row("- let x = 1;",    DEFAULT_BG),
            row("+ let x = 2;",    DEFAULT_BG),
            row("❯ 1. Yes",        DEFAULT_BG),
        ];

        let p = detect(&g).expect("prefixed prompt should be detected");
        assert_eq!(p.before, vec!["let x = 1;".to_string()]);
        assert_eq!(p.after,  vec!["let x = 2;".to_string()]);
    }

    /// No confirmation menu on screen ⇒ no prompt.
    #[test]
    fn no_menu_no_prompt() {
        let mut g = Grid::new();
        g.cells = vec![row("just some output", DEFAULT_BG)];
        assert!(detect(&g).is_none());
    }

    /// Any numbered menu — not just edit prompts — counts as waiting on the
    /// user, including options other than "Yes" and boxed menu lines.
    #[test]
    fn waiting_detected_for_any_numbered_menu() {
        let mut g = Grid::new();
        g.cells = vec![
            row("Which approach should we take?", DEFAULT_BG),
            row("│ ❯ 1. Use a HashMap", DEFAULT_BG),
            row("│   2. Use a Vec",     DEFAULT_BG),
        ];
        assert!(is_waiting_on_user(&g));
    }

    /// Claude's plain "❯ " input prompt must not read as a question menu.
    #[test]
    fn input_prompt_is_not_waiting() {
        let mut g = Grid::new();
        g.cells = vec![
            row("some output",  DEFAULT_BG),
            row("❯ type here",  DEFAULT_BG),
        ];
        assert!(!is_waiting_on_user(&g));
    }
}
