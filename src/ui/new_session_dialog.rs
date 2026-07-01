use iced::{Alignment, Background, Border, Element, Length, Theme};
use iced::widget::{button, column, container, row, scrollable, text, text_input, Column, Space};

use crate::app::Message;
use crate::file_browser::{DirEdit, FileBrowserState};
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
            text("New Project").size(22),
            button(text("Preferences").size(buttons::TEXT_SIZE))
                .on_press(Message::PrefsOpened)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        ]
        .spacing(12)
        .align_y(Alignment::Center),

        column![
            text("Project name:").size(13),
            text_input("My project", name_input)
                .on_input(Message::NewSessionNameEdited)
                .width(Length::Fill),
            text("Shown in the project tab; leave blank for a default.")
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

        {
            let mut picker = column![
                row![
                    text(dir_browser.current_dir.display().to_string()).size(11),
                    Space::with_width(Length::Fill),
                    button(text("+ New Folder").size(buttons::TEXT_SIZE))
                        .on_press(Message::DirEditStart(DirEdit::Create {
                            draft: String::new(),
                        }))
                        .padding(buttons::PADDING)
                        .style(buttons::secondary),
                ]
                .align_y(Alignment::Center),
            ]
            .spacing(3);

            if let Some(error) = &dir_browser.error {
                picker = picker.push(
                    text(error.clone()).size(11).style(|theme: &Theme| {
                        iced::widget::text::Style {
                            color: Some(theme.palette().danger),
                        }
                    }),
                );
            }

            picker.push(
                scrollable(dir_listing(dir_browser)).height(Length::Fixed(180.0)),
            )
        },
    ]
    .spacing(16)
    .align_x(Alignment::Start)
    .padding(28);

    let mut footer = row![]
        .spacing(12)
        .width(Length::Fill)
        .align_y(Alignment::Center);

    if can_cancel {
        footer = footer.push(
            button(text("Cancel").size(buttons::TEXT_SIZE))
                .on_press(Message::NewSessionCancelled)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        );
    }

    footer = footer.push(iced::widget::horizontal_space());
    footer = footer.push(
        button(text("Create Project").size(buttons::TEXT_SIZE))
            .on_press(Message::NewSessionCreated(SessionKind::Terminal))
            .padding(buttons::PADDING)
            .style(buttons::primary),
    );

    body = body.push(footer);

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

    // Inline "new folder" input at the top of the listing.
    if let Some(DirEdit::Create { draft }) = &state.edit {
        let input = text_input("New folder name", draft)
            .on_input(Message::DirEditDraftChanged)
            .on_submit(Message::DirEditConfirm)
            .size(buttons::TEXT_SIZE)
            .width(Length::Fill);

        let confirm = button(text("✓").size(buttons::TEXT_SIZE))
            .on_press(Message::DirEditConfirm)
            .padding(buttons::PADDING)
            .style(buttons::success);
        let cancel = button(text("✕").size(buttons::TEXT_SIZE))
            .on_press(Message::DirEditCancel)
            .padding(buttons::PADDING)
            .style(buttons::secondary);

        list = list.push(
            row![input, confirm, cancel]
                .spacing(4)
                .align_y(Alignment::Center),
        );
    }

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
