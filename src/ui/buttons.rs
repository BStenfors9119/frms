//! Centralized button styling.
//!
//! Every button in the app pulls its visual from this module so that:
//! - Padding / radius / text size are uniform.
//! - Hover, press, and disabled states behave the same everywhere.
//! - The glassy, slightly translucent feel is consistent across themes.
//!
//! Apply by combining the `padding()` helper with one of the style closures:
//! `button(label).padding(buttons::PADDING).style(buttons::primary)`.

use iced::gradient::Linear;
use iced::widget::button;
use iced::{Background, Border, Color, Gradient, Radians, Shadow, Theme, Vector};

// ── uniform sizing ────────────────────────────────────────────────────────────

pub const PADDING:    [u16; 2] = [7, 14];
pub const RADIUS:     f32       = 6.0;
pub const TEXT_SIZE:  u16       = 12;

// ── helpers ───────────────────────────────────────────────────────────────────

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

fn lighten(c: Color, amount: f32) -> Color {
    Color {
        r: (c.r + amount).clamp(0.0, 1.0),
        g: (c.g + amount).clamp(0.0, 1.0),
        b: (c.b + amount).clamp(0.0, 1.0),
        a: c.a,
    }
}

/// Linear interpolate `c` toward black. `t = 0.0` returns `c`; `t = 1.0` returns black.
fn mix_black(c: Color, t: f32) -> Color {
    let k = 1.0 - t.clamp(0.0, 1.0);
    Color { r: c.r * k, g: c.g * k, b: c.b * k, a: c.a }
}

fn contrast_text(bg: Color) -> Color {
    let luminance = 0.299 * bg.r + 0.587 * bg.g + 0.114 * bg.b;
    if luminance > 0.55 {
        Color::from_rgb(0.08, 0.09, 0.10)
    } else {
        Color::from_rgb(0.96, 0.95, 0.93)
    }
}

/// Glassy beveled button. Three-stop vertical gradient — bright highlight at
/// the top, the deepened base in the body, fading toward black at the bottom —
/// with a soft hairline edge and a drop shadow.
fn glass_style(base: Color, status: button::Status) -> button::Style {
    // Push the base toward black so the colored fill reads deep, not bright.
    let deep = mix_black(base, 0.30);

    let (lift_top, lift_mid, sink_bot, alpha) = match status {
        button::Status::Hovered  => (0.22, 0.06, 0.40, 0.96),
        button::Status::Pressed  => (0.05, -0.03, 0.55, 0.92),
        button::Status::Disabled => (0.05,  0.0,  0.50, 0.40),
        _                        => (0.16,  0.0,  0.50, 0.92),
    };

    let highlight = with_alpha(lighten(deep, lift_top), alpha);
    let body      = with_alpha(lighten(deep, lift_mid), alpha);
    let bottom    = with_alpha(mix_black(deep, sink_bot), alpha);

    let bg = Background::Gradient(Gradient::Linear(
        Linear::new(Radians(std::f32::consts::PI))
            .add_stop(0.0,  highlight)
            .add_stop(0.18, body)
            .add_stop(1.0,  bottom),
    ));

    // Hairline outer edge — slightly brighter than the highlight to suggest a
    // beveled rim catching light.
    let edge = with_alpha(lighten(base, 0.22), 0.55);

    button::Style {
        background: Some(bg),
        text_color: contrast_text(deep),
        border: Border {
            color:  edge,
            width:  1.0,
            radius: RADIUS.into(),
        },
        shadow: Shadow {
            color:  Color { a: 0.40, ..Color::BLACK },
            offset: Vector::new(0.0, 2.0),
            blur_radius: 6.0,
        },
    }
}

// ── public styles (closures usable as `Button::style`) ────────────────────────

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    glass_style(theme.extended_palette().primary.base.color, status)
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    glass_style(theme.extended_palette().secondary.base.color, status)
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    glass_style(theme.extended_palette().danger.base.color, status)
}

pub fn success(theme: &Theme, status: button::Status) -> button::Style {
    glass_style(theme.extended_palette().success.base.color, status)
}

/// Warm amber used to flag tabs whose Claude terminal has stopped on a
/// question and is waiting for the user to answer.
pub const ATTENTION: Color = Color { r: 0.85, g: 0.58, b: 0.16, a: 1.0 };

/// Attention-grabbing tab/toggle variant — amber regardless of theme so a
/// waiting Claude session stands out in both the session bar and the center
/// tab bar. `selected` keeps the active tab a touch brighter, matching the
/// active/inactive contrast of the normal styles.
pub fn attention_tab(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |_theme, status| {
        let base = if selected { ATTENTION } else { mix_black(ATTENTION, 0.25) };
        glass_style(base, status)
    }
}

/// Toggle-style button — `selected` picks primary (active) vs secondary
/// (inactive) without changing size.
pub fn toggle(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |theme, status| {
        let palette = theme.extended_palette();
        let base = if selected {
            palette.primary.base.color
        } else {
            palette.secondary.base.color
        };
        glass_style(base, status)
    }
}

/// Tab-style button driven by an explicit primary/secondary/tertiary triple
/// (used by the session tab bar so the active tab clearly stands out).
pub fn tab(active: Color, inactive: Color, hover: Color, selected: bool)
    -> impl Fn(&Theme, button::Status) -> button::Style
{
    move |_theme, status| {
        let base = match status {
            button::Status::Hovered => hover,
            _ => if selected { active } else { inactive },
        };
        glass_style(base, status)
    }
}

/// The same text color the `tab` style chooses for its label — exposed so
/// embedded icon buttons inside a tab can match.
pub fn tab_text_color(active: Color, inactive: Color, selected: bool) -> Color {
    let base = if selected { active } else { inactive };
    contrast_text(mix_black(base, 0.30))
}

/// Transparent inline icon button. Picks up hover from a translucent overlay
/// of the foreground color so the same closure works on light and dark tabs.
/// Use when you want a button to read as part of a surrounding tab/card.
pub fn embedded_icon(fg: Color)
    -> impl Fn(&Theme, button::Status) -> button::Style + Copy
{
    move |_theme, status| {
        let overlay_alpha = match status {
            button::Status::Hovered => 0.20,
            button::Status::Pressed => 0.32,
            _ => 0.0,
        };
        button::Style {
            background: (overlay_alpha > 0.0).then(|| {
                Background::Color(Color { a: overlay_alpha, ..fg })
            }),
            text_color: fg,
            border: Border { color: Color::TRANSPARENT, width: 0.0, radius: 4.0.into() },
            shadow: Shadow::default(),
        }
    }
}

/// Flat row inside a dropdown/popup menu — transparent until hovered, then a
/// subtle highlight. Used by the "+ Agent" menu so each choice reads as a list
/// item rather than a raised button.
pub fn menu_item(_theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Some(Background::Color(Color::from_rgb8(58, 58, 72))),
        button::Status::Pressed => Some(Background::Color(Color::from_rgb8(70, 70, 86))),
        _ => None,
    };
    button::Style {
        background: bg,
        text_color: Color::from_rgb(0.90, 0.90, 0.93),
        border: Border { color: Color::TRANSPARENT, width: 0.0, radius: 4.0.into() },
        shadow: Shadow::default(),
    }
}
