//! Full-screen file picker for choosing a local file to copy to a receiver.
//!
//! Mirrors the directory chooser in `new_session_dialog`, but lists files as
//! well as directories: clicking a directory navigates into it, clicking a
//! file selects it (and triggers the copy). Kept dependency-free — the app
//! ships no native file-dialog tool, so it reuses the bundled file browser.

use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, row, scrollable, text, Space};

use crate::app::Message;
use crate::file_browser::FileBrowserState;
use crate::fonts::UI_FONT;
use crate::theme::ThemeColors;
use crate::ui::buttons;

pub fn view<'a>(
    browser:       &'a FileBrowserState,
    receiver_name: &'a str,
    colors:        ThemeColors,
) -> Element<'a, Message> {
    let header = row![
        text(format!("Choose a file to copy to “{receiver_name}”")).size(18),
        Space::with_width(Length::Fill),
        button(text("Cancel").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::ReceiverPickerCancel)
            .padding(buttons::PADDING)
            .style(buttons::secondary),
    ]
    .align_y(Alignment::Center)
    .spacing(12);

    let body = column![
        header,
        text(browser.current_dir.display().to_string()).size(11),
        container(scrollable(listing(browser)).height(Length::Fixed(360.0)))
            .style(container::bordered_box)
            .padding(6),
        text("Click a folder to open it; click a file to copy it.").size(11),
    ]
    .spacing(12)
    .padding(28);

    let surface = colors.surface;
    let border  = colors.tertiary;
    let bg      = colors.background;

    let dialog = container(body)
        .width(Length::Fixed(620.0))
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(surface)),
            border:     Border { color: border, width: 1.0, radius: 6.0.into() },
            ..Default::default()
        });

    container(dialog)
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

fn listing(state: &FileBrowserState) -> iced::widget::Column<'_, Message> {
    let mut list = column![].spacing(1);

    if let Some(parent) = state.current_dir.parent() {
        let parent_path = parent.to_path_buf();
        list = list.push(
            button(text("..").size(buttons::TEXT_SIZE))
                .on_press(Message::ReceiverPickerBrowse(parent_path))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }

    // Directories first (navigable), then files (selectable).
    for entry in state.entries.iter().filter(|e| e.is_dir) {
        list = list.push(
            button(text(format!("> {}", entry.name)).size(buttons::TEXT_SIZE))
                .on_press(Message::ReceiverPickerBrowse(entry.path.clone()))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }
    for entry in state.entries.iter().filter(|e| !e.is_dir) {
        list = list.push(
            button(text(format!("  {}", entry.name)).size(buttons::TEXT_SIZE))
                .on_press(Message::ReceiverPickerChoose(entry.path.clone()))
                .padding(buttons::PADDING)
                .style(buttons::secondary)
                .width(Length::Fill),
        );
    }

    list
}
