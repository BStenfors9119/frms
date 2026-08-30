//! Per-model breakdown of a usage window — what the header bars can't show.
//!
//! The usage API hands back one utilization percentage per window and nothing
//! about which model spent it. Clicking a bar opens this card, which totals the
//! tokens each model produced inside that same window by reading Claude Code's
//! own transcripts (see [`crate::stats::load_model_breakdown`]).
//!
//! The card is explicit about the gap between the two numbers: tokens are not
//! utilization, and the limit weighs models differently, so a model's share of
//! the tokens is not its share of the bar.

use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{button, center, container, mouse_area, opaque, progress_bar, row, scrollable, text, Column, Space};

use crate::app::{Message, UsageBreakdown};
use crate::fonts::UI_FONT;
use crate::stats::{ModelBreakdown, ModelTokens, UsageScope};
use crate::theme::ThemeColors;
use crate::ui::buttons;

/// Tallest the model list grows before it scrolls.
const LIST_HEIGHT: f32 = 300.0;

/// Full-window overlay: dimmed backdrop with the centered card. Clicking the
/// backdrop dismisses it.
pub fn overlay<'a>(card: &'a UsageBreakdown, colors: ThemeColors) -> Element<'a, Message> {
    let dim = move |_: &Theme| text::Style { color: Some(colors.secondary) };

    let mut body = Column::new()
        .spacing(12)
        .push(text(format!("{} — usage by model", card.window.label())).font(UI_FONT).size(16))
        .push(scope_picker(card))
        .push(
            text(format!(
                "Scanned since {} · {}",
                crate::stats::format_epoch_dated(card.since),
                span_note(card),
            ))
            .font(UI_FONT)
            .size(12)
            .style(dim),
        );

    body = body.push(match &card.data {
        None       => text("Reading Claude Code transcripts…").font(UI_FONT).size(13).into(),
        Some(data) => results(data, colors),
    });

    // What the numbers do and don't mean. Worth the three lines: a reader who
    // assumes token share equals bar share will draw the wrong conclusion about
    // which model to drop.
    body = body
        .push(
            text(
                "Tokens come from Claude Code's local transcripts — every project on \
                 this machine, but nothing from another machine or the web app. They \
                 are not the same measure as the bar: the limit weighs models \
                 differently, so a model's token share is not its share of the bar. \
                 \"Limit window\" tracks the bar exactly and so starts at the last \
                 reset; \"Last 7 days\" ignores the reset and shows everything you \
                 have run recently.",
            )
            .font(UI_FONT)
            .size(11)
            .style(dim),
        )
        .push(
            row![
                button(text("Rescan").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::UsageBreakdownRefreshed)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
                Space::with_width(Length::Fill),
                button(text("Close").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::UsageBreakdownClosed)
                    .padding(buttons::PADDING)
                    .style(buttons::primary),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );

    let card_el = container(body)
        .padding(20)
        .width(Length::Fixed(560.0))
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(colors.surface)),
            border:     Border { color: colors.tertiary, width: 1.0, radius: 8.0.into() },
            text_color: Some(colors.text),
            ..Default::default()
        });

    opaque(
        mouse_area(
            center(opaque(card_el)).style(|_theme: &Theme| container::Style {
                background: Some(Background::Color(Color { a: 0.55, ..Color::BLACK })),
                ..Default::default()
            }),
        )
        .on_press(Message::UsageBreakdownClosed),
    )
}

/// The two scopes as a small segmented control. Worth the row: early in a
/// weekly window "Limit window" and "Last 7 days" are wildly different spans,
/// and a reader who doesn't know that reads a two-model weekly list as "I only
/// ran two models this week" when the truth is "the week reset on Sunday".
fn scope_picker<'a>(card: &'a UsageBreakdown) -> Element<'a, Message> {
    let tab = |scope: UsageScope| {
        button(text(scope.label(card.window)).font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::UsageBreakdownScopeChanged(scope))
            .padding(buttons::PADDING)
            .style(buttons::toggle(card.scope == scope))
    };

    row![tab(UsageScope::LimitWindow), tab(UsageScope::Trailing)]
        .spacing(6)
        .align_y(Alignment::Center)
        .into()
}

/// How much of the window the scan actually covers — the line that explains why
/// a freshly-reset weekly card can look identical to the 5-hour one.
fn span_note(card: &UsageBreakdown) -> String {
    match card.scope {
        UsageScope::Trailing => format!("trailing {}, ending now", card.window.span_label()),
        UsageScope::LimitWindow => {
            let elapsed = (crate::stats::now_epoch() - card.since).max(0);
            let length  = card.window.length_secs();
            if elapsed >= length {
                format!("the full {} the bar measures", card.window.span_label())
            } else {
                format!(
                    "{} of {} elapsed — anything earlier counted against the previous window",
                    fmt_span(elapsed),
                    card.window.span_label(),
                )
            }
        }
    }
}

/// A duration in its coarsest still-precise unit — "2.8 d", "3.5 h", "42 min".
fn fmt_span(secs: i64) -> String {
    let s = secs as f64;
    if s >= 86_400.0     { format!("{:.1} d", s / 86_400.0) }
    else if s >= 3_600.0 { format!("{:.1} h", s / 3_600.0) }
    else                 { format!("{:.0} min", s / 60.0) }
}

/// The finished scan: a row per model, biggest first, then the window total.
fn results<'a>(data: &'a ModelBreakdown, colors: ThemeColors) -> Element<'a, Message> {
    let dim = move |_: &Theme| text::Style { color: Some(colors.secondary) };

    if data.rows.is_empty() {
        let msg = if data.no_transcripts {
            "No Claude Code transcripts found in ~/.claude/projects."
        } else {
            "No Claude replies recorded in this window."
        };
        return text(msg).font(UI_FONT).size(13).style(dim).into();
    }

    let total = data.total().max(1);
    let mut list = Column::new().spacing(10);
    for r in &data.rows {
        list = list.push(model_row(r, total, colors));
    }

    Column::new()
        .spacing(10)
        .push(
            container(scrollable(list).height(Length::Shrink))
                .max_height(LIST_HEIGHT)
                .width(Length::Fill)
                .padding(8)
                .style(move |_theme: &Theme| container::Style {
                    border: Border { color: colors.border, width: 1.0, radius: 6.0.into() },
                    ..Default::default()
                }),
        )
        .push(
            text(format!(
                "{} tokens across {} replies · {} transcript{} read",
                fmt_tokens(data.total()),
                data.replies(),
                data.files,
                if data.files == 1 { "" } else { "s" },
            ))
            .font(UI_FONT)
            .size(12)
            .style(dim),
        )
        .into()
}

/// One model: name, total and share on the headline row, a share bar beneath,
/// then the split that makes up the total. Cache reads usually dwarf everything
/// else in agentic use, which is exactly what the split is there to show.
fn model_row<'a>(r: &'a ModelTokens, window_total: u64, colors: ThemeColors) -> Element<'a, Message> {
    let share = r.total() as f32 / window_total as f32;

    let headline = row![
        text(r.model.as_str()).font(UI_FONT).size(13),
        Space::with_width(Length::Fill),
        text(fmt_tokens(r.total())).font(UI_FONT).size(13),
        container(text(format!("{:.0}%", share * 100.0)).font(UI_FONT).size(13))
            .width(Length::Fixed(44.0))
            .align_x(Alignment::End),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let bar = progress_bar(0.0..=1.0, share)
        .width(Length::Fill)
        .height(Length::Fixed(8.0))
        .style(move |_theme: &Theme| progress_bar::Style {
            background: Background::Color(Color::from_rgb8(45, 45, 55)),
            bar:        Background::Color(colors.primary),
            border:     Border { color: Color::TRANSPARENT, width: 0.0, radius: 4.0.into() },
        });

    let detail = format!(
        "in {} · out {} · cache read {} · cache write {} · {} repl{}",
        fmt_tokens(r.input),
        fmt_tokens(r.output),
        fmt_tokens(r.cache_read),
        fmt_tokens(r.cache_write),
        r.replies,
        if r.replies == 1 { "y" } else { "ies" },
    );

    Column::new()
        .spacing(4)
        .push(headline)
        .push(bar)
        .push(
            text(detail).font(UI_FONT).size(11).style(move |_: &Theme| text::Style {
                color: Some(colors.secondary),
            }),
        )
        .into()
}

/// Token counts run from a handful to billions, so scale the unit rather than
/// printing a wall of digits.
pub fn fmt_tokens(n: u64) -> String {
    const B: f64 = 1_000_000_000.0;
    const M: f64 = 1_000_000.0;
    const K: f64 = 1_000.0;
    let f = n as f64;
    if f >= B      { format!("{:.2} B", f / B) }
    else if f >= M { format!("{:.1} M", f / M) }
    else if f >= K { format!("{:.1} K", f / K) }
    else           { n.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_scale_by_magnitude() {
        assert_eq!(fmt_tokens(0), "0");
        assert_eq!(fmt_tokens(999), "999");
        assert_eq!(fmt_tokens(80_917), "80.9 K");
        assert_eq!(fmt_tokens(6_105_518), "6.1 M");
        assert_eq!(fmt_tokens(1_148_141_500), "1.15 B");
    }
}
