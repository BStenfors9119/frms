//! Right-side plugin panel UI — tab control hosting DB / Profile / Notes.

use iced::{Alignment, Background, Border, Color, Element, Font, Length, Theme};
use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, text, text_editor,
    text_input, Column, Space,
};

use crate::app::Message;
use crate::db::TableRef;
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::notes::{NoteFormat, NoteViewMode, NotesState};
use crate::plugin_panel::{DockSide, PluginPanel, PluginSlot, PluginTab};
use crate::prefs::Prefs;
use crate::receivers::ReceiversState;
use crate::terminals::TerminalsState;
use crate::theme::{FontScale, Mode, Palette, TerminalFontScale};
use crate::ui::{buttons, with_tip};
use crate::ui::preview;
use crate::ui::receivers_panel;
use crate::ui::terminal as terminal_ui;

/// Context the Receivers tab needs from the active session: whether a DB is
/// connected and its tables (for the import picker).
pub struct ReceiverCtx<'a> {
    pub state:        &'a ReceiversState,
    pub db_connected: bool,
    pub db_tables:    &'a [TableRef],
    pub font_scale:   TerminalFontScale,
}

/// What the Profile tab needs from the app beyond `Prefs`: whether the `claude`
/// CLI is installed (Claude Code status) and whether usage telemetry is on.
pub struct ProfileCtx {
    pub claude_installed:  bool,
    pub telemetry_enabled: bool,
}

/// Widget id for the Terminals-plugin Name field — used to focus it after
/// `+ New` so the user can name the terminal, then Tab/Enter into the shell.
pub const TERMINAL_NAME_INPUT: &str = "terminal-plugin-name";

/// Widget id for the Notes-plugin Title field — used to focus it after
/// `+ New` so the user can title the note, then Tab/Enter into the body.
pub const NOTES_TITLE_INPUT: &str = "notes-title-input";

const HANDLE_WIDTH: f32 = 5.0;

pub fn view<'a>(
    panel:     &'a PluginPanel,
    prefs:     &'a Prefs,
    notes:     &'a NotesState,
    project:   Option<&'a str>,
    terminals: &'a TerminalsState,
    receivers: &'a ReceiversState,
    db_connected: bool,
    db_tables: &'a [TableRef],
    claude_installed: bool,
    telemetry_enabled: bool,
) -> Element<'a, Message> {
    let rctx = ReceiverCtx {
        state:        receivers,
        db_connected,
        db_tables,
        font_scale:   prefs.terminal_font_scale,
    };
    let pctx = ProfileCtx { claude_installed, telemetry_enabled };
    let content: Element<'_, Message> = if panel.split {
        let top    = slot_view(panel, PluginSlot::Top,    prefs, notes, project, terminals, &rctx, &pctx);
        let bottom = slot_view(panel, PluginSlot::Bottom, prefs, notes, project, terminals, &rctx, &pctx);
        let divider = mouse_area(
            container(Space::new(Length::Fill, Length::Fixed(HANDLE_WIDTH)))
                .width(Length::Fill)
                .style(handle_bg),
        )
        .interaction(iced::mouse::Interaction::ResizingVertically)
        .on_press(Message::PluginSplitResizeStart);

        column![
            container(top).height(Length::Fixed(panel.top_height)).width(Length::Fill),
            divider,
            container(bottom).height(Length::Fill).width(Length::Fill),
        ]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        slot_view(panel, PluginSlot::Top, prefs, notes, project, terminals, &rctx, &pctx)
    };

    let panel_body = container(content).width(Length::Fill).height(Length::Fill);

    let handle = mouse_area(
        container(Space::new(Length::Fixed(HANDLE_WIDTH), Length::Fill))
            .height(Length::Fill)
            .style(handle_bg),
    )
    .interaction(iced::mouse::Interaction::ResizingHorizontally)
    .on_press(Message::PluginPanelResizeStart);

    // The drag handle lives on the panel's *inner* edge: right of the body when
    // docked left, left of the body when docked right.
    let inner = match panel.dock {
        DockSide::Left  => row![panel_body, handle],
        DockSide::Right => row![handle, panel_body],
    };

    container(inner.spacing(0).width(Length::Fill).height(Length::Fill))
        .width(Length::Fixed(panel.width))
        .height(Length::Fill)
        .style(panel_bg)
        .into()
}

/// One slot of the panel — its tab bar plus the active plugin body. The top
/// slot also carries the dock / split / hide controls.
fn slot_view<'a>(
    panel:     &'a PluginPanel,
    slot:      PluginSlot,
    prefs:     &'a Prefs,
    notes:     &'a NotesState,
    project:   Option<&'a str>,
    terminals: &'a TerminalsState,
    rctx:      &ReceiverCtx<'a>,
    pctx:      &ProfileCtx,
) -> Element<'a, Message> {
    let tab = panel.tab(slot);
    let body: Element<'_, Message> = match tab {
        PluginTab::Profile   => profile_body(prefs, pctx),
        PluginTab::Notes     => notes_body(notes, project),
        PluginTab::Terminals => terminals_body(terminals, prefs.terminal_font_scale),
        PluginTab::Receivers => receivers_panel::view(rctx),
    };

    column![
        tab_bar(panel, slot),
        container(body)
            .padding(10)
            .width(Length::Fill)
            .height(Length::Fill),
    ]
    .spacing(0)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn tab_bar(panel: &PluginPanel, slot: PluginSlot) -> Element<'static, Message> {
    let active = panel.tab(slot);
    // Suppress the per-tab ✕ when only one plugin remains so the strip can't be
    // emptied — the lone tab then has no close affordance.
    let closable = panel.shown_count() > 1;

    let mut r = row![].spacing(4).align_y(Alignment::Center);
    for t in panel.shown_tabs() {
        let select = button(text(t.label().to_string()).font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::PluginTabClicked(slot, t))
            .padding(buttons::PADDING)
            .style(buttons::toggle(active == t));
        let entry: Element<'static, Message> = if closable {
            row![
                select,
                button(text("\u{2715}").font(ICON_FONT).size(9))
                    .on_press(Message::PluginTabHidden(t))
                    .padding([7, 6])
                    .style(buttons::secondary),
            ]
            .spacing(2)
            .align_y(Alignment::Center)
            .into()
        } else {
            select.into()
        };
        r = r.push(entry);
    }

    // `+` reveals a picker of the hidden plugins; clicking one re-adds it. Only
    // shown when something is actually hidden.
    if panel.hidden_tabs().next().is_some() {
        r = r.push(
            button(text("+").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::PluginAddPickerToggled)
                .padding([7, 10])
                .style(buttons::toggle(panel.adding)),
        );
        if panel.adding {
            for t in panel.hidden_tabs() {
                r = r.push(
                    button(text(t.label().to_string()).font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::PluginTabShown(t))
                        .padding(buttons::PADDING)
                        .style(buttons::secondary),
                );
            }
        }
    }

    r = r.push(iced::widget::Space::with_width(Length::Fill));

    // Only the top slot carries the panel-wide controls so they aren't doubled.
    if slot == PluginSlot::Top {
        // Dock arrow points to the edge the panel would move to.
        let dock_icon = match panel.dock {
            DockSide::Right => "\u{25C0}", // ◀ move to the left edge
            DockSide::Left  => "\u{25B6}", // ▶ move to the right edge
        };
        r = r.push(icon_ctrl(dock_icon, Message::PluginPanelDockToggled));
        // ▤ divided box = split into two slots; ▢ single box = collapse to one.
        let split_icon = if panel.split { "\u{25A2}" } else { "\u{25A4}" };
        r = r.push(icon_ctrl(split_icon, Message::PluginPanelSplitToggled));
        // ✕ hides the panel entirely.
        r = r.push(icon_ctrl("\u{2715}", Message::PluginPanelHide));
    }

    container(r)
        .padding([6, 8])
        .width(Length::Fill)
        .style(tab_bar_bg)
        .into()
}

/// A panel-wide control rendered as an icon glyph (Dock / Split / Hide).
fn icon_ctrl(glyph: &'static str, msg: Message) -> Element<'static, Message> {
    button(text(glyph).font(ICON_FONT).size(14))
        .on_press(msg)
        .padding(buttons::PADDING)
        .style(buttons::secondary)
        .into()
}

// ── Profile plugin ────────────────────────────────────────────────────────────

fn profile_body<'a>(prefs: &'a Prefs, pctx: &ProfileCtx) -> Element<'a, Message> {
    let palette_picker = labeled_row(
        "Theme:",
        pick_list(Palette::ALL, Some(prefs.palette), Message::PrefsPaletteChanged)
            .text_size(13)
            .into(),
    );
    let mode_picker = labeled_row(
        "Light / Dark:",
        pick_list(Mode::ALL, Some(prefs.mode), Message::PrefsModeChanged)
            .text_size(13)
            .into(),
    );
    let font_picker = labeled_row(
        "Font size:",
        pick_list(FontScale::ALL, Some(prefs.font_scale), Message::PrefsFontScaleChanged)
            .text_size(13)
            .into(),
    );
    let term_font_picker = labeled_row(
        "Terminal font:",
        pick_list(
            TerminalFontScale::ALL,
            Some(prefs.terminal_font_scale),
            Message::PrefsTerminalFontScaleChanged,
        )
        .text_size(13)
        .into(),
    );

    column![
        text("Appearance").size(15),
        palette_picker,
        mode_picker,
        font_picker,
        term_font_picker,
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        claude_status_section(pctx),
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        privacy_section(pctx),
    ]
    .spacing(8)
    .into()
}

/// Privacy / telemetry controls. Telemetry is on by default; this is the opt-out
/// and the disclosure of exactly what's collected (anonymous, no PII).
fn privacy_section(pctx: &ProfileCtx) -> Element<'static, Message> {
    let toggle = iced::widget::checkbox("Share anonymous usage data", pctx.telemetry_enabled)
        .on_toggle(Message::PrefsTelemetryToggled)
        .text_size(13);

    column![
        text("Privacy").size(15),
        toggle,
        text("Helps improve frms. Sends an anonymous id, app version, OS, coarse \
              feature usage, and crash reports — never file names, project \
              paths, prompts, keys, or any personal data. Off any time here.")
            .size(11),
    ]
    .spacing(6)
    .into()
}

/// Claude Code status. Every agent pane — Build (PTY) and the Research / Chat
/// chats — drives the `claude` CLI, which authenticates through its own login
/// (a Claude Pro/Max subscription, no API key). This section reflects whether
/// the CLI is installed and points the user at the one-shot setup helper.
fn claude_status_section(pctx: &ProfileCtx) -> Element<'static, Message> {
    let status = if pctx.claude_installed {
        text("\u{2713} Claude Code is installed.").size(12)
    } else {
        text("\u{2717} Claude Code (the 'claude' CLI) was not found.").size(12)
    };

    // Build the guidance line. When installed we only need the sign-in reminder;
    // when missing we point at the bundled installer (frms-setup-claude).
    let guidance = if pctx.claude_installed {
        "Agent panes sign in through Claude Code. If you haven't yet, run \
         'claude' once in a terminal to log in (a Claude Pro/Max subscription \
         — no API key needed)."
    } else {
        "Agent panes need it. Install and sign in with one command in a \
         terminal:  frms-setup-claude  — it installs Node.js/npm + Claude Code, \
         then logs you in (a Claude Pro/Max subscription, no API key needed)."
    };

    column![
        text("Claude Code").size(15),
        status,
        text(guidance).size(11),
    ]
    .spacing(6)
    .into()
}

fn labeled_row<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![
        text(label).size(13).width(Length::Fixed(110.0)),
        control,
    ]
    .align_y(Alignment::Center)
    .spacing(8)
    .into()
}

// ── Notes plugin ──────────────────────────────────────────────────────────────

fn notes_body<'a>(notes: &'a NotesState, project: Option<&'a str>) -> Element<'a, Message> {
    let list = notes_list(notes, project);
    let editor = notes_editor(notes, project);

    row![
        container(list)
            .width(Length::Fixed(160.0))
            .height(Length::Fill),
        container(editor)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, 8]),
    ]
    .spacing(8)
    .height(Length::Fill)
    .into()
}

fn notes_list<'a>(notes: &'a NotesState, project: Option<&'a str>) -> Element<'a, Message> {
    let new_btn = button(text("+ New").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::NotesNew)
        .padding(buttons::PADDING)
        .style(buttons::primary)
        .width(Length::Fill);

    // Only this project's notes plus global ones — a note pinned to another
    // project stays out of the way.
    let visible: Vec<_> = notes.notes.iter()
        .filter(|n| n.visible_in(project))
        .collect();

    let mut list = Column::new().spacing(2).width(Length::Fill);
    if visible.is_empty() {
        list = list.push(text("(no notes)").size(11));
    } else {
        for n in visible {
            let selected = notes.selected == Some(n.id);
            let label = if n.title.is_empty() { "Untitled".to_string() } else { n.title.clone() };
            let name_text = text(label).font(UI_FONT).size(buttons::TEXT_SIZE);
            // Project-pinned notes carry a pushpin (same idiom as pinned
            // terminals); global notes are unmarked. Pin glyph needs the
            // icon font.
            let content: Element<'_, Message> = if n.project.is_some() {
                row![
                    text("\u{1F588}").font(ICON_FONT).size(buttons::TEXT_SIZE),
                    name_text,
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .into()
            } else {
                name_text.into()
            };
            list = list.push(
                button(content)
                    .on_press(Message::NotesSelected(n.id))
                    .padding(buttons::PADDING)
                    .style(buttons::toggle(selected))
                    .width(Length::Fill),
            );
        }
    }

    column![
        new_btn,
        scrollable(list).height(Length::Fill),
    ]
    .spacing(6)
    .height(Length::Fill)
    .into()
}

fn notes_editor<'a>(notes: &'a NotesState, project: Option<&'a str>) -> Element<'a, Message> {
    // A note left selected from another project isn't in the visible list —
    // treat it the same as no selection so it can't be edited blind.
    let selected = notes.selected
        .and_then(|id| notes.notes.iter().find(|n| n.id == id))
        .filter(|n| n.visible_in(project));
    let Some(note) = selected else {
        return container(
            text("Select a note on the left, or click + New to create one.")
                .size(12),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    };
    let id = note.id;

    // 🖈 pushpin (matches the Terminals plugin) — pinned notes belong to this
    // project only; unpinned ones are global and show in every project.
    let pinned    = note.project.is_some();
    let pin_style = if pinned { buttons::primary } else { buttons::secondary };
    let pin_tip   = if pinned {
        "Pinned to this project — click to make global"
    } else {
        "Global note — click to pin to this project"
    };

    let header = row![
        text_input("Title", &notes.title_input)
            .id(text_input::Id::new(NOTES_TITLE_INPUT))
            .on_input(Message::NotesTitleEdited)
            .on_submit(Message::NotesFocusBody)
            .size(14)
            .width(Length::Fill),
        with_tip(
            button(text("\u{1F588}").font(ICON_FONT).size(14))
                .on_press(Message::NotesPinToggled(id))
                .padding(buttons::PADDING)
                .style(pin_style),
            pin_tip,
        ),
        button(text("→ Claude").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::NotesSendToClaude)
            .padding(buttons::PADDING)
            .style(buttons::primary),
        button(text("Delete").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::NotesDelete(id))
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let editor = || {
        text_editor(&notes.body_content)
            .on_action(Message::NotesContentAction)
            .key_binding(notes_key_binding)
            .font(Font::MONOSPACE)
            .height(Length::Fill)
    };
    let rendered = || {
        container(preview::markdown(&notes.body_content.text()))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(container::bordered_box)
    };

    let body: Element<'_, Message> = match notes.view_mode {
        NoteViewMode::Edit    => editor().into(),
        NoteViewMode::Preview => rendered().into(),
        NoteViewMode::Split   => row![
            container(editor()).width(Length::FillPortion(1)).height(Length::Fill),
            container(rendered()).width(Length::FillPortion(1)).height(Length::Fill),
        ]
        .spacing(8)
        .height(Length::Fill)
        .into(),
    };

    column![header, notes_format_bar(notes.view_mode), body]
        .spacing(8)
        .height(Length::Fill)
        .into()
}

/// Markdown formatting toolbar shown above the note body editor, with the
/// Edit / Split / Preview view-mode toggle on the right. Formatting buttons
/// are hidden in Preview mode since there is no editor to apply them to.
fn notes_format_bar<'a>(mode: NoteViewMode) -> Element<'a, Message> {
    let btn = |label: &'a str, tip: &'a str, fmt: NoteFormat| {
        with_tip(
            button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::NotesFormat(fmt))
                .padding([4, 8])
                .style(buttons::secondary),
            tip,
        )
    };
    let mode_btn = |label: &'a str, tip: &'a str, m: NoteViewMode| {
        with_tip(
            button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::NotesViewMode(m))
                .padding([4, 8])
                .style(buttons::toggle(mode == m)),
            tip,
        )
    };

    let mut bar = row![].spacing(3).align_y(Alignment::Center);
    if mode != NoteViewMode::Preview {
        bar = bar
            .push(btn("B",   "Bold (Ctrl+B)",        NoteFormat::Bold))
            .push(btn("I",   "Italic (Ctrl+I)",      NoteFormat::Italic))
            .push(btn("S",   "Strikethrough",        NoteFormat::Strikethrough))
            .push(btn("<>",  "Inline code (Ctrl+E)", NoteFormat::Code))
            .push(btn("H1",  "Heading 1",            NoteFormat::H1))
            .push(btn("H2",  "Heading 2",            NoteFormat::H2))
            .push(btn("H3",  "Heading 3",            NoteFormat::H3))
            .push(btn("•",   "Bullet list",          NoteFormat::Bullet))
            .push(btn("1.",  "Numbered list",        NoteFormat::Numbered))
            .push(btn(">",   "Quote",                NoteFormat::Quote))
            .push(btn("```", "Code block",           NoteFormat::CodeBlock))
            .push(btn("[↗]", "Link (Ctrl+K)",        NoteFormat::Link));
    }
    bar = bar
        .push(Space::new(Length::Fill, Length::Shrink))
        .push(mode_btn("Edit",    "Markdown source only",         NoteViewMode::Edit))
        .push(mode_btn("Split",   "Source and preview side by side", NoteViewMode::Split))
        .push(mode_btn("Preview", "Rendered preview only",        NoteViewMode::Preview));
    bar.into()
}

/// Keyboard shortcuts for the note body editor — Ctrl+B/I/E/K apply markdown
/// formatting; everything else falls through to the default bindings.
fn notes_key_binding(
    kp: text_editor::KeyPress,
) -> Option<text_editor::Binding<Message>> {
    use iced::keyboard::Key;
    use text_editor::{Binding, Status};

    if kp.status == Status::Focused && kp.modifiers.command() {
        if let Key::Character(c) = &kp.key {
            let fmt = match c.as_str() {
                "b" => Some(NoteFormat::Bold),
                "i" => Some(NoteFormat::Italic),
                "e" => Some(NoteFormat::Code),
                "k" => Some(NoteFormat::Link),
                _   => None,
            };
            if let Some(fmt) = fmt {
                return Some(Binding::Custom(Message::NotesFormat(fmt)));
            }
        }
    }
    Binding::from_key_press(kp)
}

// ── Terminals plugin ──────────────────────────────────────────────────────────

fn terminals_body(
    terms: &TerminalsState,
    font_scale: TerminalFontScale,
) -> Element<'_, Message> {
    row![
        container(terminals_list(terms))
            .width(Length::Fixed(160.0))
            .height(Length::Fill),
        container(terminal_body(terms, font_scale))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, 8]),
    ]
    .spacing(8)
    .height(Length::Fill)
    .into()
}

fn terminals_list(terms: &TerminalsState) -> Element<'_, Message> {
    let new_btn = button(text("+ New").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::TerminalPluginNew)
        .padding(buttons::PADDING)
        .style(buttons::primary)
        .width(Length::Fill);

    let mut list = Column::new().spacing(2).width(Length::Fill);
    if terms.terminals.is_empty() {
        list = list.push(text("(no terminals)").size(11));
    } else {
        for t in &terms.terminals {
            let selected = terms.selected == Some(t.id);
            let label = if t.name.is_empty() {
                format!("#{}", t.id)
            } else {
                t.name.clone()
            };
            let name_text = text(label).font(UI_FONT).size(buttons::TEXT_SIZE);
            // Pinned terminals carry a filled star so they read as persistent
            // even when not selected. The star needs the icon font.
            let content: Element<'_, Message> = if t.pinned {
                row![
                    text("\u{2605}").font(ICON_FONT).size(buttons::TEXT_SIZE),
                    name_text,
                ]
                .spacing(4)
                .align_y(Alignment::Center)
                .into()
            } else {
                name_text.into()
            };
            list = list.push(
                button(content)
                    .on_press(Message::TerminalPluginSelected(t.id))
                    .padding(buttons::PADDING)
                    .style(buttons::toggle(selected))
                    .width(Length::Fill),
            );
        }
    }

    column![
        new_btn,
        scrollable(list).height(Length::Fill),
    ]
    .spacing(6)
    .height(Length::Fill)
    .into()
}

fn terminal_body(terms: &TerminalsState, font_scale: TerminalFontScale) -> Element<'_, Message> {
    let Some(id) = terms.selected else {
        return container(
            text("Click + New to start a shell terminal in this session's working directory.")
                .size(12),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    };

    let Some(t) = terms.terminal(id) else {
        return container(text("Terminal not found.").size(12)).padding(16).into();
    };

    // 🖈 pushpin (matches the session-tab pin) — pinned terminals are
    // respawned on next launch. Primary style when pinned, secondary when not.
    let pin_glyph = "\u{1F588}";
    let pin_style = if t.pinned { buttons::primary } else { buttons::secondary };
    let header = row![
        text_input("Name", &terms.name_input)
            .id(text_input::Id::new(TERMINAL_NAME_INPUT))
            .on_input(Message::TerminalPluginNameEdited)
            .on_submit(Message::TerminalPluginFocusTerminal)
            .size(14)
            .width(Length::Fill),
        button(text(pin_glyph).font(ICON_FONT).size(14))
            .on_press(Message::TerminalPluginPinToggled(id))
            .padding(buttons::PADDING)
            .style(pin_style),
        button(text("Delete").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::TerminalPluginDelete(id))
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let body = terminal_ui::view(
        t.id,
        t.pane.grid.clone(),
        t.pane.master(),
        terms.focused,
        true,
        font_scale,
        t.pane.selection,
    );

    column![header, body]
        .spacing(8)
        .height(Length::Fill)
        .into()
}

// ── styling ───────────────────────────────────────────────────────────────────

fn tab_bar_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(24, 24, 32))),
        border:     Border { color: Color::from_rgb8(55, 55, 68), width: 0.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn panel_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(28, 28, 36))),
        border:     Border { color: Color::from_rgb8(55, 55, 68), width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

fn handle_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(55, 55, 68))),
        border:     Border { color: Color::TRANSPARENT, width: 0.0, radius: 0.0.into() },
        ..Default::default()
    }
}
