//! Build identity — the one place the app's version comes from.
//!
//! `Cargo.toml`'s `version` is the single source of truth: the crate version is
//! baked in here via `CARGO_PKG_VERSION`, the packaging scripts read the same
//! field (`_package/_release.sh` bumps it and mirrors it into the deb control
//! file), and everything that reports a version — the Profile tab, `--version`,
//! telemetry — reads it from this module rather than re-deriving it.
//!
//! [`COMMIT`] and [`BUILD_DATE`] are stamped by `build.rs`. They're what turns
//! "0.1.2" into something actionable in a bug report: two builds of the same
//! version from different commits are otherwise indistinguishable. When the
//! source tree isn't a git checkout (a packaged tarball, say) `COMMIT` is
//! `"unknown"` — treat it as informational, never as something to parse.

/// Semantic version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Short git hash the binary was built from, `+` suffixed when the working tree
/// had uncommitted changes. `"unknown"` outside a git checkout.
pub const COMMIT: &str = env!("FRMS_GIT_COMMIT");

/// UTC date of the build, `YYYY-MM-DD`.
pub const BUILD_DATE: &str = env!("FRMS_BUILD_DATE");

/// Target triple this binary was compiled for (e.g. `x86_64-pc-windows-gnu`).
/// The Windows and Linux builds ship the same version number, so this is what
/// distinguishes them in a report.
pub const TARGET: &str = env!("FRMS_BUILD_TARGET");

/// One-line identity for the terminal: `frms 0.1.2 (a1b2c3d 2026-08-12)`.
pub fn line() -> String {
    format!("frms {VERSION} ({COMMIT} {BUILD_DATE})")
}

/// The build stamp shown under the version in the Profile tab — commit, build
/// date, and target, in one scannable line.
pub fn build_details() -> String {
    format!("{COMMIT} · {BUILD_DATE} · {TARGET}")
}

/// Everything at once, for `--version` and for pasting into a bug report.
pub fn full() -> String {
    format!("{}\ntarget: {TARGET}", line())
}
