//! Embedded fonts — bundled directly so the IDE renders the same on any
//! machine, regardless of what's installed system-wide.

/// Liberation Sans — UI text and button labels.
pub static LIBERATION_SANS: &[u8] =
    include_bytes!("../assets/fonts/LiberationSans-Regular.ttf");

/// Liberation Sans Bold / Italic / Bold-Italic — bundled so inline markdown
/// styling in the preview (`**bold**`, `*italic*`) renders with real faces
/// instead of silently falling back to Regular on machines without system
/// Liberation fonts.
pub static LIBERATION_SANS_BOLD: &[u8] =
    include_bytes!("../assets/fonts/LiberationSans-Bold.ttf");
pub static LIBERATION_SANS_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/LiberationSans-Italic.ttf");
pub static LIBERATION_SANS_BOLD_ITALIC: &[u8] =
    include_bytes!("../assets/fonts/LiberationSans-BoldItalic.ttf");

/// Noto Sans Symbols 2 — broad coverage of dingbats, geometric shapes, and
/// miscellaneous icon-like symbols (pencils, arrows, ...). OFL-licensed.
pub static NOTO_SYMBOLS_2: &[u8] =
    include_bytes!("../assets/fonts/NotoSansSymbols2-Regular.ttf");

/// Reset arrows (↺ U+21BA, ↻ U+21BB) only — a ~1.5 KB subset of DejaVu Sans
/// (renamed per the Vera-derivative license). Neither Liberation Sans nor
/// Noto Sans Symbols 2 carries these code points, so the header's usage
/// reset-time indicator gets its own micro-font.
pub static RESET_ARROW: &[u8] =
    include_bytes!("../assets/fonts/ResetArrow.ttf");

/// Liberation Mono — fixed-pitch font for the terminal canvas. Bundling it
/// (rather than relying on `Font::MONOSPACE`, which resolves to an unknown
/// system font) gives the terminal a glyph advance we can count on: a
/// Courier-metric clone whose advance is exactly 0.6 em. The cell geometry in
/// `ui::terminal` is derived from that ratio so glyphs sit one-per-cell with no
/// overlap or drift on any machine.
pub static LIBERATION_MONO: &[u8] =
    include_bytes!("../assets/fonts/LiberationMono-Regular.ttf");

/// Default UI font. Use this for regular text and button labels.
pub const UI_FONT: iced::Font = iced::Font::with_name("Liberation Sans");

/// Font for icon glyphs (pencils, ×, arrows, ...). The Liberation Sans we
/// ship is Latin-only so anything outside Latin tofus — switch text widgets
/// that render icon glyphs over to this font.
pub const ICON_FONT: iced::Font = iced::Font::with_name("Noto Sans Symbols 2");

/// Font for the ↺ reset-time indicator in the header usage bars. Covers only
/// U+21BA/U+21BB — use [`ICON_FONT`] for anything else.
pub const RESET_FONT: iced::Font = iced::Font::with_name("FRMS Reset Arrow");

/// Fixed-pitch font for the terminal canvas. Bound explicitly to the bundled
/// Liberation Mono face: `Family::Monospace` lets cosmic-text resolve to
/// whichever monospace it finds in the system stack (Noto Sans Mono, DejaVu,
/// Nimbus, ...) and those faces have different glyph advances, so the cell
/// grid — sized by `MONO_ADVANCE_RATIO` below — no longer matches the
/// rendered glyphs and characters bunch up or drift. Naming the bundled face
/// pins both the font and its 0.6 em advance together.
pub const MONO_FONT: iced::Font = iced::Font::with_name("Liberation Mono");

/// Glyph advance as a fraction of point size. 0.6 em is the Liberation Mono
/// advance — the cell grid is sized from this, so the font picked above must
/// share it. Don't change this without changing `MONO_FONT` to a face with
/// matching metrics.
pub const MONO_ADVANCE_RATIO: f32 = 0.6;
