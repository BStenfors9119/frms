//! User notes persisted at `$HOME/.frms/notes.json`.
//!
//! Notes are edited inside the Notes plugin tab. Each note is either scoped
//! to a project (the session's working directory) or global — global notes
//! show up in every project. Persistence mirrors `prefs.rs`: hand-rolled
//! JSON so unknown fields round-trip and a corrupt file falls back to
//! defaults.

use std::path::PathBuf;

use iced::widget::text_editor;
use serde_json::{json, Value};

/// Markdown formatting actions exposed by the notes toolbar (and keyboard
/// shortcuts). Inline variants wrap the selection in marker pairs; line
/// variants prefix the current line (or every line of a multi-line selection).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteFormat {
    Bold,
    Italic,
    Strikethrough,
    Code,
    H1,
    H2,
    H3,
    Bullet,
    Numbered,
    Quote,
    CodeBlock,
    Link,
}

/// How the selected note's body is displayed: raw markdown, rendered
/// preview, or both side by side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoteViewMode {
    #[default]
    Edit,
    Split,
    Preview,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id:    u64,
    pub title: String,
    pub body:  String,
    /// Project this note is pinned to — the session's working directory.
    /// `None` means global: the note shows up in every project.
    pub project: Option<String>,
}

impl Note {
    /// Whether this note should appear when `project` is the active
    /// session's working directory. Global notes appear everywhere.
    pub fn visible_in(&self, project: Option<&str>) -> bool {
        match self.project.as_deref() {
            None    => true,
            Some(p) => Some(p) == project,
        }
    }
}

pub struct NotesState {
    pub notes:           Vec<Note>,
    /// Currently-edited note id; `None` when no note is selected.
    pub selected:        Option<u64>,
    /// Editable title for the selected note (mirrors `notes[i].title`).
    pub title_input:     String,
    /// Live text_editor buffer for the selected note's body.
    pub body_content:    text_editor::Content,
    /// Edit / Split / Preview display mode for the note body.
    pub view_mode:       NoteViewMode,
    /// True while a freshly-created note is being titled — Tab/Enter then
    /// moves keyboard focus from the title field into the body editor.
    pub naming:          bool,
    next_id:             u64,
}

impl NotesState {
    pub fn load() -> Self {
        let notes = read_from_disk().unwrap_or_default();
        let next_id = notes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        let selected = notes.first().map(|n| n.id);
        let title_input = selected
            .and_then(|id| notes.iter().find(|n| n.id == id))
            .map(|n| n.title.clone())
            .unwrap_or_default();
        let body_text = selected
            .and_then(|id| notes.iter().find(|n| n.id == id))
            .map(|n| n.body.clone())
            .unwrap_or_default();
        Self {
            notes,
            selected,
            title_input,
            body_content: text_editor::Content::with_text(&body_text),
            view_mode: NoteViewMode::default(),
            naming: false,
            next_id,
        }
    }

    pub fn save(&self) {
        let Some(path) = notes_path() else { return; };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let v: Vec<Value> = self.notes.iter().map(|n| json!({
            "id":      n.id,
            "title":   n.title,
            "body":    n.body,
            "project": n.project,
        })).collect();
        let doc = json!({ "notes": v });
        if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
            let _ = std::fs::write(&path, bytes);
        }
    }

    /// Create a note pinned to `project` (the active session's working
    /// directory) — pass `None` for a global note.
    pub fn create(&mut self, project: Option<String>) {
        // Flush any pending edits to the previously-selected note before
        // switching focus to the new one.
        self.flush_buffers();

        let id = self.next_id;
        self.next_id += 1;
        self.notes.push(Note {
            id,
            title: String::from("Untitled"),
            body:  String::new(),
            project,
        });
        self.selected     = Some(id);
        self.title_input  = String::from("Untitled");
        self.body_content = text_editor::Content::new();
        self.naming       = true;
        self.save();
    }

    /// Create a new note pre-filled with `body` (e.g. text highlighted in a
    /// terminal) and select it. The title is the first non-empty line,
    /// truncated so a pasted wall of text doesn't become a mile-wide title.
    pub fn create_from(&mut self, body: &str, project: Option<String>) {
        self.flush_buffers();

        let title = title_from(body);
        let id = self.next_id;
        self.next_id += 1;
        self.notes.push(Note {
            id,
            title: title.clone(),
            body:  body.to_string(),
            project,
        });
        self.selected     = Some(id);
        self.title_input  = title;
        self.body_content = text_editor::Content::with_text(body);
        self.save();
    }

    pub fn select(&mut self, id: u64) {
        self.naming = false;
        if self.selected == Some(id) { return; }
        self.flush_buffers();
        if let Some(n) = self.notes.iter().find(|n| n.id == id) {
            self.selected     = Some(id);
            self.title_input  = n.title.clone();
            self.body_content = text_editor::Content::with_text(&n.body);
        }
    }

    /// Delete `id`. `project` is the active project, so the fallback
    /// selection lands on a note that's actually visible in the list.
    pub fn delete(&mut self, id: u64, project: Option<&str>) {
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            self.notes.remove(pos);
            if self.selected == Some(id) {
                // Fall through to the nearest visible note — previous ones
                // first, then following — or none.
                let fallback = self.notes.iter().take(pos).rev()
                    .find(|n| n.visible_in(project))
                    .or_else(|| self.notes.iter().skip(pos).find(|n| n.visible_in(project)));
                if let Some(n) = fallback {
                    self.selected     = Some(n.id);
                    self.title_input  = n.title.clone();
                    self.body_content = text_editor::Content::with_text(&n.body);
                } else {
                    self.selected     = None;
                    self.title_input  = String::new();
                    self.body_content = text_editor::Content::new();
                }
            }
            self.save();
        }
    }

    /// Toggle the selected note between pinned-to-`project` and global.
    pub fn toggle_pin(&mut self, id: u64, project: Option<String>) {
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            n.project = if n.project.is_some() { None } else { project };
            self.save();
        }
    }

    pub fn edit_title(&mut self, s: String) {
        self.title_input = s;
        if let Some(id) = self.selected {
            if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
                n.title = self.title_input.clone();
            }
            self.save();
        }
    }

    /// Push the live `text_editor::Content` text into the selected note.
    /// Called after each edit action so disk and memory agree.
    pub fn commit_body(&mut self) {
        if let Some(id) = self.selected {
            let text = self.body_content.text();
            if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
                if n.body != text {
                    n.body = text;
                    self.save();
                }
            }
        }
    }

    /// Apply a markdown formatting action to the body editor. The caller
    /// (app::update) follows up with `commit_body()` to persist.
    pub fn apply_format(&mut self, fmt: NoteFormat) {
        use NoteFormat::*;
        match fmt {
            Bold          => self.wrap_inline("**", "**"),
            Italic        => self.wrap_inline("*", "*"),
            Strikethrough => self.wrap_inline("~~", "~~"),
            Code          => self.wrap_inline("`", "`"),
            H1            => self.prefix_lines(&|_| "# ".into()),
            H2            => self.prefix_lines(&|_| "## ".into()),
            H3            => self.prefix_lines(&|_| "### ".into()),
            Bullet        => self.prefix_lines(&|_| "- ".into()),
            Numbered      => self.prefix_lines(&|i| format!("{}. ", i + 1)),
            Quote         => self.prefix_lines(&|_| "> ".into()),
            CodeBlock     => self.wrap_code_block(),
            Link          => self.insert_link(),
        }
    }

    fn perform(&mut self, action: text_editor::Action) {
        self.body_content.perform(action);
    }

    /// Paste replaces the current selection (or inserts at the cursor).
    fn paste(&mut self, s: String) {
        self.perform(text_editor::Action::Edit(text_editor::Edit::Paste(
            std::sync::Arc::new(s),
        )));
    }

    /// Wrap the selection in `prefix`/`suffix`; with no selection, insert the
    /// empty pair and park the cursor between the markers.
    fn wrap_inline(&mut self, prefix: &str, suffix: &str) {
        use text_editor::{Action, Motion};
        match self.body_content.selection() {
            Some(sel) => self.paste(format!("{prefix}{sel}{suffix}")),
            None => {
                self.paste(format!("{prefix}{suffix}"));
                for _ in 0..suffix.chars().count() {
                    self.perform(Action::Move(Motion::Left));
                }
            }
        }
    }

    /// Prefix the current line — or, for a multi-line selection, every
    /// selected line — with the marker produced by `prefix_for(line_index)`
    /// (the index only matters for numbered lists).
    fn prefix_lines(&mut self, prefix_for: &dyn Fn(usize) -> String) {
        use text_editor::{Action, Motion};
        if let Some(sel) = self.body_content.selection() {
            if sel.contains('\n') {
                let prefixed = sel
                    .lines()
                    .enumerate()
                    .map(|(i, l)| format!("{}{}", prefix_for(i), l))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.paste(prefixed);
                return;
            }
        }
        self.perform(Action::Move(Motion::Home));
        self.paste(prefix_for(0));
        self.perform(Action::Move(Motion::End));
    }

    /// Fence the selection in ``` markers; with no selection, insert an empty
    /// fence and park the cursor on the blank line inside it.
    fn wrap_code_block(&mut self) {
        use text_editor::{Action, Motion};
        match self.body_content.selection() {
            Some(sel) => {
                let sel = sel.strip_suffix('\n').unwrap_or(&sel);
                self.paste(format!("```\n{sel}\n```"));
            }
            None => {
                self.paste("```\n\n```".into());
                self.perform(Action::Move(Motion::Up));
            }
        }
    }

    /// Insert `[selection](url)` and leave the `url` placeholder selected so
    /// the user can type straight over it.
    fn insert_link(&mut self) {
        use text_editor::{Action, Motion};
        let label = self
            .body_content
            .selection()
            .unwrap_or_else(|| "text".into());
        self.paste(format!("[{label}](url)"));
        self.perform(Action::Move(Motion::Left));
        self.perform(Action::Select(Motion::WordLeft));
    }

    fn flush_buffers(&mut self) {
        if let Some(id) = self.selected {
            let title = self.title_input.clone();
            let body  = self.body_content.text();
            if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
                let mut changed = false;
                if n.title != title { n.title = title; changed = true; }
                if n.body  != body  { n.body  = body;  changed = true; }
                if changed { self.save(); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::widget::text_editor::Action;

    fn state_with_body(body: &str) -> NotesState {
        NotesState {
            notes:        vec![],
            selected:     None,
            title_input:  String::new(),
            body_content: text_editor::Content::with_text(body),
            view_mode:    NoteViewMode::default(),
            naming:       false,
            next_id:      1,
        }
    }

    #[test]
    fn bold_wraps_selection() {
        let mut s = state_with_body("hello");
        s.body_content.perform(Action::SelectAll);
        s.apply_format(NoteFormat::Bold);
        assert_eq!(s.body_content.text().trim_end(), "**hello**");
    }

    #[test]
    fn heading_prefixes_current_line() {
        let mut s = state_with_body("title line");
        s.apply_format(NoteFormat::H2);
        assert_eq!(s.body_content.text().trim_end(), "## title line");
    }

    #[test]
    fn numbered_list_numbers_each_selected_line() {
        let mut s = state_with_body("a\nb\nc");
        s.body_content.perform(Action::SelectAll);
        s.apply_format(NoteFormat::Numbered);
        assert_eq!(s.body_content.text().trim_end(), "1. a\n2. b\n3. c");
    }

    #[test]
    fn code_block_fences_selection() {
        let mut s = state_with_body("let x = 1;");
        s.body_content.perform(Action::SelectAll);
        s.apply_format(NoteFormat::CodeBlock);
        assert_eq!(s.body_content.text().trim_end(), "```\nlet x = 1;\n```");
    }

    #[test]
    fn bold_without_selection_parks_cursor_between_markers() {
        let mut s = state_with_body("");
        s.apply_format(NoteFormat::Bold);
        // Typing after the format should land between the markers.
        s.body_content.perform(Action::Edit(text_editor::Edit::Insert('x')));
        assert_eq!(s.body_content.text().trim_end(), "**x**");
    }

    #[test]
    fn link_leaves_url_placeholder_selected() {
        let mut s = state_with_body("docs");
        s.body_content.perform(Action::SelectAll);
        s.apply_format(NoteFormat::Link);
        assert_eq!(s.body_content.text().trim_end(), "[docs](url)");
        assert_eq!(s.body_content.selection().as_deref(), Some("url"));
    }

    #[test]
    fn note_visibility_scopes_to_project() {
        let global = Note { id: 1, title: "g".into(), body: String::new(), project: None };
        let pinned = Note { id: 2, title: "p".into(), body: String::new(), project: Some("/a".into()) };
        // Global notes show everywhere; pinned ones only in their project.
        assert!(global.visible_in(Some("/a")));
        assert!(global.visible_in(None));
        assert!(pinned.visible_in(Some("/a")));
        assert!(!pinned.visible_in(Some("/b")));
        assert!(!pinned.visible_in(None));
    }

    #[test]
    fn title_from_uses_first_non_empty_line() {
        assert_eq!(title_from("\n\n  error: it broke\ndetails"), "error: it broke");
    }

    #[test]
    fn title_from_truncates_long_lines() {
        let long = "x".repeat(60);
        let title = title_from(&long);
        assert_eq!(title.chars().count(), 41); // 40 chars + ellipsis
        assert!(title.ends_with('…'));
    }

    #[test]
    fn title_from_falls_back_on_blank_text() {
        assert_eq!(title_from("  \n \n"), "Terminal snippet");
    }

    #[test]
    fn quote_prefixes_multi_line_selection() {
        let mut s = state_with_body("one\ntwo");
        s.body_content.perform(Action::SelectAll);
        s.apply_format(NoteFormat::Quote);
        assert_eq!(s.body_content.text().trim_end(), "> one\n> two");
    }
}

/// Derive a note title from captured text: the first non-empty line,
/// truncated so a pasted wall of text doesn't become a mile-wide title.
fn title_from(body: &str) -> String {
    const TITLE_MAX: usize = 40;
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| {
            if l.chars().count() > TITLE_MAX {
                format!("{}…", l.chars().take(TITLE_MAX).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .unwrap_or_else(|| String::from("Terminal snippet"))
}

fn notes_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".frms").join("notes.json"))
}

fn read_from_disk() -> Option<Vec<Note>> {
    let path  = notes_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let arr = v.get("notes")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|n| {
                Some(Note {
                    id:    n.get("id")?.as_u64()?,
                    title: n.get("title")?.as_str()?.to_string(),
                    body:  n.get("body")?.as_str()?.to_string(),
                    // Missing or null = global note (pre-project files too).
                    project: n.get("project")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                })
            })
            .collect(),
    )
}
