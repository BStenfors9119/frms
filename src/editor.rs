use std::fs;
use std::path::PathBuf;

use iced::widget::text_editor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    Markdown,
    Html,
}

pub struct EditorState {
    pub path: Option<PathBuf>,
    pub content: text_editor::Content,
    pub preview_active: bool,
    /// HTML preview render state — populated by an async headless render.
    pub preview_loading: bool,
    pub preview_image:   Option<Vec<u8>>,
    pub preview_error:   Option<String>,
    /// Path the cached `preview_image` was rendered for (used to detect staleness).
    pub preview_image_path: Option<PathBuf>,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            path: None,
            content: text_editor::Content::new(),
            preview_active: false,
            preview_loading: false,
            preview_image:   None,
            preview_error:   None,
            preview_image_path: None,
        }
    }

    pub fn open(&mut self, path: PathBuf) {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| format!("// Could not read file: {e}"));
        self.content = text_editor::Content::with_text(&text);
        self.path = Some(path);
        // Opening a different file invalidates any cached HTML preview.
        self.preview_image      = None;
        self.preview_image_path = None;
        self.preview_error      = None;
        self.preview_loading    = false;
    }

    /// Returns the preview kind for the currently-open file's extension,
    /// or `None` if previewing isn't applicable.
    pub fn preview_kind(&self) -> Option<PreviewKind> {
        let ext = self.path.as_ref()?.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" => Some(PreviewKind::Markdown),
            "html" | "htm"    => Some(PreviewKind::Html),
            _ => None,
        }
    }
}