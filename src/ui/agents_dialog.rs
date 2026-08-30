//! "Change model" dialog — retarget one or many of a project's running AI
//! sessions at a different model, instead of closing them and starting over.
//!
//! The card is two lists: the session's agent panes (tick the ones to change)
//! and the models on offer, grouped by tool exactly like the "+ Agent" menu.
//! Applying reports what it did, since not every pane switches the same way —
//! Claude sessions and chat panes keep their conversation, other CLIs restart
//! (see [`crate::app::Frms::apply_model_change`]).

use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{button, center, container, mouse_area, opaque, row, scrollable, text, Column, Space};

use crate::agent::{AgentPane, AiTool, ToolModel};
use crate::app::{Message, AgentsDialog};
use crate::fonts::UI_FONT;
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Tallest the agent list grows before it scrolls — keeps the card a sane size
/// for a project with many agent tabs open.
const LIST_HEIGHT: f32 = 176.0;

/// Full-window overlay: dimmed backdrop with the centered dialog card.
/// Clicking the backdrop dismisses without applying anything.
pub fn overlay<'a>(
    dialog: &'a AgentsDialog,
    panes:  &'a [AgentPane],
    colors: ThemeColors,
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
        .push(text("Change agent model").font(UI_FONT).size(16))
        .push(
            text(
                "Switch the model on the AI sessions below without closing them. \
                 Claude sessions and chat panes keep their conversation; tools \
                 that can't switch in place restart their CLI.",
            )
            .font(UI_FONT)
            .size(12),
        )
        .push(section_header_row("AGENTS", panes.len(), colors))
        .push(agent_list(dialog, panes, picked, colors))
        .push(text("MODEL").font(UI_FONT).size(10).style(move |_: &Theme| text::Style {
            color: Some(colors.primary),
        }))
        .push(model_list(picked, colors));

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

/// One tickable row per agent pane: name, which tool it runs, and the model
/// it's on now. Rows the picked model can't apply to say so and stay untouched
/// by Apply — ticking them is still allowed so the user can pick a model from
/// the other tool's list next without re-ticking everything.
fn agent_list<'a>(
    dialog: &'a AgentsDialog,
    panes:  &'a [AgentPane],
    picked: Option<(AiTool, ToolModel)>,
    colors: ThemeColors,
) -> Element<'a, Message> {
    let mut list = Column::new().spacing(3);

    for (i, p) in panes.iter().enumerate() {
        let id       = p.id();
        let selected = dialog.selected.contains(&id);
        let eligible = picked.map_or(true, |(tool, _)| p.accepts_models_from(tool));

        let name = p.name().map(str::to_owned)
            .unwrap_or_else(|| format!("{} {}", p.kind().label(), i + 1));
        // Build panes name their CLI; chat panes are Claude-only, so their kind
        // ("Research" / "Chat") already says everything the tool would.
        let detail = if p.kind().is_agentic() {
            format!("{} · {}", p.tool().label(), p.model_label())
        } else {
            p.model_label()
        };

        let mark = if selected { "✓" } else { "  " };
        let mut label = format!("{mark}  {name}   —   {detail}");
        if !eligible {
            label.push_str("   (not applicable)");
        }

        let mut btn = button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::AgentsDialogPaneToggled(id))
            .width(Length::Fill)
            .padding(buttons::PADDING);
        btn = if eligible {
            btn.style(buttons::toggle(selected))
        } else {
            btn.style(buttons::menu_item)
        };
        list = list.push(btn);
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

/// The models on offer, grouped under their tool — the same ordering as the
/// "+ Agent" menu, so the two pickers read alike. The picked row stays lit.
fn model_list<'a>(picked: Option<(AiTool, ToolModel)>, colors: ThemeColors) -> Element<'a, Message> {
    let mut list = Column::new().spacing(2);

    for &tool in AiTool::ALL {
        let note = if tool.switches_model_in_place() { "keeps the session" } else { "restarts the CLI" };
        list = list.push(
            container(
                text(format!("{} — {note}", tool.label()))
                    .font(UI_FONT)
                    .size(buttons::TEXT_SIZE.saturating_sub(1))
                    .style(move |_: &Theme| text::Style { color: Some(colors.secondary) }),
            )
            .padding(iced::Padding::from([4, 6]).top(6.0)),
        );
        for &model in tool.models() {
            let selected = picked == Some((tool, model));
            let mark  = if selected { "✓" } else { "  " };
            list = list.push(
                button(text(format!("{mark}  {}", model.label)).font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::AgentsDialogModelPicked(tool, model))
                    .width(Length::Fill)
                    .padding(buttons::PADDING)
                    .style(buttons::toggle(selected)),
            );
        }
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
