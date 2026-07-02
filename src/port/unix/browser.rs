use super::Sys;
use crate::port::api::Browser;

impl Browser for Sys {
    fn chromium_binaries() -> Vec<String> {
        ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"]
            .into_iter()
            .map(String::from)
            .collect()
    }
}
