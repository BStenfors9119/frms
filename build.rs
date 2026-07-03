//! Build script: embed the Windows icon resource into `frms.exe` so Explorer,
//! the taskbar, and shortcuts show the app icon. (The runtime winit icon set in
//! `main.rs` only covers the live window; Explorer/shortcut icons come from the
//! executable's embedded resource.) No-op on non-Windows targets.
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

fn main() {
    println!("cargo:rerun-if-changed=assets/frms.ico");
    println!("cargo:rerun-if-changed=build.rs");

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
