//! Native Claude chat backend for `Research` / `Chat` agent panes.
//!
//! These panes are the conversational counterpart to a `Build` pane: the same
//! Claude Code harness with the same reach into the project, driven as a chat
//! rather than as a terminal. Rather than calling the Anthropic Messages API
//! directly (which would need a separately-managed API key), they run the
//! `claude` CLI headless:
//!
//! ```text
//! claude -p --model <model> --output-format stream-json --verbose \
//!        --include-partial-messages --tools <set> --permission-mode <mode> \
//!        [--resume <session_id>]
//! ```
//!
//! This reuses the CLI's own login — a Claude Pro/Max subscription, no API key
//! anywhere — so the single auth path for the whole app is `claude` itself
//! (install/sign-in via `frms-setup-claude`).
//!
//! **Tools and the working directory.** The run happens *in the session's
//! project directory* with a real tool set, so the pane can list directories,
//! read and edit files, and (at the widest setting) run commands — the same
//! things a Build pane does. How far that goes is the user's call, via
//! [`ChatAccess`]. Headless runs can't answer a permission prompt, so each level
//! also pins a `--permission-mode` that pre-approves exactly the tools it
//! grants; without that the CLI stalls the moment Claude reaches for a tool,
//! which is what "it hangs when it tries to do something" looks like.
//!
//! The prompt is the user's latest turn, written to the CLI on **stdin** (no
//! arg-length or shell-escaping concerns). Multi-turn continuity is the CLI's
//! job: the first turn omits `--resume`, captures the `session_id` from the
//! streamed events, and later turns pass `--resume <session_id>` so the server
//! holds the conversation — we never resend the transcript.
//!
//! [`stream_completion`] returns a `Stream` of app messages — `on_delta` for
//! each streamed text fragment, `on_activity` for each tool the model reaches
//! for (so a pane working through files doesn't look frozen), and a single
//! `on_end` carrying either the session id (on success) or an error string. It
//! is driven from `app::update` with `Task::run`, the same shape as the PTY
//! reader in [`crate::terminal::pty_subscription`] (closures keep this module
//! free of any dependency on `app::Message`).

use std::path::PathBuf;
use std::process::Stdio;

use iced::futures::channel::mpsc::Sender;
use iced::futures::{SinkExt, Stream};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::terminal::TerminalId;

// ── access level ────────────────────────────────────────────────────────────

/// How much of the project a chat pane's headless `claude` run may touch.
///
/// Each level pairs a `--tools` set with the `--permission-mode` that
/// pre-approves it. That pairing is the whole point: a headless run has no way
/// to answer an approval prompt, so a tool the mode doesn't cover doesn't just
/// get refused — the turn stalls on a question nobody can see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChatAccess {
    /// Look, don't touch: read files, search the tree, fetch the web. Nothing
    /// is written and no commands run.
    ReadOnly,
    /// Read plus file edits. Enough for the pane to study the project and make
    /// the change it just talked through, with the shell still off the table.
    ReadEdit,
    /// Everything Claude Code can do, `Bash` included — the same reach as a
    /// Build pane, minus the terminal. The default: a chat pane is expected to
    /// work the project, not just talk about it.
    #[default]
    Full,
}

impl ChatAccess {
    pub const ALL: &'static [ChatAccess] =
        &[ChatAccess::ReadOnly, ChatAccess::ReadEdit, ChatAccess::Full];

    /// Label for the Profile-tab picker.
    pub fn label(self) -> &'static str {
        match self {
            ChatAccess::ReadOnly => "Read only",
            ChatAccess::ReadEdit => "Read + edit files",
            ChatAccess::Full     => "Full (files + shell)",
        }
    }

    /// One line on what the level actually permits, shown under the picker.
    pub fn detail(self) -> &'static str {
        match self {
            ChatAccess::ReadOnly =>
                "Chat panes can read and search the project and fetch web pages, but never write.",
            ChatAccess::ReadEdit =>
                "Chat panes can read, search, and edit project files. No shell commands.",
            ChatAccess::Full =>
                "Chat panes can also run shell commands — the same reach as a Build pane.",
        }
    }

    /// Stable token for persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            ChatAccess::ReadOnly => "read_only",
            ChatAccess::ReadEdit => "read_edit",
            ChatAccess::Full     => "full",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "read_only" => Some(ChatAccess::ReadOnly),
            "read_edit" => Some(ChatAccess::ReadEdit),
            "full"      => Some(ChatAccess::Full),
            _           => None,
        }
    }

    /// Value for the CLI's `--tools` flag: named built-ins, or `default` for
    /// the whole set.
    fn tools(self) -> &'static str {
        match self {
            ChatAccess::ReadOnly =>
                "Read,Glob,Grep,WebFetch,WebSearch,TodoWrite,Task",
            ChatAccess::ReadEdit =>
                "Read,Glob,Grep,Write,Edit,NotebookEdit,WebFetch,WebSearch,TodoWrite,Task",
            ChatAccess::Full => "default",
        }
    }

    /// Value for `--permission-mode`. `acceptEdits` covers reads and file
    /// writes without prompting; anything that can reach the shell needs
    /// `bypassPermissions`, since nothing else pre-approves `Bash`.
    fn permission_mode(self) -> &'static str {
        match self {
            ChatAccess::ReadOnly | ChatAccess::ReadEdit => "acceptEdits",
            ChatAccess::Full                            => "bypassPermissions",
        }
    }
}

impl std::fmt::Display for ChatAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Stream a chat completion for pane `id` against `model`, sending the user's
/// latest `prompt`, run in `cwd` (the pane's project directory) at `access`.
/// When `resume` carries a prior `session_id`, the turn continues that
/// conversation. Emits `on_delta(id, chunk)` for each streamed text fragment,
/// `on_activity(id, line)` for each tool call the model makes, and exactly one
/// `on_end(id, result)` — `Ok(session_id)` when the run completes (so the
/// caller can resume it next turn) or `Err(message)` on failure.
pub fn stream_completion<M, D, A, E>(
    id:          TerminalId,
    model:       &'static str,
    prompt:      String,
    resume:      Option<String>,
    cwd:         Option<PathBuf>,
    access:      ChatAccess,
    on_delta:    D,
    on_activity: A,
    on_end:      E,
) -> impl Stream<Item = M>
where
    M: Send + 'static,
    D: Fn(TerminalId, String) -> M + Send + 'static,
    A: Fn(TerminalId, String) -> M + Send + 'static,
    E: Fn(TerminalId, Result<String, String>) -> M + Send + 'static,
{
    iced::stream::channel(256, move |mut output| async move {
        let result = run(
            id, model, prompt, resume, cwd, access,
            &on_delta, &on_activity, &mut output,
        ).await;
        let _ = output.send(on_end(id, result)).await;
    })
}

async fn run<M, D, A>(
    id:          TerminalId,
    model:       &str,
    prompt:      String,
    resume:      Option<String>,
    cwd:         Option<PathBuf>,
    access:      ChatAccess,
    on_delta:    &D,
    on_activity: &A,
    output:      &mut Sender<M>,
) -> Result<String, String>
where
    D: Fn(TerminalId, String) -> M,
    A: Fn(TerminalId, String) -> M,
{
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg("--model").arg(model)
        .arg("--output-format").arg("stream-json")
        .arg("--verbose")
        .arg("--include-partial-messages")
        // The tool set and the mode that pre-approves it travel together — see
        // `ChatAccess`. A tool the mode doesn't cover would stop the run on an
        // approval prompt no one can answer.
        .arg("--tools").arg(access.tools())
        .arg("--permission-mode").arg(access.permission_mode());
    if let Some(session) = &resume {
        cmd.arg("--resume").arg(session);
    }
    // Run in the pane's project directory: that's what makes its tools useful
    // (and loads the project's CLAUDE.md, exactly as a Build pane does). It's
    // fixed for the pane's lifetime, so `--resume` lands in the same place
    // every turn. Fall back to a temp dir for a pane with no project.
    cmd.current_dir(cwd.unwrap_or_else(std::env::temp_dir));

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run claude: {e} — is Claude Code installed? (run frms-setup-claude)"))?;

    // Feed the prompt on stdin, then close it so `claude -p` starts the request.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).await
            .map_err(|e| format!("claude stdin failed: {e}"))?;
        drop(stdin);
    }

    let stdout = child.stdout.take().ok_or_else(|| "claude produced no output".to_string())?;
    let mut lines = BufReader::new(stdout).lines();

    let mut session_id = String::new();
    let mut result_text = String::new();
    let mut saw_delta = false;
    let mut err_msg: Option<String> = None;

    // Output is newline-delimited JSON (one event per line).
    while let Some(line) = lines.next_line().await
        .map_err(|e| format!("stream read failed: {e}"))?
    {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };

        // Every event carries the session id; keep the latest so the caller can
        // resume the conversation on the next turn.
        if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
            if !sid.is_empty() {
                session_id = sid.to_string();
            }
        }

        match v.get("type").and_then(|t| t.as_str()) {
            // Streamed token: stream_event → content_block_delta → delta.text.
            Some("stream_event") => {
                let ev = v.get("event");
                if ev.and_then(|e| e.get("type")).and_then(|t| t.as_str())
                    == Some("content_block_delta")
                {
                    if let Some(text) = ev
                        .and_then(|e| e.get("delta"))
                        .and_then(|d| d.get("text"))
                        .and_then(|t| t.as_str())
                    {
                        saw_delta = true;
                        let _ = output.send(on_delta(id, text.to_string())).await;
                    }
                }
            }
            // A completed assistant message. Its text already arrived as
            // deltas, but its `tool_use` blocks are the only place the pane
            // learns what Claude is *doing* — without them a turn spent reading
            // files is a blank screen for a minute or more.
            Some("assistant") => {
                let blocks = v.get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array());
                for block in blocks.into_iter().flatten() {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                        continue;
                    }
                    let Some(name) = block.get("name").and_then(|n| n.as_str()) else { continue };
                    let line = match tool_summary(block.get("input")) {
                        Some(detail) => format!("{name}  {detail}"),
                        None         => name.to_string(),
                    };
                    let _ = output.send(on_activity(id, line)).await;
                }
            }
            // Terminal event: success carries the full reply in `result`; an
            // error sets `is_error` / a non-success `subtype`.
            Some("result") => {
                let is_error = v.get("is_error").and_then(|b| b.as_bool()).unwrap_or(false)
                    || v.get("subtype").and_then(|s| s.as_str()) != Some("success");
                if let Some(text) = v.get("result").and_then(|r| r.as_str()) {
                    result_text = text.to_string();
                }
                if is_error {
                    let msg = v.get("result").and_then(|r| r.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| v.get("error").and_then(|e| e.as_str()))
                        .unwrap_or("claude request failed")
                        .to_string();
                    err_msg = Some(msg);
                }
            }
            _ => {}
        }
    }

    if let Some(msg) = err_msg {
        return Err(msg);
    }

    // Fallback: if no partial deltas arrived but the run produced a final
    // answer, surface it as a single chunk so the pane still shows the reply.
    if !saw_delta && !result_text.is_empty() {
        let _ = output.send(on_delta(id, result_text)).await;
        saw_delta = true;
    }

    let status = child.wait().await.map_err(|e| format!("claude wait failed: {e}"))?;
    if !status.success() && !saw_delta {
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr).await;
        }
        let detail = stderr.trim();
        let detail = if detail.is_empty() { "claude request failed" } else { detail };
        return Err(detail.to_string());
    }

    Ok(session_id)
}

/// The one field of a tool's input worth showing beside its name — the file
/// being edited, the command being run, the thing being searched for. `None`
/// for tools whose input says nothing useful in one line, which then show as
/// just the tool name. Long values are clipped so an activity line stays a line.
fn tool_summary(input: Option<&serde_json::Value>) -> Option<String> {
    const KEYS: &[&str] = &[
        "file_path",   // Read / Write / Edit / NotebookEdit
        "command",     // Bash
        "pattern",     // Glob / Grep
        "url",         // WebFetch
        "query",       // WebSearch
        "description", // Task (subagent)
        "path",
    ];
    const MAX: usize = 72;

    let input = input?;
    let raw = KEYS.iter()
        .find_map(|k| input.get(k).and_then(|v| v.as_str()))?
        .trim();
    if raw.is_empty() {
        return None;
    }
    // Keep it to one line — a heredoc'd Bash command would otherwise smear
    // across the transcript.
    let one_line = raw.lines().next().unwrap_or(raw);
    Some(clip(one_line, MAX))
}

/// Truncate on a char boundary, marking the cut with an ellipsis.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The `--tools` value as the CLI reads it: exact names, so `TodoWrite`
    /// isn't mistaken for `Write`.
    fn granted(access: ChatAccess) -> Vec<&'static str> {
        access.tools().split(',').map(str::trim).collect()
    }

    #[test]
    fn access_levels_pair_tools_with_a_mode_that_pre_approves_them() {
        // Read-only grants nothing that writes to the project or shells out…
        let read_only = granted(ChatAccess::ReadOnly);
        for banned in ["Write", "Edit", "NotebookEdit", "Bash"] {
            assert!(!read_only.contains(&banned), "read-only must not grant {banned}");
        }
        assert!(read_only.contains(&"Read"), "read-only must still be able to read");

        // …read+edit adds the file writers but still no shell…
        let read_edit = granted(ChatAccess::ReadEdit);
        for granted_tool in ["Read", "Write", "Edit", "NotebookEdit"] {
            assert!(read_edit.contains(&granted_tool), "read+edit must grant {granted_tool}");
        }
        assert!(!read_edit.contains(&"Bash"), "read+edit must not reach the shell");
        // …and both are covered by acceptEdits, so nothing prompts.
        assert_eq!(ChatAccess::ReadOnly.permission_mode(), "acceptEdits");
        assert_eq!(ChatAccess::ReadEdit.permission_mode(), "acceptEdits");

        // Full opens the whole set — which only bypassPermissions pre-approves,
        // since acceptEdits would stop and ask before running a command.
        assert_eq!(ChatAccess::Full.tools(), "default");
        assert_eq!(ChatAccess::Full.permission_mode(), "bypassPermissions");
    }

    #[test]
    fn access_token_round_trips_and_defaults_to_full() {
        for &a in ChatAccess::ALL {
            assert_eq!(ChatAccess::from_str(a.as_str()), Some(a));
        }
        assert_eq!(ChatAccess::from_str("retired-level"), None);
        assert_eq!(ChatAccess::default(), ChatAccess::Full);
    }

    #[test]
    fn tool_summary_picks_the_telling_field() {
        assert_eq!(
            tool_summary(Some(&json!({"file_path": "src/app.rs", "offset": 40}))).as_deref(),
            Some("src/app.rs"),
        );
        assert_eq!(
            tool_summary(Some(&json!({"command": "cargo test", "description": "run tests"}))).as_deref(),
            Some("cargo test"),
        );
        // Nothing recognizable → the caller falls back to the bare tool name.
        assert_eq!(tool_summary(Some(&json!({"todos": []}))), None);
        assert_eq!(tool_summary(None), None);
    }

    #[test]
    fn tool_summary_stays_one_clipped_line() {
        let heredoc = json!({"command": "cat <<'EOF' > f\nline two\nEOF"});
        assert_eq!(tool_summary(Some(&heredoc)).as_deref(), Some("cat <<'EOF' > f"));

        let long = "x".repeat(200);
        let summary = tool_summary(Some(&json!({"file_path": long}))).unwrap();
        assert!(summary.chars().count() <= 72);
        assert!(summary.ends_with('…'));
    }
}
