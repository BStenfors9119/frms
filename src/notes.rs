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

/// Label shown for notes that haven't been assigned a category.
pub const UNCATEGORIZED_LABEL: &str = "General";

/// One indent step in the note body editor. Two spaces because notes are
/// markdown, where a nested list item is exactly that far in.
const INDENT: &str = "  ";

/// A choice in the note's category `pick_list`. Kept as a dedicated enum (rather
/// than raw strings) so the "uncategorized" and "new category" entries can't
/// collide with a user-typed category name. Category values are flat strings for
/// now; a future nested-folder model can encode a path in `Named` without
/// changing this surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CategoryChoice {
    /// No category — displayed as [`UNCATEGORIZED_LABEL`].
    Uncategorized,
    /// An existing category name.
    Named(String),
    /// The "+ New category…" entry that opens the name field.
    NewCategory,
}

impl std::fmt::Display for CategoryChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CategoryChoice::Uncategorized => write!(f, "{UNCATEGORIZED_LABEL}"),
            CategoryChoice::Named(s)      => write!(f, "{s}"),
            CategoryChoice::NewCategory   => write!(f, "+ New category…"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Note {
    pub id:    u64,
    pub title: String,
    pub body:  String,
    /// Project this note is pinned to — the session's working directory.
    /// `None` means global: the note shows up in every project.
    pub project: Option<String>,
    /// User-assigned category within the project (e.g. a submodule name), used
    /// to group the notes list. `None`/empty = uncategorized.
    pub category: Option<String>,
    /// The id of the BookStack page this note is tied to, if the optional sync
    /// ([`crate::bookstack`]) has run. `None` until then; a note that already has
    /// one is reconciled with that page instead of creating a duplicate.
    pub bookstack_page_id: Option<u64>,
    /// The page's `updated_at`, and a [`crate::bookstack::content_hash`] of the
    /// note's title+body, both recorded at the last successful sync. Together
    /// they let the two-way sync tell which side changed: the page moved remotely
    /// when its current `updated_at` differs from `bookstack_synced_at`, and the
    /// note changed locally when its current hash differs from
    /// `bookstack_synced_hash`. `None` until the note has synced (or for notes
    /// pushed by the earlier one-way sync).
    pub bookstack_synced_at:   Option<String>,
    pub bookstack_synced_hash: Option<u64>,
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
    /// Set when the body editor's key binding has just consumed a Tab press
    /// (see `ui::plugin_panel::notes_key_binding`). Tab is exempt from the
    /// `Status::Captured` gate in `main_key_listener`, so the same press also
    /// reaches `Message::TabPressed`; this flag tells that handler to stand
    /// down instead of feeding a tab byte to the terminal. Transient.
    pub tab_in_body:     bool,
    /// True while the "+ New category…" name field is open for the selected
    /// note. Transient UI state, not persisted.
    pub adding_category: bool,
    /// Buffer for the new-category name field. Transient, not persisted.
    pub category_input:  String,
    /// Search text filtering the notes list — matched against each note's
    /// title, body, and category. Transient rather than persisted: a filter
    /// left on from a previous launch would look like lost notes.
    pub search:          String,
    /// Categories the user has collapsed in the list. The empty string is the
    /// key for the uncategorized ("General") group. Persisted in `notes.json`
    /// so the list reopens the way the user left it.
    pub collapsed_categories: Vec<String>,
    /// True while a BookStack sync ([`crate::bookstack`]) is in flight, so the
    /// Notes tab can show progress and refuse a second concurrent run.
    /// Transient — not persisted.
    pub bookstack_syncing: bool,
    /// Last BookStack sync result line (success summary or error), shown under
    /// the notes list. Transient — cleared on relaunch.
    pub bookstack_status:  Option<String>,
    /// Notes that changed on both sides in the last sync, awaiting the user's
    /// choice ([`crate::bookstack::Conflict`]). Transient — a re-sync regenerates
    /// them, so there is no point persisting the (possibly stale) remote text.
    pub bookstack_conflicts: Vec<crate::bookstack::Conflict>,
    /// The note id whose conflict diff is open in the editor pane, if any.
    /// Transient.
    pub bookstack_conflict_open: Option<u64>,
    /// BookStack pages that no note points at, offered for the user to import as
    /// new notes rather than created automatically — so a sync never silently
    /// floods the list. Transient (re-generated each sync).
    pub bookstack_pending_imports: Vec<crate::bookstack::PulledNote>,
    /// BookStack page ids the user dismissed from the import list. Persisted, and
    /// fed back into the sync so a dismissed page isn't offered again. (Clearing
    /// notes.json, or re-importing, is the way back.)
    pub bookstack_declined_pages: Vec<u64>,
    next_id:             u64,
    /// Where `save()` writes. The real `notes.json` path in the running app;
    /// `None` in unit tests so mutating tests never touch the user's real file.
    persist_path:        Option<PathBuf>,
}

impl NotesState {
    pub fn load() -> Self {
        let (notes, collapsed_categories, declined_pages) = read_from_disk().unwrap_or_default();
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
            tab_in_body: false,
            adding_category: false,
            category_input: String::new(),
            search: String::new(),
            collapsed_categories,
            bookstack_syncing: false,
            bookstack_status:  None,
            bookstack_conflicts: Vec::new(),
            bookstack_conflict_open: None,
            bookstack_pending_imports: Vec::new(),
            bookstack_declined_pages: declined_pages,
            next_id,
            persist_path: notes_path(),
        }
    }

    pub fn save(&self) {
        let Some(path) = self.persist_path.clone() else { return; };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let v: Vec<Value> = self.notes.iter().map(|n| json!({
            "id":       n.id,
            "title":    n.title,
            "body":     n.body,
            "project":  n.project,
            "category": n.category,
            "bookstack_page_id":   n.bookstack_page_id,
            "bookstack_synced_at": n.bookstack_synced_at,
            "bookstack_synced_hash": n.bookstack_synced_hash,
        })).collect();
        let doc = json!({
            "notes": v,
            "collapsed_categories": self.collapsed_categories,
            "bookstack_declined_pages": self.bookstack_declined_pages,
        });
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

        // Inherit the "active" category — the one the currently-selected note
        // sits in — so a new note lands in the group the user is working in.
        // Uncategorized when nothing is selected (or the selection is itself
        // uncategorized).
        let category = self.active_category();

        let id = self.next_id;
        self.next_id += 1;
        self.notes.push(Note {
            id,
            title: String::from("Untitled"),
            body:  String::new(),
            project,
            category,
            bookstack_page_id: None,
            bookstack_synced_at: None,
            bookstack_synced_hash: None,
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
            category: None,
            bookstack_page_id: None,
            bookstack_synced_at: None,
            bookstack_synced_hash: None,
        });
        self.selected     = Some(id);
        self.title_input  = title;
        self.body_content = text_editor::Content::with_text(body);
        self.save();
    }

    pub fn select(&mut self, id: u64) {
        self.naming = false;
        // A half-typed new-category name belongs to the note being left.
        self.adding_category = false;
        self.category_input.clear();
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

    /// The category the user is currently "in" — the selected note's category.
    /// `None` when nothing is selected or the selection is uncategorized.
    pub fn active_category(&self) -> Option<String> {
        self.selected
            .and_then(|id| self.notes.iter().find(|n| n.id == id))
            .and_then(|n| n.category.clone())
    }

    /// Whether `note` matches the current search text. Every whitespace-
    /// separated term must appear somewhere in the note's title, body, or
    /// category (case-insensitive), so terms can be given in any order and a
    /// search can mix a category with a word from the body. An empty search
    /// matches everything.
    ///
    /// The body is searched, not just the title — a note worth finding is
    /// often one whose title didn't turn out to describe it.
    pub fn matches_search(&self, note: &Note) -> bool {
        let terms: Vec<String> = self.search.split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if terms.is_empty() {
            return true;
        }
        let hay = format!(
            "{}\n{}\n{}",
            note.title.to_lowercase(),
            note.body.to_lowercase(),
            note.category.as_deref().unwrap_or("").to_lowercase(),
        );
        terms.iter().all(|t| hay.contains(t.as_str()))
    }

    /// The notes the list should render for `project`: those visible there
    /// (this project's plus the global ones) that also match the search.
    pub fn visible_notes(&self, project: Option<&str>) -> Vec<&Note> {
        self.notes.iter()
            .filter(|n| n.visible_in(project))
            .filter(|n| self.matches_search(n))
            .collect()
    }

    /// True when a search is currently narrowing the notes list.
    pub fn is_searching(&self) -> bool {
        !self.search.trim().is_empty()
    }

    /// Distinct non-empty categories among the notes visible in `project`,
    /// sorted for a stable dropdown / list order.
    ///
    /// Deliberately *not* filtered by the search: this also feeds the selected
    /// note's category picker, which must offer every existing category even
    /// while a search is hiding most of them.
    pub fn categories_in(&self, project: Option<&str>) -> Vec<String> {
        let mut cats: Vec<String> = self.notes.iter()
            .filter(|n| n.visible_in(project))
            .filter_map(|n| n.category.clone())
            .collect();
        cats.sort();
        cats.dedup();
        cats
    }

    /// Assign `category` to note `id`. An empty/whitespace name clears it back
    /// to uncategorized. Also closes the new-category name field.
    pub fn set_category(&mut self, id: u64, category: Option<String>) {
        let normalized = category
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty());
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == id) {
            n.category = normalized;
            self.save();
        }
        self.adding_category = false;
        self.category_input.clear();
    }

    /// Open the "+ New category…" name field for the selected note.
    pub fn begin_new_category(&mut self) {
        self.adding_category = true;
        self.category_input.clear();
    }

    /// Collapse/expand a category group in the list. `key` is the category name,
    /// or the empty string for the uncategorized ("General") group.
    pub fn toggle_category_collapsed(&mut self, key: &str) {
        if let Some(pos) = self.collapsed_categories.iter().position(|c| c == key) {
            self.collapsed_categories.remove(pos);
        } else {
            self.collapsed_categories.push(key.to_string());
        }
        self.save();
    }

    pub fn is_category_collapsed(&self, key: &str) -> bool {
        self.collapsed_categories.iter().any(|c| c == key)
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

    /// Tab in the body editor: indent every line of a multi-line selection,
    /// otherwise insert one indent step at the cursor (replacing a single-line
    /// selection, the way an ordinary editor does).
    pub fn indent(&mut self) {
        if let Some(sel) = self.body_content.selection() {
            if sel.contains('\n') {
                let indented = sel
                    .lines()
                    .map(|l| format!("{INDENT}{l}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.paste(indented);
                return;
            }
        }
        self.paste(INDENT.into());
    }

    /// Shift+Tab: strip one indent step from every line of a multi-line
    /// selection, otherwise from the start of the line the cursor is on.
    pub fn outdent(&mut self) {
        if let Some(sel) = self.body_content.selection() {
            if sel.contains('\n') {
                let stripped = sel
                    .lines()
                    .map(strip_indent)
                    .collect::<Vec<_>>()
                    .join("\n");
                self.paste(stripped);
                return;
            }
        }
        self.outdent_current_line();
    }

    /// Remove one indent step from the cursor's line, leaving the cursor over
    /// the same character it started on (or at the line start when it sat
    /// inside the whitespace being removed).
    fn outdent_current_line(&mut self) {
        use text_editor::{Action, Edit, Motion};
        let (row, col) = self.body_content.cursor_position();
        let Some(line) = self.body_content.line(row).map(|l| l.to_string()) else {
            return;
        };
        let removed = line.len() - strip_indent(&line).len();
        if removed == 0 {
            return;
        }
        // `col` is a byte offset into the line; the text between the deleted
        // indent and the cursor tells us how far right to walk afterwards.
        // Motion::Right moves by grapheme, so count characters, not bytes.
        let back = line
            .get(removed.min(col)..col)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        self.perform(Action::Move(Motion::Home));
        for _ in 0..removed {
            self.perform(Action::Edit(Edit::Delete));
        }
        for _ in 0..back {
            self.perform(Action::Move(Motion::Right));
        }
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

    /// Notes to push to BookStack, with each note's project and category
    /// resolved to the book and chapter names the sync files it under: the
    /// project's directory basename (or `"Global"`), and the category (or the
    /// `"General"` label). Call [`NotesState::flush_buffers`] first so the
    /// in-progress note's edits are included.
    pub fn notes_for_bookstack(&self) -> Vec<crate::bookstack::NoteForSync> {
        self.notes.iter().map(|n| crate::bookstack::NoteForSync {
            id:    n.id,
            title: if n.title.trim().is_empty() { String::from("Untitled") } else { n.title.clone() },
            body:  n.body.clone(),
            project_name: n.project.as_deref().map(project_book_name)
                .unwrap_or_else(|| crate::bookstack::GLOBAL_BOOK.to_string()),
            category: n.category.clone().unwrap_or_else(|| UNCATEGORIZED_LABEL.to_string()),
            page_id:  n.bookstack_page_id,
            synced_at:   n.bookstack_synced_at.clone(),
            synced_hash: n.bookstack_synced_hash,
        }).collect()
    }

    /// Apply a completed two-way [`crate::bookstack::reconcile`]: record the new
    /// baselines on pushed notes, overwrite notes with newer remote content, stash
    /// any conflicts, and hold pages authored in BookStack as pending imports for
    /// the user to review (rather than creating them automatically). Returns the
    /// status line.
    pub fn apply_reconcile(&mut self, out: crate::bookstack::Outcome) -> String {
        let mut summary = out.summary();

        // Pushed notes: record the post-sync baseline so the next run sees them
        // as in sync.
        for r in &out.synced {
            if let Some(n) = self.notes.iter_mut().find(|n| n.id == r.note_id) {
                n.bookstack_page_id     = Some(r.page_id);
                n.bookstack_synced_at   = Some(r.synced_at.clone());
                n.bookstack_synced_hash = Some(r.synced_hash);
            }
        }

        // Pulled updates: newer remote content wins over the (unchanged) local
        // note. Refresh the live buffers if the note is on screen.
        for u in &out.remote_updates {
            if let Some(n) = self.notes.iter_mut().find(|n| n.id == u.note_id) {
                n.title    = u.title.clone();
                n.body     = u.body.clone();
                n.category = category_from_chapter(&u.category);
                n.bookstack_synced_at   = Some(u.synced_at.clone());
                n.bookstack_synced_hash = Some(u.synced_hash);
            }
            if self.selected == Some(u.note_id) {
                self.title_input  = u.title.clone();
                self.body_content = text_editor::Content::with_text(&u.body);
            }
        }

        // Pages authored in BookStack are *offered*, not auto-created — a sync
        // must never silently flood the list. They wait in the import list until
        // the user imports or dismisses each one.
        self.bookstack_pending_imports = out.new_notes;
        if !self.bookstack_pending_imports.is_empty() {
            summary.push_str(&format!(
                ", {} new page(s) to import", self.bookstack_pending_imports.len()));
        }

        self.bookstack_conflicts = out.conflicts;
        self.save();
        summary
    }

    /// The page ids the user has dismissed, in the form the sync consumes so a
    /// dismissed page is never offered again.
    pub fn bookstack_declined(&self) -> std::collections::HashSet<u64> {
        self.bookstack_declined_pages.iter().copied().collect()
    }

    /// Import one pending BookStack page as a new note, pinned under `project`
    /// when its book maps to one. Removes it from the pending list.
    pub fn import_pending(&mut self, page_id: u64, project: Option<&str>) {
        let Some(pos) = self.bookstack_pending_imports.iter().position(|p| p.page_id == page_id)
        else { return; };
        let p = self.bookstack_pending_imports.remove(pos);
        self.create_from_pulled(&p, project);
        self.save();
    }

    /// Import every pending BookStack page as a new note.
    pub fn import_all_pending(&mut self, project: Option<&str>) {
        let pending = std::mem::take(&mut self.bookstack_pending_imports);
        if pending.is_empty() { return; }
        for p in &pending {
            self.create_from_pulled(p, project);
        }
        self.save();
    }

    /// Dismiss one pending BookStack page: drop it from the list and remember not
    /// to offer it again.
    pub fn dismiss_pending(&mut self, page_id: u64) {
        self.bookstack_pending_imports.retain(|p| p.page_id != page_id);
        if !self.bookstack_declined_pages.contains(&page_id) {
            self.bookstack_declined_pages.push(page_id);
        }
        self.save();
    }

    /// Dismiss every pending BookStack page at once.
    pub fn dismiss_all_pending(&mut self) {
        let pending = std::mem::take(&mut self.bookstack_pending_imports);
        for p in pending {
            if !self.bookstack_declined_pages.contains(&p.page_id) {
                self.bookstack_declined_pages.push(p.page_id);
            }
        }
        self.save();
    }

    /// Turn a pulled BookStack page into a new local note, mapping its book name
    /// back to a project path (via the projects the current notes use, plus the
    /// active `project`) and its chapter back to a category.
    fn create_from_pulled(&mut self, p: &crate::bookstack::PulledNote, project: Option<&str>) {
        let note_project = if p.project_name == crate::bookstack::GLOBAL_BOOK {
            None
        } else {
            self.book_to_project_map(project).get(&p.project_name).cloned()
        };
        let id = self.next_id;
        self.next_id += 1;
        self.notes.push(Note {
            id,
            title: p.title.clone(),
            body:  p.body.clone(),
            project: note_project,
            category: category_from_chapter(&p.category),
            bookstack_page_id:     Some(p.page_id),
            bookstack_synced_at:   Some(p.synced_at.clone()),
            bookstack_synced_hash: Some(p.synced_hash),
        });
    }

    /// Reverse map of BookStack book name → full project path, drawn from the
    /// projects the current notes are pinned to (plus the active project), so a
    /// pulled page is filed under the right project.
    fn book_to_project_map(&self, project: Option<&str>) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for n in &self.notes {
            if let Some(p) = &n.project {
                map.entry(project_book_name(p)).or_insert_with(|| p.clone());
            }
        }
        if let Some(p) = project {
            map.entry(project_book_name(p)).or_insert_with(|| p.to_string());
        }
        map
    }

    /// The conflict currently open in the editor pane, if any.
    pub fn open_conflict(&self) -> Option<&crate::bookstack::Conflict> {
        let id = self.bookstack_conflict_open?;
        self.bookstack_conflicts.iter().find(|c| c.note_id == id)
    }

    /// Open a conflict's diff view in the editor pane.
    pub fn show_conflict(&mut self, note_id: u64) {
        self.bookstack_conflict_open = Some(note_id);
    }

    /// Close the conflict diff view, returning to the normal editor.
    pub fn close_conflict(&mut self) {
        self.bookstack_conflict_open = None;
    }

    /// Resolve a conflict by keeping the BookStack version: overwrite the local
    /// note with the remote content and baseline it, so the next sync sees both
    /// sides in agreement.
    pub fn resolve_conflict_use_remote(&mut self, note_id: u64) {
        let Some(c) = self.bookstack_conflicts.iter().find(|c| c.note_id == note_id).cloned() else {
            return;
        };
        let hash = crate::bookstack::content_hash(&c.remote_title, &c.remote_body);
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == note_id) {
            n.title    = c.remote_title.clone();
            n.body     = c.remote_body.clone();
            n.category = category_from_chapter(&c.remote_category);
            n.bookstack_synced_at   = Some(c.remote_synced_at.clone());
            n.bookstack_synced_hash = Some(hash);
        }
        if self.selected == Some(note_id) {
            self.title_input  = c.remote_title.clone();
            self.body_content = text_editor::Content::with_text(&c.remote_body);
        }
        self.clear_conflict(note_id);
    }

    /// Resolve a conflict by keeping the local version: leave the note untouched
    /// but mark the remote side as "seen" (baseline its `updated_at` and drop the
    /// content hash), so the next sync no longer reports a remote change and
    /// pushes the local note up instead.
    pub fn resolve_conflict_keep_local(&mut self, note_id: u64) {
        let Some(c) = self.bookstack_conflicts.iter().find(|c| c.note_id == note_id).cloned() else {
            return;
        };
        if let Some(n) = self.notes.iter_mut().find(|n| n.id == note_id) {
            n.bookstack_synced_at   = Some(c.remote_synced_at.clone());
            // Clearing the hash forces "local changed" next sync, so the local
            // note is pushed up to overwrite the remote page.
            n.bookstack_synced_hash = None;
        }
        self.clear_conflict(note_id);
    }

    /// Drop a resolved conflict from the pending list and close its view if open.
    fn clear_conflict(&mut self, note_id: u64) {
        self.bookstack_conflicts.retain(|c| c.note_id != note_id);
        if self.bookstack_conflict_open == Some(note_id) {
            self.bookstack_conflict_open = None;
        }
        self.save();
    }

    pub fn flush_buffers(&mut self) {
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
            tab_in_body:  false,
            adding_category: false,
            category_input:  String::new(),
            search:       String::new(),
            collapsed_categories: Vec::new(),
            bookstack_syncing: false,
            bookstack_status:  None,
            bookstack_conflicts: Vec::new(),
            bookstack_conflict_open: None,
            bookstack_pending_imports: Vec::new(),
            bookstack_declined_pages: Vec::new(),
            next_id:      1,
            // None → save() is a no-op, so mutating tests never write to the
            // user's real ~/.frms/notes.json.
            persist_path: None,
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
    fn tab_inserts_indent_at_cursor() {
        let mut s = state_with_body("item");
        s.indent();
        assert_eq!(s.body_content.text().trim_end(), "  item");
    }

    #[test]
    fn tab_indents_every_selected_line() {
        let mut s = state_with_body("a\nb");
        s.body_content.perform(Action::SelectAll);
        s.indent();
        assert_eq!(s.body_content.text().trim_end(), "  a\n  b");
    }

    #[test]
    fn shift_tab_outdents_selected_lines() {
        let mut s = state_with_body("  a\n\tb\nc");
        s.body_content.perform(Action::SelectAll);
        s.outdent();
        assert_eq!(s.body_content.text().trim_end(), "a\nb\nc");
    }

    #[test]
    fn shift_tab_outdents_current_line_and_keeps_cursor() {
        use iced::widget::text_editor::Motion;
        let mut s = state_with_body("    x");
        s.body_content.perform(Action::Move(Motion::End));
        s.outdent();
        assert_eq!(s.body_content.text().trim_end(), "  x");
        assert_eq!(s.body_content.cursor_position(), (0, 3));
    }

    #[test]
    fn shift_tab_leaves_a_flush_left_line_alone() {
        let mut s = state_with_body("x");
        s.outdent();
        assert_eq!(s.body_content.text().trim_end(), "x");
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
        let global = Note { id: 1, title: "g".into(), body: String::new(), project: None, category: None, bookstack_page_id: None, bookstack_synced_at: None, bookstack_synced_hash: None };
        let pinned = Note { id: 2, title: "p".into(), body: String::new(), project: Some("/a".into()), category: None, bookstack_page_id: None, bookstack_synced_at: None, bookstack_synced_hash: None };
        // Global notes show everywhere; pinned ones only in their project.
        assert!(global.visible_in(Some("/a")));
        assert!(global.visible_in(None));
        assert!(pinned.visible_in(Some("/a")));
        assert!(!pinned.visible_in(Some("/b")));
        assert!(!pinned.visible_in(None));
    }

    fn note_with_category(id: u64, project: Option<&str>, category: Option<&str>) -> Note {
        Note {
            id,
            title: format!("n{id}"),
            body: String::new(),
            project: project.map(String::from),
            category: category.map(String::from),
            bookstack_page_id: None,
            bookstack_synced_at: None,
            bookstack_synced_hash: None,
        }
    }

    fn note_with_text(id: u64, title: &str, body: &str) -> Note {
        Note {
            id,
            title: title.to_string(),
            body: body.to_string(),
            project: None,
            category: None,
            bookstack_page_id: None,
            bookstack_synced_at: None,
            bookstack_synced_hash: None,
        }
    }

    /// The body is searched, not just the title — finding a note by something
    /// written inside it is the main thing the search is for.
    #[test]
    fn search_matches_title_body_and_category() {
        let mut s = state_with_body("");
        s.notes = vec![
            note_with_text(1, "Deploy runbook", "ssh into the box"),
            note_with_text(2, "Standup", "mentioned the runbook"),
            note_with_category(3, None, Some("runbook")),
        ];

        s.search = "runbook".into();
        let ids: Vec<u64> = s.visible_notes(None).iter().map(|n| n.id).collect();
        assert_eq!(ids, [1, 2, 3]);

        // Terms are ANDed and order-independent, so a title word plus a body
        // word narrows to the one note carrying both.
        s.search = "ssh deploy".into();
        let ids: Vec<u64> = s.visible_notes(None).iter().map(|n| n.id).collect();
        assert_eq!(ids, [1]);
    }

    #[test]
    fn search_is_case_insensitive_and_empty_matches_all() {
        let mut s = state_with_body("");
        s.notes = vec![note_with_text(1, "TELEMETRY", "")];
        s.search = "telemetry".into();
        assert_eq!(s.visible_notes(None).len(), 1);

        // Whitespace-only is not a filter — it must not empty the list.
        s.search = "   ".into();
        assert!(!s.is_searching());
        assert_eq!(s.visible_notes(None).len(), 1);
    }

    /// The search narrows the *list* only. Another project's notes stay hidden
    /// even when they match, and the category picker still offers every
    /// category so a filtered view can't strand a note's categorization.
    #[test]
    fn search_respects_project_scope_and_leaves_categories_intact() {
        let mut s = state_with_body("");
        s.notes = vec![
            note_with_category(1, Some("/a"), Some("core")),
            note_with_category(2, Some("/b"), Some("other")),
        ];
        s.search = "n2".into(); // titles are "n<id>"
        assert!(s.visible_notes(Some("/a")).is_empty());
        assert_eq!(s.categories_in(Some("/a")), vec!["core"]);
    }

    #[test]
    fn categories_in_lists_sorted_unique_categories_for_project() {
        let mut s = state_with_body("");
        s.notes = vec![
            note_with_category(1, Some("/a"), Some("web-ui")),
            note_with_category(2, Some("/a"), Some("core")),
            note_with_category(3, Some("/a"), Some("core")),   // dup
            note_with_category(4, Some("/a"), None),           // uncategorized
            note_with_category(5, Some("/b"), Some("other")),  // different project
            note_with_category(6, None,       Some("global")), // global — visible everywhere
        ];
        assert_eq!(s.categories_in(Some("/a")), vec!["core", "global", "web-ui"]);
    }

    #[test]
    fn set_category_normalizes_blank_to_none() {
        let mut s = state_with_body("");
        s.notes = vec![note_with_category(1, None, None)];
        s.set_category(1, Some("  ".into()));
        assert_eq!(s.notes[0].category, None);
        s.set_category(1, Some("  core  ".into()));
        assert_eq!(s.notes[0].category.as_deref(), Some("core"));
    }

    #[test]
    fn new_note_inherits_active_category() {
        let mut s = state_with_body("");
        s.notes = vec![note_with_category(1, Some("/a"), Some("core"))];
        s.selected = Some(1);
        s.next_id = 2;
        s.create(Some("/a".into()));
        let new = s.notes.iter().find(|n| Some(n.id) == s.selected).unwrap();
        assert_eq!(new.category.as_deref(), Some("core"));
    }

    #[test]
    fn new_note_uncategorized_when_no_active_category() {
        let mut s = state_with_body("");
        s.notes = vec![note_with_category(1, Some("/a"), None)];
        s.selected = Some(1);
        s.next_id = 2;
        s.create(Some("/a".into()));
        let new = s.notes.iter().find(|n| Some(n.id) == s.selected).unwrap();
        assert_eq!(new.category, None);
    }

    #[test]
    fn toggle_category_collapsed_round_trips() {
        let mut s = state_with_body("");
        assert!(!s.is_category_collapsed("core"));
        s.toggle_category_collapsed("core");
        assert!(s.is_category_collapsed("core"));
        s.toggle_category_collapsed("core");
        assert!(!s.is_category_collapsed("core"));
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

    #[test]
    fn category_from_chapter_maps_general_to_uncategorized() {
        assert_eq!(category_from_chapter("General"), None);
        assert_eq!(category_from_chapter("   "), None);
        assert_eq!(category_from_chapter("core"), Some("core".to_string()));
    }

    #[test]
    fn apply_reconcile_records_baseline_on_pushed_notes() {
        use crate::bookstack::{Outcome, SyncedRef};
        let mut s = state_with_body("");
        s.notes = vec![note_with_text(1, "t", "b")];
        let out = Outcome {
            created: 1,
            synced: vec![SyncedRef { note_id: 1, page_id: 42, synced_at: "T1".into(), synced_hash: 99 }],
            ..Default::default()
        };
        s.apply_reconcile(out);
        let n = &s.notes[0];
        assert_eq!(n.bookstack_page_id, Some(42));
        assert_eq!(n.bookstack_synced_at.as_deref(), Some("T1"));
        assert_eq!(n.bookstack_synced_hash, Some(99));
    }

    #[test]
    fn apply_reconcile_overwrites_note_with_remote_update() {
        use crate::bookstack::{Outcome, RemoteUpdate};
        let mut s = state_with_body("");
        s.notes = vec![note_with_category(1, Some("/a"), None)];
        let out = Outcome {
            pulled_updated: 1,
            remote_updates: vec![RemoteUpdate {
                note_id: 1, title: "New".into(), body: "fresh".into(),
                category: "core".into(), synced_at: "T2".into(), synced_hash: 7,
            }],
            ..Default::default()
        };
        s.apply_reconcile(out);
        let n = &s.notes[0];
        assert_eq!(n.title, "New");
        assert_eq!(n.body, "fresh");
        assert_eq!(n.category.as_deref(), Some("core"));
        assert_eq!(n.bookstack_synced_at.as_deref(), Some("T2"));
    }

    /// Pages authored in BookStack are held for review, not auto-created, and
    /// importing one files it under the project whose basename matches its book.
    #[test]
    fn remote_pages_are_staged_then_imported() {
        use crate::bookstack::{Outcome, PulledNote};
        let mut s = state_with_body("");
        s.next_id = 5;
        let out = Outcome {
            pulled_new: 1,
            new_notes: vec![PulledNote {
                page_id: 88, title: "Away note".into(), body: "wrote on phone".into(),
                project_name: "frms".into(), category: "General".into(),
                synced_at: "T3".into(), synced_hash: 3,
            }],
            ..Default::default()
        };
        s.apply_reconcile(out);
        // Nothing created yet — it waits in the import list.
        assert!(s.notes.is_empty());
        assert_eq!(s.bookstack_pending_imports.len(), 1);

        s.import_pending(88, Some("/home/me/dev/frms"));
        assert!(s.bookstack_pending_imports.is_empty());
        let n = s.notes.iter().find(|n| n.bookstack_page_id == Some(88)).unwrap();
        assert_eq!(n.title, "Away note");
        assert_eq!(n.project.as_deref(), Some("/home/me/dev/frms"));
        assert_eq!(n.category, None); // "General" → uncategorized
    }

    /// A global page (book "Global") imports as a global note.
    #[test]
    fn imported_global_page_becomes_a_global_note() {
        use crate::bookstack::{Outcome, PulledNote};
        let mut s = state_with_body("");
        let out = Outcome {
            new_notes: vec![PulledNote {
                page_id: 90, title: "g".into(), body: String::new(),
                project_name: crate::bookstack::GLOBAL_BOOK.into(), category: "General".into(),
                synced_at: "T".into(), synced_hash: 1,
            }],
            ..Default::default()
        };
        s.apply_reconcile(out);
        s.import_all_pending(Some("/home/me/dev/frms"));
        let n = s.notes.iter().find(|n| n.bookstack_page_id == Some(90)).unwrap();
        assert_eq!(n.project, None);
    }

    /// Dismissing a pending page drops it and records it as declined so the sync
    /// won't offer it again.
    #[test]
    fn dismissing_a_pending_page_declines_it() {
        use crate::bookstack::{Outcome, PulledNote};
        let mut s = state_with_body("");
        let out = Outcome {
            new_notes: vec![PulledNote {
                page_id: 42, title: "orphan".into(), body: String::new(),
                project_name: crate::bookstack::GLOBAL_BOOK.into(), category: "General".into(),
                synced_at: "T".into(), synced_hash: 1,
            }],
            ..Default::default()
        };
        s.apply_reconcile(out);
        s.dismiss_pending(42);
        assert!(s.bookstack_pending_imports.is_empty());
        assert!(s.notes.is_empty());
        assert!(s.bookstack_declined().contains(&42));
    }

    #[test]
    fn resolve_conflict_use_remote_overwrites_and_clears() {
        use crate::bookstack::Conflict;
        let mut s = state_with_body("");
        s.notes = vec![note_with_text(1, "mine", "local")];
        s.bookstack_conflicts = vec![Conflict {
            note_id: 1,
            local_title: "mine".into(), local_body: "local".into(),
            remote_title: "theirs".into(), remote_body: "remote".into(),
            remote_category: "General".into(), remote_synced_at: "T9".into(),
        }];
        s.resolve_conflict_use_remote(1);
        assert!(s.bookstack_conflicts.is_empty());
        let n = &s.notes[0];
        assert_eq!(n.title, "theirs");
        assert_eq!(n.body, "remote");
        assert_eq!(n.bookstack_synced_at.as_deref(), Some("T9"));
        // Baseline hash matches the adopted remote content, so the next sync
        // sees no local change.
        assert_eq!(n.bookstack_synced_hash, Some(crate::bookstack::content_hash("theirs", "remote")));
    }

    #[test]
    fn resolve_conflict_keep_local_marks_remote_seen_and_forces_push() {
        use crate::bookstack::Conflict;
        let mut s = state_with_body("");
        s.notes = vec![note_with_text(1, "mine", "local")];
        s.bookstack_conflicts = vec![Conflict {
            note_id: 1,
            local_title: "mine".into(), local_body: "local".into(),
            remote_title: "theirs".into(), remote_body: "remote".into(),
            remote_category: "General".into(), remote_synced_at: "T9".into(),
        }];
        s.resolve_conflict_keep_local(1);
        assert!(s.bookstack_conflicts.is_empty());
        let n = &s.notes[0];
        // Local content is untouched…
        assert_eq!(n.title, "mine");
        assert_eq!(n.body, "local");
        // …the remote side is marked seen, and the hash is cleared so the next
        // sync counts the note as locally changed and pushes it.
        assert_eq!(n.bookstack_synced_at.as_deref(), Some("T9"));
        assert_eq!(n.bookstack_synced_hash, None);
    }
}

/// Strip one indent step from the front of `line` — a single leading tab, or
/// up to `INDENT.len()` leading spaces. Lines already flush left come back
/// unchanged.
fn strip_indent(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix('\t') {
        return rest;
    }
    let spaces = line.len() - line.trim_start_matches(' ').len();
    &line[spaces.min(INDENT.len())..]
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
    Some(crate::port::dirs::config()?.join("notes.json"))
}

/// The BookStack book name for a project-pinned note: the working directory's
/// basename (e.g. `/home/me/dev/frms` → `frms`), which reads far better as a
/// book title than the full path. Falls back to the raw string if it has no
/// final component.
fn project_book_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| path.to_string())
}

/// The note category a BookStack chapter name maps back to: the `"General"`
/// chapter (and any empty name) is our uncategorized bucket, so it becomes
/// `None`; every other chapter is a named category.
fn category_from_chapter(chapter: &str) -> Option<String> {
    let c = chapter.trim();
    if c.is_empty() || c == UNCATEGORIZED_LABEL {
        None
    } else {
        Some(c.to_string())
    }
}

fn read_from_disk() -> Option<(Vec<Note>, Vec<String>, Vec<u64>)> {
    let path  = notes_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let arr = v.get("notes")?.as_array()?;
    let notes = arr.iter()
        .filter_map(|n| {
            Some(Note {
                id:    n.get("id")?.as_u64()?,
                title: n.get("title")?.as_str()?.to_string(),
                body:  n.get("body")?.as_str()?.to_string(),
                // Missing or null = global note (pre-project files too).
                project: n.get("project")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                // Missing or null/empty = uncategorized (pre-category files).
                category: n.get("category")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from),
                // Missing (never synced, or a pre-sync file) = no page yet.
                bookstack_page_id: n.get("bookstack_page_id").and_then(Value::as_u64),
                // Missing (pre-two-way-sync files) = no baseline; the first
                // reconcile adopts one (see [`crate::bookstack::reconcile`]).
                bookstack_synced_at: n.get("bookstack_synced_at")
                    .and_then(|v| v.as_str()).map(String::from),
                bookstack_synced_hash: n.get("bookstack_synced_hash").and_then(Value::as_u64),
            })
        })
        .collect();
    // Missing (pre-collapse-persistence files) = nothing collapsed.
    let collapsed = v.get("collapsed_categories")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    // Missing (pre-import-review files) = nothing dismissed.
    let declined = v.get("bookstack_declined_pages")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default();
    Some((notes, collapsed, declined))
}
