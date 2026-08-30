//! User preferences persisted as JSON at `$HOME/.frms/prefs.json`.
//! Designed to grow — add fields here, give them a sensible default in
//! `Default`, and they'll round-trip automatically.

use std::path::{Path, PathBuf};

use iced::Color;
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
    /// Share the *optional* tier of anonymous telemetry ([`crate::telemetry`]) —
    /// feature usage, errors, crash reports. On by default; the user can turn it
    /// off in the Profile tab. It does not cover the essential install/launch
    /// counts, which always report. Hand-written `Default` below keeps this
    /// `true` (a derived `Default` would make it `false`, so a missing/corrupt
    /// prefs file would silently opt out).
    pub telemetry:           bool,
    /// Whether the user has seen the first-run telemetry notice. Until this is
    /// `true` the notice modal is shown, so the disclosure lands before any
    /// optional data is collected.
    pub telemetry_notice_ack: bool,
    /// Whether the user has accepted the Non-Disclosure Agreement. Until `true`,
    /// the first-run NDA modal blocks the app (declining exits). This is the
    /// cross-format acceptance that replaced the deb-only debconf gate.
    pub nda_accepted:        bool,
    /// Highlight color for "a container of other things" — git-submodule
    /// directories in the file browser and note categories in the Notes plugin.
    /// User-chosen in the Profile tab; defaults to the built-in violet.
    pub submodule_color:     Color,
    /// Which AI CLI a new Build agent pane uses by default — the quick "New
    /// Agent" shortcut launches this tool, and the "+ Agent" picker lists it
    /// first. Chosen in the Profile tab; defaults to Claude Code.
    pub default_tool:        crate::agent::AiTool,
    /// When true, the "+ Agent" picker hides tools that aren't on `PATH`, so the
    /// user only sees CLIs they actually have installed. Off by default (all
    /// presets shown); flipping it on is the opt-in PATH auto-detect. Falls back
    /// to showing all tools if nothing is installed, so the picker never empties.
    pub filter_tools_to_installed: bool,
    /// How much of the project the native chat panes (Research/Chat) may touch.
    /// They run the same Claude Code harness as a Build pane, in the session's
    /// working directory, so this is the ceiling on what that harness is handed:
    /// read-only, read plus file edits, or the full set including the shell
    /// (the default — a chat pane is expected to do the work, not just describe
    /// it). Chosen in the Profile tab; see [`crate::chat_api::ChatAccess`].
    pub chat_access:         crate::chat_api::ChatAccess,
    /// Short `ls` options automatically applied in Terminals-tab shells, as a
    /// canonically-ordered subset of [`LS_FLAGS`] (e.g. `"lha"`). Each new shell
    /// aliases `ls` to `ls -<flags>`; empty leaves `ls` untouched. Always read
    /// and written through [`Prefs::ls_flag`] / [`Prefs::set_ls_flag`] so the
    /// order can't drift and make the same set of checkboxes round-trip
    /// differently.
    pub ls_flags:            String,
    /// Whether this machine is an immutable/atomic OS where day-to-day work
    /// happens inside containers — Fedora Silverblue's `toolbox`, or
    /// `distrobox` anywhere. When on, a pinned shell terminal remembers the
    /// container it was left in and re-enters it on the next launch, and the
    /// Terminals tab badges whichever container each shell is currently inside.
    /// When off, none of that surfaces and shells always come back on the host.
    ///
    /// Defaults to whether either tool is on `PATH` (see
    /// [`container_tool_installed`]), so Silverblue users get it without
    /// configuring anything and users on a conventional distro never see it —
    /// overridable in either direction from the Profile tab.
    pub immutable_system:    bool,
    /// Path to the service-account key the proxy authenticates with
    /// (`auth.json`). Empty means the proxy falls back to Application Default
    /// Credentials. Stored here, not in a script, so it is never hard-coded.
    pub cloud_sql_credentials: String,
}

/// True when a container tool frms knows how to re-enter is installed. Only
/// ever consulted for the *default* of [`Prefs::immutable_system`] — once the
/// user has saved prefs, their explicit choice wins whatever is on `PATH`.
fn container_tool_installed() -> bool {
    crate::port::shell::on_path("toolbox") || crate::port::shell::on_path("distrobox")
}

/// The `ls` short options the Profile tab offers, in the order they're shown
/// and assembled into the alias. `ls` treats these as an unordered set, so
/// fixing the order here is purely so a given set of checkboxes always produces
/// the same string in `prefs.json`.
pub const LS_FLAGS: [(char, &str); 5] = [
    ('l', "long listing"),
    ('h', "human-readable sizes"),
    ('a', "include dotfiles"),
    ('t', "newest first"),
    ('r', "reverse order"),
];

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
            submodule_color:     crate::ui::buttons::SUBMODULE,
            default_tool:        crate::agent::AiTool::default(),
            filter_tools_to_installed: false,
            chat_access:         crate::chat_api::ChatAccess::default(),
            ls_flags:            String::new(),
            immutable_system:    container_tool_installed(),
            cloud_sql_credentials: String::new(),
        }
    }
}

/// Keep only recognised flags, in [`LS_FLAGS`] order, without repeats. Applied
/// on load as well as on every toggle, so a hand-edited `prefs.json` can't feed
/// arbitrary text into the alias the shells run.
fn normalize_ls_flags(raw: &str) -> String {
    LS_FLAGS.iter()
        .map(|(f, _)| *f)
        .filter(|f| raw.contains(*f))
        .collect()
}

/// Parse a `#RRGGBB` (or bare `RRGGBB`) hex string into a color. Returns `None`
/// on any malformed input so callers can fall back to the default.
fn color_from_hex(s: &str) -> Option<Color> {
    let s = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::from_rgb8(r, g, b))
}

/// Format a color as an uppercase `#RRGGBB` hex string (alpha dropped —
/// submodule highlight colors are always opaque).
fn color_to_hex(c: Color) -> String {
    let ch = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", ch(c.r), ch(c.g), ch(c.b))
}

fn prefs_path() -> Option<PathBuf> {
    Some(crate::port::dirs::config()?.join("prefs.json"))
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
            // Absent/invalid → the built-in violet default.
            submodule_color: v.get("submodule_color")
                .and_then(Value::as_str)
                .and_then(color_from_hex)
                .unwrap_or(crate::ui::buttons::SUBMODULE),
            // Absent/unknown → Claude Code (the original single tool).
            default_tool: v.get("default_tool")
                .and_then(Value::as_str)
                .and_then(crate::agent::AiTool::from_str)
                .unwrap_or_default(),
            // Absent → off (show every preset, no PATH filtering).
            filter_tools_to_installed: v.get("filter_tools_to_installed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // Absent/unknown → full access, matching a Build pane's reach.
            chat_access: v.get("chat_access")
                .and_then(Value::as_str)
                .and_then(crate::chat_api::ChatAccess::from_str)
                .unwrap_or_default(),
            // Absent → no flags, i.e. `ls` is left exactly as the shell has it.
            ls_flags: v.get("ls_flags")
                .and_then(Value::as_str)
                .map(normalize_ls_flags)
                .unwrap_or_default(),
            // Absent → fall back to detection, so an existing prefs file written
            // before this key still lights the feature up on Silverblue.
            immutable_system: v.get("immutable_system")
                .and_then(Value::as_bool)
                .unwrap_or_else(container_tool_installed),
            cloud_sql_credentials: v.get("cloud_sql_credentials")
                .and_then(Value::as_str)
                .map(str::to_string)
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
            "telemetry":           self.telemetry,
            "telemetry_notice_ack": self.telemetry_notice_ack,
            "nda_accepted":        self.nda_accepted,
            "submodule_color":     color_to_hex(self.submodule_color),
            "default_tool":        self.default_tool.as_str(),
            "filter_tools_to_installed": self.filter_tools_to_installed,
            "chat_access":         self.chat_access.as_str(),
            "ls_flags":            self.ls_flags,
            "immutable_system":    self.immutable_system,
            "cloud_sql_credentials": self.cloud_sql_credentials,
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
        crate::port::dirs::home().unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn colors(&self) -> ThemeColors {
        ThemeColors::for_theme(self.palette, self.mode)
    }

    /// Whether the `ls` short option `flag` is switched on.
    pub fn ls_flag(&self, flag: char) -> bool {
        self.ls_flags.contains(flag)
    }

    /// Switch an `ls` short option on or off, re-normalising so the stored
    /// string stays in [`LS_FLAGS`] order.
    pub fn set_ls_flag(&mut self, flag: char, on: bool) {
        let mut raw: String = self.ls_flags.chars().filter(|c| *c != flag).collect();
        if on {
            raw.push(flag);
        }
        self.ls_flags = normalize_ls_flags(&raw);
    }

    /// The command a new shell should alias `ls` to — `ls -lha` and friends —
    /// or `None` when no flags are picked and `ls` should be left alone.
    pub fn ls_command(&self) -> Option<String> {
        if self.ls_flags.is_empty() {
            return None;
        }
        Some(format!("ls -{}", self.ls_flags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_flags_stay_in_canonical_order_however_they_were_toggled() {
        let mut p = Prefs::default();
        assert_eq!(p.ls_command(), None, "no flags → `ls` is left alone");

        // Toggled in reverse of the display order…
        for f in ['a', 'h', 'l'] {
            p.set_ls_flag(f, true);
        }
        assert_eq!(p.ls_flags, "lha");
        assert_eq!(p.ls_command().unwrap(), "ls -lha");

        // …and toggling one off leaves the rest in order.
        p.set_ls_flag('h', false);
        assert_eq!(p.ls_flags, "la");
        assert!(!p.ls_flag('h'));
    }

    #[test]
    fn hand_edited_flags_are_filtered_to_the_offered_set() {
        // Anything not in LS_FLAGS is dropped rather than reaching the alias.
        assert_eq!(normalize_ls_flags("l; rm -rf /"), "lr");
        assert_eq!(normalize_ls_flags("llll"), "l");
        assert_eq!(normalize_ls_flags(""), "");
    }
}
