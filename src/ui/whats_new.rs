//! "What's New" modal — shown once after an upgrade, with compact feature
//! callouts and a keyboard-shortcut reference. Dismissed with OK, which stamps
//! the current version into prefs so it won't reappear until the next release.

use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, container, row, text, Column, Row, Space};

use crate::app::Message;
use crate::fonts::UI_FONT;
use crate::theme::ThemeColors;
use crate::ui::buttons;
use crate::version::VERSION;

struct Callout {
    title: &'static str,
    detail: &'static str,
}

const CALLOUTS: &[Callout] = &[
    Callout {
        title:  "Agent Management",
        detail: "Start, rename, switch models, or stop — all from one dialog (Ctrl+Shift+M)",
    },
    Callout {
        title:  "Multi-Tool Agents",
        detail: "Claude, Windsurf, and more side by side — pick your default in Profile",
    },
    Callout {
        title:  "Token Usage",
        detail: "Live bars in the header — click one for a per-model breakdown",
    },
    Callout {
        title:  "Chat Panes",
        detail: "Research and Chat modes via headless Claude, with configurable access",
    },
    Callout {
        title:  "Profile & Appearance",
        detail: "Themes, font scale, privacy, and accent / active colors via HSL picker",
    },
    Callout {
        title:  "Notes & Workspace",
        detail: "Categorized notes, split-view sessions, and persistent layouts",
    },
];

const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+Shift+T",        "New agent"),
    ("Ctrl+Shift+M",        "Agents dialog"),
    ("Ctrl+Shift+L",        "Find in terminal"),
    ("Ctrl+Shift+E",        "File browser"),
    ("Ctrl+Shift+F",        "Full-screen toolbox"),
    ("Ctrl+Shift+N",        "Selection \u{2192} note"),
    ("Ctrl+Shift+C / V",    "Terminal copy / paste"),
    ("Ctrl+1\u{2026}9",     "Jump to session"),
    ("Ctrl+Shift+1\u{2026}9", "Jump to agent"),
    ("Ctrl+Shift+S",        "Search docs"),
    ("Ctrl+Shift+G",        "Search notes"),
    ("Ctrl+S",              "Save file"),
];

/// True when the user hasn't seen the current version yet.
pub fn has_news(last_seen: Option<&str>) -> bool {
    match last_seen {
        Some(v) => v != VERSION,
        None    => false,
    }
}

/// Full-screen modal with compact feature callouts and keyboard shortcuts.
pub fn view(colors: ThemeColors) -> Element<'static, Message> {
    let surface   = colors.surface;
    let border_c  = colors.tertiary;
    let bg        = colors.background;
    let txt       = colors.text;
    let secondary = colors.secondary;
    let bdr       = colors.border;
    let accent    = buttons::ACCENT_TINT;

    let mut body = Column::new().spacing(12);

    body = body.push(
        text(format!("What's New in frms {VERSION}"))
            .font(UI_FONT)
            .size(22)
            .style(move |_: &Theme| text::Style { color: Some(accent) }),
    );

    // ── feature callouts ─────────────────────────────────────────────────
    for callout in CALLOUTS {
        let card = Column::new()
            .spacing(2)
            .push(
                text(callout.title)
                    .font(UI_FONT)
                    .size(15)
                    .style(move |_: &Theme| text::Style { color: Some(txt) }),
            )
            .push(
                text(callout.detail)
                    .font(UI_FONT)
                    .size(13)
                    .style(move |_: &Theme| text::Style { color: Some(secondary) }),
            );

        body = body.push(
            container(card.padding([6, 12]))
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    border: Border { color: bdr, width: 1.0, radius: 4.0.into() },
                    ..Default::default()
                }),
        );
    }

    // ── keyboard shortcuts ───────────────────────────────────────────────
    body = body.push(
        text("Keyboard Shortcuts")
            .font(UI_FONT)
            .size(15)
            .style(move |_: &Theme| text::Style { color: Some(accent) }),
    );

    let mid = SHORTCUTS.len().div_ceil(2);

    let build_col = |entries: &[(&'static str, &'static str)]| -> Column<'static, Message> {
        let mut col = Column::new().spacing(2);
        for &(key, desc) in entries {
            col = col.push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        container(
                            text(key).font(UI_FONT).size(12)
                                .style(move |_: &Theme| text::Style { color: Some(txt) }),
                        )
                        .width(Length::Fixed(140.0)),
                    )
                    .push(
                        text(desc).font(UI_FONT).size(12)
                            .style(move |_: &Theme| text::Style { color: Some(secondary) }),
                    ),
            );
        }
        col
    };

    body = body.push(
        Row::new()
            .spacing(24)
            .push(build_col(&SHORTCUTS[..mid]))
            .push(build_col(&SHORTCUTS[mid..])),
    );

    // ── dismiss ──────────────────────────────────────────────────────────
    body = body.push(
        row![
            Space::with_width(Length::Fill),
            button(text("OK").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::WhatsNewDismissed)
                .padding(buttons::PADDING)
                .style(buttons::primary),
        ]
        .align_y(Alignment::Center),
    );

    let card = container(body.padding(28))
        .width(Length::Fixed(560.0))
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(surface)),
            border:     Border { color: border_c, width: 1.0, radius: 6.0.into() },
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
