use super::Sys;
use crate::port::api::Shell;

impl Shell for Sys {
    fn login_shell() -> (String, Vec<String>) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (shell, Vec::new())
    }

    fn guidance_shell(lines: &[&str]) -> (String, Vec<String>) {
        // Single-quote a string for safe inclusion in a `sh -c` script: wrap in
        // quotes and replace each embedded quote with the '\'' idiom.
        fn sq(s: &str) -> String {
            format!("'{}'", s.replace('\'', "'\\''"))
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let script = format!(
            "printf '%s\\n' {}; exec {}",
            lines.iter().map(|l| sq(l)).collect::<Vec<_>>().join(" "),
            sq(&shell),
        );
        ("/bin/sh".to_string(), vec!["-c".to_string(), script])
    }

    fn terminal_env() -> Vec<(String, String)> {
        // CommandBuilder inherits the parent env verbatim and sets no TERM of
        // its own. When the IDE is launched from a desktop entry (rather than a
        // shell), TERM is unset — children then assume a dumb terminal: no
        // colour anywhere, and terminfo users like `clear` fail outright. So
        // advertise the same xterm flavor children saw when the IDE was run
        // from a terminal during development.
        vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ]
    }

    fn on_path(cmd: &str) -> bool {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths)
                .any(|dir| std::fs::metadata(dir.join(cmd)).map(|m| m.is_file()).unwrap_or(false))
        })
    }
}
