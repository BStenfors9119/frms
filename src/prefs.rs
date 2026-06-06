//! User preferences persisted as JSON at `$HOME/.frms/prefs.json`.
//! Designed to grow — add fields here, give them a sensible default in
//! `Default`, and they'll round-trip automatically.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::theme::{FontScale, Mode, Palette, TerminalFontScale, ThemeColors};

#[derive(Debug, Clone, Default)]
pub struct Prefs {
    /// Default working directory for new sessions. When set, the new-session
    /// dialog opens with the picker rooted here instead of `$HOME`.
    pub dev_dir:             Option<PathBuf>,
    pub palette:             Palette,
    pub mode:                Mode,
    pub font_scale:          FontScale,
    pub terminal_font_scale: TerminalFontScale,
}

fn prefs_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".frms").join("prefs.json"))
}

impl Prefs {
    pub fn load() -> Self {
        let Some(path)  = prefs_path()                               else { return Self::default(); };
        let Ok(bytes)   = std::fs::read(&path)                       else { return Self::default(); };
        let Ok(v)       = serde_json::from_slice::<Value>(&bytes)    else { return Self::default(); };

        Self {
            dev_dir: v.get("dev_dir")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            palette: v.get("palette")
                .and_then(Value::as_str)
                .and_then(Palette::from_str)
                .unwrap_or_default(),
            mode: v.get("mode")
                .and_then(Value::as_str)
                .and_then(Mode::from_str)
                .unwrap_or_default(),
            font_scale: v.get("font_scale")
                .and_then(Value::as_str)
                .and_then(FontScale::from_str)
                .unwrap_or_default(),
            terminal_font_scale: v.get("terminal_font_scale")
                .and_then(Value::as_str)
                .and_then(TerminalFontScale::from_str)
                .unwrap_or_default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = prefs_path() else { return; };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let v = json!({
            "dev_dir":             self.dev_dir.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "palette":             self.palette.as_str(),
            "mode":                self.mode.as_str(),
            "font_scale":          self.font_scale.as_str(),
            "terminal_font_scale": self.terminal_font_scale.as_str(),
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
            let _ = std::fs::write(&path, bytes);
        }
    }

    /// Resolve the directory the new-session picker should open in.
    /// Falls back to `$HOME`, then `.`, if `dev_dir` is unset or invalid.
    pub fn new_session_start_dir(&self) -> PathBuf {
        if let Some(d) = self.dev_dir.as_deref() {
            if Path::new(d).is_dir() {
                return d.to_path_buf();
            }
        }
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn colors(&self) -> ThemeColors {
        ThemeColors::for_theme(self.palette, self.mode)
    }
}
