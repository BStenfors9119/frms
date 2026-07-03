//! Windows receiver copy over SSH.
//!
//! There's no `sshpass` on Windows, and native OpenSSH deliberately refuses to
//! take a password non-interactively — so password-based scp needs PuTTY's
//! `pscp`, which accepts `-pw`. When PuTTY isn't installed we return a clear,
//! actionable error rather than a cryptic "program not found".

use super::Sys;
use crate::port::api::{CopyCommand, Transfer};
use std::path::PathBuf;

impl Transfer for Sys {
    fn scp_command(
        _host: &str,
        _user: &str,
        password: &str,
        port: u16,
        files: &[PathBuf],
        dest: &str,
    ) -> Result<CopyCommand, String> {
        if !crate::port::shell::on_path("pscp") {
            return Err(
                "Copying to receivers over SSH needs PuTTY's pscp on Windows \
                 (there is no sshpass). Install PuTTY and ensure pscp.exe is on \
                 your PATH, or set up key-based SSH auth."
                    .to_string(),
            );
        }

        // `pscp -pw <password> -batch -P <port> <files…> user@host:dest`.
        // `-batch` disables interactive prompts (fails instead of hanging on an
        // unknown host key). Note: unlike sshpass, `-pw` puts the password on the
        // argv — PuTTY's only non-interactive option.
        let mut args = vec![
            "-pw".to_string(),
            password.to_string(),
            "-batch".to_string(),
            "-P".to_string(),
            port.to_string(),
        ];
        for f in files {
            args.push(f.to_string_lossy().into_owned());
        }
        args.push(dest.to_string());

        Ok(CopyCommand {
            program: "pscp".to_string(),
            args,
            envs: Vec::new(),
        })
    }
}
