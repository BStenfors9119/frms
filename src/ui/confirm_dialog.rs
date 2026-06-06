//! Centered confirmation dialog overlaid on the IDE — used before closing a
//! session tab or a Claude tab, since the inline ✕ sits right next to the
//! other tab icons and is easy to hit by accident.

use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{button, center, column, container, mouse_area, opaque, row, text, Space};

use crate::app::Message;
use crate::fonts::UI_FONT;
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Full-window overlay: dimmed backdrop with a centered card. Clicking the
/// backdrop cancels; the card offers explicit Cancel / confirm buttons.
/// Stack this on top of the IDE view (`iced::widget::stack`).
pub fn overlay<'a>(
    title:      String,
    detail:     String,
    confirm:    &'a str,
    on_confirm: Message,
    on_cancel:  Message,
    colors:     ThemeColors,
) -> Element<'a, Message> {
    let card = container(
        column![
            text(title).font(UI_FONT).size(16),
            text(detail).font(UI_FONT).size(13),
            row![
                Space::with_width(Length::Fill),
                button(text("Cancel").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(on_cancel.clone())
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
                button(text(confirm).font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(on_confirm)
                    .padding(buttons::PADDING)
                    .style(buttons::danger),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(14),
    )
    .padding(20)
    .width(Length::Fixed(380.0))
    .style(move |_theme: &Theme| container::Style {
        background: Some(Background::Color(colors.surface)),
        border:     Border { color: colors.tertiary, width: 1.0, radius: 8.0.into() },
        text_color: Some(colors.text),
        ..Default::default()
    });

    // `opaque` blocks mouse events from reaching the IDE underneath; the
    // mouse_area turns a click on the dimmed backdrop into a cancel.
    opaque(
        mouse_area(
            center(opaque(card)).style(|_theme: &Theme| container::Style {
                background: Some(Background::Color(Color { a: 0.55, ..Color::BLACK })),
                ..Default::default()
            }),
        )
        .on_press(on_cancel),
    )
}
