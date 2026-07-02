use super::Sys;
use crate::port::api::Dirs;
use std::path::PathBuf;

impl Dirs for Sys {
    fn home() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    }

    fn config() -> Option<PathBuf> {
        // Roaming profile — mirrors `$HOME/.frms` as "user config that follows
        // the user". Fall back to constructing it from the home dir if the env
        // var is missing (rare, but keeps behaviour predictable).
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| Self::home().map(|h| h.join("AppData").join("Roaming")))
            .map(|p| p.join("frms"))
    }

    fn cache() -> Option<PathBuf> {
        // Local (non-roaming) profile for regenerable cache data.
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| Self::home().map(|h| h.join("AppData").join("Local")))
            .map(|p| p.join("frms").join("cache"))
    }
}
