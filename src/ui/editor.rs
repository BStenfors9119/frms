use iced::{Alignment, Background, Border, Color, Element, Font, Length, Renderer, Theme};
use iced::widget::{button, column, container, image, mouse_area, row, scrollable, stack, text, text_editor, text_input, Column};

use crate::agent::{AgentKind, AgentPane, ChatPane, ChatRole};
use crate::app::{self, Message};
use crate::claude_prompt::PendingPrompt;
use crate::db_panel::DbPanel;
use crate::editor::{EditorState, PreviewKind};
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::session::CenterTab;
use crate::terminal::TerminalId;
use crate::theme::TerminalFontScale;
use crate::ui::buttons;

pub fn view<'a>(
    state:           &'a EditorState,
    db:              &'a DbPanel,
    active_tab:      CenterTab,
    panes:           &'a [AgentPane],
    active_terminal: TerminalId,
    pending_prompt:  Option<&'a PendingPrompt>,
    prompt_source:   Option<TerminalId>,
    renaming:        &'a Option<(TerminalId, String)>,
    agent_menu_open: bool,
) -> Element<'a, Message> {
    let tab_content: Element<'_, Message> = match active_tab {
        CenterTab::Editor   => editor_view(state),
        CenterTab::Database => crate::ui::db_panel::query_tab(db),
        CenterTab::Claude(tid) => claude_view(
            panes, tid, active_terminal, pending_prompt, prompt_source,
        ),
    };

    let body = Column::new()
        .spacing(0)
        .height(Length::Fill)
        .push(tab_bar(active_tab, panes, renaming))
        .push(tab_content);

    // The "+ Agent" dropdown floats over the editor body when open. A
    // transparent backdrop fills the pane so a click anywhere outside the menu
    // dismisses it; the menu itself sits on top, anchored just under the tab
    // bar on the right where its button lives.
    if !agent_menu_open {
        return body.into();
    }
    let backdrop = mouse_area(
        container(text(""))
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_press(Message::AgentMenuToggled);

    stack![body, backdrop, agent_menu()].into()
}

/// The floating list of agent kinds shown when the "+ Agent" button is open.
/// Anchored top-right (under the button) as a small popup card.
fn agent_menu<'a>() -> Element<'a, Message> {
    let mut items = Column::new().spacing(2);
    for &kind in AgentKind::ALL {
        items = items.push(
            button(text(kind.menu_label()).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentAdded(kind))
                .width(Length::Fill)
                .padding(buttons::PADDING)
                .style(buttons::menu_item),
        );
    }
    let card = container(items)
        .padding(4)
        .width(Length::Fixed(180.0))
        .style(menu_card_bg);

    // Anchored under the "+ Agent" button, which is pinned just after the
    // Editor/Database tabs. The left inset approximates those two tabs' combined
    // width so the menu drops straight down from the button.
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Start)
        .padding(iced::Padding::ZERO.top(40.0).left(168.0))
        .into()
}

fn menu_card_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(34, 34, 44))),
        border:     Border { color: Color::from_rgb8(70, 70, 86), width: 1.0, radius: 6.0.into() },
        ..Default::default()
    }
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
    panes:           &'a [AgentPane],
    tid:             TerminalId,
    active_terminal: TerminalId,
    pending_prompt:  Option<&'a PendingPrompt>,
    prompt_source:   Option<TerminalId>,
) -> Element<'a, Message> {
    let Some(pane) = panes.iter().find(|p| p.id() == tid) else {
        return container(text("Agent not found").size(13))
            .padding(16).width(Length::Fill).height(Length::Fill).into();
    };
    // Research/Chat panes render the native chat UI instead of a terminal.
    if let Some(chat) = pane.as_chat() {
        return chat_view(chat);
    }
    let Some(t) = pane.as_terminal() else {
        return container(text("Agent not found").size(13))
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

/// Native chat pane body (Research / Chat): a scrollable transcript above a
/// message input box. No PTY — replies stream in from the Messages API.
fn chat_view(chat: &ChatPane) -> Element<'_, Message> {
    let id = chat.id;

    let mut transcript = column![]
        .spacing(10)
        .padding([8, 8])
        .width(Length::Fill);
    if chat.messages.is_empty() && !chat.streaming {
        transcript = transcript.push(
            text(format!("{} — start the conversation below.", chat.kind.label()))
                .font(UI_FONT)
                .size(12),
        );
    }
    for m in &chat.messages {
        transcript = transcript.push(chat_bubble(m.role, &m.text));
    }
    if chat.streaming {
        let body = if chat.pending.is_empty() { "…" } else { chat.pending.as_str() };
        transcript = transcript.push(chat_bubble(ChatRole::Assistant, body));
    }

    let history = scrollable(transcript)
        .width(Length::Fill)
        .height(Length::Fill);

    let mut col = Column::new()
        .spacing(0)
        .height(Length::Fill)
        .push(container(history).height(Length::Fill).width(Length::Fill));

    if let Some(err) = &chat.error {
        col = col.push(
            container(text(format!("Error: {err}")).size(12))
                .padding([6, 10])
                .width(Length::Fill),
        );
    }

    let input = text_input("Message Claude…", &chat.input)
        .id(app::chat_input_id(id))
        .on_input(move |s| Message::ChatInputEdited(id, s))
        .on_submit(Message::ChatSubmitted(id))
        .padding(8)
        .size(13)
        .width(Length::Fill);
    let send = button(text("Send").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::ChatSubmitted(id))
        .padding(buttons::PADDING)
        .style(buttons::primary);
    let input_row = row![input, send]
        .spacing(6)
        .padding([8, 8])
        .align_y(Alignment::Center);

    col.push(input_row).into()
}

/// One transcript turn, tinted and labelled by author.
fn chat_bubble(role: ChatRole, body: &str) -> Element<'static, Message> {
    let label = match role {
        ChatRole::User      => "You",
        ChatRole::Assistant => "Claude",
    };
    let inner = column![
        text(label).font(UI_FONT).size(11),
        text(body.to_string()).size(13),
    ]
    .spacing(3);

    container(inner)
        .padding([8, 10])
        .width(Length::Fill)
        .style(move |_theme| chat_bubble_style(role))
        .into()
}

fn chat_bubble_style(role: ChatRole) -> container::Style {
    let bg = match role {
        ChatRole::User      => Color::from_rgb8(40, 44, 60),
        ChatRole::Assistant => Color::from_rgb8(30, 30, 38),
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border:     Border { color: Color::from_rgb8(55, 55, 68), width: 1.0, radius: 6.0.into() },
        text_color: Some(Color::from_rgb(0.90, 0.90, 0.93)),
        ..Default::default()
    }
}

fn tab_bar<'a>(
    active:    CenterTab,
    panes:     &'a [AgentPane],
    renaming:  &'a Option<(TerminalId, String)>,
) -> Element<'a, Message> {
    // The Editor/Database tabs stay pinned on the left; the agent tabs (and
    // the "New Agent" button that follows them) live in their own strip below.
    let mut claude_tabs = row![]
        .spacing(4)
        .align_y(Alignment::Center);

    for (i, p) in panes.iter().enumerate() {
        let selected = active == CenterTab::Claude(p.id());
        let renaming_this = matches!(renaming, Some((id, _)) if *id == p.id());
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
            let label = p.name().map(str::to_owned)
                .unwrap_or_else(|| format!("{} {}", p.kind().label(), i + 1));
            // Only offer a close button when more than one Claude tab exists —
            // a session always keeps at least one (see `close_claude_terminal`).
            claude_tab_button(label, selected, p.id(), panes.len() > 1, p.waiting_on_user())
        };
        claude_tabs = claude_tabs.push(tab);
    }

    // Let the Claude strip keep its natural width and scroll horizontally once
    // it overflows the pane — otherwise the tabs bunch up / get clipped when
    // there are many sessions or the pane is shrunk. The scrollbar only
    // materialises when the content is wider than the available space, so the
    // common (few-tabs) case looks exactly as before. Mirrors ui::session_bar.
    let claude_strip = scrollable(claude_tabs)
        .width(Length::Fill)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(4).scroller_width(4),
        ));

    // "+ Agent" opens a dropdown menu (see `agent_menu`) to start a Build
    // (Claude Code PTY), Research (Opus chat), or Chat (Sonnet chat) pane. It's
    // a *tool* button — grouped with the Editor/Database tabs on the left, not
    // an agent session itself — and pinned just after the Database tab so the
    // dropdown can anchor reliably beneath it.
    let add_agent = button(
        text("+ Agent").font(UI_FONT).size(buttons::TEXT_SIZE),
    )
    .on_press(Message::AgentMenuToggled)
    .padding(buttons::PADDING)
    .style(buttons::primary);

    // Group just the open agent tabs inside a bordered callout with a subtle
    // header so the AI agent sessions read as their own region rather than
    // blending into the Editor/Database/+ Agent tool buttons on the left.
    let agent_callout = container(
        column![
            text("AI AGENT SESSIONS")
                .font(UI_FONT)
                .size(9)
                .style(agent_callout_label),
            claude_strip,
        ]
        .spacing(3),
    )
    .padding([5, 9])
    .width(Length::Fill)
    .style(agent_callout_bg);

    let bar = row![
        tab_button("Editor",   active == CenterTab::Editor,   CenterTab::Editor),
        tab_button("Database", active == CenterTab::Database, CenterTab::Database),
        add_agent,
        agent_callout,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

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

/// Bordered "AI Agent Sessions" callout — a slightly raised panel with a
/// rounded accent outline so the agent tabs visually separate from the
/// Editor/Database/+ Agent tool buttons. Both the raised surface and the
/// outline are derived from the active theme's palette so the callout tracks
/// whatever theme the user picks in their profile.
fn agent_callout_bg(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        // A subtly raised surface relative to the tab-bar background.
        background: Some(Background::Color(palette.background.weak.color)),
        border:     Border {
            // The theme's primary accent gives the callout its colored outline.
            color:  palette.primary.base.color,
            width:  1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// Header label inside the agent callout — tinted with the theme's primary
/// accent so it stays in line with the selected profile theme.
fn agent_callout_label(theme: &Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.extended_palette().primary.base.color),
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
