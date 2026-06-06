/// Splash-screen video pipeline.
///
/// Flow:
///   1. `ensure_yt_dlp()` — locate or auto-download the yt-dlp binary.
///   2. `frame_subscription()` — spawn yt-dlp piped directly into ffmpeg.
///      No intermediate URL is resolved; ffmpeg reads from stdin so it cannot
///      make HTTP range requests and will always start from frame 0.
///   3. `spawn_audio()` — parallel yt-dlp → ffmpeg pipeline that routes audio
///      straight to the system sound device (PulseAudio / PipeWire → ALSA).
///   4. `blend_to_black()` — per-frame pixel blend used to drive the fade-out.

use std::path::PathBuf;

use iced::futures::SinkExt;
use tokio::io::AsyncReadExt;

// ── step 1: locate / download yt-dlp ─────────────────────────────────────────

#[allow(dead_code)]
pub async fn ensure_yt_dlp() -> Result<PathBuf, String> {
    // 1a. Check $PATH
    let which = tokio::process::Command::new("which")
        .arg("yt-dlp")
        .output()
        .await;
    if let Ok(out) = which {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    // 1b. Check per-user cache
    let home      = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let cache_dir  = PathBuf::from(&home).join(".cache").join("frms");
    let cache_path = cache_dir.join("yt-dlp");
    if cache_path.exists() {
        return Ok(cache_path);
    }

    // 1c. Download the standalone binary with curl
    eprintln!("[splash] downloading yt-dlp…");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("mkdir: {e}"))?;

    let status = tokio::process::Command::new("curl")
        .args([
            "-fsSL", "-o", cache_path.to_str().unwrap(),
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp",
        ])
        .status()
        .await
        .map_err(|e| format!("curl: {e}"))?;

    if !status.success() {
        return Err("yt-dlp download failed".into());
    }

    let _ = tokio::process::Command::new("chmod")
        .args(["+x", cache_path.to_str().unwrap()])
        .status().await;

    eprintln!("[splash] yt-dlp ready at {}", cache_path.display());
    Ok(cache_path)
}

// ── step 2: frame subscription ────────────────────────────────────────────────
//
// yt-dlp downloads the video stream to its stdout; ffmpeg reads from that pipe
// via stdin.  Because ffmpeg has no seekable URL it cannot make HTTP range
// requests to read the WebM index — it must start decoding from byte 0.

pub fn frame_subscription<Msg>(
    yt_dlp:      PathBuf,
    youtube_url: String,
    on_frame:    impl Fn(Vec<u8>) -> Msg + Send + Sync + 'static,
    on_ended:    impl Fn()        -> Msg + Send + Sync + 'static,
) -> iced::Subscription<Msg>
where
    Msg: Send + 'static,
{
    iced::Subscription::run_with_id(
        0xF_4A_1E_5C_u64,  // stable ID — only one video subscription at a time
        iced::stream::channel(4, move |mut tx| {
            let yt_dlp = yt_dlp.clone();
            let url    = youtube_url.clone();
            async move {
                // Spawn yt-dlp with std::process::Command so its stdout fd is
                // a plain OS pipe with no tokio async I/O registration.  This
                // lets us hand the fd cleanly to ffmpeg's stdin without any
                // epoll conflict that would close the pipe prematurely.
                let mut ytdlp_cmd = std::process::Command::new(&yt_dlp);
                if let Some(node) = find_node() {
                    ytdlp_cmd.arg("--js-runtimes")
                             .arg(format!("node:{}", node.display()));
                }
                ytdlp_cmd.args([
                    "--no-playlist",
                    "-f",
                    "243/244/vp9/bestvideo[vcodec^=vp9][height<=480]/bestvideo[height<=480]/worst",
                    "-o", "-",
                    &url,
                ]);

                let mut ytdlp_child = match ytdlp_cmd
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[splash] yt-dlp spawn (video): {e}");
                        let _ = tx.send(on_ended()).await;
                        loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
                    }
                };

                // std::process::ChildStdout → std::process::Stdio is a clean,
                // supported conversion with no raw-fd gymnastics.
                let ffmpeg_stdin = std::process::Stdio::from(ytdlp_child.stdout.take().unwrap());

                let mut ffmpeg = match tokio::process::Command::new("ffmpeg")
                    .args([
                        "-i",      "pipe:0",
                        "-vf",     "scale=854:480",
                        "-r",      "24",
                        "-f",      "image2pipe",
                        "-vcodec", "mjpeg",
                        "-q:v",    "5",
                        "pipe:1",
                    ])
                    .stdin(ffmpeg_stdin)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[splash] ffmpeg spawn: {e}");
                        let _ = tx.send(on_ended()).await;
                        loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
                    }
                };

                if let Some(mut stdout) = ffmpeg.stdout.take() {
                    let mut reader = tokio::io::BufReader::new(&mut stdout);
                    loop {
                        match read_jpeg_frame(&mut reader).await {
                            Some(jpeg) => {
                                if tx.send(on_frame(jpeg)).await.is_err() { break; }
                            }
                            None => {
                                let _ = tx.send(on_ended()).await;
                                break;
                            }
                        }
                    }
                }
                let _ = ffmpeg.wait().await;
                // Reap yt-dlp off the async thread (it finishes when ffmpeg closes stdin).
                let _ = tokio::task::spawn_blocking(move || { let _ = ytdlp_child.wait(); }).await;
                loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; }
            }
        }),
    )
}

// ── step 3: fade-to-black pixel blend ────────────────────────────────────────

/// Decode the JPEG, multiply every pixel toward black by `t` (0.0 = original,
/// 1.0 = full black), then return raw RGBA bytes + (width, height).
pub fn blend_to_black(jpeg: &[u8], t: f32) -> Option<(u32, u32, Vec<u8>)> {
    use std::io::Cursor;
    let img = image::load(Cursor::new(jpeg), image::ImageFormat::Jpeg).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let factor = 1.0 - t.clamp(0.0, 1.0);
    let pixels: Vec<u8> = rgba.into_raw()
        .chunks(4)
        .flat_map(|px| {
            [
                (px[0] as f32 * factor) as u8,
                (px[1] as f32 * factor) as u8,
                (px[2] as f32 * factor) as u8,
                255,
            ]
        })
        .collect();
    Some((w, h, pixels))
}

// ── step 4: audio playback ────────────────────────────────────────────────────

/// Spawn a yt-dlp → ffmpeg audio pipeline that plays directly to the system
/// sound device.  Returns both child processes so the caller can kill them when
/// the splash ends.  Tries PulseAudio / PipeWire-pulse first, then ALSA.
pub fn spawn_audio(yt_dlp: &PathBuf, youtube_url: &str) -> Vec<std::process::Child> {
    if let Some(children) = try_audio(yt_dlp, youtube_url, "pulse") {
        eprintln!("[splash] audio started (pulse)");
        return children;
    }
    if let Some(children) = try_audio(yt_dlp, youtube_url, "alsa") {
        eprintln!("[splash] audio started (alsa)");
        return children;
    }
    eprintln!("[splash] audio unavailable");
    Vec::new()
}

fn try_audio(
    yt_dlp:      &PathBuf,
    youtube_url: &str,
    audio_fmt:   &str,
) -> Option<Vec<std::process::Child>> {
    // Build yt-dlp command for the best audio-only stream.
    let mut ytdlp_cmd = std::process::Command::new(yt_dlp);
    if let Some(node) = find_node() {
        ytdlp_cmd.arg("--js-runtimes")
                 .arg(format!("node:{}", node.display()));
    }
    ytdlp_cmd.args([
        "--no-playlist",
        "-f", "bestaudio/worstaudio/worst",
        "-o", "-",
        youtube_url,
    ]);
    let mut ytdlp = ytdlp_cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;

    let ytdlp_stdout = ytdlp.stdout.take()?;

    // ffmpeg output args differ slightly between pulse and alsa.
    let ffmpeg_out_args: &[&str] = match audio_fmt {
        "pulse" => &["-f", "pulse", "-name", "frms-splash", "default"],
        _       => &["-f", "alsa", "default"],
    };

    let mut ffmpeg_cmd = std::process::Command::new("ffmpeg");
    ffmpeg_cmd.args(["-i", "pipe:0", "-vn"])
              .args(ffmpeg_out_args)
              .stdin(ytdlp_stdout)
              .stdout(std::process::Stdio::null())
              .stderr(std::process::Stdio::null());

    let ffmpeg = ffmpeg_cmd.spawn().ok()?;
    Some(vec![ytdlp, ffmpeg])
}

// ── node discovery ────────────────────────────────────────────────────────────

/// Probe common Node.js install locations (nvm, system, etc.) so yt-dlp can
/// solve YouTube's JS challenges.
fn find_node() -> Option<PathBuf> {
    if let Ok(out) = std::process::Command::new("which").arg("node").output() {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let nvm_dir = PathBuf::from(&home).join(".nvm").join("versions").join("node");
        if let Ok(entries) = std::fs::read_dir(&nvm_dir) {
            let mut versions: Vec<_> = entries.flatten().collect();
            versions.sort_by_key(|e| e.file_name());
            if let Some(latest) = versions.last() {
                let candidate = latest.path().join("bin").join("node");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

// ── MJPEG frame extraction ────────────────────────────────────────────────────

async fn read_jpeg_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> Option<Vec<u8>> {
    let mut frame: Vec<u8> = Vec::with_capacity(32 * 1024);
    let mut prev    = 0u8;
    let mut started = false;
    let mut tmp     = [0u8; 1];

    loop {
        match reader.read(&mut tmp).await {
            Ok(0) | Err(_) => return if frame.is_empty() { None } else { Some(frame) },
            Ok(_) => {}
        }
        let b = tmp[0];
        if !started {
            if prev == 0xFF && b == 0xD8 {
                frame.push(0xFF);
                frame.push(0xD8);
                started = true;
            }
        } else {
            frame.push(b);
            if prev == 0xFF && b == 0xD9 {
                return Some(frame);
            }
        }
        prev = b;
    }
}
