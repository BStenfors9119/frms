use iced::{Alignment, Background, Border, Color, Element, Font, Length, Renderer, Theme};
use iced::widget::{button, column, container, image, row, scrollable, text, text_editor, text_input, Column};

use crate::app::Message;
use crate::claude_prompt::PendingPrompt;
use crate::db_panel::DbPanel;
use crate::editor::{EditorState, PreviewKind};
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::session::CenterTab;
use crate::terminal::{TerminalId, TerminalPane};
use crate::theme::TerminalFontScale;
use crate::ui::buttons;

pub fn view<'a>(
    state:           &'a EditorState,
    db:              &'a DbPanel,
    active_tab:      CenterTab,
    terminals:       &'a [TerminalPane],
    active_terminal: TerminalId,
    pending_prompt:  Option<&'a PendingPrompt>,
    prompt_source:   Option<TerminalId>,
    renaming:        &'a Option<(TerminalId, String)>,
) -> Element<'a, Message> {
    let tab_content: Element<'_, Message> = match active_tab {
        CenterTab::Editor   => editor_view(state),
        CenterTab::Database => crate::ui::db_panel::query_tab(db),
        CenterTab::Claude(tid) => claude_view(
            terminals, tid, active_terminal, pending_prompt, prompt_source,
        ),
    };

    Column::new()
        .spacing(0)
        .height(Length::Fill)
        .push(tab_bar(active_tab, terminals, renaming))
        .push(tab_content)
        .into()
}

fn editor_view<'a>(state: &'a EditorState) -> Element<'a, Message> {
    let filename = state
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("No file open"));

    let preview_kind = state.preview_kind();
    let showing_preview = preview_kind.is_some() && state.preview_active;

    let mut header = row![text(filename).size(13)]
        .spacing(8)
        .align_y(Alignment::Center);

    if preview_kind.is_some() {
        let label = if showing_preview { "Code" } else { "Preview" };
        let toggle = button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::EditorTogglePreview)
            .padding(buttons::PADDING)
            .style(buttons::toggle(showing_preview));
        header = header.push(toggle);

        if preview_kind == Some(PreviewKind::Html) && showing_preview {
            let refresh = button(text("Refresh").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::EditorRefreshPreview)
                .padding(buttons::PADDING)
                .style(buttons::secondary);
            header = header.push(refresh);
        }
    }

    let body: Element<'_, Message> = match (preview_kind, state.preview_active) {
        (Some(PreviewKind::Markdown), true) => {
            crate::ui::preview::markdown(&state.content.text())
        }
        (Some(PreviewKind::Html), true) => html_preview(state),
        _ => text_editor(&state.content)
            .on_action(Message::EditorAction)
            .font(Font::MONOSPACE)
            .height(Length::Fill)
            .into(),
    };

    Column::new()
        .spacing(4)
        .height(Length::Fill)
        .push(container(header).padding([8, 8]))
        .push(container(body).padding([0, 8]).height(Length::Fill))
        .into()
}

/// Claude tab body: the terminal canvas, optionally split with a permission-
/// prompt response panel docked to the right of the terminal.
fn claude_view<'a>(
    terminals:       &'a [TerminalPane],
    tid:             TerminalId,
    active_terminal: TerminalId,
    pending_prompt:  Option<&'a PendingPrompt>,
    prompt_source:   Option<TerminalId>,
) -> Element<'a, Message> {
    let Some(t) = terminals.iter().find(|t| t.id == tid) else {
        return container(text("Claude terminal not found").size(13))
            .padding(16).width(Length::Fill).height(Length::Fill).into();
    };
    let term = crate::ui::terminal::view(
        tid,
        t.grid.clone(),
        t.master(),
        active_terminal == tid,
        false,
        TerminalFontScale::default(),
        t.selection,
    );
    let term_bordered = container(term)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(4);

    let show_prompt = pending_prompt.is_some() && prompt_source == Some(tid);
    if show_prompt {
        row![
            container(term_bordered).width(Length::FillPortion(2)).height(Length::Fill),
            container(crate::ui::prompt_response::view(pending_prompt))
                .width(Length::FillPortion(1))
                .height(Length::Fill),
        ]
        .spacing(4)
        .height(Length::Fill)
        .into()
    } else {
        term_bordered.into()
    }
}

fn tab_bar<'a>(
    active:    CenterTab,
    terminals: &'a [TerminalPane],
    renaming:  &'a Option<(TerminalId, String)>,
) -> Element<'a, Message> {
    let mut bar = row![
        tab_button("Editor",   active == CenterTab::Editor,   CenterTab::Editor),
        tab_button("Database", active == CenterTab::Database, CenterTab::Database),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    for (i, t) in terminals.iter().enumerate() {
        let selected = active == CenterTab::Claude(t.id);
        let renaming_this = matches!(renaming, Some((id, _)) if *id == t.id);
        let tab: Element<'a, Message, Theme, Renderer> = if renaming_this {
            let draft = renaming.as_ref().map(|(_, s)| s.as_str()).unwrap_or("");
            row![
                text_input::<Message, Theme, Renderer>("Tab name", draft)
                    .id(text_input::Id::new("claude-rename"))
                    .on_input(Message::ClaudeRenameEdited)
                    .on_submit(Message::ClaudeRenameConfirmed)
                    .width(Length::Fixed(120.0))
                    .size(buttons::TEXT_SIZE),
                button(text("✓").font(ICON_FONT).size(13))
                    .on_press(Message::ClaudeRenameConfirmed)
                    .padding([buttons::PADDING[0] - 2, 8])
                    .style(buttons::primary),
                button(text("✕").font(ICON_FONT).size(13))
                    .on_press(Message::ClaudeRenameCancelled)
                    .padding([buttons::PADDING[0] - 2, 8])
                    .style(buttons::secondary),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
        } else {
            let label = t.name.clone().unwrap_or_else(|| format!("Claude {}", i + 1));
            // Only offer a close button when more than one Claude tab exists —
            // a session always keeps at least one (see `close_claude_terminal`).
            claude_tab_button(label, selected, t.id, terminals.len() > 1, t.waiting_on_user)
        };
        bar = bar.push(tab);
    }

    let add = button(text("+ Claude").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::ClaudeSessionAdded)
        .padding(buttons::PADDING)
        .style(buttons::secondary);
    bar = bar.push(add);

    container(bar)
        .padding([6, 8])
        .width(Length::Fill)
        .style(tab_bar_bg)
        .into()
}

fn tab_button(label: &str, selected: bool, tab: CenterTab) -> Element<'static, Message> {
    button(text(label.to_string()).font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::CenterTabSelected(tab))
        .padding(buttons::PADDING)
        .style(buttons::toggle(selected))
        .into()
}

/// Tab containing the Claude terminal name + an inline ✎ to start renaming and
/// (when more than one Claude tab is open) an inline ✕ to delete it. Mirrors the
/// embedded-icon pattern used in `ui::session_bar` so the label and icons read
/// as one tab rather than separate stacked buttons.
///
/// When `waiting` is true the tab turns amber and gains a ● badge — Claude has
/// stopped on a question and is waiting for the user to answer.
fn claude_tab_button(
    label:    String,
    selected: bool,
    tid:      TerminalId,
    closable: bool,
    waiting:  bool,
) -> Element<'static, Message> {
    let fg = if selected {
        Color::from_rgb(0.96, 0.95, 0.93)
    } else {
        Color::from_rgb(0.85, 0.85, 0.88)
    };
    let edit_btn = button(text("✎").font(ICON_FONT).size(12))
        .on_press(Message::ClaudeRenameStarted(tid))
        .padding([2, 6])
        .style(buttons::embedded_icon(fg));

    let mut inner = row![]
        .spacing(4)
        .align_y(Alignment::Center);

    if waiting {
        inner = inner.push(text("●").font(ICON_FONT).size(11));
    }
    inner = inner
        .push(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
        .push(edit_btn);

    if closable {
        let close_btn = button(text("✕").font(ICON_FONT).size(12))
            .on_press(Message::ClaudeSessionClosed(tid))
            .padding([2, 6])
            .style(buttons::embedded_icon(fg));
        inner = inner.push(close_btn);
    }

    let tab = button(inner)
        .on_press(Message::CenterTabSelected(CenterTab::Claude(tid)))
        .padding([buttons::PADDING[0], 10]);
    if waiting {
        tab.style(buttons::attention_tab(selected)).into()
    } else {
        tab.style(buttons::toggle(selected)).into()
    }
}

fn tab_bar_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(24, 24, 32))),
        border:     Border { color: Color::from_rgb8(55, 55, 68), width: 0.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn html_preview(state: &EditorState) -> Element<'_, Message> {
    if state.preview_loading {
        return container(text("Rendering preview...").size(13))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }
    if let Some(bytes) = state.preview_image.as_deref() {
        let handle = image::Handle::from_bytes(bytes.to_vec());
        return scrollable(image(handle))
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }
    if let Some(err) = state.preview_error.as_deref() {
        return container(
            column![
                text("HTML preview failed").size(14),
                text(err.to_string()).size(11),
                text("Install chromium / google-chrome / firefox to render previews.").size(11),
            ]
            .spacing(6),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    }
    container(text("Click Preview to render this file.").size(13))
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
