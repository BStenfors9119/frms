//! Color palettes, light/dark modes, and font scaling.
//!
//! `Palette × Mode` defines the visible colors. `FontScale` is applied as
//! the global Iced scale factor so every widget grows/shrinks together.

use std::fmt;

use iced::Color;
use iced::theme::Palette as IcedPalette;
use iced::Theme as IcedTheme;

// ── palette ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Palette {
    Blues,
    Greens,
    Yellows,
    Grays,
}

impl Palette {
    pub const ALL: &'static [Palette] = &[
        Palette::Blues,
        Palette::Greens,
        Palette::Yellows,
        Palette::Grays,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Palette::Blues   => "Blues",
            Palette::Greens  => "Greens",
            Palette::Yellows => "Yellows",
            Palette::Grays   => "Grays",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Blues"   => Some(Palette::Blues),
            "Greens"  => Some(Palette::Greens),
            "Yellows" => Some(Palette::Yellows),
            "Grays"   => Some(Palette::Grays),
            _ => None,
        }
    }
}

impl Default for Palette {
    fn default() -> Self { Palette::Blues }
}

impl fmt::Display for Palette {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── light/dark ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub const ALL: &'static [Mode] = &[Mode::Light, Mode::Dark];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Light => "Light",
            Mode::Dark  => "Dark",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Light" => Some(Mode::Light),
            "Dark"  => Some(Mode::Dark),
            _ => None,
        }
    }
}

impl Default for Mode {
    fn default() -> Self { Mode::Dark }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── font scale ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontScale {
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl FontScale {
    pub const ALL: &'static [FontScale] = &[
        FontScale::Small,
        FontScale::Medium,
        FontScale::Large,
        FontScale::ExtraLarge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FontScale::Small      => "Small",
            FontScale::Medium     => "Medium",
            FontScale::Large      => "Large",
            FontScale::ExtraLarge => "Extra Large",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Small"       => Some(FontScale::Small),
            "Medium"      => Some(FontScale::Medium),
            "Large"       => Some(FontScale::Large),
            "Extra Large" => Some(FontScale::ExtraLarge),
            _ => None,
        }
    }

    /// Multiplier passed to `Application::scale_factor()`.
    pub fn factor(self) -> f64 {
        match self {
            FontScale::Small      => 0.85,
            FontScale::Medium     => 1.00,
            FontScale::Large      => 1.15,
            FontScale::ExtraLarge => 1.30,
        }
    }
}

impl Default for FontScale {
    fn default() -> Self { FontScale::Medium }
}

impl fmt::Display for FontScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── terminal font scale ───────────────────────────────────────────────────────

/// Multiplier applied to the per-cell size in the terminal canvas. The
/// auto-fit baseline (Medium) is constrained by the pane's width — a narrow
/// pane gets ~10 px text — so the larger steps grow aggressively to give
/// usable readability in narrow plugin panels. Going larger trades visible
/// columns for glyph size; the cursor row is auto-scrolled to stay on-screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalFontScale {
    Small,
    Medium,
    Large,
    ExtraLarge,
    Huge,
}

impl TerminalFontScale {
    pub const ALL: &'static [TerminalFontScale] = &[
        TerminalFontScale::Small,
        TerminalFontScale::Medium,
        TerminalFontScale::Large,
        TerminalFontScale::ExtraLarge,
        TerminalFontScale::Huge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TerminalFontScale::Small      => "Small",
            TerminalFontScale::Medium     => "Medium",
            TerminalFontScale::Large      => "Large",
            TerminalFontScale::ExtraLarge => "Extra Large",
            TerminalFontScale::Huge       => "Huge",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Small"       => Some(TerminalFontScale::Small),
            "Medium"      => Some(TerminalFontScale::Medium),
            "Large"       => Some(TerminalFontScale::Large),
            "Extra Large" => Some(TerminalFontScale::ExtraLarge),
            "Huge"        => Some(TerminalFontScale::Huge),
            _ => None,
        }
    }

    pub fn factor(self) -> f32 {
        match self {
            TerminalFontScale::Small      => 0.85,
            TerminalFontScale::Medium     => 1.00,
            TerminalFontScale::Large      => 1.50,
            TerminalFontScale::ExtraLarge => 2.25,
            TerminalFontScale::Huge       => 3.25,
        }
    }
}

impl Default for TerminalFontScale {
    fn default() -> Self { TerminalFontScale::Medium }
}

impl fmt::Display for TerminalFontScale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── derived colors ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    pub primary:    Color,
    pub secondary:  Color,
    pub tertiary:   Color,
    /// Window background — also fed into `iced::Theme`'s palette.
    #[allow(dead_code)]
    pub background: Color,
    /// Panel/card background — reserved for future panel styling.
    #[allow(dead_code)]
    pub surface:    Color,
    pub border:     Color,
    pub text:       Color,
    /// Bright companion to the palette's one hue — light yellow, or mint for
    /// the Yellows palette — that marks the active project / agent tab (see
    /// `buttons::glass_active`).
    pub current:    Color,
}

impl ThemeColors {
    pub fn for_theme(palette: Palette, mode: Mode) -> Self {
        // Scandinavian-inspired palette: muted, low-saturation tones with
        // warm neutrals. Each palette gives three shades of the same hue —
        // primary (most present), secondary (softer), tertiary (whisper).
        let (primary, secondary, tertiary) = match (palette, mode) {
            (Palette::Blues, Mode::Light) => (
                rgb(107, 140, 174),  // dusty steel blue
                rgb(148, 176, 194),  // soft fog
                rgb(216, 226, 233),  // pale ice
            ),
            (Palette::Blues, Mode::Dark) => (
                rgb(143, 168, 194),  // muted sky
                rgb(108, 132, 156),  // calm slate
                rgb(60,  74,  90),   // deep dusk
            ),
            (Palette::Greens, Mode::Light) => (
                rgb(122, 145, 129),  // sage
                rgb(162, 181, 164),  // soft moss
                rgb(218, 226, 217),  // pale celadon
            ),
            (Palette::Greens, Mode::Dark) => (
                rgb(156, 179, 160),  // misty sage
                rgb(106, 126, 112),  // deep moss
                rgb(61,  74,  65),   // forest dusk
            ),
            (Palette::Yellows, Mode::Light) => (
                rgb(170, 148, 96),   // dusty ochre
                rgb(198, 182, 138),  // soft straw
                rgb(232, 224, 198),  // pale parchment
            ),
            (Palette::Yellows, Mode::Dark) => (
                rgb(198, 178, 128),  // muted wheat
                rgb(146, 130, 90),   // deep ochre
                rgb(80,  72,  52),   // umber dusk
            ),
            (Palette::Grays, Mode::Light) => (
                rgb(132, 137, 142),  // warm stone
                rgb(173, 177, 182),  // driftwood
                rgb(220, 222, 225),  // soft pebble
            ),
            (Palette::Grays, Mode::Dark) => (
                rgb(180, 183, 188),  // light stone
                rgb(124, 128, 133),  // mid stone
                rgb(63,  67,  71),   // charcoal
            ),
        };

        // The "you are here" band on the active project / agent tab. The
        // palettes are monochrome, so another shade of the same hue wouldn't
        // set the tab apart; instead the band is a light, luminous yellow — a
        // lit edge the gradient brightens into at the tab's bottom. Pale enough
        // to stay distinct from the saturated amber attention color. The
        // Yellows palette is the exception: yellow would vanish into its ochre
        // tabs, so it gets mint green instead.
        let current = match (palette, mode) {
            (Palette::Yellows, Mode::Light) => rgb(96,  222, 170),  // mint
            (Palette::Yellows, Mode::Dark)  => rgb(146, 240, 198),  // bright mint
            (_,                Mode::Light) => rgb(250, 226, 112),  // light yellow
            (_,                Mode::Dark)  => rgb(255, 238, 150),  // pale yellow
        };

        let (background, surface, border, text) = match (palette, mode) {
            // Light backgrounds tinted with the active palette so the whole
            // window reads as "a muted shade of Blue/Green/Gray".
            (Palette::Blues, Mode::Light) => (
                rgb(176, 196, 213),  // muted blue
                rgb(196, 211, 224),  // panel
                rgb(148, 173, 194),  // border
                rgb(28,  34,  42),
            ),
            (Palette::Greens, Mode::Light) => (
                rgb(180, 200, 184),  // muted celadon
                rgb(202, 217, 204),  // panel
                rgb(154, 176, 158),  // border
                rgb(28,  38,  30),
            ),
            (Palette::Yellows, Mode::Light) => (
                rgb(206, 196, 162),  // muted straw
                rgb(220, 212, 182),  // panel
                rgb(178, 166, 130),  // border
                rgb(40,  36,  24),
            ),
            (Palette::Grays, Mode::Light) => (
                rgb(186, 192, 198),  // muted pebble
                rgb(206, 211, 216),  // panel
                rgb(164, 170, 176),  // border
                rgb(32,  34,  37),
            ),
            // Dark mode is a neutral near-black across all palettes — the
            // theme color shows through buttons and accents, not the canvas.
            (_, Mode::Dark) => (
                rgb(15,  16,  18),   // near-black neutral
                rgb(22,  24,  27),   // raised panel — barely lighter
                rgb(48,  51,  55),   // subtle stone border
                rgb(232, 228, 221),  // warm off-white
            ),
        };

        Self { primary, secondary, tertiary, background, surface, border, text, current }
    }
}

// ── Iced theme ────────────────────────────────────────────────────────────────

/// Build an `iced::Theme` directly from a `ThemeColors`. Used both as a
/// drop-in for `iced_theme(palette, mode)` and to build interpolated themes
/// frame-by-frame during palette/mode transitions.
pub fn iced_theme_from(c: ThemeColors) -> IcedTheme {
    IcedTheme::custom(
        "frms".to_string(),
        IcedPalette {
            background: c.background,
            text:       c.text,
            primary:    c.primary,
            // Muted semantic colors that sit alongside the Scandinavian palette.
            success:    rgb(126, 161, 132),  // soft sage
            danger:     rgb(192, 120, 115),  // dusty terracotta
        },
    )
}

// ── HSL ───────────────────────────────────────────────────────────────────────

/// RGB → HSL, hue in degrees.
pub fn to_hsl(c: Color) -> (f32, f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON);
    let h = if max == c.r {
        60.0 * (((c.g - c.b) / d) % 6.0)
    } else if max == c.g {
        60.0 * ((c.b - c.r) / d + 2.0)
    } else {
        60.0 * ((c.r - c.g) / d + 4.0)
    };
    ((h + 360.0) % 360.0, s.clamp(0.0, 1.0), l)
}

/// HSL → RGB, hue in degrees.
pub fn from_hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h {
        h if h < 60.0  => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _              => (c, 0.0, x),
    };
    Color { r: r + m, g: g + m, b: b + m, a: 1.0 }
}

#[cfg(test)]
mod hsl_tests {
    use super::*;

    #[test]
    fn hsl_round_trips_the_band_colors() {
        for c in [rgb(250, 226, 112), rgb(96, 222, 170), rgb(107, 140, 174)] {
            let (h, s, l) = to_hsl(c);
            let back = from_hsl(h, s, l);
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!((a - b).abs() < 1e-3, "{c:?} → ({h}, {s}, {l}) → {back:?}");
            }
        }
    }
}

// ── interpolation ─────────────────────────────────────────────────────────────

impl ThemeColors {
    pub fn lerp(a: ThemeColors, b: ThemeColors, t: f32) -> Self {
        Self {
            primary:    lerp_color(a.primary,    b.primary,    t),
            secondary:  lerp_color(a.secondary,  b.secondary,  t),
            tertiary:   lerp_color(a.tertiary,   b.tertiary,   t),
            background: lerp_color(a.background, b.background, t),
            surface:    lerp_color(a.surface,    b.surface,    t),
            border:     lerp_color(a.border,     b.border,     t),
            text:       lerp_color(a.text,       b.text,       t),
            current:    lerp_color(a.current,    b.current,    t),
        }
    }
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Cosine ease-in-out — gentle on both ends, no acceleration jolt.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    0.5 - 0.5 * (t * std::f32::consts::PI).cos()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}
