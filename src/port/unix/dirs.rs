use super::Sys;
use crate::port::api::Dirs;
use std::path::PathBuf;

impl Dirs for Sys {
    fn home() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    }

    fn config() -> Option<PathBuf> {
        Self::home().map(|h| h.join(".frms"))
    }

    fn cache() -> Option<PathBuf> {
        Self::home().map(|h| h.join(".cache").join("frms"))
    }
}
