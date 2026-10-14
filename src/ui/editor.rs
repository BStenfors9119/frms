use iced::{Alignment, Background, Border, Color, ContentFit, Element, Font, Length, Renderer, Theme};
use iced::alignment::Horizontal;
use iced::keyboard::{self, key};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{button, column, container, image, row, scrollable, text, text_editor, text_input, Column, Space};

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

/// Font size shared by the code editor and its line-number gutter. Set on both
/// so their rows line up; 16.0 is iced's default text size, so turning the
/// gutter on doesn't change the size of the editor text.
const CODE_TEXT_SIZE: f32 = 16.0;

/// Line height shared by the editor and the gutter — iced's `text_editor`
/// default (`LineHeight::Relative(1.3)`), matched here so the numbers sit on
/// the same baselines as the code.
const CODE_LINE_HEIGHT: f32 = 1.3;

use crate::agent::{AgentPane, ChatPane, ChatRole};
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
    band:            Color,
    terminal_font_scale: TerminalFontScale,
    show_line_numbers: bool,
) -> Element<'a, Message> {
    let tab_content: Element<'_, Message> = match active_tab {
        CenterTab::Editor   => editor_view(state, show_line_numbers),
        CenterTab::Database => crate::ui::db_panel::query_tab(db, cloud_sql_creds),
        CenterTab::Claude(tid) => claude_view(
            panes, tid, active_terminal, pending_prompt, prompt_source, terminal_font_scale,
        ),
    };

    Column::new()
        .spacing(0)
        .height(Length::Fill)
        .push(tab_bar(active_tab, panes, renaming, strip_overflow, band))
        .push(tab_content)
        .into()
}

/// Widget id of a pane's find-bar input, so opening the bar can drop the caret
/// straight into it (see `Message::FindOpened`).
pub fn find_input_id(id: TerminalId) -> text_input::Id {
    text_input::Id::new(format!("find-input-{id}"))
}

// ── editor tab ────────────────────────────────────────────────────────────────

/// Editor tab body: a filename header with the preview toggle, over either the
/// code buffer or the rendered preview.
fn editor_view<'a>(state: &'a EditorState, show_line_numbers: bool) -> Element<'a, Message> {
    let filename = state
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("No file open"));

    let preview_kind = state.preview_kind();
    // Images have no code view — the buffer would be binary garbage — so they
    // are preview-only and the toggle doesn't apply.
    let image_only = preview_kind == Some(PreviewKind::Image);
    let showing_preview = preview_kind.is_some() && (state.preview_active || image_only);

    // A dot beside the name is the unsaved marker — the Save button says the
    // same thing by lighting up, but the name is where the eye already is.
    let title = if state.dirty { format!("\u{2022} {filename}") } else { filename };
    let mut header = row![text(title).size(13)]
        .spacing(8)
        .align_y(Alignment::Center);

    // Preview-only files (images) have an empty buffer that isn't their
    // contents, so saving would truncate them — `EditorState::save` guards it
    // and the button isn't offered.
    if !state.is_preview_only() {
        let mut save = button(text("Save").font(UI_FONT).size(buttons::TEXT_SIZE))
            .padding(buttons::PADDING)
            .style(if state.dirty { buttons::primary } else { buttons::secondary });
        if state.path.is_some() {
            save = save.on_press(Message::EditorSave);
        }
        header = header.push(save);
    }

    if preview_kind.is_some() && !image_only {
        let label = if showing_preview { "Code" } else { "Preview" };
        header = header.push(
            button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::EditorTogglePreview)
                .padding(buttons::PADDING)
                .style(buttons::toggle(showing_preview)),
        );

    }

    // The line-number toggle only applies to the code view — images have none,
    // and a preview renders no lines to number. Offer it whenever code is on
    // screen, previewable or not.
    if !image_only && !showing_preview {
        header = header.push(crate::ui::with_tip(
            button(text("Line #").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::EditorToggleLineNumbers)
                .padding(buttons::PADDING)
                .style(buttons::toggle(show_line_numbers)),
            "Show or hide line numbers",
        ));
    }

    let body: Element<'_, Message> = match (preview_kind, showing_preview) {
        (Some(PreviewKind::Image), _)       => image_preview(state),
        (Some(PreviewKind::Markdown), true) => crate::ui::preview::markdown(&state.content.text()),
        (Some(PreviewKind::Html), true)     => crate::ui::preview::html(&state.content.text()),
        _ => code_view(state, show_line_numbers),
    };

    if let Some(err) = &state.save_error {
        header = header.push(
            text(format!("Save failed: {err}"))
                .font(UI_FONT)
                .size(11)
                .color(Color::from_rgb8(239, 68, 68)),
        );
    }

    Column::new()
        .spacing(4)
        .height(Length::Fill)
        .push(container(header).padding([8, 8]))
        .push(container(body).padding([0, 8]).height(Length::Fill))
        .into()
}

/// The editable code buffer, optionally with a line-number gutter down its left.
///
/// Without the gutter this is the plain `text_editor` filling the pane and
/// scrolling internally (following the caret as you type), exactly as before.
///
/// With the gutter, the editor is laid out at its natural full height
/// (`Length::Shrink`) inside an outer `scrollable`, so the gutter and the text
/// scroll as one and the numbers stay aligned to their lines. Wrapping is turned
/// off in this mode: one logical line then occupies exactly one visual row, which
/// is what keeps a line and its number on the same baseline (a wrapped line would
/// push every number below it out of step). Long lines scroll horizontally within
/// the editor as the caret moves.
fn code_view<'a>(state: &'a EditorState, show_line_numbers: bool) -> Element<'a, Message> {
    let editor = text_editor(&state.content)
        .on_action(Message::EditorAction)
        .font(Font::MONOSPACE)
        .size(CODE_TEXT_SIZE)
        .line_height(LineHeight::Relative(CODE_LINE_HEIGHT));

    if !show_line_numbers {
        return editor.height(Length::Fill).into();
    }

    let editor = editor.wrapping(Wrapping::None).height(Length::Shrink);

    let line_count = state.content.line_count().max(1);
    let numbers: String = (1..=line_count)
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    // Width the widest number needs, sized from the digit count against the
    // monospace advance (~0.6em) so the gutter grows with the file rather than
    // being clipped or leaving a wide gap.
    let digits = line_count.to_string().len();
    let gutter_w = digits as f32 * (CODE_TEXT_SIZE * 0.6) + 12.0;

    let gutter = container(
        text(numbers)
            .font(Font::MONOSPACE)
            .size(CODE_TEXT_SIZE)
            .line_height(LineHeight::Relative(CODE_LINE_HEIGHT))
            .width(Length::Fill)
            .align_x(Horizontal::Right)
            .color(Color::from_rgb8(110, 110, 122)),
    )
    .width(Length::Fixed(gutter_w))
    // 5px top matches the editor's default padding so line 1 lines up; the
    // right gap sets the numbers off from the code.
    .padding([5, 6]);

    scrollable(row![gutter, editor].width(Length::Fill))
        .height(Length::Fill)
        .into()
}

// ── agent tab ─────────────────────────────────────────────────────────────────

/// Agent tab body. A Build pane is the terminal canvas, optionally with the
/// find bar above it and a permission-prompt response panel docked to its
/// right; Research/Chat panes render the native chat UI instead.
fn claude_view<'a>(
    panes:           &'a [AgentPane],
    tid:             TerminalId,
    active_terminal: TerminalId,
    pending_prompt:  Option<&'a PendingPrompt>,
    prompt_source:   Option<TerminalId>,
    font_scale:      TerminalFontScale,
) -> Element<'a, Message> {
    let not_found = || -> Element<'a, Message> {
        container(text("Agent not found").size(13))
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    };

    let Some(pane) = panes.iter().find(|p| p.id() == tid) else {
        return not_found();
    };
    if let Some(chat) = pane.as_chat() {
        return chat_view(chat);
    }
    let Some(t) = pane.as_terminal() else {
        // A tab with no process behind it. Stopped panes stay this way until
        // asked back (that's the point of Stop), so they need a body; an
        // unstarted one is about to be spawned by `spawn_visible_panes` and
        // shows this for a single frame.
        return idle_pane(tid, pane.is_stopped());
    };

    let term = crate::ui::terminal::view(crate::ui::terminal::View {
        id:         tid,
        grid:       t.grid.clone(),
        master:     t.master(),
        focused:    active_terminal == tid,
        is_shell:   false,
        font_scale,
        selection:  t.selection,
        // Tint the two sides of the conversation apart — these panes run an AI
        // CLI, which is what `crate::transcript`'s heuristics look for.
        transcript: pane.kind().is_agentic(),
        find:       t.find.as_ref().map(|f| f.highlights()),
    });

    let mut body = Column::new().spacing(0).height(Length::Fill);
    if let Some(find) = &t.find {
        let total = find.hits.len();
        let label = match total {
            0 => String::from("no matches"),
            n => {
                let more = if find.capped { "+" } else { "" };
                format!("{}/{n}{more}", find.current + 1)
            }
        };
        body = body.push(find_bar(tid, &find.query, label));
    }
    body = body.push(
        container(term)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(4),
    );

    // The permission-prompt panel only belongs beside the pane that raised it.
    if pending_prompt.is_some() && prompt_source == Some(tid) {
        return row![
            container(body).width(Length::FillPortion(2)).height(Length::Fill),
            container(crate::ui::prompt_response::view(pending_prompt))
                .width(Length::FillPortion(1))
                .height(Length::Fill),
        ]
        .spacing(4)
        .height(Length::Fill)
        .into();
    }
    body.into()
}

/// Body of a Build tab whose CLI isn't running: the one the user stopped from
/// the Agents dialog, waiting to be started again.
fn idle_pane<'a>(tid: TerminalId, stopped: bool) -> Element<'a, Message> {
    let headline = if stopped {
        "This agent is stopped."
    } else {
        "Starting…"
    };
    let mut col = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .push(text(headline).font(UI_FONT).size(14));

    if stopped {
        col = col
            .push(
                text("Its tab, name and model are kept — only the CLI was ended.")
                    .font(UI_FONT)
                    .size(12)
                    .color(Color::from_rgb8(140, 140, 152)),
            )
            .push(
                button(text("Start again").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::AgentRestarted(tid))
                    .padding(buttons::PADDING)
                    .style(buttons::primary),
            );
    }

    container(col)
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// Find-in-pane bar: the query field, a hit counter, step buttons and a close.
/// Shared by Build panes (searching the terminal grid) and chat panes
/// (searching the transcript) — both drive the same `Find*` messages.
fn find_bar(id: TerminalId, query: &str, position: String) -> Element<'_, Message> {
    let step = |glyph: &'static str, forward: bool, tip: &'static str| {
        crate::ui::with_tip(
            button(text(glyph).font(ICON_FONT).size(12))
                .on_press(Message::FindStepped(id, forward))
                .padding([2, 8])
                .style(buttons::secondary),
            tip,
        )
    };

    let bar = row![
        text("Find").font(UI_FONT).size(11),
        text_input("search this conversation…", query)
            .id(find_input_id(id))
            .on_input(move |q| Message::FindQueryChanged(id, q))
            // Enter walks the hits, so a repeated press steps rather than
            // re-running the search.
            .on_submit(Message::FindStepped(id, true))
            .size(12)
            .width(Length::Fill),
        text(position).font(UI_FONT).size(11),
        step("\u{25B4}", false, "Previous match"),  // ▴
        step("\u{25BE}", true,  "Next match"),      // ▾
        crate::ui::with_tip(
            button(text("\u{2715}").font(ICON_FONT).size(12))
                .on_press(Message::FindClosed(id))
                .padding([2, 8])
                .style(buttons::secondary),
            "Close find (Ctrl+Shift+L)",
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(bar).padding([4, 8]).width(Length::Fill).into()
}

// ── chat panes (Research / Chat) ──────────────────────────────────────────────

/// Native chat pane body: a scrollable transcript above the composer. No PTY —
/// replies stream in from the headless `claude` run in [`crate::chat_api`].
fn chat_view(chat: &ChatPane) -> Element<'_, Message> {
    let id = chat.id;

    let mut transcript = Column::new()
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
    for (i, m) in chat.messages.iter().enumerate() {
        // `mark` is None for a non-hit, Some(true) for the current hit — the
        // bubble tints itself accordingly while a find is open.
        let mark = chat.find.as_ref().and_then(|f| f.mark(i));
        transcript = transcript.push(chat_bubble(id, i, m, mark));
    }

    // The in-flight reply: the tool calls it has made so far, then whatever
    // text has streamed in. A turn spent working produces no text for a long
    // stretch, and these lines are what keep the pane from looking hung.
    if chat.streaming {
        let mut live = Column::new().spacing(4).width(Length::Fill);
        for line in &chat.activity {
            live = live.push(
                text(line.as_str())
                    .font(UI_FONT)
                    .size(11)
                    .color(Color::from_rgb8(140, 140, 152)),
            );
        }
        let body = if chat.pending.is_empty() { "…" } else { chat.pending.as_str() };
        live = live.push(text(body.to_string()).font(UI_FONT).size(13));

        transcript = transcript.push(
            container(
                column![text("Claude").font(UI_FONT).size(11), live].spacing(3),
            )
            .padding([8, 10])
            .width(Length::Fill)
            .style(|_theme: &Theme| chat_bubble_style(ChatRole::Assistant, None)),
        );
    }

    let history = scrollable(transcript)
        .id(crate::app::chat_scroll_id(id))
        // Tracks whether the view is at the bottom, so streaming text only
        // auto-scrolls while the user hasn't read back up the transcript.
        .on_scroll(move |v| Message::ChatScrolled(id, v))
        .width(Length::Fill)
        .height(Length::Fill);

    let mut col = Column::new().spacing(0).height(Length::Fill);

    if let Some(find) = &chat.find {
        let total = find.hits.len();
        let label = match total {
            0 => String::from("no matches"),
            n => format!("{}/{n}", find.current + 1),
        };
        col = col.push(find_bar(id, &find.query, label));
    }

    col = col.push(container(history).height(Length::Fill).width(Length::Fill));

    if let Some(err) = &chat.error {
        col = col.push(
            container(text(format!("Error: {err}")).font(UI_FONT).size(12))
                .padding([6, 10])
                .width(Length::Fill),
        );
    }

    let composer = text_editor(&chat.input)
        .placeholder("Message Claude…")
        .on_action(move |a| Message::ChatInputAction(id, a))
        .font(UI_FONT)
        .size(13)
        .height(Length::Fixed(CHAT_INPUT_HEIGHT))
        .key_binding(move |kp| {
            use iced::widget::text_editor::Binding;
            // Enter sends, Shift+Enter inserts a newline — the convention every
            // chat UI uses, and the reason this widget needs custom bindings.
            if kp.key == keyboard::Key::Named(key::Named::Enter) && !kp.modifiers.shift() {
                return Some(Binding::Custom(Message::ChatSubmitted(id)));
            }
            if kp.modifiers.control() && kp.modifiers.shift() {
                // The composer owns the clipboard chords (its own selection,
                // not the terminal's)…
                return match kp.key.as_ref() {
                    keyboard::Key::Character("c") => Some(Binding::Custom(Message::ChatCopy(id))),
                    keyboard::Key::Character("x") => Some(Binding::Custom(Message::ChatCut(id))),
                    keyboard::Key::Character("v") => Some(Binding::Custom(Message::ChatPaste(id))),
                    // …and every other Ctrl+Shift chord belongs to the app
                    // (pane/agent/session navigation, find, the toolbox), so it
                    // falls through to the global listener untouched.
                    _ => None,
                };
            }
            Binding::from_key_press(kp)
        });

    let send = button(text("Send").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::ChatSubmitted(id))
        .padding(buttons::PADDING)
        .style(buttons::primary);

    col.push(
        row![composer, send]
            .spacing(6)
            .padding([8, 8])
            .align_y(Alignment::End),
    )
    .into()
}

/// One transcript turn: the author's label with per-message actions, over the
/// message body.
///
/// The body is a *read-only* `text_editor` rather than a `text` so the user can
/// select and copy from it — `Message::ChatBubbleAction` drops edits and keeps
/// only cursor/selection movement.
fn chat_bubble<'a>(
    id:      TerminalId,
    index:   usize,
    message: &'a crate::agent::ChatMessage,
    mark:    Option<bool>,
) -> Element<'a, Message> {
    let label = match message.role {
        ChatRole::User      => "You",
        ChatRole::Assistant => "Claude",
    };

    let action = |glyph: &'static str, msg: Message, tip: &'static str| {
        crate::ui::with_tip(
            button(text(glyph).font(ICON_FONT).size(11))
                .on_press(msg)
                .padding([2, 6])
                .style(buttons::secondary),
            tip,
        )
    };

    let header = row![
        text(label).font(UI_FONT).size(11),
        Space::with_width(Length::Fill),
        action(
            "\u{29C9}",                                   // ⧉
            Message::ChatBubbleCopy(id, index),
            "Copy the selection",
        ),
        action(
            "\u{270E}",                                   // ✎
            Message::ChatSendToNote(id, index),
            "Send the selection to a note",
        ),
    ]
    .spacing(4)
    .align_y(Alignment::Center);

    let body = text_editor(&message.content)
        .on_action(move |a| Message::ChatBubbleAction(id, index, a))
        .font(UI_FONT)
        .size(13)
        .height(Length::Shrink);

    let role = message.role;
    container(column![header, body].spacing(3))
        .padding([8, 10])
        .width(Length::Fill)
        .style(move |_theme: &Theme| chat_bubble_style(role, mark))
        .into()
}

/// Bubble tint: the two authors are told apart by background, and an open find
/// lifts its hits — brightest on the one the user is standing on.
fn chat_bubble_style(role: ChatRole, mark: Option<bool>) -> container::Style {
    let bg = match role {
        ChatRole::User      => Color::from_rgb8(40, 44, 60),
        ChatRole::Assistant => Color::from_rgb8(30, 30, 38),
    };
    let (border, width) = match mark {
        Some(true)  => (Color::from_rgb8(250, 204, 21), 2.0), // current hit
        Some(false) => (Color::from_rgb8(133, 110, 30), 1.0), // other hits
        None        => (Color::from_rgb8(55, 55, 68), 1.0),
    };
    container::Style {
        background: Some(Background::Color(bg)),
        border:     Border { color: border, width, radius: 6.0.into() },
        text_color: Some(Color::from_rgb(0.90, 0.90, 0.93)),
        ..Default::default()
    }
}

fn tab_bar<'a>(
    active:    CenterTab,
    panes:     &'a [AgentPane],
    renaming:  &'a Option<(TerminalId, String)>,
    overflow:  bool,
    band:      Color,
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
            claude_tab_button(
                label, selected, p.id(), panes.len() > 1, p.needs_attention(), p.is_live(), i, band,
            )
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
/// stopped on a question and is waiting for the user to answer. The selected
/// tab wears the quiet bottom band instead (`band` is the palette's companion
/// color, `ThemeColors::current`), matching how the active project tab is
/// marked.
///
/// A `live` pane — one holding a running agent session, see
/// [`AgentPane::is_live`] — writes its label in that same `band` color, so the
/// running agents stand out at a glance while the band marks the one in use.
/// A waiting tab keeps its usual label: the amber already says more, and the
/// band color would wash out against it.
///
/// `index` is the pane's position in the strip, which doubles as its
/// Ctrl+Shift+<digit> shortcut for the first nine.
fn claude_tab_button(
    label:    String,
    selected: bool,
    tid:      TerminalId,
    closable: bool,
    waiting:  bool,
    live:     bool,
    index:    usize,
    band:     Color,
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
    let mut label = text(label).font(UI_FONT).size(buttons::TEXT_SIZE);
    if live && !waiting {
        label = label.style(move |_: &Theme| iced::widget::text::Style {
            color: Some(band),
        });
    }
    inner = inner
        .push(label)
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
        tab.style(buttons::agent_tab(band, selected)).into()
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

