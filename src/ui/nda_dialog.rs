//! First-run NDA gate — a full-screen modal shown once, before the app can be
//! used, requiring acceptance of the Non-Disclosure Agreement. Declining exits
//! the app. This is the cross-format acceptance (rpm, deb, bare binary) that
//! replaced the deb-only debconf gate, so every install path records the same
//! per-user acceptance. The agreement text is `_docs/NDA.md`, embedded at build
//! time. See `Frms::view` and the `NdaChoice` handler.

use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, scrollable, text, row, Space};

use crate::app::Message;
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// The agreement, embedded from the source-of-truth markdown.
const NDA_MD: &str = include_str!("../../_docs/NDA.md");

/// Strip the markdown to readable plain text for the dialog: drop the HTML
/// comment header, bold markers, heading hashes, and horizontal rules.
fn nda_body() -> String {
    let mut out = String::new();
    let mut in_comment = false;
    for line in NDA_MD.lines() {
        if line.contains("<!--") {
            in_comment = true;
        }
        if in_comment {
            if line.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        let mut l = line.replace("**", "");
        let trimmed = l.trim_start();
        if trimmed.starts_with('#') {
            l = trimmed.trim_start_matches('#').trim_start().to_string();
        }
        if l.trim() == "---" {
            l = String::new();
        }
        out.push_str(&l);
        out.push('\n');
    }
    out.trim().to_string()
}

pub fn view<'a>(colors: ThemeColors) -> Element<'a, Message> {
    let surface = colors.surface;
    let border  = colors.tertiary;
    let bg      = colors.background;

    let agreement = scrollable(
        text(nda_body()).size(12),
    )
    .height(Length::Fill);

    let card = container(
        column![
            text("frms — Non-Disclosure Agreement").size(22),
            text("Please read the agreement below. You must accept it to use frms.")
                .size(12),
            container(agreement)
                .height(Length::Fill)
                .width(Length::Fill)
                .padding(12)
                .style(move |_theme: &Theme| container::Style {
                    background: Some(Background::Color(bg)),
                    border:     Border { color: border, width: 1.0, radius: 4.0.into() },
                    ..Default::default()
                }),
            row![
                Space::with_width(Length::Fill),
                button(text("Decline & Exit").size(buttons::TEXT_SIZE))
                    .on_press(Message::NdaChoice(false))
                    .padding(buttons::PADDING)
                    .style(buttons::danger),
                button(text("I Accept").size(buttons::TEXT_SIZE))
                    .on_press(Message::NdaChoice(true))
                    .padding(buttons::PADDING)
                    .style(buttons::primary),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(14)
        .padding(28),
    )
    .width(Length::Fixed(680.0))
    .height(Length::Fixed(620.0))
    .style(move |_theme: &Theme| container::Style {
        background: Some(Background::Color(surface)),
        border:     Border { color: border, width: 1.0, radius: 6.0.into() },
        ..Default::default()
    });

    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(bg)),
            ..Default::default()
        })
        .into()
}
