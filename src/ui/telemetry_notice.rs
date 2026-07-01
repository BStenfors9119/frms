//! First-run telemetry/diagnostics notice — a full-screen modal shown once,
//! before any telemetry is sent, so the user knows what's collected and can opt
//! out up front. Mirrors `_docs/TELEMETRY_NOTICE.md` and the Profile > Privacy
//! text. Both choices acknowledge the notice (it won't show again); the choice
//! only sets whether telemetry is on. See `Frms::view` / the `TelemetryNotice*`
//! handlers.

use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, row, text, Space};

use crate::app::Message;
use crate::theme::ThemeColors;
use crate::ui::buttons;

pub fn view<'a>(colors: ThemeColors) -> Element<'a, Message> {
    let surface = colors.surface;
    let border  = colors.tertiary;
    let bg      = colors.background;

    let bullet = |s: &'a str| row![
        text("•").size(13),
        text(s).size(13),
    ]
    .spacing(8)
    .align_y(Alignment::Start);

    let card = container(
        column![
            text("Telemetry & Diagnostics").size(22),
            text("To improve frms and fix problems, the app collects a small \
                  amount of anonymous usage and diagnostic data.")
                .size(13),

            text("What it collects").size(15),
            column![
                bullet("An anonymous install id — not tied to you or any account"),
                bullet("App version, operating system, and CPU architecture"),
                bullet("Coarse feature usage — e.g. pane/model types and session starts"),
                bullet("Error and crash diagnostics (scrubbed) to help fix bugs"),
            ]
            .spacing(4),

            text("What it never collects").size(15),
            text("No names, accounts, hostnames, file names or paths, file \
                  contents, your prompts or chats, database data, or API keys. \
                  Error text is scrubbed and truncated before it's sent.")
                .size(12),

            text("It's on by default. You can change this any time in \
                  Profile → Privacy, or set DO_NOT_TRACK=1 before launch.")
                .size(12),

            row![
                Space::with_width(Length::Fill),
                button(text("Turn it off").size(buttons::TEXT_SIZE))
                    .on_press(Message::TelemetryNoticeChoice(false))
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
                button(text("Keep it on").size(buttons::TEXT_SIZE))
                    .on_press(Message::TelemetryNoticeChoice(true))
                    .padding(buttons::PADDING)
                    .style(buttons::primary),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(14)
        .align_x(Alignment::Start)
        .padding(28),
    )
    .width(Length::Fixed(560.0))
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
