//! "Agents" dialog — the one place a project's AI sessions are managed.
//!
//! It replaced a "+ Agent" dropdown and a separate "Model" button, which were
//! two entry points into one subject. Everything about the set of running
//! agents happens here:
//!
//!   * **start** one — name it, then pick a model and press New agent, or take
//!     a Claude chat mode (Research / Chat), which carry no model choice. The
//!     name is optional and applies to whichever of those buttons is pressed;
//!   * **retarget** open ones at another model without closing them. Claude
//!     sessions and chat panes keep their conversation; tools that can't switch
//!     in place restart their CLI, so Apply reports what it actually did (see
//!     [`crate::app::Frms::apply_model_change`]);
//!   * **close** one that is finished with. Each agent holds a live CLI
//!     process, so closing is how the user gets those resources back — it asks
//!     first, inline in the row, since a stray click would end a session.

use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{
    button, center, container, mouse_area, opaque, row, scrollable, text, text_input,
    Column, Row, Space,
};

use crate::agent::{AgentKind, AgentPane, AiTool, ToolModel};
use crate::app::{Message, AgentsDialog};
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Tallest the agent list grows before it scrolls — keeps the card a sane size
/// for a project with many agent tabs open.
const LIST_HEIGHT: f32 = 176.0;

/// Blank width reserved down the right of a scrolling list for its vertical
/// scrollbar. Iced paints the bar *over* the content, so without this the ✕ on
/// an agent row — the rightmost thing in the list — sits underneath it. Same
/// reasoning as `ui::editor`'s `AGENT_SCROLLBAR_GUTTER` for horizontal bars:
/// `max(width, scroller) + 2·margin` (10px at iced's defaults) plus room to
/// breathe.
const SCROLLBAR_GUTTER: u16 = 14;

/// Widget id of the NEW row's name field, so opening the dialog can drop the
/// caret into it (see `Message::AgentsDialogOpened`).
pub fn name_input_id() -> text_input::Id {
    text_input::Id::new("agent-name")
}

/// Full-window overlay: dimmed backdrop with the centered dialog card.
/// Clicking the backdrop dismisses without applying anything.
pub fn overlay<'a>(
    dialog:           &'a AgentsDialog,
    panes:            &'a [AgentPane],
    default_tool:     AiTool,
    filter_installed: bool,
    colors:           ThemeColors,
) -> Element<'a, Message> {
    let picked = dialog.choice;

    // Panes that would actually change if Apply were pressed now: ticked, still
    // present, and able to take a model from the picked tool's list.
    let eligible = picked
        .map(|(tool, _)| {
            panes.iter()
                .filter(|p| dialog.selected.contains(&p.id()))
                .filter(|p| p.accepts_models_from(tool))
                .count()
        })
        .unwrap_or(0);

    let mut card_body = Column::new()
        .spacing(12)
        .push(text("Agents").font(UI_FONT).size(16))
        .push(
            text(
                "Name and start a session, switch the model on open ones without \
                 closing them, or stop one you're done with to free its CLI.",
            )
            .font(UI_FONT)
            .size(12),
        )
        .push(section_header_row("AGENTS", panes.len(), colors))
        .push(agent_list(dialog, panes, picked, colors))
        .push(model_header_row(dialog, colors))
        .push(model_list(dialog, picked, default_tool, filter_installed, colors))
        .push(new_agent_row(picked, &dialog.name, colors));

    if let Some(outcome) = dialog.outcome {
        card_body = card_body.push(text(outcome.summary()).font(UI_FONT).size(12));
    }

    // Apply is live only once a model is picked and it lands on something.
    let apply_label = if eligible > 0 {
        format!("Apply to {eligible} {}", if eligible == 1 { "agent" } else { "agents" })
    } else {
        "Apply".to_string()
    };
    let mut apply = button(text(apply_label).font(UI_FONT).size(buttons::TEXT_SIZE))
        .padding(buttons::PADDING)
        .style(buttons::primary);
    if eligible > 0 {
        apply = apply.on_press(Message::AgentsDialogApplied);
    }

    card_body = card_body.push(
        row![
            Space::with_width(Length::Fill),
            button(text("Close").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentsDialogClosed)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
            apply,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );

    let card = container(card_body)
        .padding(20)
        .width(Length::Fixed(460.0))
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(colors.surface)),
            border:     Border { color: colors.tertiary, width: 1.0, radius: 8.0.into() },
            text_color: Some(colors.text),
            ..Default::default()
        });

    opaque(
        mouse_area(
            center(opaque(card)).style(|_theme: &Theme| container::Style {
                background: Some(Background::Color(Color { a: 0.55, ..Color::BLACK })),
                ..Default::default()
            }),
        )
        .on_press(Message::AgentsDialogClosed),
    )
}

/// "AGENTS" header with the All / None shortcuts on the right.
fn section_header_row<'a>(label: &'a str, count: usize, colors: ThemeColors) -> Element<'a, Message> {
    let all_none: Element<'a, Message> = if count > 1 {
        row![
            button(text("All").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentsDialogSelectAll(true))
                .padding([2, 8])
                .style(buttons::secondary),
            button(text("None").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentsDialogSelectAll(false))
                .padding([2, 8])
                .style(buttons::secondary),
        ]
        .spacing(6)
        .into()
    } else {
        Space::with_width(Length::Shrink).into()
    };

    row![
        text(label).font(UI_FONT).size(10).style(move |_: &Theme| text::Style {
            color: Some(colors.primary),
        }),
        Space::with_width(Length::Fill),
        all_none,
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// One row per agent pane: a tickable label (name, tool, model) plus a ✕ that
/// closes it.
///
/// Rows the picked model can't apply to say so and stay untouched by Apply —
/// ticking them is still allowed so the user can pick a model from the other
/// tool's list next without re-ticking everything.
///
/// Stop kills the pane's CLI and **keeps its tab** — the point is reclaiming
/// the process, not losing the session's place. Deleting a tab is a separate
/// action and lives on the tab's own ✕. Rows say whether they hold a process:
/// "stopped" for one the user ended, "not started" for a tab never opened, and
/// neither offers a Stop since there is nothing left to kill.
fn agent_list<'a>(
    dialog: &'a AgentsDialog,
    panes:  &'a [AgentPane],
    picked: Option<(AiTool, ToolModel)>,
    colors: ThemeColors,
) -> Element<'a, Message> {
    let mut list = Column::new()
        .spacing(3)
        .padding(iced::Padding::default().right(SCROLLBAR_GUTTER));

    for (i, p) in panes.iter().enumerate() {
        let id   = p.id();
        let name = p.name().map(str::to_owned)
            .unwrap_or_else(|| format!("{} {}", p.kind().label(), i + 1));

        // Stopping ends a running CLI and loses its context, so Stop arms an
        // inline confirmation rather than acting — inline, not as a second
        // overlay stacked on this one, which would bury the list it refers to.
        if dialog.confirming == Some(id) {
            list = list.push(
                row![
                    text(format!("Stop “{name}”? The tab stays."))
                        .font(UI_FONT)
                        .size(buttons::TEXT_SIZE),
                    Space::with_width(Length::Fill),
                    button(text("Stop").font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::AgentsDialogPaneStopped(id))
                        .padding([2, 10])
                        .style(buttons::danger),
                    button(text("Keep running").font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::AgentsDialogPaneStopCancelled)
                        .padding([2, 10])
                        .style(buttons::secondary),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .padding([4, 8]),
            );
            continue;
        }

        let selected = dialog.selected.contains(&id);
        let eligible = picked.map_or(true, |(tool, _)| p.accepts_models_from(tool));

        // Build panes name their CLI; chat panes are Claude-only, so their kind
        // ("Research" / "Chat") already says everything the tool would.
        let mut detail = if p.kind().is_agentic() {
            format!("{} · {}", p.tool().label(), p.model_label())
        } else {
            p.model_label()
        };
        if p.is_stopped() {
            detail.push_str(" · stopped");
        } else if p.is_pending() {
            detail.push_str(" · not started");
        }

        // The check mark (U+2713) lives in ICON_FONT — Liberation Sans (UI_FONT)
        // has no Dingbats, so folding it into the label string tofu-boxes it. A
        // fixed-width cell keeps the label aligned whether the row is ticked or
        // not.
        let mark = text(if selected { "\u{2713}" } else { "" })
            .font(ICON_FONT)
            .size(buttons::TEXT_SIZE)
            .width(Length::Fixed(16.0));

        let mut label = format!("{name}   —   {detail}");
        if !eligible {
            label.push_str("   (not applicable)");
        }

        let content = row![mark, text(label).font(UI_FONT).size(buttons::TEXT_SIZE)]
            .spacing(6)
            .align_y(Alignment::Center);

        let mut btn = button(content)
            .on_press(Message::AgentsDialogPaneToggled(id))
            .width(Length::Fill)
            .padding(buttons::PADDING);
        btn = if eligible {
            btn.style(buttons::toggle(selected))
        } else {
            btn.style(buttons::menu_item)
        };

        let mut r = row![btn].spacing(4).align_y(Alignment::Center);
        // Only a running Build pane has a process to reclaim.
        if p.can_stop() {
            r = r.push(
                crate::ui::with_tip(
                    button(text("Stop").font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::AgentsDialogPaneStopRequested(id))
                        .padding([2, 8])
                        .style(buttons::secondary),
                    "Stop this agent's CLI — the tab stays, delete it with its own ✕",
                ),
            );
        }
        list = list.push(r);
    }

    container(scrollable(list).height(Length::Shrink))
        .max_height(LIST_HEIGHT)
        .width(Length::Fill)
        .padding(2)
        .style(move |_theme: &Theme| container::Style {
            border: Border { color: colors.border, width: 1.0, radius: 6.0.into() },
            ..Default::default()
        })
        .into()
}

/// The "start a new one" row. A Build pane takes whichever model is picked
/// above — the same list that drives Apply, so one choice serves both verbs.
/// The Claude chat modes carry no model choice, so they are plain buttons.
///
/// `name` is the draft in the field above the buttons: every one of these
/// buttons starts its agent under that name, so a session can be called what it
/// is from the moment it exists rather than being renamed out of "Build 3"
/// afterwards. Blank keeps the positional label, so naming stays optional.
fn new_agent_row<'a>(
    picked: Option<(AiTool, ToolModel)>,
    name:   &'a str,
    colors: ThemeColors,
) -> Element<'a, Message> {
    // Only a name with something in it travels — a field of spaces is no name,
    // and `Session::add_agent` would drop it anyway.
    let typed = (!name.trim().is_empty()).then(|| name.trim().to_string());

    // Enter in the field starts a Build agent, but only once a model is picked
    // — otherwise the same press that names the agent would silently do nothing.
    let mut name_field = text_input("Name this agent (optional)", name)
        .id(name_input_id())
        .on_input(Message::AgentsDialogNameEdited)
        .font(UI_FONT)
        .size(buttons::TEXT_SIZE)
        .padding(buttons::PADDING)
        .width(Length::Fill);
    if let Some((tool, model)) = picked {
        name_field = name_field.on_submit(
            Message::AgentAdded(AgentKind::Build, tool, model, typed.clone()),
        );
    }

    let mut new_build = button(
        text(match picked {
            Some((_, model)) => format!("+ New agent on {}", model.label),
            None             => String::from("+ New agent (pick a model)"),
        })
        .font(UI_FONT)
        .size(buttons::TEXT_SIZE),
    )
    .padding(buttons::PADDING)
    .style(buttons::primary);
    if let Some((tool, model)) = picked {
        new_build =
            new_build.on_press(Message::AgentAdded(AgentKind::Build, tool, model, typed.clone()));
    }

    // Research / Chat are Claude-only and pick their own model by kind.
    let claude_default = *AiTool::Claude.default_model();
    let mut chats = Row::new().spacing(6).align_y(Alignment::Center);
    for &kind in AgentKind::ALL {
        if kind.is_agentic() {
            continue;
        }
        chats = chats.push(
            button(text(kind.menu_label()).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentAdded(kind, AiTool::Claude, claude_default, typed.clone()))
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        );
    }

    Column::new()
        .spacing(6)
        .push(
            text("NEW").font(UI_FONT).size(10).style(move |_: &Theme| text::Style {
                color: Some(colors.primary),
            }),
        )
        .push(name_field)
        .push(row![new_build, Space::with_width(Length::Fill), chats].align_y(Alignment::Center))
        .into()
}

/// The "MODEL" header, with a Refresh button that requeries Claude's live model
/// list from the API (see [`crate::models`]) so a newly-shipped model appears in
/// the picker below without restarting frms. The last requery's result — a count
/// or a short failure reason — sits under the header. Devin/Windsurf has no
/// models endpoint yet, so Refresh only touches the Claude list.
fn model_header_row<'a>(dialog: &'a AgentsDialog, colors: ThemeColors) -> Element<'a, Message> {
    let label = text("MODEL — applies to the ticked agents, or starts a new one")
        .font(UI_FONT)
        .size(10)
        .style(move |_: &Theme| text::Style { color: Some(colors.primary) });

    // No arrow glyph on the button: ICON_FONT lacks the Arrows block and UI_FONT
    // is Latin-only, so "↻" would tofu — plain "Refresh" is safe in either.
    let mut refresh = button(
        text(if dialog.refreshing { "Refreshing…" } else { "Refresh" })
            .font(UI_FONT)
            .size(buttons::TEXT_SIZE),
    )
    .padding([2, 8])
    .style(buttons::secondary);
    if !dialog.refreshing {
        refresh = refresh.on_press(Message::AgentsDialogRefreshModels);
    }

    let mut col = Column::new().spacing(3).push(
        row![
            label,
            Space::with_width(Length::Fill),
            crate::ui::with_tip(refresh, "Requery Claude's available models from the API"),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );
    if let Some(status) = &dialog.models_status {
        col = col.push(
            text(status.clone())
                .font(UI_FONT)
                .size(10)
                .style(move |_: &Theme| text::Style { color: Some(colors.secondary) }),
        );
    }
    col.into()
}

/// The models on offer, tabbed by tool. With three tools each carrying a long
/// list, stacking them all made for a tall scroll; instead a row of tool tabs
/// sits on top and only the active tool's models show below. The user's default
/// tool leads the tabs so its models are nearest to hand, and `filter_installed`
/// hides tools that aren't on `PATH` — the same ordering and opt-in filter the
/// old "+ Agent" dropdown applied. The pick lives in `picked` (which carries its
/// own tool), so it stays lit across tab switches and never moves when the tab
/// does.
fn model_list<'a>(
    dialog:           &'a AgentsDialog,
    picked:           Option<(AiTool, ToolModel)>,
    default_tool:     AiTool,
    filter_installed: bool,
    colors:           ThemeColors,
) -> Element<'a, Message> {
    let mut tools: Vec<AiTool> = AiTool::ALL.iter().copied().collect();
    tools.sort_by_key(|&t| t != default_tool);
    if filter_installed {
        let installed: Vec<AiTool> = tools.iter().copied().filter(|t| t.on_path()).collect();
        // Never empty the picker: fall back to all tools if none are installed.
        if !installed.is_empty() {
            tools = installed;
        }
    }

    // The tab the dialog points at, unless `filter_installed` just hid it — then
    // fall back to the first visible tool so the list never renders empty.
    let active = if tools.contains(&dialog.model_tool) {
        dialog.model_tool
    } else {
        tools.first().copied().unwrap_or(default_tool)
    };

    // One tab per tool.
    let mut tabs = Row::new().spacing(4);
    for &tool in &tools {
        let on = tool == active;
        tabs = tabs.push(
            button(text(tool.label()).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentsDialogModelToolSelected(tool))
                .padding(buttons::PADDING)
                .style(buttons::toggle(on)),
        );
    }

    // The active tool's models, under a one-line note on how a switch lands.
    let note = if active.switches_model_in_place() { "keeps the session" } else { "restarts the CLI" };
    // Gutter matched to the agent list above so the two lists end on the same
    // edge, scrollbar or not.
    let mut list = Column::new()
        .spacing(2)
        .padding(iced::Padding::default().right(SCROLLBAR_GUTTER))
        .push(
            container(
                text(format!("{} — {note}", active.label()))
                    .font(UI_FONT)
                    .size(buttons::TEXT_SIZE.saturating_sub(1))
                    .style(move |_: &Theme| text::Style { color: Some(colors.secondary) }),
            )
            .padding(iced::Padding::from([4, 6]).top(2.0)),
        );
    for &model in active.models() {
        let selected = picked == Some((active, model));
        let mark  = if selected { "✓" } else { "  " };
        list = list.push(
            button(text(format!("{mark}  {}", model.label)).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::AgentsDialogModelPicked(active, model))
                .width(Length::Fill)
                .padding(buttons::PADDING)
                .style(buttons::toggle(selected)),
        );
    }

    Column::new()
        .spacing(6)
        .push(tabs)
        .push(
            container(scrollable(list).height(Length::Shrink))
                .max_height(LIST_HEIGHT)
                .width(Length::Fill)
                .padding(2)
                .style(move |_theme: &Theme| container::Style {
                    border: Border { color: colors.border, width: 1.0, radius: 6.0.into() },
                    ..Default::default()
                }),
        )
        .into()
}

#[cfg(test)]
mod tests {
    use crate::app::ModelChangeOutcome;

    #[test]
    fn outcome_summary_names_each_delivery() {
        let o = ModelChangeOutcome { in_place: 2, respawned: 1, skipped: 1 };
        assert_eq!(o.changed(), 3);
        assert_eq!(o.summary(), "2 agents switched · 1 restarted · 1 skipped (different tool)");

        let one = ModelChangeOutcome { in_place: 1, ..Default::default() };
        assert_eq!(one.summary(), "1 agent switched");

        assert_eq!(ModelChangeOutcome::default().summary(), "Nothing to change.");
    }
}
