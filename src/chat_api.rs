//! Native Claude chat backend for `Research` / `Chat` agent panes.
//!
//! These panes are non-agentic question/answer chats. Rather than calling the
//! Anthropic Messages API directly (which would need a separately-managed API
//! key), they drive the `claude` (Claude Code) CLI in headless mode:
//!
//! ```text
//! claude -p --model <model> --output-format stream-json --verbose \
//!        --include-partial-messages --tools "" [--resume <session_id>]
//! ```
//!
//! This reuses the CLI's own login — a Claude Pro/Max subscription, no API key
//! anywhere — so the single auth path for the whole app is `claude` itself
//! (install/sign-in via `frms-setup-claude`). `--tools ""` strips every tool so
//! the run is pure Q&A (no file/Bash access), and the command runs in a neutral
//! working directory so a chat pane doesn't pull in the project's context.
//!
//! The prompt is the user's latest turn, written to the CLI on **stdin** (no
//! arg-length or shell-escaping concerns). Multi-turn continuity is the CLI's
//! job: the first turn omits `--resume`, captures the `session_id` from the
//! streamed events, and later turns pass `--resume <session_id>` so the server
//! holds the conversation — we never resend the transcript.
//!
//! [`stream_completion`] returns a `Stream` of app messages — `on_delta` for
//! each streamed text fragment and a single `on_end` carrying either the
//! session id (on success) or an error string. It is driven from `app::update`
//! with `Task::run`, the same shape as the PTY reader in
//! [`crate::terminal::pty_subscription`] (closures keep this module free of any
//! dependency on `app::Message`).

use std::process::Stdio;

use iced::futures::channel::mpsc::Sender;
use iced::futures::{SinkExt, Stream};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::terminal::TerminalId;

/// Stream a chat completion for pane `id` against `model`, sending the user's
/// latest `prompt`. When `resume` carries a prior `session_id`, the turn
/// continues that conversation. Emits `on_delta(id, chunk)` for each streamed
/// text fragment and exactly one `on_end(id, result)` — `Ok(session_id)` when
/// the run completes (so the caller can resume it next turn) or `Err(message)`
/// on failure.
pub fn stream_completion<M, D, E>(
    id:       TerminalId,
    model:    &'static str,
    prompt:   String,
    resume:   Option<String>,
    on_delta: D,
    on_end:   E,
) -> impl Stream<Item = M>
where
    M: Send + 'static,
    D: Fn(TerminalId, String) -> M + Send + 'static,
    E: Fn(TerminalId, Result<String, String>) -> M + Send + 'static,
{
    iced::stream::channel(256, move |mut output| async move {
        let result = run(id, model, prompt, resume, &on_delta, &mut output).await;
        let _ = output.send(on_end(id, result)).await;
    })
}

async fn run<M, D>(
    id:       TerminalId,
    model:    &str,
    prompt:   String,
    resume:   Option<String>,
    on_delta: &D,
    output:   &mut Sender<M>,
) -> Result<String, String>
where
    D: Fn(TerminalId, String) -> M,
{
    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg("--model").arg(model)
        .arg("--output-format").arg("stream-json")
        .arg("--verbose")
        .arg("--include-partial-messages")
        // Empty tool list → pure Q&A: the model can't read files or run Bash.
        .arg("--tools").arg("");
    if let Some(session) = &resume {
        cmd.arg("--resume").arg(session);
    }
    // A neutral cwd keeps a chat pane from loading the project's CLAUDE.md /
    // auto-memory; it's also stable across a pane's turns, so `--resume` lands
    // in the same place every time.
    cmd.current_dir(std::env::temp_dir());

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
