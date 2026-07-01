use iced::{Alignment, Background, Border, Color, Element, Length, Renderer, Theme};
use iced::widget::{button, container, row, scrollable, text, text_input, tooltip};

use crate::app::{Message, SplitSlot};
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::session::Session;
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Renders the session tab bar.
///
/// When `slot` is `Some`, the bar lives inside a split slot — clicking a tab
/// swaps that slot to display the chosen session rather than mutating the
/// global active session, and `active` should be the slot's current session
/// index (so the right tab is highlighted on its side).
pub fn view<'a>(
    sessions:    &'a [Session],
    active:      usize,
    renaming:    &'a Option<(usize, String)>,
    split_ids:   Option<(usize, usize)>,
    slot:        Option<SplitSlot>,
    colors:      ThemeColors,
) -> Element<'a, Message, Theme, Renderer> {
    let mut tabs: iced::widget::Row<'a, Message, Theme, Renderer> =
        row![].spacing(6).padding([4, 8]);

    for (idx, session) in sessions.iter().enumerate() {
        let tab: Element<'a, Message, Theme, Renderer> =
            if matches!(renaming, Some((i, _)) if *i == idx) {
                let draft = renaming.as_ref().map(|(_, s)| s.as_str()).unwrap_or("");
                row![
                    text_input::<Message, Theme, Renderer>("Session name", draft)
                        .id(text_input::Id::new("session-rename"))
                        .on_input(Message::SessionRenameEdited)
                        .on_submit(Message::SessionRenameConfirmed)
                        .width(Length::Fixed(120.0)),
                    icon_btn("✓", 16, Message::SessionRenameConfirmed, buttons::primary),
                    icon_btn("✕", 14, Message::SessionRenameCancelled, buttons::secondary),
                ]
                .spacing(6)
                .into()
            } else {
                let in_split = split_ids
                    .map(|(top, bot)| session.id == top || session.id == bot)
                    .unwrap_or(false);
                session_tab(idx, session, idx == active, in_split, slot, colors)
            };

        tabs = tabs.push(tab);
    }

    tabs = tabs.push(
        button(text("+").font(UI_FONT).size(22))
            .on_press(Message::NewSessionRequested)
            .padding([buttons::PADDING[0] - 4, 14])
            .style(buttons::primary),
    );

    // Let the row keep its natural width and scroll horizontally once the
    // session tabs overflow the pane — otherwise they bunch up / get clipped.
    // The scrollbar only materialises when the content is wider than the bar,
    // so the common (few-sessions) case looks exactly as before.
    let bar = scrollable(tabs)
        .width(Length::Fill)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(4).scroller_width(4),
        ));

    let surface = colors.surface;
    let accent  = colors.tertiary;
    container(bar)
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(surface)),
            border:     Border { color: accent, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into()
}

/// Single-button tab containing the session name, an inline pencil for
/// rename, and an inline × for close — all inside the same glass surface so
/// it reads as one tab rather than three buttons.
fn session_tab<'a>(
    idx:        usize,
    session:    &'a Session,
    is_active:  bool,
    in_split:   bool,
    slot:       Option<SplitSlot>,
    colors:     ThemeColors,
) -> Element<'a, Message, Theme, Renderer> {
    // Any Claude terminal in this session stopped on a question? Turn the
    // whole session tab amber so the user notices even while working in a
    // different session.
    let waiting = session.panes.iter().any(|p| p.waiting_on_user());

    let fg = if waiting {
        buttons::tab_text_color(buttons::ATTENTION, buttons::ATTENTION, is_active)
    } else {
        buttons::tab_text_color(colors.primary, colors.secondary, is_active)
    };

    let edit_btn = button(text("✎").font(ICON_FONT).size(13))
        .on_press(Message::SessionRenameStarted(idx))
        .padding([2, 6])
        .style(buttons::embedded_icon(fg));

    // ⮃ toggles the session into the bottom row of the two-up split view.
    // While in the split, the highlight color flips to the accent so the
    // user can see at a glance which two sessions are paired.
    // (U+2B83 — ⇅ U+21C5 is absent from the bundled Noto Sans Symbols 2
    // and rendered as a tofu box.)
    let split_fg = if in_split { colors.tertiary } else { fg };
    let split_btn = button(text("⮃").font(ICON_FONT).size(13))
        .on_press(Message::SessionSplitToggled(session.id))
        .padding([2, 6])
        .style(buttons::embedded_icon(split_fg));

    // 🖈 marks the session as pinned — pinned sessions are saved and restored
    // on the next launch; unpinned ones are ephemeral. Always a light color so
    // it never blends into the tab: unmistakable gold when pinned, off-white
    // when not. (Solid pushpin U+1F588 — the old hollow star's hairline
    // strokes were nearly invisible at this size.)
    let pin_fg = if session.pinned {
        Color::from_rgb(1.0, 0.80, 0.20)
    } else {
        Color::from_rgb(0.93, 0.92, 0.90)
    };
    let pin_btn = button(text("🖈").font(ICON_FONT).size(14))
        .on_press(Message::SessionPinToggled(session.id))
        .padding([2, 6])
        .style(buttons::embedded_icon(pin_fg));

    let close_btn = button(text("✕").font(ICON_FONT).size(13))
        .on_press(Message::SessionCloseRequested(idx))
        .padding([2, 6])
        .style(buttons::embedded_icon(fg));

    let on_press = match slot {
        Some(s) => Message::SplitSlotSessionSelected(s, idx),
        None    => Message::SessionSelected(idx),
    };

    let mut inner = row![].spacing(6).align_y(Alignment::Center);
    if waiting {
        // ● badge — a Claude terminal inside is waiting on an answer.
        inner = inner.push(text("●").font(ICON_FONT).size(11));
    }
    inner = inner
        .push(
            text(session.name.as_str())
                .font(UI_FONT)
                .size(buttons::TEXT_SIZE),
        )
        .push(with_tip(edit_btn, "Rename session"))
        .push(with_tip(
            split_btn,
            if in_split { "Remove from split view" } else { "Show in split view" },
        ))
        .push(with_tip(
            pin_btn,
            if session.pinned {
                "Unpin (session won't be restored on restart)"
            } else {
                "Pin — restore this session on next launch"
            },
        ))
        .push(with_tip(close_btn, "Close session"));

    let tab = button(inner)
        .on_press(on_press)
        .padding([buttons::PADDING[0], 10]);
    if waiting {
        tab.style(buttons::attention_tab(is_active)).into()
    } else {
        tab.style(buttons::tab(
            colors.primary,
            colors.secondary,
            colors.tertiary,
            is_active,
        ))
        .into()
    }
}

/// Wrap an inline tab icon in a hover tooltip — the glyph buttons are tiny,
/// so spell out what each one does.
fn with_tip<'a>(
    btn: iced::widget::Button<'a, Message, Theme, Renderer>,
    tip: &'a str,
) -> Element<'a, Message, Theme, Renderer> {
    tooltip(btn, text(tip).font(UI_FONT).size(11), tooltip::Position::Bottom)
        .gap(4)
        .padding(6)
        .style(container::rounded_box)
        .into()
}

/// Single-glyph glass button using `ICON_FONT` at a custom size.
fn icon_btn<'a, F>(
    glyph: &str,
    size:  u16,
    msg:   Message,
    style: F,
) -> iced::widget::Button<'a, Message, Theme, Renderer>
where
    F: Fn(&Theme, button::Status) -> button::Style + 'a,
{
    button(text(glyph.to_owned()).font(ICON_FONT).size(size))
        .on_press(msg)
        .padding([buttons::PADDING[0] - 2, 10])
        .style(style)
}
