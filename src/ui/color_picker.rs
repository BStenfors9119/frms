//! A small HSL color picker: a preview swatch and `#RRGGBB` field over hue /
//! saturation / lightness sliders, each painted with the range of colors it
//! picks between. iced ships no picker widget, so this is stock sliders with
//! transparent rails stacked on gradient bars.

use std::f32::consts::FRAC_PI_2;
use std::ops::RangeInclusive;

use iced::gradient::Linear;
use iced::widget::{column, container, row, slider, stack, text, text_input, Space};
use iced::{Alignment, Background, Border, Color, Element, Gradient, Length, Radians, Theme};

use crate::app::Message;
use crate::fonts::UI_FONT;
use crate::theme::{from_hsl, to_hsl};

/// Saturation and lightness stop short of their ends: a fully desaturated,
/// black, or white color has no hue, so reaching an end would snap the hue
/// slider back to red and lose the hue the user had dialed in.
const S_MIN: f32 = 0.05;
const L_MIN: f32 = 0.08;
const L_MAX: f32 = 0.95;

/// Height of a slider row, and of the painted bar running through it.
const ROW_H: f32 = 16.0;
const BAR_H: f32 = 8.0;

/// The picker, showing `color`. `hex` is the text for the hex field — normally
/// `color` as `#RRGGBB`, or whatever partial value the user is typing.
///
/// Dragging a slider sends `on_change` with the new color on every move and
/// `on_release` once it's let go, so the caller can apply the color live and
/// save it once per drag. Hex edits go to `on_hex` raw; the caller decides
/// when they parse. `extra` sits at the end of the top row (e.g. a Reset).
pub fn view<'a>(
    color:      Color,
    hex:        String,
    on_change:  impl Fn(Color) -> Message + Copy + 'a,
    on_release: Message,
    on_hex:     impl Fn(String) -> Message + 'a,
    extra:      Element<'a, Message>,
) -> Element<'a, Message> {
    let (h, s, l) = to_hsl(color);
    let s = s.max(S_MIN);
    let l = l.clamp(L_MIN, L_MAX);

    let swatch = container(Space::new(Length::Fixed(22.0), Length::Fixed(22.0)))
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color:  Color { a: 0.35, ..Color::WHITE },
                width:  1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        });
    let hex_field = text_input("#RRGGBB", &hex)
        .on_input(on_hex)
        .font(UI_FONT)
        .size(13)
        .width(Length::Fixed(90.0));
    let top = row![swatch, hex_field, extra].spacing(8).align_y(Alignment::Center);

    // The hue bar is painted at a fixed, vivid saturation and lightness: at the
    // current ones a muted pick would turn the whole bar grey and hide what the
    // slider chooses between.
    let hue_stops = (0..=6).map(|i| from_hsl((i as f32 * 60.0) % 360.0, 0.85, 0.55)).collect();
    let hue = slider_row(
        "H", 0.0..=359.0, h, 1.0, hue_stops, color,
        move |h| on_change(from_hsl(h, s, l)), on_release.clone(),
    );
    let sat = slider_row(
        "S", S_MIN..=1.0, s, 0.01,
        vec![from_hsl(h, S_MIN, l), from_hsl(h, 1.0, l)], color,
        move |s| on_change(from_hsl(h, s, l)), on_release.clone(),
    );
    let lit = slider_row(
        "L", L_MIN..=L_MAX, l, 0.01,
        vec![from_hsl(h, s, L_MIN), from_hsl(h, s, 0.5), from_hsl(h, s, L_MAX)], color,
        move |l| on_change(from_hsl(h, s, l)), on_release,
    );

    column![top, hue, sat, lit].spacing(6).into()
}

/// One labelled slider over a bar painted with `stops`, spread evenly left to
/// right. The rails are transparent so the bar shows through; the handle wears
/// the `picked` color.
#[allow(clippy::too_many_arguments)]
fn slider_row<'a>(
    label:      &'static str,
    range:      RangeInclusive<f32>,
    value:      f32,
    step:       f32,
    stops:      Vec<Color>,
    picked:     Color,
    on_change:  impl Fn(f32) -> Message + 'a,
    on_release: Message,
) -> Element<'a, Message> {
    let last = (stops.len().saturating_sub(1)).max(1) as f32;
    let gradient = stops.iter().enumerate().fold(
        Linear::new(Radians(FRAC_PI_2)),
        |g, (i, c)| g.add_stop(i as f32 / last, *c),
    );
    let bar = container(Space::new(Length::Fill, Length::Fixed(BAR_H)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Gradient(Gradient::Linear(gradient))),
            border: Border { radius: (BAR_H / 2.0).into(), ..Default::default() },
            ..Default::default()
        });
    let bar = container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .align_y(Alignment::Center);

    let input = slider(range, value, on_change)
        .step(step)
        .on_release(on_release)
        .height(ROW_H)
        .style(move |_: &Theme, _| slider::Style {
            rail: slider::Rail {
                backgrounds: (
                    Background::Color(Color::TRANSPARENT),
                    Background::Color(Color::TRANSPARENT),
                ),
                width:  BAR_H,
                border: Border::default(),
            },
            handle: slider::Handle {
                shape:        slider::HandleShape::Circle { radius: 7.0 },
                background:   Background::Color(picked),
                border_width: 2.0,
                border_color: Color::WHITE,
            },
        });

    row![
        text(label).font(UI_FONT).size(11).width(Length::Fixed(12.0)),
        stack![bar, input],
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}
