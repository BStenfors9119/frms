use iced::{Element, Length};
use iced::widget::{button, column, container, image, mouse_area, row, scrollable, text, text_input};

use crate::app::Message;
use crate::cdp::{VIEWPORT_H, VIEWPORT_W};
use crate::fonts::UI_FONT;
use crate::ui::buttons;

/// Browser pane — URL bar + interactive screenshot view.
pub fn view<'a>(
    url_input:  &'a str,
    screenshot: Option<&'a [u8]>,
    loading:    bool,
    error:      Option<&'a str>,
    cdp_active: bool,
) -> Element<'a, Message> {
    let go_btn = button(text("Go").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::BrowserUrlSubmitted)
        .padding(buttons::PADDING)
        .style(buttons::primary);

    let refresh_btn = if loading {
        button(text("Refresh").font(UI_FONT).size(buttons::TEXT_SIZE))
            .padding(buttons::PADDING)
            .style(buttons::secondary)
    } else {
        button(text("Refresh").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::BrowserRefresh)
            .padding(buttons::PADDING)
            .style(buttons::secondary)
    };

    let url_bar = row![
        text_input("http://localhost:3000", url_input)
            .on_input(Message::BrowserUrlEdited)
            .on_submit(Message::BrowserUrlSubmitted)
            .width(Length::Fill),
        go_btn,
        refresh_btn,
    ]
    .spacing(4);

    let body: Element<'a, Message> = if loading {
        container(text("Loading...").size(14))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    } else if let Some(bytes) = screenshot {
        let handle = image::Handle::from_bytes(bytes.to_vec());
        let img = image(handle)
            .width(Length::Fixed(VIEWPORT_W as f32))
            .height(Length::Fixed(VIEWPORT_H as f32));

        if cdp_active {
            // Wrap in mouse_area so clicks and cursor movement are forwarded to CDP
            scrollable(
                mouse_area(img)
                    .on_move(|p| Message::BrowserMouseMoved(p.x, p.y))
                    .on_press(Message::BrowserMousePressed),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            scrollable(img)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    } else if let Some(err) = error {
        container(
            column![
                text("Failed to load page").size(14),
                text(err).size(11),
                text("Make sure chromium or google-chrome is installed.").size(11),
            ]
            .spacing(6),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        container(
            column![
                text("Browser Preview").size(14),
                text("Enter a URL above and press Go or hit Enter.").size(12),
                text("Requires chromium or google-chrome to be installed.").size(11),
            ]
            .spacing(8),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    };

    column![url_bar, body]
        .spacing(4)
        .padding(8)
        .height(Length::Fill)
        .into()
}
