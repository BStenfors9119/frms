use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, row, scrollable, text, text_input, Column};

use crate::app::Message;
use crate::file_browser::FileBrowserState;
use crate::session::SessionKind;
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Full-screen overlay: choose a session type and working directory.
///
/// `can_cancel` is false on first launch when no sessions exist yet — the user
/// must pick something rather than dismiss the dialog into an empty IDE.
pub fn view<'a>(
    name_input:  &'a str,
    dir_input:   &'a str,
    dir_browser: &'a FileBrowserState,
    can_cancel:  bool,
    colors:      ThemeColors,
) -> Element<'a, Message> {
    let mut body: Column<'a, Message> = column![
        row![
            text("New Session").size(22),
            button(text("Preferences").size(buttons::TEXT_SIZE))
                .on_press(Message::PrefsOpened)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        ]
        .spacing(12)
        .align_y(Alignment::Center),

        column![
            text("Session name:").size(13),
            text_input("My session", name_input)
                .on_input(Message::NewSessionNameEdited)
                .width(Length::Fill),
            text("Shown in the session tab; leave blank for a default.")
                .size(11),
        ]
        .spacing(4),

        column![
            text("Working directory:").size(13),
            text_input("/home/user/project", dir_input)
                .on_input(Message::NewSessionDirEdited)
                .on_submit(Message::NewSessionCreated(SessionKind::Terminal))
                .width(Length::Fill),
            text("Claude and the shell terminal will start in this directory.")
                .size(11),
        ]
        .spacing(4),

        column![
            text(dir_browser.current_dir.display().to_string()).size(11),
            scrollable(dir_listing(dir_browser))
                .height(Length::Fixed(180.0)),
        ]
        .spacing(3),

        text("Choose a session type:").size(14),
        row![
            session_type_button(
                "Terminal Session",
                "File browser + editor + 2 Claude panes + shell",
                SessionKind::Terminal,
            ),
            session_type_button(
                "Browser Session",
                "Live browser preview + 2 Claude panes + shell",
                SessionKind::Browser,
            ),
        ]
        .spacing(16),
    ]
    .spacing(16)
    .align_x(Alignment::Start)
    .padding(28);

    if can_cancel {
        body = body.push(
            button(text("Cancel").size(buttons::TEXT_SIZE))
                .on_press(Message::NewSessionCancelled)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        );
    }

    let surface = colors.surface;
    let border  = colors.tertiary;
    let bg      = colors.background;

    let dialog = container(body)
        .width(Length::Fixed(560.0))
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

fn dir_listing(state: &FileBrowserState) -> iced::widget::Column<'_, Message> {
    let mut list = column![].spacing(1);

    if let Some(parent) = state.current_dir.parent() {
        let parent_path = parent.to_path_buf();
        list = list.push(
            button(text("..").size(buttons::TEXT_SIZE))
                .on_press(Message::NewSessionDirBrowsed(parent_path))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }

    for entry in state.entries.iter().filter(|e| e.is_dir) {
        let label = format!("> {}", entry.name);
        list = list.push(
            button(text(label).size(buttons::TEXT_SIZE))
                .on_press(Message::NewSessionDirBrowsed(entry.path.clone()))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }

    list
}

fn session_type_button(
    title:    &'static str,
    subtitle: &'static str,
    kind:     SessionKind,
) -> Element<'static, Message> {
    button(
        column![
            text(title).size(15),
            text(subtitle).size(11),
        ]
        .spacing(4),
    )
    .on_press(Message::NewSessionCreated(kind))
    .padding(buttons::PADDING)
    .style(buttons::primary)
    .width(Length::Fixed(240.0))
    .into()
}
