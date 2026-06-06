use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// In-progress directory operation in the file browser pane. Only one can be
/// active at a time — starting a new one replaces the old.
#[derive(Clone, Debug)]
pub enum DirEdit {
    /// Creating a new directory inside `current_dir`; `draft` is its name.
    Create { draft: String },
    /// Renaming the directory at `path`; `draft` is the new name.
    Rename { path: PathBuf, draft: String },
    /// Waiting for the user to confirm deletion of the directory at `path`.
    ConfirmDelete { path: PathBuf },
}

pub struct FileBrowserState {
    pub current_dir: PathBuf,
    pub entries: Vec<Entry>,
    /// In-progress create/rename/delete on a directory, if any.
    pub edit: Option<DirEdit>,
    /// Error from the last directory operation — shown inline until the next
    /// successful operation or navigation.
    pub error: Option<String>,
}

impl FileBrowserState {
    pub fn new() -> Self {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let entries = read_dir(&dir);
        Self { current_dir: dir, entries, edit: None, error: None }
    }

    pub fn navigate(&mut self, path: PathBuf) {
        let entries = read_dir(&path);
        self.current_dir = path;
        self.entries = entries;
        self.edit = None;
        self.error = None;
    }

    /// Re-read the current directory (after a create/rename/delete).
    pub fn refresh(&mut self) {
        self.entries = read_dir(&self.current_dir);
    }

    /// Apply the in-progress edit. On success the edit closes and the listing
    /// refreshes; on failure the edit stays open with `error` set so the user
    /// can fix the name (or give up via cancel).
    pub fn confirm_edit(&mut self) {
        let Some(edit) = self.edit.clone() else { return };

        let result = match &edit {
            DirEdit::Create { draft } => valid_name(draft).and_then(|name| {
                fs::create_dir(self.current_dir.join(name))
                    .map_err(|e| format!("Couldn't create folder: {e}"))
            }),
            DirEdit::Rename { path, draft } => valid_name(draft).and_then(|name| {
                let to = path.with_file_name(name);
                if to == *path {
                    Ok(()) // same name — nothing to do
                } else {
                    fs::rename(path, &to)
                        .map_err(|e| format!("Couldn't rename folder: {e}"))
                }
            }),
            DirEdit::ConfirmDelete { path } => fs::remove_dir_all(path)
                .map_err(|e| format!("Couldn't delete folder: {e}")),
        };

        match result {
            Ok(()) => {
                self.edit = None;
                self.error = None;
                self.refresh();
            }
            Err(e) => self.error = Some(e),
        }
    }
}

/// Trimmed, non-empty, single path component — everything a folder name
/// typed into the inline input must be.
fn valid_name(draft: &str) -> Result<&str, String> {
    let name = draft.trim();
    if name.is_empty() {
        Err("Folder name can't be empty".into())
    } else if name.contains(std::path::is_separator) {
        Err("Folder name can't contain a path separator".into())
    } else if name == "." || name == ".." {
        Err(format!("\"{name}\" isn't a valid folder name"))
    } else {
        Ok(name)
    }
}

fn read_dir(path: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = match fs::read_dir(path) {
        Ok(iter) => iter
            .filter_map(|e| e.ok())
            .map(|e| Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
            })
            .collect(),
        Err(_) => Vec::new(),
    };

    // Directories first, then alphabetical within each group
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    entries
}
