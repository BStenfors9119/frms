use iced::{Alignment, Background, Border, Color, ContentFit, Element, Font, Length, Renderer, Theme};
use iced::keyboard::{self, key};
use iced::widget::{button, column, container, image, mouse_area, row, scrollable, stack, text, text_editor, text_input, Column};

/// Height of the chat composer: tall enough for ~4 lines before its own
/// vertical scroll kicks in.
const CHAT_INPUT_HEIGHT: f32 = 84.0;

/// Scrollable id of the agent-tab strip, so the ❮ ❯ buttons can scroll it.
/// Only one strip is ever on screen (the active session's), so one id serves.
pub fn agent_strip_id() -> scrollable::Id {
    scrollable::Id::new("agent-tab-strip")
}

/// How far one ❮ / ❯ click nudges the agent-tab strip — a bit more than a
/// typical tab's width, so each click reveals a fresh tab without skipping one.
pub const AGENT_TAB_NUDGE: f32 = 160.0;

/// Blank height reserved under the agent tabs for their horizontal scrollbar.
/// Iced paints a horizontal bar *over* the bottom `max(width, scroller) +
/// 2·margin` of the scrollable, so the strip pads itself by that much (14px for
/// the bar below) plus breathing room — the bar then lands in empty space
/// rather than across the tab labels. Dropping the callout's "AI AGENT
/// SESSIONS" heading freed about this much height, so the callout itself stays
/// the size it was.
const AGENT_SCROLLBAR_GUTTER: f32 = 18.0;

use crate::agent::{AgentKind, AgentPane, ChatPane, ChatRole};
use crate::app::Message;
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
    strip_overflow:  bool,
    cloud_sql_creds: &'a str,
) -> Element<'a, Message> {
    let tab_content: Element<'_, Message> = match active_tab {
        CenterTab::Editor   => editor_view(state),
        CenterTab::Database => crate::ui::db_panel::query_tab(db, cloud_sql_creds),
        CenterTab::Claude(tid) => claude_view(
            panes, tid, active_terminal, pending_prompt, prompt_source,
        ),
    };

    Column::new()
        .spacing(0)
        .height(Length::Fill)
        .push(tab_bar(active_tab, panes, renaming, strip_overflow))
        .push(tab_content)
        .into()
}

fn tab_bar<'a>(
    active:    CenterTab,
    panes:     &'a [AgentPane],
    renaming:  &'a Option<(TerminalId, String)>,
    overflow:  bool,
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
            claude_tab_button(label, selected, p.id(), panes.len() > 1, p.needs_attention(), i)
        };
        claude_tabs = claude_tabs.push(tab);
    }

    // Let the Claude strip keep its natural width and scroll horizontally once
    // it overflows the pane — otherwise the tabs bunch up / get clipped when
    // there are many sessions or the pane is shrunk. The scrollbar only
    // materialises when the content is wider than the available space, so the
    // common (few-tabs) case looks exactly as before. Mirrors ui::session_bar.
    //
    // Iced paints a horizontal scrollbar *over* the bottom of the content, so
    // when there is one the tabs reserve a blank gutter under themselves — the
    // bar can then stay chunky enough to grab without covering half of every
    // tab label. When everything fits there's no bar to dodge, so the gutter
    // goes away rather than leaving the tabs stranded at the top of a
    // too-tall callout. `overflow` comes from the strip's own last report;
    // see `Frms::refresh_agent_strip_overflow`.
    let gutter = if overflow { AGENT_SCROLLBAR_GUTTER } else { 0.0 };
    let claude_tabs = container(claude_tabs)
        .padding(iced::Padding::default().bottom(gutter));

    let claude_strip = scrollable(claude_tabs)
        .id(agent_strip_id())
        .on_scroll(Message::AgentStripViewport)
        .width(Length::Fill)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new()
                .width(10)
                .scroller_width(10)
                .margin(2),
        ));

    // ❮ ❯ (U+276E/U+276F) rather than ‹ › — the bundled Noto Sans Symbols 2
    // has no General Punctuation, so the guillemets rendered as tofu boxes.
    //
    // Each button carries the strip's scrollbar gutter as bottom padding too,
    // so it centers on the row of tabs rather than on the tabs-plus-gutter and
    // drifting low.
    let nudge = move |glyph: &'static str, forward: bool, tip: &'a str| -> Element<'a, Message> {
        container(crate::ui::with_tip(
            button(text(glyph).font(ICON_FONT).size(13))
                .on_press(Message::AgentTabsNudged { forward })
                .padding([2, 6])
                .style(buttons::secondary),
            tip,
        ))
        .padding(iced::Padding::default().bottom(gutter))
        .into()
    };

    let claude_strip = row![
        nudge("\u{276E}", false, "Scroll agent tabs left"),   // ❮
        claude_strip,
        nudge("\u{276F}", true,  "Scroll agent tabs right"),  // ❯
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    // "Agents" opens the one dialog that manages this project's AI sessions:
    // start a new one, retarget open ones at another model, or close one that's
    // done with. It replaced a "+ Agent" dropdown and a separate "Model" button
    // — two entry points for one subject. It's a *tool* button, grouped with
    // the Editor/Database tabs rather than being an agent session itself.
    let manage_agents = button(
        text("Agents").font(UI_FONT).size(buttons::TEXT_SIZE),
    )
    .on_press(Message::AgentsDialogOpened)
    .padding(buttons::PADDING)
    .style(buttons::primary);

    // Group just the open agent tabs inside a bordered callout so the AI agent
    // sessions read as their own region rather than blending into the
    // Editor/Database/+ Agent tool buttons on the left. The outline alone says
    // that — the old "AI AGENT SESSIONS" heading spent a line of height on it,
    // and that line now goes to the tab strip's scrollbar gutter instead.
    let agent_callout = container(claude_strip)
    .padding([5, 9])
    .width(Length::Fill)
    .style(agent_callout_bg);

    let bar = row![
        tab_button("Editor", active == CenterTab::Editor, CenterTab::Editor, None),
        tab_button(
            "Database",
            active == CenterTab::Database,
            CenterTab::Database,
            Some("Open the Database tool (Ctrl+Shift+D)"),
        ),
        crate::ui::with_tip(
            manage_agents,
            "Add, switch models, or close agent sessions (Ctrl+Shift+M)",
        ),
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

fn tab_button(
    label:    &str,
    selected: bool,
    tab:      CenterTab,
    tip:      Option<&'static str>,
) -> Element<'static, Message> {
    let btn = button(text(label.to_string()).font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::CenterTabSelected(tab))
        .padding(buttons::PADDING)
        .style(buttons::toggle(selected));
    // A tooltip only where there's something to say — the keyboard shortcut.
    match tip {
        Some(tip) => crate::ui::with_tip(btn, tip),
        None      => btn.into(),
    }
}

/// Tab containing the Claude terminal name + an inline ✎ to start renaming and
/// (when more than one Claude tab is open) an inline ✕ to delete it. Mirrors the
/// embedded-icon pattern used in `ui::session_bar` so the label and icons read
/// as one tab rather than separate stacked buttons.
///
/// When `waiting` is true the tab turns amber and gains a ● badge — Claude has
/// stopped on a question and is waiting for the user to answer.
///
/// `index` is the pane's position in the strip, which doubles as its
/// Ctrl+Shift+<digit> shortcut for the first nine.
fn claude_tab_button(
    label:    String,
    selected: bool,
    tid:      TerminalId,
    closable: bool,
    waiting:  bool,
    index:    usize,
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

    // Ctrl+Shift+1…9 jumps straight to an agent pane, so label each tab with
    // its digit — the same badge `ui::session_bar` puts on project tabs for
    // Ctrl+1…9. Only the first nine have a shortcut; later tabs get no badge,
    // rather than a number that promises a chord which doesn't exist.
    if index < 9 {
        inner = inner.push(
            text(format!("{}", index + 1))
                .font(UI_FONT)
                .size(buttons::TEXT_SIZE - 1)
                .style(move |_: &Theme| iced::widget::text::Style {
                    color: Some(Color { a: 0.55, ..fg }),
                }),
        );
    }
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

/// Image files are shown as a scaled-to-fit preview with no code view.
fn image_preview(state: &EditorState) -> Element<'_, Message> {
    let body: Element<'_, Message> = if let Some(handle) = state.preview_handle.clone() {
        // `Contain` scales down to the pane and leaves smaller images at their
        // natural size, so a 16px icon doesn't blow up to fill the pane.
        image(handle)
            .content_fit(ContentFit::Contain)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    } else if let Some(err) = state.preview_error.as_deref() {
        column![
            text("Could not open image").size(14),
            text(err.to_string()).size(11),
        ]
        .spacing(6)
        .into()
    } else {
        text("No image open").size(13).into()
    };

    container(body)
        .padding(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn html_preview(state: &EditorState) -> Element<'_, Message> {
    if state.preview_loading {
        return container(text("Rendering preview...").size(13))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    }
    if let Some(handle) = state.preview_handle.clone() {
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
