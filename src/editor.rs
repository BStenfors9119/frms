use std::fs;
use std::path::PathBuf;

use iced::widget::text_editor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    Markdown,
    Html,
    /// A raster image. Unlike the other kinds this one has no code view — the
    /// buffer would be binary garbage — so image files are preview-only.
    Image,
}

/// Extensions rendered by `PreviewKind::Image`. Limited to what the `image`
/// crate decodes with the features iced pulls in (`image/default`); SVG is
/// deliberately absent, as it needs iced's separate `svg` widget.
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "tif", "tiff", "webp", "qoi",
];

fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub struct EditorState {
    pub path: Option<PathBuf>,
    pub content: text_editor::Content,
    /// Unsaved-changes flag — set on edits, cleared on open/save.
    pub dirty: bool,
    /// Last save failure, shown in the header until the next successful save.
    pub save_error: Option<String>,
    pub preview_active: bool,
    /// HTML preview render state — populated by an async headless render.
    pub preview_loading: bool,
    pub preview_error:   Option<String>,
    /// The rendered preview currently on screen: an open image file's bytes, or
    /// the screenshot an HTML render produced.
    ///
    /// Held as a `Handle` rather than a `Vec<u8>` so it is built exactly once —
    /// at open, or when a render lands. `Handle::from_bytes` stamps a fresh
    /// `Id::unique()` on every call, so building it per-frame in `view()` would
    /// defeat the renderer's texture cache and re-decode plus re-upload the
    /// image on every redraw.
    pub preview_handle: Option<iced::widget::image::Handle>,
    /// Path `preview_handle` was built for (used to detect staleness).
    pub preview_image_path: Option<PathBuf>,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            path: None,
            content: text_editor::Content::new(),
            dirty: false,
            save_error: None,
            preview_active: false,
            preview_loading: false,
            preview_error:   None,
            preview_handle:  None,
            preview_image_path: None,
        }
    }

    pub fn open(&mut self, path: PathBuf) {
        self.dirty = false;
        self.save_error = None;
        // Opening a different file invalidates any cached preview.
        self.preview_handle     = None;
        self.preview_image_path = None;
        self.preview_error      = None;
        self.preview_loading    = false;

        if is_image_path(&path) {
            // Images have no text buffer — `read_to_string` on a PNG would only
            // put a UTF-8 error in the editor. Load the bytes for the preview
            // instead. Reading them here (rather than handing the renderer a
            // path-backed handle) means a file edited on disk shows fresh on
            // reopen, since iced keys its texture cache on the path.
            self.content = text_editor::Content::new();
            match fs::read(&path) {
                Ok(bytes) => {
                    self.preview_handle = Some(iced::widget::image::Handle::from_bytes(bytes));
                    self.preview_image_path = Some(path.clone());
                }
                Err(err) => self.preview_error = Some(err.to_string()),
            }
            // `preview_active` is left as-is: images render preview-only
            // regardless of it, and forcing it true here would leak into the
            // next .md/.html file opened, showing it rendered when the user
            // last had the code view.
            self.path = Some(path);
            return;
        }

        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| format!("// Could not read file: {e}"));
        self.content = text_editor::Content::with_text(&text);
        self.path = Some(path);
    }

    /// Write the buffer back to its file. Clears the dirty flag on success and
    /// records the error otherwise. A no-op when no file is open.
    pub fn save(&mut self) {
        // Preview-only files (images) have an empty buffer that is not their
        // contents — writing it would truncate the file. The UI hides Save for
        // them; this guards any other caller.
        if self.is_preview_only() { return }
        let Some(path) = &self.path else { return };
        match fs::write(path, self.content.text()) {
            Ok(())   => { self.dirty = false; self.save_error = None; }
            Err(err) => { self.save_error = Some(err.to_string()); }
        }
    }

    /// Returns the preview kind for the currently-open file's extension,
    /// or `None` if previewing isn't applicable.
    pub fn preview_kind(&self) -> Option<PreviewKind> {
        let ext = self.path.as_ref()?.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" => Some(PreviewKind::Markdown),
            "html" | "htm"    => Some(PreviewKind::Html),
            e if IMAGE_EXTS.contains(&e) => Some(PreviewKind::Image),
            _ => None,
        }
    }

    /// True when the open file is shown as a preview with no code view, and so
    /// has no editable buffer to save.
    pub fn is_preview_only(&self) -> bool {
        self.preview_kind() == Some(PreviewKind::Image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1 PNG — smallest thing that is genuinely non-UTF-8 on disk.
    const TINY_PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a,
        0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
        0x89,
    ];

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("frms-editor-test-{name}"));
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn image_extensions_map_to_image_preview() {
        for name in ["a.png", "a.JPG", "a.jpeg", "a.gif", "a.webp"] {
            let mut e = EditorState::new();
            e.path = Some(PathBuf::from(name));
            assert_eq!(e.preview_kind(), Some(PreviewKind::Image), "{name}");
            assert!(e.is_preview_only(), "{name}");
        }
        // SVG needs iced's separate svg widget, so it stays a text file.
        let mut e = EditorState::new();
        e.path = Some(PathBuf::from("a.svg"));
        assert_eq!(e.preview_kind(), None);
    }

    #[test]
    fn opening_an_image_loads_a_handle_not_a_text_buffer() {
        let path = temp_file("open.png", TINY_PNG);
        let mut e = EditorState::new();
        e.open(path.clone());

        assert!(e.preview_handle.is_some());
        assert!(e.preview_error.is_none());
        // The old read_to_string path left a UTF-8 error in the buffer.
        assert_eq!(e.content.text().trim(), "");
        assert!(!e.dirty);
        fs::remove_file(path).ok();
    }

    #[test]
    fn saving_an_open_image_never_truncates_it() {
        let path = temp_file("save.png", TINY_PNG);
        let mut e = EditorState::new();
        e.open(path.clone());
        // Force the state the UI refuses to produce: a dirty preview-only file.
        e.dirty = true;
        e.save();

        assert_eq!(fs::read(&path).unwrap(), TINY_PNG);
        fs::remove_file(path).ok();
    }

    #[test]
    fn opening_an_image_leaves_preview_active_alone() {
        // Otherwise the next .md opened would show rendered instead of code.
        let path = temp_file("leak.png", TINY_PNG);
        let mut e = EditorState::new();
        e.preview_active = false;
        e.open(path.clone());
        assert!(!e.preview_active);
        fs::remove_file(path).ok();
    }

    #[test]
    fn text_files_still_open_as_text() {
        let path = temp_file("note.md", b"# hello");
        let mut e = EditorState::new();
        e.open(path.clone());
        assert_eq!(e.content.text().trim(), "# hello");
        assert_eq!(e.preview_kind(), Some(PreviewKind::Markdown));
        assert!(e.preview_handle.is_none());
        fs::remove_file(path).ok();
    }
}