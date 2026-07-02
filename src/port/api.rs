//! The platform contract.
//!
//! Each OS module (`unix/`, `windows/`) implements every trait defined here on
//! its own [`Sys`](super) handle. Because the traits are shared, the compiler
//! rejects a Windows build that is missing a method the Unix side has (and
//! vice-versa) — the two platforms can never silently drift out of sync.
//!
//! This file, plus the two `#[cfg_attr]` lines in [`mod.rs`](super), is the
//! entire cross-platform surface of frms. Behaviour lives in the OS modules;
//! `cfg` lives only in `mod.rs`.

use std::path::PathBuf;

/// Filesystem locations for frms' per-user data.
///
/// These return directories, they do not create them — callers `create_dir_all`
/// as needed, matching the previous inline `$HOME/.frms` behaviour.
pub trait Dirs {
    /// The user's home directory: `$HOME` on unix, `%USERPROFILE%` on Windows.
    fn home() -> Option<PathBuf>;
    /// frms' config dir: `$HOME/.frms` on unix, `%APPDATA%\frms` on Windows.
    fn config() -> Option<PathBuf>;
    /// frms' cache dir: `$HOME/.cache/frms` on unix, `%LOCALAPPDATA%\frms` on Windows.
    fn cache() -> Option<PathBuf>;
}

/// Spawning shells and locating executables — everything about *how* frms runs
/// child programs in its PTYs.
pub trait Shell {
    /// The interactive login shell as `(program, args)`. `$SHELL`/`/bin/sh` on
    /// unix; `%ComSpec%`/`cmd.exe` on Windows. Args are usually empty but exist
    /// so a shell that needs flags (e.g. PowerShell) can supply them.
    fn login_shell() -> (String, Vec<String>);

    /// A `(program, args)` that prints each line of `lines`, then hands the
    /// user an interactive shell. Used for the agent-pane fallback shown when
    /// the `claude` CLI isn't installed. Each platform builds this in its own
    /// shell language (`sh -c 'printf …; exec …'` vs `cmd /c "echo …& cmd"`).
    fn guidance_shell(lines: &[&str]) -> (String, Vec<String>);

    /// Extra environment variables every PTY child should inherit. `TERM` /
    /// `COLORTERM` on unix so children emit colour; empty on Windows, where the
    /// ConPTY provides the capability and native consoles ignore `TERM`.
    fn terminal_env() -> Vec<(String, String)>;

    /// Whether `cmd` resolves to an executable on `PATH`. Honours `PATHEXT` on
    /// Windows (so `claude` matches `claude.cmd`/`claude.exe`).
    fn on_path(cmd: &str) -> bool;
}

/// System clipboard access, off the UI thread.
///
/// frms deliberately shells out rather than using iced's built-in clipboard
/// (see `unix/clipboard.rs` for the Wayland deadlock that forced this); the
/// Windows impl follows the same shell-out shape via `clip.exe` / `Get-Clipboard`.
#[allow(async_fn_in_trait)] // crate-internal trait, only ever called directly
pub trait Clipboard {
    /// Read the clipboard, or `None` when empty / no backend is available.
    async fn read() -> Option<String>;
    /// Write `text` to the clipboard.
    async fn write(text: String);
}

/// Filesystem operations whose mechanism differs by platform.
pub trait Fs {
    /// Restrict `path` so only the owning user can read it. Used for files that
    /// hold plaintext credentials. `chmod 600` on unix; on Windows a no-op today
    /// (profile-directory ACLs already exclude other standard users).
    fn restrict_to_owner(path: &std::path::Path);
}

/// Platform-specific window configuration applied at startup.
pub trait Window {
    /// Apply any platform tweaks to the iced window settings. On Linux this
    /// sets the Wayland app_id / X11 `WM_CLASS` so the window binds to its
    /// `.desktop` launcher; a no-op on other platforms.
    fn configure(settings: &mut iced::window::Settings);
}
