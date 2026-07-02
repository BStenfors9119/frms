use super::Sys;
use crate::port::api::Shell;

impl Shell for Sys {
    fn login_shell() -> (String, Vec<String>) {
        // %ComSpec% is cmd.exe on every supported Windows; fall back explicitly
        // just in case it's unset in a stripped environment.
        let shell = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        (shell, Vec::new())
    }

    fn guidance_shell(lines: &[&str]) -> (String, Vec<String>) {
        // Build `cmd /c "echo …& echo …& cmd"`: print each line, then hand the
        // user an interactive cmd session. `echo.` prints a blank line; the cmd
        // metacharacters below must be caret-escaped so a message containing
        // `&`, `|`, `<`, `>`, `^`, or parens doesn't break the command line.
        fn esc(s: &str) -> String {
            let mut out = String::with_capacity(s.len());
            for c in s.chars() {
                if "&<>|^()".contains(c) {
                    out.push('^');
                }
                out.push(c);
            }
            out
        }
        let echos = lines
            .iter()
            .map(|l| if l.is_empty() { "echo.".to_string() } else { format!("echo {}", esc(l)) })
            .collect::<Vec<_>>()
            .join(" & ");
        let script = format!("{echos} & cmd");
        ("cmd".to_string(), vec!["/c".to_string(), script])
    }

    fn terminal_env() -> Vec<(String, String)> {
        // The ConPTY exposes VT capability to children directly; native console
        // programs ignore TERM, so there's nothing useful to inject here.
        Vec::new()
    }

    fn on_path(cmd: &str) -> bool {
        // A bare `claude` on Windows is really `claude.cmd`/`claude.exe`, so we
        // must try each PATHEXT suffix, not just the literal name.
        let exts: Vec<String> = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let is_file = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false);

        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let base = dir.join(cmd);
                // The name may already carry an extension.
                if is_file(base.clone()) {
                    return true;
                }
                exts.iter().any(|ext| {
                    let mut p = base.clone().into_os_string();
                    p.push(ext);
                    is_file(std::path::PathBuf::from(p))
                })
            })
        })
    }
}
