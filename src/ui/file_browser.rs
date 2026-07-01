use iced::font::Weight;
use iced::{Alignment, Element, Font, Length};
use iced::widget::{button, column, row, scrollable, text, text_input, Space};

use crate::app::Message;
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::file_browser::{DirEdit, Entry, FileBrowserState};
use crate::ui::buttons;

pub fn view(state: &FileBrowserState) -> Element<'_, Message> {
    let full_path = state.current_dir.display().to_string();

    // Big, bold name of the current directory (just its final component;
    // falls back to the full path for the filesystem root).
    let dir_name = state
        .current_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| full_path.clone());

    let bold_ui_font = Font { weight: Weight::Bold, ..UI_FONT };
    let dir_heading = text(dir_name).font(bold_ui_font).size(18);

    // + starts an inline "new folder" edit in the current directory.
    let new_folder_btn = button(text("+").font(UI_FONT).size(13))
        .on_press(Message::DirEditStart(DirEdit::Create { draft: String::new() }))
        .padding([2, 6])
        .style(buttons::secondary);

    // ◀ collapses the file-browser pane, reclaiming its width for the editor.
    let collapse_btn = button(text("◀").font(ICON_FONT).size(13))
        .on_press(Message::FileBrowserToggleCollapsed)
        .padding([2, 6])
        .style(buttons::secondary);

    let heading_row = row![
        dir_heading,
        Space::with_width(Length::Fill),
        new_folder_btn,
        collapse_btn,
    ]
    .align_y(Alignment::Center)
    .spacing(6);

    // The full path, clickable to copy it to the clipboard. Rendered as a
    // transparent button so it picks up the theme's text color and shows a
    // faint hover wash to hint that it's interactive.
    let dir_label = button(text(full_path.clone()).font(UI_FONT).size(11))
        .on_press(Message::CopyPath(full_path))
        .padding(0)
        .style(|theme: &iced::Theme, status| {
            let fg = theme.palette().text;
            let overlay = match status {
                iced::widget::button::Status::Hovered => 0.12,
                iced::widget::button::Status::Pressed => 0.20,
                _ => 0.0,
            };
            iced::widget::button::Style {
                background: (overlay > 0.0)
                    .then(|| iced::Background::Color(iced::Color { a: overlay, ..fg })),
                text_color: iced::Color { a: 0.7, ..fg },
                border: iced::Border::default(),
                shadow: iced::Shadow::default(),
            }
        })
        .width(Length::Fill);

    // Right padding keeps the row-action buttons clear of the scrollbar,
    // which iced overlays on top of the scrollable's content.
    let mut list = column![].spacing(4).padding(iced::Padding {
        top: 0.0, right: 14.0, bottom: 0.0, left: 0.0,
    });

    // Inline "new folder" input at the top of the listing.
    if let Some(DirEdit::Create { draft }) = &state.edit {
        list = list.push(edit_row("New folder name", draft));
    }

    if let Some(parent) = state.current_dir.parent() {
        let parent_path = parent.to_path_buf();
        list = list.push(
            button(text("..").size(buttons::TEXT_SIZE))
                .on_press(Message::BrowseDir(parent_path))
                .padding(buttons::PADDING)
                .style(buttons::primary)
                .width(Length::Fill),
        );
    }

    for entry in &state.entries {
        // A directory mid-rename or pending delete swaps its row for the
        // matching inline editor.
        match &state.edit {
            Some(DirEdit::Rename { path, draft }) if *path == entry.path => {
                list = list.push(edit_row("Folder name", draft));
                continue;
            }
            Some(DirEdit::ConfirmDelete { path }) if *path == entry.path => {
                list = list.push(confirm_delete_row(&entry.name));
                continue;
            }
            _ => {}
        }

        list = list.push(entry_row(entry));
    }

    let mut body = column![heading_row, dir_label].spacing(4);
    if let Some(error) = &state.error {
        body = body.push(
            text(error.clone())
                .font(UI_FONT)
                .size(11)
                .style(|theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(theme.palette().danger),
                }),
        );
    }

    body.push(scrollable(list).height(Length::Fill))
        .padding(8)
        .height(Length::Fill)
        .into()
}

/// One listing row. Directories get inline ✎ rename and ✕ delete buttons
/// after the navigation button; files are just the open button.
fn entry_row(entry: &Entry) -> Element<'static, Message> {
    let prefix = if entry.is_dir { "> " } else { "  " };
    let label  = format!("{}{}", prefix, entry.name);
    let msg    = if entry.is_dir {
        Message::BrowseDir(entry.path.clone())
    } else {
        Message::OpenFile(entry.path.clone())
    };

    let main = button(text(label).size(buttons::TEXT_SIZE))
        .on_press(msg)
        .padding(buttons::PADDING)
        .style(buttons::primary)
        .width(Length::Fill);

    if !entry.is_dir {
        // 📤 copies this file to the currently-selected TSR receiver (scp).
        let to_receiver = crate::ui::with_tip(
            button(text("\u{1F4E4}").font(ICON_FONT).size(11))
                .on_press(Message::ReceiverCopyFile(entry.path.clone()))
                .padding([4, 6])
                .style(buttons::secondary),
            "Copy to selected receiver",
        );
        return row![main, to_receiver]
            .spacing(4)
            .align_y(Alignment::Center)
            .into();
    }

    let rename = button(text("✎").font(ICON_FONT).size(11))
        .on_press(Message::DirEditStart(DirEdit::Rename {
            path:  entry.path.clone(),
            draft: entry.name.clone(),
        }))
        .padding([4, 6])
        .style(buttons::secondary);
    let delete = button(text("✕").font(ICON_FONT).size(11))
        .on_press(Message::DirEditStart(DirEdit::ConfirmDelete {
            path: entry.path.clone(),
        }))
        .padding([4, 6])
        .style(buttons::danger);

    row![main, rename, delete]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

/// Inline name editor shared by create and rename: text input + ✓ confirm /
/// ✕ cancel. Enter confirms, matching the buttons.
fn edit_row(placeholder: &str, draft: &str) -> Element<'static, Message> {
    let input = text_input(placeholder, draft)
        .on_input(Message::DirEditDraftChanged)
        .on_submit(Message::DirEditConfirm)
        .size(buttons::TEXT_SIZE)
        .width(Length::Fill);

    let confirm = button(text("✓").font(ICON_FONT).size(11))
        .on_press(Message::DirEditConfirm)
        .padding([4, 6])
        .style(buttons::success);
    let cancel = button(text("✕").font(ICON_FONT).size(11))
        .on_press(Message::DirEditCancel)
        .padding([4, 6])
        .style(buttons::secondary);

    row![input, confirm, cancel]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

/// Replaces a directory row while a delete is pending: the folder name plus
/// explicit Delete / Cancel buttons. Deletion is recursive, so it always asks.
fn confirm_delete_row(name: &str) -> Element<'static, Message> {
    let label = text(format!("Delete \"{name}\"?"))
        .font(UI_FONT)
        .size(buttons::TEXT_SIZE);

    let delete = button(text("Delete").size(buttons::TEXT_SIZE))
        .on_press(Message::DirEditConfirm)
        .padding([4, 8])
        .style(buttons::danger);
    let cancel = button(text("Cancel").size(buttons::TEXT_SIZE))
        .on_press(Message::DirEditCancel)
        .padding([4, 8])
        .style(buttons::secondary);

    row![label, Space::with_width(Length::Fill), delete, cancel]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
}

/// Thin vertical strip shown in place of the file browser when it is
/// collapsed. A single ▶ button at the top re-expands the pane.
pub fn collapsed_strip() -> Element<'static, Message> {
    let expand_btn = button(text("▶").font(ICON_FONT).size(13))
        .on_press(Message::FileBrowserToggleCollapsed)
        .padding([2, 6])
        .style(buttons::secondary);

    column![expand_btn]
        .align_x(Alignment::Center)
        .padding(6)
        .height(Length::Fill)
        .into()
}
