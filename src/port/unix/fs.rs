use super::Sys;
use crate::port::api::Fs;
use std::path::Path;

impl Fs for Sys {
    fn restrict_to_owner(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}
