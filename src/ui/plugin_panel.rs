//! Right-side plugin panel UI — tab control hosting DB / Profile / Notes.

use iced::{padding, Alignment, Background, Border, Color, Element, Font, Length, Theme};
use iced::widget::{
    button, column, container, mouse_area, pick_list, row, scrollable, stack, text, text_editor,
    text_input, Column, Space,
};

use crate::agent::AiTool;
use crate::app::Message;
use crate::db::TableRef;
use crate::docs::DocsState;
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::notes::{CategoryChoice, Note, NoteFormat, NoteViewMode, NotesState, UNCATEGORIZED_LABEL};
use crate::plugin_panel::{DockSide, PluginPanel, PluginSlot, PluginTab};
use crate::prefs::{PickedColor, Prefs};
use crate::receivers::ReceiversState;
use crate::terminals::TerminalsState;
use crate::theme::{FontScale, Mode, Palette, TerminalFontScale};
use crate::ui::{buttons, with_tip};
use crate::ui::color_picker;
use crate::ui::docs_panel;
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

/// What the Profile tab needs from the app beyond `Prefs`: whether usage
/// telemetry is on, and any half-typed color picker hex. Per-tool install
/// status is probed live in the tools section ([`AiTool::on_path`]) since the
/// tab only renders while open.
pub struct ProfileCtx {
    pub telemetry_enabled:  bool,
    pub color_draft:        Option<(PickedColor, String)>,
}

/// Widget id for the Terminals-plugin Name field — used to focus it after
/// `+ New` so the user can name the terminal, then Tab/Enter into the shell.
pub const TERMINAL_NAME_INPUT: &str = "terminal-plugin-name";

/// Widget id for the Notes-plugin Title field — used to focus it after
/// `+ New` so the user can title the note, then Tab/Enter into the body.
pub const NOTES_TITLE_INPUT: &str = "notes-title-input";

/// Widget id for the Notes-plugin search field, so the Ctrl+Shift+G chord can
/// open the tab with the caret already in it.
pub const NOTES_SEARCH_INPUT: &str = "notes-search-input";

/// Widget id for the "+ New category…" name field — focused when the user picks
/// that entry from the category dropdown so they can type straight away.
pub const NOTES_CATEGORY_INPUT: &str = "notes-category-input";

const HANDLE_WIDTH: f32 = 5.0;

pub fn view<'a>(
    panel:     &'a PluginPanel,
    prefs:     &'a Prefs,
    notes:     &'a NotesState,
    docs:      &'a DocsState,
    project:   Option<&'a str>,
    terminals: &'a TerminalsState,
    receivers: &'a ReceiversState,
    db_connected: bool,
    db_tables: &'a [TableRef],
    telemetry_enabled: bool,
    color_draft: Option<&(PickedColor, String)>,
) -> Element<'a, Message> {
    let rctx = ReceiverCtx {
        state:        receivers,
        db_connected,
        db_tables,
        font_scale:   prefs.terminal_font_scale,
    };
    let pctx = ProfileCtx {
        telemetry_enabled,
        color_draft: color_draft.cloned(),
    };
    let content: Element<'_, Message> = if panel.split {
        let top    = slot_view(panel, PluginSlot::Top,    prefs, notes, docs, project, terminals, &rctx, &pctx);
        let bottom = slot_view(panel, PluginSlot::Bottom, prefs, notes, docs, project, terminals, &rctx, &pctx);
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
        slot_view(panel, PluginSlot::Top, prefs, notes, docs, project, terminals, &rctx, &pctx)
    };

    let panel_body = container(content).width(Length::Fill).height(Length::Fill);

    // Expanded to full width: there's no neighbouring pane to resize against, so
    // drop the drag handle and let the body fill the window.
    let base: Element<'a, Message> = if panel.expanded {
        container(panel_body)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(panel_bg)
            .into()
    } else {
        let handle = mouse_area(
            container(Space::new(Length::Fixed(HANDLE_WIDTH), Length::Fill))
                .height(Length::Fill)
                .style(handle_bg),
        )
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .on_press(Message::PluginPanelResizeStart);

        // The drag handle lives on the panel's *inner* edge: right of the body
        // when docked left, left of the body when docked right.
        let inner = match panel.dock {
            DockSide::Left  => row![panel_body, handle],
            DockSide::Right => row![handle, panel_body],
        };

        container(inner.spacing(0).width(Length::Fill).height(Length::Fill))
            .width(Length::Fixed(panel.width))
            .height(Length::Fill)
            .style(panel_bg)
            .into()
    };

    // `⋯` overflow menu floats over the panel; a transparent backdrop catches
    // clicks outside it to dismiss.
    if !panel.controls_menu {
        return base;
    }
    let backdrop = mouse_area(
        container(text("")).width(Length::Fill).height(Length::Fill),
    )
    .on_press(Message::PluginPanelControlsToggled);

    stack![base, backdrop, controls_menu(panel)].into()
}

/// One slot of the panel — its tab bar plus the active plugin body. The top
/// slot also carries the dock / split / hide controls.
fn slot_view<'a>(
    panel:     &'a PluginPanel,
    slot:      PluginSlot,
    prefs:     &'a Prefs,
    notes:     &'a NotesState,
    docs:      &'a DocsState,
    project:   Option<&'a str>,
    terminals: &'a TerminalsState,
    rctx:      &ReceiverCtx<'a>,
    pctx:      &ProfileCtx,
) -> Element<'a, Message> {
    let tab = panel.tab(slot);
    let body: Element<'_, Message> = match tab {
        PluginTab::Profile   => profile_body(prefs, pctx),
        PluginTab::Notes     => notes_body(notes, project, prefs.accent_tint, prefs.bookstack_configured()),
        PluginTab::Docs      => docs_panel::view(docs, prefs.accent_tint),
        PluginTab::Terminals => terminals_body(
            terminals, prefs.terminal_font_scale, prefs.immutable_system,
        ),
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
        let selected = active == t;
        let label = text(t.label().to_string()).font(UI_FONT).size(buttons::TEXT_SIZE);
        // The ✕ lives inside the tab button, tucked into its top-right corner, so
        // a tab reads as one clean control rather than a button-plus-button pair.
        // Nested buttons work because the inner ✕ captures clicks within its own
        // bounds while the rest of the tab still selects the plugin.
        let inner: Element<'static, Message> = if closable {
            let close = button(text("\u{2715}").font(ICON_FONT).size(9))
                .on_press(Message::PluginTabHidden(t))
                .padding([0, 3])
                .style(buttons::toggle_icon(selected));
            row![label, close]
                .spacing(6)
                .align_y(Alignment::Start)
                .into()
        } else {
            label.into()
        };
        r = r.push(
            button(inner)
                .on_press(Message::PluginTabClicked(slot, t))
                .padding(buttons::PADDING)
                .style(buttons::toggle(selected)),
        );
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
    // The layout controls live behind a single `☰` menu button (opening the
    // overflow menu below) so they never squish as the panel narrows; closing
    // the toolbox is common enough to earn its own `✕` button beside it.
    if slot == PluginSlot::Top {
        r = r.push(
            button(text("\u{2630}").font(ICON_FONT).size(16)) // ☰ menu
                .on_press(Message::PluginPanelControlsToggled)
                .padding(buttons::PADDING)
                .style(buttons::toggle(panel.controls_menu)),
        );
        r = r.push(with_tip(
            button(text("\u{2715}").font(ICON_FONT).size(14)) // ✕ close
                .on_press(Message::PluginPanelHide)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
            "Close toolbox",
        ));
    }

    container(r)
        .padding([6, 8])
        .width(Length::Fill)
        .style(tab_bar_bg)
        .into()
}

/// The `⋯` overflow menu of panel-wide controls, floated over the panel body
/// and anchored top-right under the `⋯` button. Labels reflect the current
/// state (e.g. "Merge to one" while split) so one entry toggles each action.
fn controls_menu<'a>(panel: &PluginPanel) -> Element<'a, Message> {
    let dock_label = match panel.dock {
        DockSide::Right => "Dock left",
        DockSide::Left  => "Dock right",
    };
    let split_label  = if panel.split { "Merge to one" } else { "Split in two" };
    // The expand entry carries its chord, the one control here that has one.
    let expand_label = if panel.expanded {
        "Restore main area (Ctrl+Shift+F)"
    } else {
        "Expand full width (Ctrl+Shift+F)"
    };

    let item = |label: &str, msg: Message| {
        button(text(label.to_string()).font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(msg)
            .width(Length::Fill)
            .padding(buttons::PADDING)
            .style(buttons::menu_item)
    };

    let items = column![
        item(dock_label,   Message::PluginPanelDockToggled),
        item(split_label,  Message::PluginPanelSplitToggled),
        item(expand_label, Message::PluginPanelExpandToggled),
    ]
    .spacing(2);

    // Wide enough for the longest entry — the expand toggle with its chord.
    let card = container(items)
        .padding(4)
        .width(Length::Fixed(230.0))
        .style(menu_card_bg);

    // Anchored just under the tab bar on the right, where the `⋯` button lives.
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .padding(iced::Padding::ZERO.top(42.0).right(8.0))
        .into()
}

fn menu_card_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(34, 34, 44))),
        border:     Border { color: Color::from_rgb8(70, 70, 86), width: 1.0, radius: 6.0.into() },
        ..Default::default()
    }
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

    let accent_picker = color_row(
        "Accent tint:",
        PickedColor::Accent,
        "Submodule folders in the file browser, and note and doc categories.",
        prefs,
        pctx,
    );
    let active_picker = color_row(
        "Active color:",
        PickedColor::Active,
        "Selected tab's underline, and the names of agent tabs whose tool is running.",
        prefs,
        pctx,
    );

    let content = column![
        about_section(),
        Space::new(Length::Shrink, Length::Fixed(4.0)),
        text("Appearance").size(15),
        palette_picker,
        mode_picker,
        font_picker,
        term_font_picker,
        accent_picker,
        active_picker,
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        shell_section(prefs),
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        tools_section(prefs.default_tool, prefs.filter_tools_to_installed, prefs.chat_access),
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        bookstack_section(prefs),
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        keyboard_shortcuts_section(),
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        notifications_section(prefs),
        Space::new(Length::Shrink, Length::Fixed(8.0)),
        privacy_section(pctx),
    ]
    .spacing(8);

    // Scroll the whole tab: at larger font sizes the callout sections outgrow the
    // panel, so without this the lower sections (keyboard shortcuts) get clipped.
    // Pad the right so the scrollbar doesn't sit on top of the content.
    scrollable(content.width(Length::Fill).padding(iced::Padding::ZERO.right(8.0)))
        .height(Length::Fill)
        .into()
}

/// Which build this is. Sits at the top of the Profile tab because "what
/// version am I on?" is the first thing a user needs when reporting something,
/// and the bottom of a scrollable tab is where that question goes to die.
///
/// The build stamp line (commit, date, target) is what makes the version
/// actionable — two builds of `0.1.2` from different commits look identical
/// without it. Everything here comes from [`crate::version`]; `frms --version`
/// prints the same values.
fn about_section() -> Element<'static, Message> {
    let name = text(format!("frms {}", crate::version::VERSION))
        .font(UI_FONT)
        .size(15);

    let stamp = container(
        text(crate::version::build_details())
            .font(Font::MONOSPACE)
            .size(10),
    )
    .style(|theme: &Theme| {
        let p = theme.extended_palette();
        container::Style {
            // Muted against the panel: present when looked for, quiet otherwise.
            text_color: Some(mix(p.background.base.text, p.background.base.color, 0.45)),
            ..Default::default()
        }
    });

    column![name, stamp].spacing(2).into()
}

/// A Profile color setting: a full [`color_picker`] showing the color in
/// effect, a Reset back to its default once the user has picked their own,
/// and a line on what the color marks.
fn color_row<'a>(
    label: &'a str,
    which: PickedColor,
    hint:  &'a str,
    prefs: &Prefs,
    pctx:  &ProfileCtx,
) -> Element<'a, Message> {
    let color = prefs.picked_color(which);
    let hex = match &pctx.color_draft {
        Some((w, draft)) if *w == which => draft.clone(),
        _                               => crate::prefs::color_to_hex(color),
    };

    let reset = button(text("Reset").font(UI_FONT).size(buttons::TEXT_SIZE))
        .padding([4, 10])
        .on_press_maybe(
            prefs.picked_color_is_custom(which).then_some(Message::PrefsColorReset(which)),
        )
        .style(buttons::secondary);
    let picker = color_picker::view(
        color,
        hex,
        move |c| Message::PrefsColorChanged(which, c),
        Message::PrefsColorCommitted,
        move |s| Message::PrefsColorHexEdited(which, s),
        reset.into(),
    );

    row![
        text(label).size(13).width(Length::Fixed(110.0)),
        column![picker, text(hint).size(11)].spacing(6),
    ]
    .spacing(8)
    .into()
}

/// Linearly blend `a` toward `b` by `t` (0.0 = all `a`, 1.0 = all `b`), keeping
/// full opacity. Used to derive theme-aware callout tints and readable headings.
fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}

/// Accent hues for the profile callouts. Warm coral for Claude Code, teal for
/// Windsurf, cool blue for the keyboard reference — each gets a tinted box +
/// accent bar via [`callout`].
const CLAUDE_ACCENT:   Color = Color { r: 0.85, g: 0.47, b: 0.34, a: 1.0 };
const WINDSURF_ACCENT: Color = Color { r: 0.18, g: 0.72, b: 0.66, a: 1.0 };
const GEMINI_ACCENT:   Color = Color { r: 0.26, g: 0.52, b: 0.96, a: 1.0 };
const KEYS_ACCENT:     Color = Color { r: 0.36, g: 0.55, b: 0.94, a: 1.0 };

/// The callout accent for a given AI tool.
fn tool_accent(tool: AiTool) -> Color {
    match tool {
        AiTool::Claude   => CLAUDE_ACCENT,
        AiTool::Windsurf => WINDSURF_ACCENT,
        AiTool::Gemini   => GEMINI_ACCENT,
    }
}

/// An accent tinted for legibility against the current theme — brightened on
/// dark themes, darkened on light ones — so it never washes out against the
/// panel behind it.
fn readable_accent(accent: Color, theme: &Theme) -> Color {
    if theme.extended_palette().is_dark {
        mix(accent, Color::WHITE, 0.20)
    } else {
        mix(accent, Color::BLACK, 0.35)
    }
}

/// Wrap a section in a highlighted "callout" box: a tinted panel with a colored
/// bar down the left edge and an accent-colored heading, so it reads as a
/// pulled-out note instead of blending into the surrounding body text.
fn callout<'a>(
    accent:  Color,
    heading: &'a str,
    body:    Element<'a, Message>,
) -> Element<'a, Message> {
    let heading = container(text(heading).size(14))
        .style(move |theme: &Theme| container::Style {
            text_color: Some(readable_accent(accent, theme)),
            ..Default::default()
        });

    let inner = column![heading, body].spacing(6);

    // Tinted panel; square left corners so it butts flush against the accent bar.
    // The tint and outline are derived from the theme so the panel reads as a
    // dark accent wash in dark mode and a light one in light mode, and the body
    // text inherits the theme's legible foreground color.
    let panel = container(inner)
        .padding([10, 12])
        .width(Length::Fill)
        .style(move |theme: &Theme| {
            let p = theme.extended_palette();
            let bg = mix(p.background.base.color, accent, if p.is_dark { 0.16 } else { 0.12 });
            container::Style {
                background: Some(Background::Color(bg)),
                text_color: Some(p.background.base.text),
                border: Border {
                    color:  Color { a: 0.55, ..accent },
                    width:  1.5,
                    radius: iced::border::Radius {
                        top_left: 0.0, bottom_left: 0.0, top_right: 8.0, bottom_right: 8.0,
                    },
                },
                ..Default::default()
            }
        });

    // The accent bar is the outer container's background, revealed as a 3px
    // stripe by the left padding. Height stays Shrink, so the callout is exactly
    // as tall as its content instead of a `Fill` bar stretching it to the
    // scrollable's full viewport height.
    container(panel)
        .width(Length::Fill)
        .padding(iced::Padding::ZERO.left(3.0))
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(accent)),
            border:     Border { radius: 8.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// Reference list of the app's keyboard shortcuts, grouped by the part of the
/// UI each one drives so a user hunting for "the toolbox one" can scan straight
/// to it. Purely informational — the bindings themselves live in
/// `app::app_chord` / `app::handle_key`; keep the two in sync.
fn keyboard_shortcuts_section() -> Element<'static, Message> {
    // (area, [(keys, what it does)]).
    let groups: &[(&str, &[(&str, &str)])] = &[
        ("Projects", &[
            ("Ctrl+Shift+]  /  [",       "Next / previous project session"),
            ("Ctrl+1 … 9",               "Jump to project session N (number row or keypad)"),
            ("Ctrl+Shift+E",             "Show / hide the file explorer"),
        ]),
        ("Agent panes", &[
            ("Ctrl+Shift+→  /  ←",       "Cycle panes (includes the toolbox)"),
            ("Ctrl+Shift+A",             "Cycle agent panes only (skips the toolbox)"),
            ("Ctrl+Shift+1 … 9",         "Jump straight to agent pane N — the panes it skips are not started"),
            ("Ctrl+Shift+↓  /  ↑",       "Next / previous pane needing attention"),
            ("Ctrl+Shift+T",             "New agent pane"),
            ("Ctrl+Shift+W",             "Close the focused agent pane"),
            ("Ctrl+Shift+M",             "Change the model on one or many agents"),
            ("Ctrl+Shift+L",             "Find in the agent conversation (Enter walks the hits)"),
            ("Ctrl+Shift+Enter  /  R",   "Answer the surfaced prompt (Accept / Reject)"),
        ]),
        ("Database", &[
            ("Ctrl+Shift+D",             "Open this project's Database tool"),
        ]),
        ("Toolbox", &[
            ("Ctrl+Shift+B",             "Toggle the toolbox panel"),
            ("Ctrl+Shift+F",             "Expand to full width / restore the main area"),
            ("Ctrl+Shift+PgDn  /  PgUp", "Cycle the toolbox's tools"),
            ("Ctrl+Shift+S",             "Search this project's _specs / _docs"),
            ("Ctrl+Shift+G",             "Search notes"),
        ]),
        ("Terminal", &[
            ("Ctrl+Shift+C  /  V",       "Copy / paste in the terminal"),
            ("Ctrl+Shift+N",             "New note from the selection"),
        ]),
        ("App", &[
            ("Ctrl+Shift+Q",             "Quit frms"),
        ]),
    ];

    let mut col = column![].spacing(4);
    for (i, (area, shortcuts)) in groups.iter().enumerate() {
        // Breathing room above every group but the first, which sits directly
        // under the callout's own heading.
        if i > 0 {
            col = col.push(Space::new(Length::Shrink, Length::Fixed(6.0)));
        }
        col = col.push(shortcut_group_header(area));
        for (keys, desc) in shortcuts.iter() {
            col = col.push(
                row![
                    container(text(*keys).font(Font::MONOSPACE).size(11))
                        .width(Length::Fixed(215.0)),
                    text(*desc).font(UI_FONT).size(11),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
    }
    callout(KEYS_ACCENT, "Keyboard shortcuts", col.into())
}

/// One group label inside the shortcut reference: an accent-tinted, upper-case
/// name with a hairline running out to the right edge, so the groups read as
/// section dividers rather than just more rows of text.
fn shortcut_group_header(area: &str) -> Element<'static, Message> {
    let label = container(text(area.to_uppercase()).font(UI_FONT).size(10))
        .style(|theme: &Theme| container::Style {
            text_color: Some(readable_accent(KEYS_ACCENT, theme)),
            ..Default::default()
        });

    // A 1px container background rather than `horizontal_rule`, which takes its
    // color from the theme's rule style and so can't carry the accent.
    let rule = container(Space::new(Length::Fill, Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(Color {
                a: 0.35,
                ..readable_accent(KEYS_ACCENT, theme)
            })),
            ..Default::default()
        });

    row![label, rule]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

/// Desktop-notification controls. On by default; this is the opt-out for the
/// background toast frms raises when an agent needs attention while frms isn't
/// the focused window.
fn notifications_section(prefs: &Prefs) -> Element<'static, Message> {
    let toggle = iced::widget::checkbox("Notify when an agent needs attention", prefs.notifications)
        .on_toggle(Message::PrefsNotificationsToggled)
        .text_size(13);

    column![
        text("Notifications").size(15),
        toggle,
        text("Raises a desktop notification when a Build session stops on a \
              question, or a Research/Chat reply finishes, while frms is in the \
              background. Uses your desktop's notification service (notify-send \
              on Linux); nothing is shown while you're looking at that pane.")
            .size(11),
    ]
    .spacing(6)
    .into()
}

/// Privacy / telemetry controls. The optional tier is on by default and this is
/// its opt-out; the install/app-open count always reports and is stated here so
/// the toggle isn't read as covering more than it does. Disclosure of exactly
/// what's collected (anonymous, no PII) either way.
fn privacy_section(pctx: &ProfileCtx) -> Element<'static, Message> {
    let toggle = iced::widget::checkbox("Share anonymous usage data", pctx.telemetry_enabled)
        .on_toggle(Message::PrefsTelemetryToggled)
        .text_size(13);

    column![
        text("Privacy").size(15),
        toggle,
        text("Helps improve frms. Sends coarse feature usage and crash reports — \
              never file names, project paths, prompts, keys, or any personal \
              data. Off any time here.")
            .size(11),
        text("frms always reports that it was installed and each time it's \
              opened, with an anonymous install id, app version, and OS. That \
              part isn't covered by the toggle.")
            .size(11),
    ]
    .spacing(6)
    .into()
}

/// Everything about the shells the Terminals tab spawns: the immutable-system
/// toggle that makes them remember their toolbox, and the `ls` flags they alias
/// `ls` to. Both only ever affect *new* shells — pushing either into a terminal
/// that's already open would type into whatever program is running there.
fn shell_section(prefs: &Prefs) -> Element<'static, Message> {
    let immutable = iced::widget::checkbox(
        "Immutable system (toolbox / distrobox)", prefs.immutable_system,
    )
    .on_toggle(Message::PrefsImmutableSystemToggled)
    .text_size(12);

    let immutable_detail = text(
        "For atomic distros like Fedora Silverblue, where the work happens \
         inside a container. A pinned terminal remembers the toolbox it was \
         left in and re-enters it on the next launch, and the Terminals tab \
         shows which one each shell is in. Leave off on a conventional distro."
    ).size(11);

    // One checkbox per flag, stacked. The panel is a narrow dock, so a row of
    // bare `-l`/`-h` boxes would either wrap badly or hide what each one does.
    let mut flags = Column::new().spacing(2);
    for (flag, label) in crate::prefs::LS_FLAGS {
        flags = flags.push(
            iced::widget::checkbox(format!("-{flag}   {label}"), prefs.ls_flag(flag))
                .on_toggle(move |on| Message::PrefsLsFlagToggled(flag, on))
                .text_size(12),
        );
    }

    // Show the exact command the shells will run, so the flag combination isn't
    // something the user has to assemble in their head.
    let preview = match prefs.ls_command() {
        Some(cmd) => text(format!("ls  →  {cmd}")).font(Font::MONOSPACE).size(11),
        None      => text("No flags added — `ls` is left exactly as your shell has it.").size(11),
    };

    column![
        text("Shell").size(15),
        immutable,
        immutable_detail,
        Space::new(Length::Shrink, Length::Fixed(4.0)),
        text("Automatic `ls` flags").size(13),
        flags,
        preview,
        text("Applied by aliasing `ls` in each new Terminals-tab shell — \
              including inside a toolbox. Terminals already open keep the \
              flags they started with.")
            .size(11),
    ]
    .spacing(6)
    .into()
}

/// AI tools status. Each agentic Build pane runs one of these CLIs in a PTY;
/// every tool owns its own login (a subscription, no API key). Leads with the
/// default-tool picker (which CLI new Build panes launch) and the opt-in
/// "installed only" filter for the "+ Agent" picker, then one callout per tool
/// in [`AiTool::ALL`] showing install state, sign-in, and — when missing — an
/// Install button that runs the tool's install command in a visible terminal.
fn tools_section(
    default_tool:    AiTool,
    filter_installed: bool,
    chat_access:     crate::chat_api::ChatAccess,
) -> Element<'static, Message> {
    let default_picker = labeled_row(
        "Default tool:",
        pick_list(AiTool::ALL, Some(default_tool), Message::PrefsDefaultToolChanged)
            .text_size(13)
            .into(),
    );
    let filter_toggle = iced::widget::checkbox(
        "Only show installed tools in the + Agent picker", filter_installed,
    )
    .on_toggle(Message::PrefsFilterToolsToggled)
    .text_size(12);

    // Research/Chat panes run the same Claude Code harness as a Build pane, in
    // the session's project directory — this is the ceiling on what it's handed.
    let access_picker = labeled_row(
        "Chat pane access:",
        pick_list(
            crate::chat_api::ChatAccess::ALL,
            Some(chat_access),
            Message::PrefsChatAccessChanged,
        )
        .text_size(13)
        .into(),
    );
    let access_detail = text(chat_access.detail()).font(UI_FONT).size(11);

    let mut col = column![
        text("AI tools").size(15),
        default_picker,
        filter_toggle,
        access_picker,
        access_detail,
    ]
    .spacing(8);
    for &tool in AiTool::ALL {
        col = col.push(tool_callout(tool));
    }
    col.into()
}

/// One tool's status callout: install state, sign-in hint, and an Install
/// button when the CLI isn't on `PATH`.
fn tool_callout(tool: AiTool) -> Element<'static, Message> {
    let installed = tool.on_path();
    let label     = tool.label();

    let status = if installed {
        text(format!("\u{2713} {label} is installed.")).size(12)
    } else {
        text(format!("\u{2717} {label} (the '{}' CLI) was not found.", tool.binary())).size(12)
    };

    let mut body = column![status].spacing(6);

    if installed {
        // Just the sign-in reminder — the tool handles its own auth.
        body = body.push(
            text(format!("Agent panes sign in through {label}. If you haven't yet, {}",
                         tool.login_hint()))
                .size(11),
        );
    } else {
        // Point at the install command and offer to run it in a terminal.
        body = body.push(
            text(format!("Agent panes need it. Install runs:  {}  — then {}",
                         tool.install_command(), tool.login_hint()))
                .size(11),
        );
        body = body.push(install_button(tool));
    }

    callout(tool_accent(tool), label, body.into())
}

/// "Install <tool>" button — emits [`Message::InstallTool`], which opens a
/// Terminals-plugin shell running the tool's install command.
fn install_button(tool: AiTool) -> Element<'static, Message> {
    let accent = tool_accent(tool);
    button(text(format!("Install {}", tool.label())).size(12).font(UI_FONT))
        .padding([5, 12])
        .on_press(Message::InstallTool(tool))
        .style(move |_theme, status| {
            let bg = match status {
                button::Status::Hovered => mix(accent, Color::WHITE, 0.12),
                _                       => accent,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: Color::WHITE,
                border: Border { radius: 5.0.into(), ..Default::default() },
                ..button::Style::default()
            }
        })
        .into()
}

/// Optional BookStack notes-sync config. Everything here is empty by default;
/// until a URL and both token halves are filled in, the "Sync to BookStack"
/// button in the Notes tab stays hidden, so a user without a BookStack instance
/// can leave this untouched. The token is stored in plaintext in `prefs.json`,
/// the same as the Cloud SQL credential path — stated here so that's no surprise.
fn bookstack_section<'a>(prefs: &'a Prefs) -> Element<'a, Message> {
    let status = if prefs.bookstack_configured() {
        text("Configured — the Notes tab shows a “Sync with BookStack” button.")
            .font(UI_FONT).size(11)
    } else {
        text("Fill in the URL and API token to enable syncing notes.")
            .font(UI_FONT).size(11)
    };

    column![
        text("BookStack sync").size(15),
        text("Two-way sync between your notes and a BookStack wiki. Each project \
              becomes a book, each category a chapter, and each note a page — all \
              filed under a shelf. Syncing pushes local edits, pulls pages you \
              wrote in BookStack, and flags notes that changed on both sides for \
              you to resolve. Optional: leave blank if you don't use BookStack.")
            .font(UI_FONT).size(11),
        bookstack_field("URL:", "https://wiki.example.com", &prefs.bookstack_url,
                        false, Message::PrefsBookStackUrlEdited),
        bookstack_field("Token ID:", "API token id", &prefs.bookstack_token_id,
                        false, Message::PrefsBookStackTokenIdEdited),
        bookstack_field("Token secret:", "API token secret", &prefs.bookstack_token_secret,
                        true, Message::PrefsBookStackTokenSecretEdited),
        bookstack_field("Shelf:", crate::prefs::DEFAULT_BOOKSTACK_SHELF, &prefs.bookstack_shelf,
                        false, Message::PrefsBookStackShelfEdited),
        text("Create the token in BookStack under your profile → API Tokens. \
              It's stored in plaintext in prefs.json.")
            .font(UI_FONT).size(11),
        status,
    ]
    .spacing(6)
    .into()
}

/// One labeled config field for [`bookstack_section`]. `secure` masks the input
/// (used for the token secret).
fn bookstack_field<'a>(
    label:       &'a str,
    placeholder: &'a str,
    value:       &'a str,
    secure:      bool,
    on_input:    fn(String) -> Message,
) -> Element<'a, Message> {
    let input = text_input(placeholder, value)
        .on_input(on_input)
        .secure(secure)
        .size(12)
        .width(Length::Fill);
    labeled_row(label, input.into())
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

fn notes_body<'a>(
    notes:           &'a NotesState,
    project:         Option<&'a str>,
    folder:          Color,
    bookstack_ready: bool,
) -> Element<'a, Message> {
    let list = notes_list(notes, project, folder, bookstack_ready);
    let editor = notes_editor(notes, project, folder);

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

fn notes_list<'a>(
    notes:           &'a NotesState,
    project:         Option<&'a str>,
    folder:          Color,
    bookstack_ready: bool,
) -> Element<'a, Message> {
    let new_btn = button(text("+ New").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::NotesNew)
        .padding(buttons::PADDING)
        .style(buttons::primary)
        .width(Length::Fill);

    // Searches title, body, and category — see `NotesState::matches_search`.
    // ✕ clears it, and only appears while there's something to clear.
    let search = text_input("Search notes…", &notes.search)
        .id(text_input::Id::new(NOTES_SEARCH_INPUT))
        .on_input(Message::NotesSearchChanged)
        .size(12)
        .width(Length::Fill);
    let search_row: Element<'_, Message> = if notes.is_searching() {
        row![
            search,
            button(text("\u{2715}").font(ICON_FONT).size(10))
                .on_press(Message::NotesSearchCleared)
                .padding([4, 6])
                .style(buttons::secondary),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .into()
    } else {
        search.into()
    };

    // Only this project's notes plus global ones — a note pinned to another
    // project stays out of the way — and only those matching the search.
    let visible: Vec<&Note> = notes.visible_notes(project);

    let mut list = Column::new().spacing(2).width(Length::Fill);
    if visible.is_empty() {
        // Distinguish "you have no notes" from "your search hid them all" —
        // otherwise a stale filter reads as data loss.
        let msg = if notes.is_searching() { "(no matches)" } else { "(no notes)" };
        list = list.push(text(msg).size(11));
    } else {
        // A search forces every group open: the user asked to see matches, and
        // hiding some behind a fold they collapsed days ago would read as the
        // search having missed them. Collapse state is left untouched, so the
        // list folds back the way they left it once the search is cleared.
        let searching = notes.is_searching();
        let categories = notes.categories_in(project);
        if categories.is_empty() {
            // No categories in use — keep the list flat and uncluttered.
            for n in &visible {
                list = list.push(note_row(n, notes.selected == Some(n.id), None));
            }
        } else {
            // Grouped: each named category (sorted), then the uncategorized
            // "General" group last. Empty groups are skipped.
            for cat in &categories {
                let members: Vec<&&Note> = visible.iter()
                    .filter(|n| n.category.as_deref() == Some(cat.as_str()))
                    .collect();
                if members.is_empty() { continue; }
                let collapsed = !searching && notes.is_category_collapsed(cat);
                list = list.push(category_header(cat, cat, collapsed, members.len(), folder));
                if !collapsed {
                    for n in members {
                        list = list.push(note_row(n, notes.selected == Some(n.id), Some(folder)));
                    }
                }
            }
            let uncategorized: Vec<&&Note> = visible.iter()
                .filter(|n| n.category.is_none())
                .collect();
            if !uncategorized.is_empty() {
                let collapsed = !searching && notes.is_category_collapsed("");
                list = list.push(
                    category_header(UNCATEGORIZED_LABEL, "", collapsed, uncategorized.len(), folder),
                );
                if !collapsed {
                    for n in uncategorized {
                        list = list.push(note_row(n, notes.selected == Some(n.id), Some(folder)));
                    }
                }
            }
        }
    }

    let mut col = column![
        new_btn,
        search_row,
        scrollable(list).height(Length::Fill),
    ]
    .spacing(6)
    .height(Length::Fill);

    // The BookStack sync footer only appears once the feature is configured in
    // Profile, so a user without a BookStack instance never sees it.
    if bookstack_ready {
        col = col.push(bookstack_footer(notes));
    }

    col.into()
}

/// Two-way "Sync" button, the last run's status line, and — when the last sync
/// turned up notes that changed on both sides — a list of those conflicts, each
/// a button that opens its diff in the editor pane. Shown at the bottom of the
/// notes list when a BookStack instance is configured. The button is disabled
/// (and relabelled) while a sync is in flight.
fn bookstack_footer<'a>(notes: &'a NotesState) -> Element<'a, Message> {
    let syncing = notes.bookstack_syncing;
    let label = if syncing { "Syncing…" } else { "Sync with BookStack" };
    let mut btn = button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
        .padding(buttons::PADDING)
        .style(buttons::secondary)
        .width(Length::Fill);
    // No on_press while syncing → the button reads as disabled and can't queue
    // a second concurrent run.
    if !syncing {
        btn = btn.on_press(Message::NotesSyncToBookStack);
    }

    let mut col = column![btn].spacing(4);
    if let Some(s) = notes.bookstack_status.as_deref() {
        col = col.push(text(s.to_string()).font(UI_FONT).size(10));
    }

    // Conflicts (both sides changed) are never auto-resolved — list each so the
    // user can open its diff and pick a side.
    if !notes.bookstack_conflicts.is_empty() {
        col = col.push(
            text(format!("\u{26A0} {} conflict(s) — review:", notes.bookstack_conflicts.len()))
                .font(UI_FONT).size(10)
                .color(CONFLICT_COLOR),
        );
        for c in &notes.bookstack_conflicts {
            let title = if c.local_title.trim().is_empty() { "Untitled" } else { c.local_title.as_str() };
            col = col.push(
                button(text(title.to_string()).font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::NotesConflictShow(c.note_id))
                    .padding(buttons::PADDING)
                    .style(buttons::secondary)
                    .width(Length::Fill),
            );
        }
    }

    // Pages authored in BookStack that no note points at — offered for import
    // rather than dumped into the list. Each has an Import (✓) and Dismiss (✕).
    if !notes.bookstack_pending_imports.is_empty() {
        col = col.push(
            text(format!("{} new page(s) in BookStack:", notes.bookstack_pending_imports.len()))
                .font(UI_FONT).size(10),
        );
        for p in &notes.bookstack_pending_imports {
            let title = if p.title.trim().is_empty() { "Untitled" } else { p.title.as_str() };
            col = col.push(
                row![
                    text(title.to_string()).font(UI_FONT).size(11).width(Length::Fill),
                    button(text("\u{2713}").font(ICON_FONT).size(11))
                        .on_press(Message::NotesImportPage(p.page_id))
                        .padding([2, 6])
                        .style(buttons::primary),
                    button(text("\u{2715}").font(ICON_FONT).size(11))
                        .on_press(Message::NotesDismissImport(p.page_id))
                        .padding([2, 6])
                        .style(buttons::secondary),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            );
        }
        col = col.push(
            row![
                button(text("Import all").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::NotesImportAll)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
                button(text("Dismiss all").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::NotesDismissAllImports)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
            ]
            .spacing(4),
        );
    }

    col.into()
}

/// Accent for conflict affordances and removed diff lines.
const CONFLICT_COLOR: Color = Color { r: 0.85, g: 0.42, b: 0.42, a: 1.0 };
/// Green for added diff lines (BookStack's version).
const ADDED_COLOR: Color = Color { r: 0.40, g: 0.72, b: 0.45, a: 1.0 };
/// Muted grey for unchanged (context) diff lines.
const CONTEXT_COLOR: Color = Color { r: 0.58, g: 0.58, b: 0.62, a: 1.0 };

/// Height of the folder-colored rule drawn beside notes filed in a category.
/// Fixed rather than `Fill` — a `Fill` child inside a shrink-height row would
/// stretch to the whole scrollable — and roughly the height of one note row.
const NOTE_RULE_HEIGHT: f32 = 28.0;

/// How far a note filed under a category is indented from the folder header.
const NOTE_INDENT: f32 = 10.0;

/// One selectable note button in the list. Every note leads with a 🗎 document
/// glyph so it never reads as a folder; project-pinned notes add a pushpin
/// (same idiom as pinned terminals). `folder` — `Some` for a note filed under a
/// category — indents the row behind a rule in that folder's color, tying it to
/// the header above it.
fn note_row<'a>(n: &Note, selected: bool, folder: Option<Color>) -> Element<'a, Message> {
    let label = if n.title.is_empty() { "Untitled".to_string() } else { n.title.clone() };
    let mut content = row![
        text("\u{1F5CE}").font(ICON_FONT).size(buttons::TEXT_SIZE), // 🗎 — a note, not a folder
    ]
    .spacing(4)
    .align_y(Alignment::Center);
    if n.project.is_some() {
        content = content.push(text("\u{1F588}").font(ICON_FONT).size(buttons::TEXT_SIZE));
    }
    content = content.push(text(label).font(UI_FONT).size(buttons::TEXT_SIZE));

    let btn = button(content)
        .on_press(Message::NotesSelected(n.id))
        .padding(buttons::PADDING)
        .style(buttons::note_item(selected))
        .width(Length::Fill);

    let Some(folder) = folder else {
        return btn.into();
    };

    row![
        container(Space::new(Length::Fixed(2.0), Length::Fixed(NOTE_RULE_HEIGHT)))
            .style(move |_theme: &Theme| container::Style {
                background: Some(Background::Color(Color { a: 0.85, ..folder })),
                border: Border { radius: 1.0.into(), ..Border::default() },
                ..container::Style::default()
            }),
        btn,
    ]
    .spacing(NOTE_INDENT - 2.0)
    .padding(padding::left(NOTE_INDENT))
    .align_y(Alignment::Center)
    .into()
}

/// A collapsible category section header in the notes list. `key` is the value
/// passed back to toggle collapse (the category name, or "" for uncategorized).
/// Folders wear the submodule highlight color (the user's Profile pick, shared
/// with the file browser) plus a 🗁/🗀 folder glyph, so a group header never
/// reads as one of the theme-colored note rows filed under it.
fn category_header<'a>(
    label:     &str,
    key:       &str,
    collapsed: bool,
    count:     usize,
    folder:    Color,
) -> Element<'a, Message> {
    let caret = if collapsed { "\u{25B8}" } else { "\u{25BE}" };   // ▸ / ▾
    let glyph = if collapsed { "\u{1F5C0}" } else { "\u{1F5C1}" }; // 🗀 / 🗁
    let header = row![
        text(caret.to_string()).font(ICON_FONT).size(10),
        text(glyph.to_string()).font(ICON_FONT).size(12),
        text(label.to_string()).font(UI_FONT).size(11),
        Space::new(Length::Fill, Length::Shrink),
        text(count.to_string()).font(UI_FONT).size(10),
    ]
    .spacing(5)
    .align_y(Alignment::Center);
    button(header)
        .on_press(Message::NotesCategoryToggleCollapsed(key.to_string()))
        .padding([5, 7])
        .style(buttons::note_category(folder, collapsed))
        .width(Length::Fill)
        .into()
}

fn notes_editor<'a>(
    notes:   &'a NotesState,
    project: Option<&'a str>,
    folder:  Color,
) -> Element<'a, Message> {
    // A conflict under review takes over the pane: the diff and the choice
    // matter more than whatever note happens to be selected behind it.
    if let Some(conflict) = notes.open_conflict() {
        return conflict_view(conflict);
    }

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

    // Actions sit on their own row above the title so the name field can span
    // the full width of the pane below them. The category picker rides on the
    // left of this row; the note actions sit on the right.
    let actions = row![
        notes_category_row(notes, note, project, folder),
        Space::new(Length::Fill, Length::Shrink),
        with_tip(
            button(text("\u{1F588}").font(ICON_FONT).size(14))
                .on_press(Message::NotesPinToggled(id))
                .padding(buttons::PADDING)
                .style(pin_style),
            pin_tip,
        ),
        button(text("→ Prompt").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::NotesPromptAgent)
            .padding(buttons::PADDING)
            .style(buttons::primary),
        button(text("Delete").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::NotesDelete(id))
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let header = column![
        actions,
        text_input("Title", &notes.title_input)
            .id(text_input::Id::new(NOTES_TITLE_INPUT))
            .on_input(Message::NotesTitleEdited)
            .on_submit(Message::NotesFocusBody)
            .size(14)
            .width(Length::Fill),
    ]
    .spacing(8);

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

    column![
        header,
        notes_format_bar(notes.view_mode),
        body,
    ]
    .spacing(8)
    .height(Length::Fill)
    .into()
}

/// The conflict-resolution pane, shown in place of the editor while a conflict
/// is open. A header, the two-way choice, and a unified line diff of the local
/// note (`−`) against the BookStack page (`+`) so the user sees exactly what
/// differs before picking a side.
fn conflict_view<'a>(c: &'a crate::bookstack::Conflict) -> Element<'a, Message> {
    let id = c.note_id;
    let title = if c.local_title.trim().is_empty() { "Untitled" } else { c.local_title.as_str() };

    let header = column![
        row![
            text(format!("\u{26A0} Conflict: {title}")).font(UI_FONT).size(15).color(CONFLICT_COLOR),
            Space::new(Length::Fill, Length::Shrink),
            button(text("Close").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::NotesConflictClose)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        ]
        .align_y(Alignment::Center),
        text("This note changed both here and in BookStack since the last sync. \
              Nothing has been overwritten — choose which version to keep.")
            .font(UI_FONT).size(11),
    ]
    .spacing(6);

    let choose = row![
        button(text("Keep my version").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::NotesConflictKeepLocal(id))
            .padding(buttons::PADDING)
            .style(buttons::primary),
        button(text("Use BookStack version").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::NotesConflictUseRemote(id))
            .padding(buttons::PADDING)
            .style(buttons::danger),
        Space::new(Length::Fill, Length::Shrink),
        text("\u{2212} yours").font(Font::MONOSPACE).size(10).color(CONFLICT_COLOR),
        text("+ BookStack").font(Font::MONOSPACE).size(10).color(ADDED_COLOR),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut diff = Column::new().spacing(1).width(Length::Fill);
    // The title diffs on its own line first (when it changed), then the body.
    if c.local_title != c.remote_title {
        diff = diff.push(text("Title").font(UI_FONT).size(11));
        diff = diff.push(diff_line(DiffTag::Del, &c.local_title));
        diff = diff.push(diff_line(DiffTag::Add, &c.remote_title));
        diff = diff.push(text(" ").size(6));
    }
    for (tag, line) in line_diff(&c.local_body, &c.remote_body) {
        diff = diff.push(diff_line(tag, &line));
    }

    column![
        header,
        choose,
        container(scrollable(diff))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(6)
            .style(container::bordered_box),
    ]
    .spacing(8)
    .height(Length::Fill)
    .into()
}

/// One line of a [`line_diff`], rendered with its marker and color.
fn diff_line<'a>(tag: DiffTag, line: &str) -> Element<'a, Message> {
    let (prefix, color) = match tag {
        DiffTag::Del  => ("\u{2212} ", CONFLICT_COLOR),
        DiffTag::Add  => ("+ ", ADDED_COLOR),
        DiffTag::Same => ("  ", CONTEXT_COLOR),
    };
    text(format!("{prefix}{line}")).font(Font::MONOSPACE).size(12).color(color).into()
}

/// Which side a diff line belongs to.
#[derive(Clone, Copy)]
enum DiffTag { Same, Add, Del }

/// A unified line diff of `old` → `new`, built from a longest-common-subsequence
/// table. Sized for notes, not large files — the table is O(lines²).
fn line_diff(old: &str, new: &str) -> Vec<(DiffTag, String)> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());

    // lcs[i][j] = length of the LCS of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((DiffTag::Same, a[i].to_string()));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push((DiffTag::Del, a[i].to_string()));
            i += 1;
        } else {
            out.push((DiffTag::Add, b[j].to_string()));
            j += 1;
        }
    }
    while i < n { out.push((DiffTag::Del, a[i].to_string())); i += 1; }
    while j < m { out.push((DiffTag::Add, b[j].to_string())); j += 1; }
    out
}

/// Category picker for the selected note. Normally a dropdown of existing
/// categories plus "General" (uncategorized) and "+ New category…"; while the
/// user is naming a new category it becomes a small text field with Add/Cancel.
fn notes_category_row<'a>(
    notes:   &'a NotesState,
    note:    &Note,
    project: Option<&'a str>,
    folder:  Color,
) -> Element<'a, Message> {
    let label = text("Category").font(UI_FONT).size(11);

    if notes.adding_category {
        let input = text_input("New category", &notes.category_input)
            .id(text_input::Id::new(NOTES_CATEGORY_INPUT))
            .on_input(Message::NotesCategoryInput)
            .on_submit(Message::NotesCategoryCommit)
            .size(13)
            .width(Length::Fixed(150.0));
        return row![
            label,
            input,
            button(text("Add").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::NotesCategoryCommit)
                .padding([4, 8])
                .style(buttons::primary),
            button(text("Cancel").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::NotesCategoryCancel)
                .padding([4, 8])
                .style(buttons::secondary),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into();
    }

    // No standing category → leave the pick_list unselected so it shows its
    // "Categorize" placeholder as the button label; a chosen category shows its
    // own name instead.
    let selected = note.category.as_ref().map(|c| CategoryChoice::Named(c.clone()));
    // General (uncategorized) first, existing categories, then the new-category
    // action last — matching the grouped order in the list.
    let mut options = vec![CategoryChoice::Uncategorized];
    options.extend(notes.categories_in(project).into_iter().map(CategoryChoice::Named));
    options.push(CategoryChoice::NewCategory);

    pick_list(options, selected, Message::NotesCategorySelected)
        .placeholder("Categorize")
        .text_size(buttons::TEXT_SIZE)
        .padding(buttons::PADDING)
        .style(buttons::pick_list_note_category(note.category.is_some().then_some(folder)))
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

    // The formatting buttons live in a horizontally-scrollable strip: when the
    // pane is narrower than their combined width they scroll rather than squish
    // or push the view-mode toggle off-screen. The mode toggle stays pinned to
    // the right, always visible.
    let mut fmt = row![].spacing(3).align_y(Alignment::Center);
    if mode != NoteViewMode::Preview {
        fmt = fmt
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
    let fmt_strip = scrollable(fmt)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(4).scroller_width(4),
        ))
        .width(Length::Fill);

    row![
        fmt_strip,
        mode_btn("Edit",    "Markdown source only",            NoteViewMode::Edit),
        mode_btn("Split",   "Source and preview side by side", NoteViewMode::Split),
        mode_btn("Preview", "Rendered preview only",           NoteViewMode::Preview),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .into()
}

/// Keyboard shortcuts for the note body editor — Ctrl+B/I/E/K apply markdown
/// formatting, Tab/Shift+Tab indent and outdent; everything else falls through
/// to the default bindings.
fn notes_key_binding(
    kp: text_editor::KeyPress,
) -> Option<text_editor::Binding<Message>> {
    use iced::keyboard::{key, Key};
    use text_editor::{Binding, Status};

    // Tab indents rather than moving focus. The default binding drops it (it
    // arrives as the control character \t), so it would otherwise fall through
    // to the app-level Tab handler and reach a terminal.
    // Ctrl+Tab stays a pane-cycle chord, so only plain Tab / Shift+Tab indent.
    if kp.status == Status::Focused
        && matches!(kp.key, Key::Named(key::Named::Tab))
        && !kp.modifiers.command()
        && !kp.modifiers.alt()
    {
        return Some(Binding::Custom(if kp.modifiers.shift() {
            Message::NotesOutdent
        } else {
            Message::NotesIndent
        }));
    }

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
    show_containers: bool,
) -> Element<'_, Message> {
    row![
        container(terminals_list(terms))
            .width(Length::Fixed(160.0))
            .height(Length::Fill),
        container(terminal_body(terms, font_scale, show_containers))
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

/// The `⬢ bottle` badge showing which toolbox a shell is currently inside —
/// the same hexagon toolbox itself puts in the container's prompt. Empty when
/// the shell is on the host, or when the user isn't on an immutable system.
fn container_badge(t: &crate::terminals::PluginTerminal, show: bool) -> Element<'static, Message> {
    let Some(c) = show.then(|| t.pane.container()).flatten() else {
        return Space::new(Length::Shrink, Length::Shrink).into();
    };
    row![
        text("\u{2B22}").font(ICON_FONT).size(11),
        text(c.name).font(UI_FONT).size(11),
    ]
    .spacing(3)
    .align_y(Alignment::Center)
    .into()
}

fn terminal_body(
    terms: &TerminalsState,
    font_scale: TerminalFontScale,
    show_containers: bool,
) -> Element<'_, Message> {
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
        container_badge(t, show_containers),
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

    let body = terminal_ui::view(terminal_ui::View {
        id:         t.id,
        grid:       t.pane.grid.clone(),
        master:     t.pane.master(),
        focused:    terms.focused,
        is_shell:   true,
        font_scale,
        selection:  t.pane.selection,
        transcript: false,
        find:       None,
    });

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
