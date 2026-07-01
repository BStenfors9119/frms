//! Renders the PromptResponse pane — surfaces a structured view of Claude's
//! current permission prompt and exposes Accept / Reject buttons that map
//! to "1" / "3" on Claude's numbered menu.

use std::path::Path;

use iced::font::Weight;
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Background, Border, Color, Element, Font, Length, Theme};

use crate::app::Message;
use crate::claude_prompt::PendingPrompt;
use crate::fonts::UI_FONT;
use crate::ui::buttons;

const BEFORE_BG: Color = Color { r: 0.32, g: 0.12, b: 0.12, a: 0.55 };
const BEFORE_FG: Color = Color { r: 0.95, g: 0.55, b: 0.55, a: 1.00 };
const AFTER_BG:  Color = Color { r: 0.12, g: 0.30, b: 0.14, a: 0.55 };
const AFTER_FG:  Color = Color { r: 0.65, g: 0.95, b: 0.60, a: 1.00 };

pub fn view<'a>(prompt: Option<&'a PendingPrompt>) -> Element<'a, Message> {
    let Some(p) = prompt else {
        return container(
            text("Waiting for Claude…").size(13).font(UI_FONT),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
    };

    // Bold name of the file being edited (just its final component), with the
    // full path on the row beneath it.
    let file_name = Path::new(&p.file_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.file_path.clone());
    let bold_ui_font = Font { weight: Weight::Bold, ..UI_FONT };

    let file_row = container(
        column![
            text(file_name).font(bold_ui_font).size(16),
            text(p.file_path.clone()).font(UI_FONT).size(12),
        ]
        .spacing(2),
    )
    .padding([6, 10])
    .width(Length::Fill);

    let before_row = diff_block(&p.before, "(no removal)", BEFORE_BG, BEFORE_FG);
    let after_row  = diff_block(&p.after,  "(no addition)", AFTER_BG,  AFTER_FG);

    // The diff (before + after) takes the remaining vertical space and scrolls
    // as one unit. Without this a long "after" block grows unbounded and pushes
    // the Accept/Reject row off the bottom of the pane, out of reach.
    let diff = scrollable(
        column![before_row, after_row]
            .spacing(6)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill);

    let actions = row![
        button(text("Accept").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::PromptAccepted)
            .padding(buttons::PADDING)
            .style(buttons::success),
        button(text("Reject").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::PromptRejected)
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(8)
    .padding([8, 10]);

    // `actions` is fixed-height and last, so it stays pinned and reachable at
    // the bottom while `diff` (height Fill) absorbs any overflow via scrolling.
    column![
        file_row,
        diff,
        actions,
    ]
    .spacing(6)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn diff_block<'a>(
    lines:    &'a [String],
    empty:    &'static str,
    bg:       Color,
    fg:       Color,
) -> Element<'a, Message> {
    let body: Element<'a, Message> = if lines.is_empty() {
        text(empty).size(12).font(UI_FONT).color(Color { a: 0.7, ..fg }).into()
    } else {
        let mut col = column![].spacing(0);
        for l in lines {
            col = col.push(text(l.clone()).font(Font::MONOSPACE).size(12).color(fg));
        }
        // No inner scrollable — the whole diff scrolls as one unit in `view`,
        // so each block just lays out its lines at natural height.
        col.width(Length::Fill).into()
    };

    container(body)
        .width(Length::Fill)
        .padding([6, 10])
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(bg)),
            border:     Border { color: Color::TRANSPARENT, width: 0.0, radius: 4.0.into() },
            ..Default::default()
        })
        .into()
}
