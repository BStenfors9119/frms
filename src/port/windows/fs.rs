use super::Sys;
use crate::port::api::Fs;
use std::path::Path;

impl Fs for Sys {
    fn restrict_to_owner(_path: &Path) {
        // Files under %APPDATA% / %LOCALAPPDATA% inherit the user-profile ACL,
        // which already excludes other standard users, so there's nothing to do
        // for baseline safety. Stripping inherited ACEs to lock the file to the
        // current user alone (via `icacls`) is a possible future refinement.
    }
}
