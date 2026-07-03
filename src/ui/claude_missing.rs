//! Startup notice shown when the Claude Code CLI (`claude`) isn't on the PATH.
//!
//! frms drives every agent pane — Build (PTY) and the Research / Chat chats —
//! through the `claude` CLI, so without it the core of the app can't work. This
//! full-screen prompt appears after the first-run gates whenever `claude` is
//! missing, explains how to install it, and offers a Re-check (after installing
//! in a terminal) or Continue-anyway. See `Frms::view` and the `Claude*`
//! handlers; the install command mirrors the agent-pane fallback in `session.rs`.

use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, row, text, Space};

use crate::app::Message;
use crate::fonts::MONO_FONT;
use crate::theme::ThemeColors;
use crate::ui::buttons;

pub fn view<'a>(colors: ThemeColors) -> Element<'a, Message> {
    let surface = colors.surface;
    let border  = colors.tertiary;
    let bg      = colors.background;

    let bullet = |s: &'a str| row![text("•").size(13), text(s).size(13)]
        .spacing(8)
        .align_y(Alignment::Start);

    let card = container(
        column![
            text("Claude Code is required").size(22),
            text("frms runs its AI agents through the Claude Code CLI (\"claude\"), \
                  but it isn't on your PATH — so agent panes can't start yet.")
                .size(13),

            text("Install it").size(15),
            container(
                text("npm install -g @anthropic-ai/claude-code")
                    .font(MONO_FONT)
                    .size(13),
            )
            .padding([8, 12])
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(bg)),
                border: Border { color: border, width: 1.0, radius: 4.0.into() },
                ..Default::default()
            }),
            column![
                bullet("Requires Node.js / npm. No Node yet? Install it from \
                        nodejs.org (Windows) or your package manager (Linux), \
                        then run the command above."),
                bullet("After installing, run \"claude\" once to sign in (a Claude \
                        Pro/Max subscription — no API key needed)."),
                bullet("Then Re-check below, or open a terminal pane to install \
                        without leaving frms."),
            ]
            .spacing(4),

            row![
                Space::with_width(Length::Fill),
                button(text("Continue anyway").size(buttons::TEXT_SIZE))
                    .on_press(Message::ClaudeNoticeDismissed)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
                button(text("Re-check").size(buttons::TEXT_SIZE))
                    .on_press(Message::ClaudeRecheck)
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
