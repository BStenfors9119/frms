use super::Sys;
use crate::port::api::Browser;
use std::path::PathBuf;

impl Browser for Sys {
    fn chromium_binaries() -> Vec<String> {
        // Edge ships with Windows and is Chromium-based, so it's the primary
        // target; Chrome is tried if present. Neither is on PATH by default, so
        // probe the standard install roots, then fall back to bare names in case
        // the user put one on PATH.
        let mut out = Vec::new();

        let roots = ["ProgramFiles(x86)", "ProgramFiles", "LocalAppData"];
        let candidates = [
            ["Microsoft", "Edge", "Application", "msedge.exe"],
            ["Google", "Chrome", "Application", "chrome.exe"],
        ];
        for root in roots {
            if let Some(base) = std::env::var_os(root) {
                for parts in &candidates {
                    let mut p = PathBuf::from(&base);
                    for part in parts {
                        p.push(part);
                    }
                    if let Some(s) = p.to_str() {
                        out.push(s.to_string());
                    }
                }
            }
        }
        out.push("msedge".to_string());
        out.push("chrome".to_string());
        out
    }
}
