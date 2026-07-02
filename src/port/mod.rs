//! Platform port layer — the single home of every `cfg(target_os)` /
//! `cfg(unix | windows)` in frms.
//!
//! The two `#[cfg_attr]` lines below select the OS implementation module. A
//! platform `cfg` may appear *only within `src/port/`* — the OS-picking lines
//! here, plus the occasional finer distinction inside an OS module (e.g. a
//! linux-vs-macos split within `unix/`). Any `cfg(unix)`/`cfg(windows)`/
//! `cfg(target_os = …)` *outside* `src/port/` is a review failure — add a seam
//! here instead.
//!
//! Everything else in the crate calls `port::…` and stays platform-agnostic.
//! See [`api`] for the contract each OS module implements.

mod api;

#[cfg_attr(unix, path = "unix/mod.rs")]
#[cfg_attr(windows, path = "windows/mod.rs")]
mod imp;

/// Filesystem locations for frms' data. Neutral wrappers over the selected OS
/// impl so callers write `port::dirs::config()` with no `cfg` in sight.
pub mod dirs {
    use super::api::Dirs;
    use super::imp::Sys;
    use std::path::PathBuf;

    /// The user's home directory (`$HOME` / `%USERPROFILE%`).
    pub fn home() -> Option<PathBuf> {
        <Sys as Dirs>::home()
    }
    /// frms' config dir (`$HOME/.frms` / `%APPDATA%\frms`). Not auto-created.
    pub fn config() -> Option<PathBuf> {
        <Sys as Dirs>::config()
    }
    /// frms' cache dir (`$HOME/.cache/frms` / `%LOCALAPPDATA%\frms`). Not auto-created.
    pub fn cache() -> Option<PathBuf> {
        <Sys as Dirs>::cache()
    }
}

/// Spawning shells and locating executables. Neutral wrappers over the selected
/// OS impl so callers never spell out a shell path or `cfg`.
pub mod shell {
    use super::api::Shell;
    use super::imp::Sys;

    /// The interactive login shell as `(program, args)`.
    pub fn login_shell() -> (String, Vec<String>) {
        <Sys as Shell>::login_shell()
    }
    /// A `(program, args)` that prints `lines` then drops into a shell.
    pub fn guidance_shell(lines: &[&str]) -> (String, Vec<String>) {
        <Sys as Shell>::guidance_shell(lines)
    }
    /// Extra env vars every PTY child should inherit (`TERM`/`COLORTERM` on unix).
    pub fn terminal_env() -> Vec<(String, String)> {
        <Sys as Shell>::terminal_env()
    }
    /// Whether `cmd` resolves to an executable on `PATH` (honours `PATHEXT`).
    pub fn on_path(cmd: &str) -> bool {
        <Sys as Shell>::on_path(cmd)
    }
}

/// System clipboard access, off the UI thread. Wraps the selected OS impl.
pub mod clipboard {
    use super::api::Clipboard;
    use super::imp::Sys;

    /// Read the clipboard, or `None` when empty / unavailable.
    pub async fn read() -> Option<String> {
        <Sys as Clipboard>::read().await
    }
    /// Write `text` to the clipboard.
    pub async fn write(text: String) {
        <Sys as Clipboard>::write(text).await
    }
}

/// Filesystem operations whose mechanism differs by platform.
pub mod fs {
    use super::api::Fs;
    use super::imp::Sys;
    use std::path::Path;

    /// Restrict `path` to the owning user (credentials files). See [`Fs`](super::api::Fs).
    pub fn restrict_to_owner(path: &Path) {
        <Sys as Fs>::restrict_to_owner(path)
    }
}

/// Platform-specific window configuration. Neutral wrapper over the OS impl.
pub mod window {
    use super::api::Window;
    use super::imp::Sys;

    /// Apply platform tweaks to the iced window settings at startup.
    pub fn configure(settings: &mut iced::window::Settings) {
        <Sys as Window>::configure(settings)
    }
}
