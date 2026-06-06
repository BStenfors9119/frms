//! Builds the window/taskbar icon at runtime: the wordmark "frms", with a
//! pair of eyeglasses standing in for the `m`. Liberation Sans (already
//! bundled in `assets/fonts/`) supplies the letterforms via `ab_glyph`; the
//! glasses are drawn with simple circle math.

use std::path::PathBuf;
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

use crate::fonts::LIBERATION_SANS;

pub const W: u32 = 256;
pub const H: u32 = 256;
/// White text + glasses — looks good on any taskbar background.
const FG: (u8, u8, u8) = (245, 245, 245);

pub fn build() -> Option<iced::window::Icon> {
    let buf = rgba()?.clone();
    // Write a PNG and a .desktop template once per launch so the icon is
    // installable on Wayland (where the in-process winit icon is ignored).
    let _ = export_assets(&buf);
    iced::window::icon::from_rgba(buf, W, H).ok()
}

/// Cached RGBA pixel buffer for the icon — generated lazily on first call.
pub fn rgba() -> Option<&'static Vec<u8>> {
    static BUF: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    BUF.get_or_init(render_rgba).as_ref()
}

/// `iced::widget::image::Handle` for the icon — cached after first build so
/// re-renders during animation don't pay the rasterization cost again.
pub fn image_handle() -> Option<iced::widget::image::Handle> {
    static HANDLE: OnceLock<Option<iced::widget::image::Handle>> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            rgba().map(|b| iced::widget::image::Handle::from_rgba(W, H, b.clone()))
        })
        .clone()
}

fn render_rgba() -> Option<Vec<u8>> {
    let mut buf = vec![0u8; (W * H * 4) as usize];

    let font = FontRef::try_from_slice(LIBERATION_SANS).ok()?;
    let scale = PxScale::from(120.0);
    let scaled = font.as_scaled(scale);

    // ── compute layout ───────────────────────────────────────────────────────
    // Glasses take the place where the `m` would be — sized to roughly match
    // an `m`'s footprint at this scale.
    let m_advance = scaled.h_advance(font.glyph_id('m'));
    let glasses_w = m_advance;

    let advance = |c: char| scaled.h_advance(font.glyph_id(c));
    let total_w = advance('f') + advance('r') + glasses_w + advance('s');

    // Center the wordmark horizontally; baseline lands a little below center
    // so cap-height + descender sit balanced vertically.
    let mut x = (W as f32 - total_w) / 2.0;
    let baseline = (H as f32 + scaled.ascent() - scaled.descent()) / 2.0;

    // ── letters ──────────────────────────────────────────────────────────────
    for c in ['f', 'r'] {
        x = draw_glyph(&mut buf, &font, scale, c, x, baseline);
    }

    // ── glasses (drawn where the `m` would have been) ────────────────────────
    let glasses_left  = x;
    let glasses_right = x + glasses_w;
    draw_glasses(&mut buf, glasses_left, glasses_right, baseline, &scaled);
    x = glasses_right;

    draw_glyph(&mut buf, &font, scale, 's', x, baseline);

    Some(buf)
}

// ── exporting the icon for desktop integration ────────────────────────────────

/// Render the icon and save it as a PNG at `path`. Used by
/// `frms --export-icon <path>` so install.sh can place the icon into
/// `~/.local/share/icons/` without launching the IDE.
pub fn export_png(path: &std::path::Path) -> Option<()> {
    let buf = rgba()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let img = image::RgbaImage::from_raw(W, H, buf.clone())?;
    img.save(path).ok()
}

fn export_assets(rgba: &[u8]) -> Option<()> {
    let home = std::env::var("HOME").ok()?;
    let dir  = PathBuf::from(home).join(".cache").join("frms");
    std::fs::create_dir_all(&dir).ok()?;

    let png_path     = dir.join("frms-icon.png");
    let desktop_path = dir.join("frms.desktop");

    // Write the PNG (only if absent or stale-sized).
    let need_png = match std::fs::metadata(&png_path) {
        Ok(m)  => m.len() == 0,
        Err(_) => true,
    };
    if need_png {
        if let Some(img) = image::RgbaImage::from_raw(W, H, rgba.to_vec()) {
            let _ = img.save(&png_path);
        }
    }

    // Write a launcher .desktop file. Points Icon= and StartupWMClass at the
    // canonical names so a Wayland compositor can match the running app to
    // the icon. The user copies (or symlinks) this into ~/.local/share/
    // applications/ and ~/.local/share/icons/hicolor/256x256/apps/.
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frms".into());

    let desktop = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=frms\n\
         Comment=Cross-platform IDE\n\
         Exec={exe}\n\
         Icon=frms-icon\n\
         Terminal=false\n\
         Categories=Development;IDE;\n\
         StartupWMClass=frms\n",
    );
    let _ = std::fs::write(&desktop_path, desktop);

    eprintln!("[icon] wrote {} and {}", png_path.display(), desktop_path.display());
    Some(())
}

// ── drawing helpers ───────────────────────────────────────────────────────────

fn draw_glyph(
    buf: &mut [u8],
    font: &FontRef,
    scale: PxScale,
    c: char,
    x: f32,
    baseline: f32,
) -> f32 {
    let glyph_id = font.glyph_id(c);
    let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
    let advance = font.as_scaled(scale).h_advance(glyph_id);

    if let Some(outlined) = font.outline_glyph(glyph) {
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            put_pixel(buf, px, py, coverage);
        });
    }

    x + advance
}

/// Paint a pair of eyeglasses (two circles + bridge + temple stubs) inside
/// the rectangle `[left..right] × roughly the height of a lowercase letter`.
fn draw_glasses(
    buf: &mut [u8],
    left: f32,
    right: f32,
    baseline: f32,
    scaled: &ab_glyph::PxScaleFont<&FontRef<'_>>,
) {
    // Glasses sit on the x-height band, like a lowercase `m`.
    let x_height = scaled.height() * 0.5;
    let top      = baseline - x_height;
    let bottom   = baseline;
    let cy       = (top + bottom) / 2.0;

    let total_w     = right - left;
    let bridge_w    = total_w * 0.10;
    let lens_radius = (total_w - bridge_w) / 4.0;
    let lx          = left  + lens_radius;
    let rx          = right - lens_radius;
    let radius_y    = (bottom - top) / 2.0;

    let stroke = (lens_radius * 0.18).max(2.0);

    draw_ring_ellipse(buf, lx, cy, lens_radius, radius_y, stroke);
    draw_ring_ellipse(buf, rx, cy, lens_radius, radius_y, stroke);

    // Bridge — short bar between the two lenses.
    draw_hbar(
        buf,
        lx + lens_radius - 1.0,
        rx - lens_radius + 1.0,
        cy,
        stroke,
    );

    // Temple stubs poking out the sides — sells the silhouette.
    let stub_len = lens_radius * 0.45;
    draw_hbar(buf, left  - stub_len, left  + 1.0, cy - radius_y * 0.55, stroke);
    draw_hbar(buf, right - 1.0,      right + stub_len, cy - radius_y * 0.55, stroke);
}

/// Draw the outline of an axis-aligned ellipse with the given stroke width.
fn draw_ring_ellipse(buf: &mut [u8], cx: f32, cy: f32, rx: f32, ry: f32, stroke: f32) {
    let outer_rx = rx + stroke / 2.0;
    let outer_ry = ry + stroke / 2.0;
    let inner_rx = (rx - stroke / 2.0).max(0.0);
    let inner_ry = (ry - stroke / 2.0).max(0.0);

    let x0 = (cx - outer_rx).floor() as i32 - 1;
    let x1 = (cx + outer_rx).ceil()  as i32 + 1;
    let y0 = (cy - outer_ry).floor() as i32 - 1;
    let y1 = (cy + outer_ry).ceil()  as i32 + 1;

    for py in y0..=y1 {
        for px in x0..=x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            // Outer / inner ellipse "inside" tests.
            let outer = (dx / outer_rx).powi(2) + (dy / outer_ry).powi(2);
            let inner = if inner_rx > 0.0 && inner_ry > 0.0 {
                (dx / inner_rx).powi(2) + (dy / inner_ry).powi(2)
            } else {
                f32::INFINITY
            };
            if outer <= 1.0 && inner >= 1.0 {
                // Soft anti-alias near both edges.
                let outer_dist = 1.0 - outer;
                let inner_dist = inner - 1.0;
                let edge = outer_dist.min(inner_dist).max(0.0);
                let coverage = (edge * 6.0).min(1.0);
                put_pixel(buf, px, py, coverage);
            }
        }
    }
}

/// Filled horizontal bar from `(x0, y - thickness/2)` to `(x1, y + thickness/2)`.
fn draw_hbar(buf: &mut [u8], x0: f32, x1: f32, y: f32, thickness: f32) {
    let y_lo = y - thickness / 2.0;
    let y_hi = y + thickness / 2.0;

    let px0 = x0.floor() as i32;
    let px1 = x1.ceil()  as i32;
    let py0 = y_lo.floor() as i32;
    let py1 = y_hi.ceil()  as i32;

    for py in py0..=py1 {
        for px in px0..=px1 {
            let cx = px as f32 + 0.5;
            let cy = py as f32 + 0.5;
            if cx >= x0 && cx <= x1 && cy >= y_lo && cy <= y_hi {
                put_pixel(buf, px, py, 1.0);
            }
        }
    }
}

fn put_pixel(buf: &mut [u8], x: i32, y: i32, coverage: f32) {
    if x < 0 || y < 0 || (x as u32) >= W || (y as u32) >= H {
        return;
    }
    let idx = ((y as u32 * W + x as u32) * 4) as usize;
    let a = (coverage.clamp(0.0, 1.0) * 255.0) as u8;
    if a <= buf[idx + 3] { return; }
    buf[idx]     = FG.0;
    buf[idx + 1] = FG.1;
    buf[idx + 2] = FG.2;
    buf[idx + 3] = a;
}
