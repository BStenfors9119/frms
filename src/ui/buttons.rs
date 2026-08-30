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
use iced::widget::{button, pick_list};
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

/// Linear interpolate `c` toward white. `t = 0.0` returns `c`; `t = 1.0` returns white.
fn mix_white(c: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: c.r + (1.0 - c.r) * t,
        g: c.g + (1.0 - c.g) * t,
        b: c.b + (1.0 - c.b) * t,
        a: c.a,
    }
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

/// `pick_list` styled to read as a button rather than a form select — same
/// glassy fill, border, and text color, with the handle (arrow) tinted to match
/// so it reads as a subtle affordance, not a chrome dropdown.
fn pick_list_glass(base: Color, status: pick_list::Status) -> pick_list::Style {
    // Map the pick_list's state onto the button states glass_style understands.
    let bstatus = match status {
        pick_list::Status::Hovered => button::Status::Hovered,
        pick_list::Status::Opened  => button::Status::Pressed,
        pick_list::Status::Active  => button::Status::Active,
    };
    let g = glass_style(base, bstatus);
    pick_list::Style {
        text_color:        g.text_color,
        placeholder_color: with_alpha(g.text_color, 0.85),
        handle_color:      with_alpha(g.text_color, 0.7),
        background:        g.background.unwrap_or(Background::Color(Color::TRANSPARENT)),
        border:            g.border,
    }
}

pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    glass_style(theme.extended_palette().danger.base.color, status)
}

pub fn success(theme: &Theme, status: button::Status) -> button::Style {
    glass_style(theme.extended_palette().success.base.color, status)
}

/// Default highlight violet for git-submodule directories, used when the user
/// hasn't picked their own in the Profile tab. Distinct from the primary color
/// so a submodule project reads as clearly separate from the parent repo's own
/// folders; fixed regardless of theme so it stands out in light and dark.
pub const SUBMODULE: Color = Color { r: 0.55, g: 0.44, b: 0.86, a: 1.0 };

/// File-browser row style. `submodule` carries the user's chosen highlight
/// color for submodule directories; `None` (a normal folder/file) falls back to
/// the theme's primary color. Same glass look either way, just a different base.
pub fn file_row(submodule: Option<Color>)
    -> impl Fn(&Theme, button::Status) -> button::Style + Copy
{
    move |theme, status| {
        let base = submodule
            .unwrap_or_else(|| theme.extended_palette().primary.base.color);
        glass_style(base, status)
    }
}

// ── Notes plugin: folders vs. the notes inside them ───────────────────────────
//
// Categories borrow the file browser's submodule highlight — the same color the
// user picks in Profile for "a container of other things", so folders read the
// same way in both panels. The notes filed inside them are shades of the active
// theme's primary, bright enough to sit clearly on top of the folder bar:
//
//   folder header   — the submodule tint (deepened when collapsed)
//   note (filed)    — theme primary, lightly tinted toward white
//   note (selected) — theme primary, tinted much further: the brightest row
//
// Notes therefore follow the palette like the rest of the app, and never
// collide with the folder color no matter which palette is active.

/// How far a folder header is pushed toward black, open vs. collapsed.
const FOLDER_SINK: (f32, f32) = (0.0, 0.35);
/// How far a note row is tinted toward white, unselected vs. selected.
const NOTE_TINT:   (f32, f32) = (0.30, 0.72);

/// Category (folder) row in the Notes list. `folder` is the user's submodule
/// highlight color (see [`SUBMODULE`]); a collapsed folder sinks toward black —
/// put away, but still recognisably the same color as when open.
pub fn note_category(folder: Color, collapsed: bool)
    -> impl Fn(&Theme, button::Status) -> button::Style + Copy
{
    move |_theme, status| {
        let sink = if collapsed { FOLDER_SINK.1 } else { FOLDER_SINK.0 };
        glass_style(mix_black(folder, sink), status)
    }
}

/// A note row in the Notes list — a shade of the theme's primary, kept light so
/// it reads as content sitting inside the folder rather than another folder;
/// the selected note goes brighter still so it's the lightest row on screen.
pub fn note_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |theme, status| {
        let tint = if selected { NOTE_TINT.1 } else { NOTE_TINT.0 };
        glass_style(mix_white(theme.extended_palette().primary.base.color, tint), status)
    }
}

/// The Notes editor's category dropdown. Once a note is filed the picker wears
/// the folder color, matching its group header in the list; while it's still
/// uncategorized it stays neutral, so the color means "in a folder".
pub fn pick_list_note_category(folder: Option<Color>)
    -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style + Copy
{
    move |theme, status| {
        let base = folder.unwrap_or_else(|| theme.extended_palette().secondary.base.color);
        pick_list_glass(base, status)
    }
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

/// Inline icon button that matches a [`toggle`]-styled tab — transparent so it
/// reads as part of the tab, with its glyph color computed to match the tab's
/// label (active vs inactive) in any theme. Use for an inline ✕ / control tucked
/// inside a toggle tab button.
pub fn toggle_icon(selected: bool)
    -> impl Fn(&Theme, button::Status) -> button::Style + Copy
{
    move |theme, status| {
        let palette = theme.extended_palette();
        let base = if selected {
            palette.primary.base.color
        } else {
            palette.secondary.base.color
        };
        // Same derivation glass_style uses for a toggle button's text color, so
        // the glyph sits on the tab as if it were part of the label.
        let fg = contrast_text(mix_black(base, 0.30));
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
