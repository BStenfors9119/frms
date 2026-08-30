//! Build script. Two jobs:
//!
//! 1. Stamp the build identity (git commit, UTC build date, target triple) into
//!    the binary as env vars that `src/version.rs` reads with `env!`. The
//!    version number itself comes from `Cargo.toml` via `CARGO_PKG_VERSION` —
//!    this only adds what Cargo can't know.
//! 2. Embed the Windows icon resource into `frms.exe` so Explorer,
//!    the taskbar, and shortcuts show the app icon. (The runtime winit icon set in
//!    `main.rs` only covers the live window; Explorer/shortcut icons come from the
//!    executable's embedded resource.) No-op on non-Windows targets.
//!
//! Embedding a resource requires a resource compiler for the target — `llvm-rc`
//! or `rc.exe` for the MSVC target, `windres` for the GNU target. If none is
//! found the icon simply isn't embedded: the build still succeeds and the app
//! falls back to the runtime window icon (which already carries an opaque
//! backdrop, see `icon::build`). To get the embedded icon, install a resource
//! compiler in the build environment, e.g. on Fedora: `sudo dnf install llvm`
//! (provides `llvm-rc`).
//!
//! Regenerate `assets/frms.ico` from the logo like so:
//!   cargo run -- --export-icon /tmp/logo.png   # transparent wordmark
//!   magick \( -size 256x256 xc:none -fill '#222428' \
//!            -draw 'roundrectangle 0,0,255,255,44,44' \) \
//!          /tmp/logo.png -compose over -composite \
//!          -define icon:auto-resize=256,128,64,48,32,24,16 assets/frms.ico

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-changed=assets/frms.ico");
    println!("cargo:rerun-if-changed=build.rs");

    stamp_build_identity();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/frms.ico");
    if let Err(e) = res.compile() {
        // Non-fatal: keep the build green and fall back to the runtime icon.
        println!(
            "cargo:warning=frms: Windows icon not embedded ({e}). Install a \
             resource compiler (llvm-rc / windres) in the build environment to \
             embed it; the app still builds and uses its runtime window icon."
        );
    }
}

/// Emit `FRMS_GIT_COMMIT`, `FRMS_BUILD_DATE`, and `FRMS_BUILD_TARGET` for
/// `src/version.rs`. Every one has a fallback: a source tree with no git (a
/// released tarball, a Docker build context) still compiles, it just reports
/// `unknown` for the commit.
fn stamp_build_identity() {
    println!("cargo:rustc-env=FRMS_GIT_COMMIT={}", git_commit());
    println!("cargo:rustc-env=FRMS_BUILD_DATE={}", build_date());
    println!(
        "cargo:rustc-env=FRMS_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".into())
    );

    // Re-run when HEAD moves so the stamp doesn't go stale across commits or
    // branch switches. `.git/HEAD` covers checkouts; the ref file it points at
    // covers new commits on the current branch. Missing paths are fine — Cargo
    // treats a `rerun-if-changed` on a nonexistent file as "always re-run",
    // which is the right answer when we can't tell whether HEAD moved.
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        println!("cargo:rerun-if-changed=.git/{r}");
    }
}

/// Short hash of HEAD, suffixed `+` when the working tree is dirty — so a build
/// from uncommitted changes can't be mistaken for the commit it sits on.
fn git_commit() -> String {
    let Some(hash) = git(&["rev-parse", "--short=9", "HEAD"]) else {
        return "unknown".into();
    };
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|s| !s.is_empty());
    if dirty { format!("{hash}+") } else { hash }
}

/// Run a git command in the manifest dir, returning trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    Some(s)
}

/// Today's UTC date as `YYYY-MM-DD`. Honors `SOURCE_DATE_EPOCH` when set so
/// reproducible-build environments (and the packaging scripts, if they ever set
/// it) get a stable stamp instead of "whenever this machine happened to build".
fn build_date() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .unwrap_or(0);

    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days-since-epoch → (year, month, day). Howard Hinnant's `civil_from_days`;
/// inlined rather than pulling a date crate into the build graph for one call.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
