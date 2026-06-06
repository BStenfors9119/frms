/// Chrome DevTools Protocol helpers.
/// Each public function opens a fresh WebSocket to the already-running
/// Chromium instance on `port`, performs its action, and closes the socket.
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use iced::futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMsg};

pub const VIEWPORT_W: u32 = 1280;
pub const VIEWPORT_H: u32 = 900;

// ── launch ────────────────────────────────────────────────────────────────────

/// Spawn Chromium with remote debugging on `port`.
/// Polls until CDP is reachable (up to ~5 s), then returns.
/// The spawned process is intentionally detached — it lives until the app exits.
pub async fn launch(port: u16) -> Result<(), String> {
    // If a Chromium instance on this port is already running, reuse it.
    if get_page_ws_url(port).await.is_ok() {
        return Ok(());
    }

    let port_arg = format!("--remote-debugging-port={port}");
    let size_arg = format!("--window-size={VIEWPORT_W},{VIEWPORT_H}");
    let base_flags = [
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        port_arg.as_str(),
        size_arg.as_str(),
        "about:blank",
    ];

    let mut last_err = "no Chromium binary found — install chromium or google-chrome".to_string();

    // ── 1. Direct binary names ─────────────────────────────────────────────────
    for binary in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"] {
        if try_launch(binary, &base_flags, port).await {
            return Ok(());
        }
        last_err = format!("{binary} could not start CDP on port {port}");
    }

    // ── 2. flatpak-spawn --host  (Silverblue toolbox → host binaries) ──────────
    for binary in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"] {
        let mut args = vec!["--host", binary];
        args.extend_from_slice(&base_flags);
        if try_launch("flatpak-spawn", &args, port).await {
            return Ok(());
        }
    }

    // ── 3. flatpak-spawn --host flatpak run org.chromium.Chromium ─────────────
    {
        let mut args = vec!["--host", "flatpak", "run", "org.chromium.Chromium"];
        args.extend_from_slice(&base_flags);
        if try_launch("flatpak-spawn", &args, port).await {
            return Ok(());
        }
    }

    // Note: Firefox is intentionally not tried here. Modern Firefox exposes
    // WebDriver BiDi on the remote-debugging port, not the Chromium CDP /json
    // REST endpoint that get_page_ws_url() probes. Attempting Firefox here
    // would leave orphan processes after the 5 s polling timeout with no
    // benefit. The take_screenshot fallback handles Firefox via --screenshot.

    Err(last_err)
}

/// Attempt to spawn `binary` with `args` and wait up to 5 s for CDP to
/// become reachable. Returns `true` if successful.
async fn try_launch(binary: &str, args: &[&str], port: u16) -> bool {
    let mut child = match tokio::process::Command::new(binary)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Err(_) => return false,
        Ok(c) => c,
    };

    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        // If the process already exited it failed to start — don't wait 5 s
        if let Ok(Some(_)) = child.try_wait() {
            return false;
        }
        if get_page_ws_url(port).await.is_ok() {
            return true; // child keeps running in background
        }
    }
    false
}

// ── public actions ────────────────────────────────────────────────────────────

/// Navigate to `url` and return a PNG screenshot once the page loads.
pub async fn navigate_and_screenshot(port: u16, url: String) -> Result<Vec<u8>, String> {
    let ws_url = get_page_ws_url(port).await?;
    eprintln!("[cdp] navigate_and_screenshot: ws_url={ws_url:?}");
    let (mut ws, _) = connect_async(&ws_url).await
        .map_err(|e| format!("CDP WS connect: {e}"))?;

    // Enable Page domain events and wait for acknowledgement
    send_msg(&mut ws, 1, "Page.enable", json!({})).await?;
    wait_for_id(&mut ws, 1, Duration::from_secs(3)).await?;

    // Navigate and check for an immediate failure (DNS, blocked scheme, etc.)
    send_msg(&mut ws, 2, "Page.navigate", json!({"url": url})).await?;
    let nav_result = wait_for_id(&mut ws, 2, Duration::from_secs(5)).await?;
    if let Some(err) = nav_result.get("errorText").and_then(|v| v.as_str()) {
        if !err.is_empty() {
            return Err(format!("navigate failed: {err}"));
        }
    }

    // Wait for Page.loadEventFired or timeout
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => break, // timeout — screenshot what we have
            Ok(None) => return Err("CDP WebSocket closed".into()),
            Ok(Some(Err(e))) => return Err(format!("WS error: {e}")),
            Ok(Some(Ok(WsMsg::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or_default();
                if v["method"].as_str() == Some("Page.loadEventFired") {
                    break;
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }

    screenshot_inner(&mut ws, 90).await
}

/// Capture a PNG screenshot of the current page state.
pub async fn screenshot(port: u16) -> Result<Vec<u8>, String> {
    let ws_url = get_page_ws_url(port).await?;
    let (mut ws, _) = connect_async(&ws_url).await
        .map_err(|e| format!("CDP WS connect: {e}"))?;

    screenshot_inner(&mut ws, 99).await
}

/// Dispatch a left-click at (x, y) and return an updated screenshot.
pub async fn click_and_screenshot(port: u16, x: f64, y: f64) -> Result<Vec<u8>, String> {
    let ws_url = get_page_ws_url(port).await?;
    let (mut ws, _) = connect_async(&ws_url).await
        .map_err(|e| format!("CDP WS connect: {e}"))?;

    send_msg(&mut ws, 1, "Input.dispatchMouseEvent", json!({
        "type": "mousePressed", "x": x, "y": y,
        "button": "left", "buttons": 1, "clickCount": 1,
    })).await?;
    send_msg(&mut ws, 2, "Input.dispatchMouseEvent", json!({
        "type": "mouseReleased", "x": x, "y": y,
        "button": "left", "buttons": 0, "clickCount": 1,
    })).await?;

    // Drain the two acknowledgements
    let drain_deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    let mut got = 0u32;
    while got < 2 {
        match tokio::time::timeout_at(drain_deadline, ws.next()).await {
            Err(_) | Ok(None) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(WsMsg::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or_default();
                if v.get("id").is_some() { got += 1; }
            }
            Ok(Some(Ok(_))) => {}
        }
    }

    // Small pause for click effects to render
    tokio::time::sleep(Duration::from_millis(150)).await;
    screenshot_inner(&mut ws, 99).await
}

/// Insert `text` into the focused element and return an updated screenshot.
#[allow(dead_code)]
pub async fn key_and_screenshot(port: u16, text: String) -> Result<Vec<u8>, String> {
    let ws_url = get_page_ws_url(port).await?;
    let (mut ws, _) = connect_async(&ws_url).await
        .map_err(|e| format!("CDP WS connect: {e}"))?;

    send_msg(&mut ws, 1, "Input.insertText", json!({"text": text})).await?;
    wait_for_id(&mut ws, 1, Duration::from_millis(500)).await.ok();
    screenshot_inner(&mut ws, 99).await
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Probe Chromium's /json endpoint and return the WebSocket debugger URL of
/// the first page target.
async fn get_page_ws_url(port: u16) -> Result<String, String> {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    let mut stream = tokio::time::timeout(
        Duration::from_millis(500),
        TcpStream::connect(format!("127.0.0.1:{port}")),
    )
    .await
    .map_err(|_| format!("timeout connecting to CDP port {port}"))?
    .map_err(|e| format!("connect CDP: {e}"))?;

    let req = format!("GET /json HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    // Read the HTTP response with a hard timeout. We parse Content-Length so
    // we stop after the body rather than waiting for the server to close the
    // connection (Chrome's CDP server keeps connections alive).
    let json = tokio::time::timeout(
        Duration::from_secs(3),
        read_http_json_body(&mut stream),
    )
    .await
    .map_err(|_| "timeout reading CDP /json response".to_string())?
    .map_err(|e| format!("read CDP: {e}"))?;

    let pages: Vec<Value> =
        serde_json::from_str(&json).map_err(|e| format!("parse /json: {e}"))?;

    let page = pages
        .iter()
        .find(|p| p["type"].as_str() == Some("page"))
        .or_else(|| pages.first())
        .ok_or_else(|| "no page targets in CDP /json".to_string())?;

    // Normalize hostname: Chrome returns "localhost" but on some systems that
    // resolves to ::1 (IPv6) while Chrome only listens on 127.0.0.1 (IPv4).
    page["webSocketDebuggerUrl"]
        .as_str()
        .ok_or_else(|| "no webSocketDebuggerUrl in CDP response".to_string())
        .map(|s| s.replace("localhost", "127.0.0.1"))
}

/// Read an HTTP response and return the body as a String.
/// Uses Content-Length when available so we don't wait for connection close.
async fn read_http_json_body(stream: &mut tokio::net::TcpStream) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;

    // Read up to 64 KiB in one shot; CDP /json responses are always small.
    let mut buf = vec![0u8; 65536];
    let mut filled = 0;

    // Keep reading until we have the complete headers + body.
    loop {
        let n = stream.read(&mut buf[filled..]).await?;
        filled += n;

        let data = &buf[..filled];

        // Find end of headers.
        if let Some(header_end) = find_header_end(data) {
            let headers = std::str::from_utf8(&data[..header_end]).unwrap_or("");

            // Extract Content-Length.
            let content_length = headers
                .lines()
                .find_map(|l| {
                    let lower = l.to_lowercase();
                    lower.strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                });

            let body_start = header_end + 4; // skip \r\n\r\n

            if let Some(len) = content_length {
                if filled >= body_start + len {
                    // We have the full body.
                    let body = &data[body_start..body_start + len];
                    return Ok(String::from_utf8_lossy(body).into_owned());
                }
                // Need more data — continue reading.
            } else {
                // No Content-Length: use whatever we have after headers.
                if n == 0 {
                    let body = &data[body_start..];
                    return Ok(String::from_utf8_lossy(body).into_owned());
                }
                // Server hasn't closed yet — keep reading until EOF or buffer full.
            }
        }

        if n == 0 || filled == buf.len() {
            // EOF or buffer full — return whatever body we can find.
            if let Some(header_end) = find_header_end(&buf[..filled]) {
                let body = &buf[header_end + 4..filled];
                return Ok(String::from_utf8_lossy(body).into_owned());
            }
            return Ok(String::new());
        }
    }
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn send_msg(ws: &mut Ws, id: u32, method: &str, params: Value) -> Result<(), String> {
    let payload = json!({"id": id, "method": method, "params": params}).to_string();
    ws.send(WsMsg::Text(payload.into()))
        .await
        .map_err(|e| format!("CDP send {method}: {e}"))
}

/// Read messages until we see a response with `id`, or until timeout.
async fn wait_for_id(ws: &mut Ws, id: u32, timeout: Duration) -> Result<Value, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return Err(format!("timeout waiting for CDP id={id}")),
            Ok(None) => return Err("CDP closed".into()),
            Ok(Some(Err(e))) => return Err(format!("WS error: {e}")),
            Ok(Some(Ok(WsMsg::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or_default();
                if v["id"].as_u64() == Some(id as u64) {
                    if let Some(err) = v.get("error") {
                        return Err(format!("CDP error: {err}"));
                    }
                    return Ok(v["result"].clone());
                }
            }
            Ok(Some(Ok(_))) => {}
        }
    }
}

async fn screenshot_inner(ws: &mut Ws, id: u32) -> Result<Vec<u8>, String> {
    let payload = json!({
        "id": id,
        "method": "Page.captureScreenshot",
        "params": { "format": "png" }
    })
    .to_string();
    ws.send(WsMsg::Text(payload.into()))
        .await
        .map_err(|e| format!("CDP send screenshot: {e}"))?;

    let result = wait_for_id(ws, id, Duration::from_secs(5)).await?;
    let data = result["data"]
        .as_str()
        .ok_or("no screenshot data in CDP response")?;
    B64.decode(data).map_err(|e| format!("base64 decode: {e}"))
}
