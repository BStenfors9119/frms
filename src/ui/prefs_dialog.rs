use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, row, scrollable, text, text_input};

use crate::app::Message;
use crate::file_browser::FileBrowserState;
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Full-screen overlay: edit user preferences. Theme/font settings live in
/// the Profile plugin tab; this dialog now only configures the dev dir.
pub fn view<'a>(
    dev_dir_input: &'a str,
    dir_browser:   &'a FileBrowserState,
    colors:        ThemeColors,
) -> Element<'a, Message> {
    let dev_dir_section = column![
        text("Default dev directory").size(15),
        text_input("/home/user/projects", dev_dir_input)
            .on_input(Message::PrefsDevDirEdited)
            .on_submit(Message::PrefsSaved)
            .width(Length::Fill),
        text("New sessions will open the directory picker here.")
            .size(11),
        column![
            text(dir_browser.current_dir.display().to_string()).size(11),
            scrollable(dir_listing(dir_browser))
                .height(Length::Fixed(180.0)),
        ]
        .spacing(3),
    ]
    .spacing(6);

    let surface = colors.surface;
    let border  = colors.tertiary;
    let bg      = colors.background;

    let card = container(
        column![
            text("Preferences").size(22),
            dev_dir_section,
            row![
                button(text("Save").size(buttons::TEXT_SIZE))
                    .on_press(Message::PrefsSaved)
                    .padding(buttons::PADDING)
                    .style(buttons::primary),
                button(text("Close").size(buttons::TEXT_SIZE))
                    .on_press(Message::PrefsCancelled)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
            ]
            .spacing(8),
        ]
        .spacing(18)
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

fn dir_listing(state: &FileBrowserState) -> iced::widget::Column<'_, Message> {
    let mut list = column![].spacing(1);

    if let Some(parent) = state.current_dir.parent() {
        let parent_path = parent.to_path_buf();
        list = list.push(
            button(text("..").size(buttons::TEXT_SIZE))
                .on_press(Message::PrefsDevDirBrowsed(parent_path))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }

    for entry in state.entries.iter().filter(|e| e.is_dir) {
        let label = format!("> {}", entry.name);
        list = list.push(
            button(text(label).size(buttons::TEXT_SIZE))
                .on_press(Message::PrefsDevDirBrowsed(entry.path.clone()))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }

    list
}
