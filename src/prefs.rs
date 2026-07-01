//! User preferences persisted as JSON at `$HOME/.frms/prefs.json`.
//! Designed to grow — add fields here, give them a sensible default in
//! `Default`, and they'll round-trip automatically.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::theme::{FontScale, Mode, Palette, TerminalFontScale, ThemeColors};

#[derive(Debug, Clone)]
pub struct Prefs {
    /// Default working directory for new sessions. When set, the new-session
    /// dialog opens with the picker rooted here instead of `$HOME`.
    pub dev_dir:             Option<PathBuf>,
    pub palette:             Palette,
    pub mode:                Mode,
    pub font_scale:          FontScale,
    pub terminal_font_scale: TerminalFontScale,
    /// Share anonymous usage telemetry ([`crate::telemetry`]). On by default;
    /// the user can turn it off in the Profile tab. Hand-written `Default`
    /// below keeps this `true` (a derived `Default` would make it `false`, so a
    /// missing/corrupt prefs file would silently opt out).
    pub telemetry:           bool,
    /// Whether the user has seen the first-run telemetry notice. Until this is
    /// `true`, the notice modal is shown and **no telemetry is sent** — consent
    /// before collection.
    pub telemetry_notice_ack: bool,
    /// Whether the user has accepted the Non-Disclosure Agreement. Until `true`,
    /// the first-run NDA modal blocks the app (declining exits). This is the
    /// cross-format acceptance that replaced the deb-only debconf gate.
    pub nda_accepted:        bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            dev_dir:             None,
            palette:             Palette::default(),
            mode:                Mode::default(),
            font_scale:          FontScale::default(),
            terminal_font_scale: TerminalFontScale::default(),
            telemetry:           true,
            telemetry_notice_ack: false,
            nda_accepted:        false,
        }
    }
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
            // Absent key → on (opt-out, not opt-in).
            telemetry: v.get("telemetry").and_then(Value::as_bool).unwrap_or(true),
            // Absent key → not yet acknowledged (show the first-run notice).
            telemetry_notice_ack: v.get("telemetry_notice_ack").and_then(Value::as_bool).unwrap_or(false),
            // Absent key → not yet accepted (show the NDA gate).
            nda_accepted: v.get("nda_accepted").and_then(Value::as_bool).unwrap_or(false),
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
            "telemetry":           self.telemetry,
            "telemetry_notice_ack": self.telemetry_notice_ack,
            "nda_accepted":        self.nda_accepted,
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
