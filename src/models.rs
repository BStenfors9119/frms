//! Requerying the live model catalogue.
//!
//! The per-tool model lists in [`crate::agent`] ship with a hardcoded seed, but
//! Claude's real catalogue moves — a new model can appear any week. Rather than
//! make the user rebuild (or even restart) to see it, this fetches Anthropic's
//! `/v1/models` and hands the result to [`crate::agent::set_claude_models`],
//! which swaps the live list the picker reads.
//!
//! Auth reuses the Claude Code login: the OAuth token the CLI already stored in
//! `~/.claude/.credentials.json` (the same token [`crate::stats`] reads for
//! usage). No API key, and the request is a read-only GET. Transport is `curl`
//! — the app deliberately carries no TLS crate (same shell-out as
//! [`crate::stats`], [`crate::telemetry`], and [`crate::chat_api`]).
//!
//! Windsurf/`devin` has no comparable endpoint yet, so it isn't requeried; its
//! list stays on the seed until the tool ships something to query.

use std::process::Command;

/// Outcome of a requery. `Ok` holds `(id, display_name)` pairs newest-first (the
/// order the API returns), ready for [`crate::agent::set_claude_models`]; `Err`
/// is a short, human-readable reason for the Agents dialog to show.
pub type Refresh = Result<Vec<(String, String)>, String>;

/// Fetch Claude's current model list off the UI thread.
pub async fn refresh() -> Refresh {
    tokio::task::spawn_blocking(fetch_sync)
        .await
        .unwrap_or_else(|_| Err("model refresh was interrupted".into()))
}

fn fetch_sync() -> Refresh {
    let home = crate::port::dirs::home().ok_or("no home directory")?;
    let creds_path = home.join(".claude").join(".credentials.json");
    let creds = std::fs::read(&creds_path)
        .map_err(|_| "sign in to Claude Code first (no credentials found)".to_string())?;
    let creds: serde_json::Value = serde_json::from_slice(&creds)
        .map_err(|_| "couldn't read Claude credentials".to_string())?;
    let token = creds["claudeAiOauth"]["accessToken"]
        .as_str()
        .ok_or("no Claude access token — sign in first")?;

    // Same auth + transport as `stats::read_usage`: the CLI's OAuth token, the
    // oauth beta header, and curl (no Rust TLS dependency in this app).
    let output = Command::new("curl")
        .args([
            "-sf", // silent, and fail the process on an HTTP error status
            "-H", &format!("Authorization: Bearer {token}"),
            "-H", "anthropic-beta: oauth-2025-04-20",
            "-H", "anthropic-version: 2023-06-01",
            "--max-time", "8",
            "https://api.anthropic.com/v1/models?limit=1000",
        ])
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !output.status.success() {
        return Err("couldn't reach the models API".into());
    }

    let body: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "unexpected response from the models API".to_string())?;
    let models = parse(&body);
    if models.is_empty() {
        return Err("the models API returned nothing usable".into());
    }
    Ok(models)
}

/// Pull `(id, display_name)` pairs out of a `/v1/models` body, keeping only
/// `claude-*` ids and preserving the API's newest-first order. Falls back to the
/// id when a model carries no `display_name`.
fn parse(body: &serde_json::Value) -> Vec<(String, String)> {
    let Some(data) = body["data"].as_array() else { return Vec::new() };
    data.iter()
        .filter_map(|m| {
            let id = m["id"].as_str()?;
            if !id.starts_with("claude-") {
                return None;
            }
            let display = m["display_name"].as_str().unwrap_or(id);
            Some((id.to_string(), display.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_claude_models_in_order_and_names_them() {
        let body = serde_json::json!({
            "data": [
                { "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5" },
                { "id": "gpt-5.5",         "display_name": "GPT 5.5" },
                { "id": "claude-haiku-4-5-20251001" }, // no display_name → id
            ]
        });
        let got = parse(&body);
        assert_eq!(got, vec![
            ("claude-opus-5-5".to_string(),           "Claude Opus 5.5".to_string()),
            ("claude-haiku-4-5-20251001".to_string(), "claude-haiku-4-5-20251001".to_string()),
        ]);
    }

    #[test]
    fn parse_tolerates_a_shapeless_body() {
        assert!(parse(&serde_json::json!({})).is_empty());
        assert!(parse(&serde_json::json!({ "data": "nope" })).is_empty());
    }
}
