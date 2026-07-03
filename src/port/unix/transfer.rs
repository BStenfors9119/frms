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
        // `sshpass -e scp …`: the password rides in $SSHPASS rather than the
        // argument list (which is world-readable via /proc).
        let mut args = vec![
            "-e".to_string(),
            "scp".to_string(),
            "-P".to_string(),
            port.to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
        ];
        for f in files {
            args.push(f.to_string_lossy().into_owned());
        }
        args.push(dest.to_string());

        Ok(CopyCommand {
            program: "sshpass".to_string(),
            args,
            envs: vec![("SSHPASS".to_string(), password.to_string())],
        })
    }
}
