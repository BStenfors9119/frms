//! Docs plugin tab — search this project's `_specs` and `_docs` folders by
//! name and modified date. Picking a result opens it in the editor, the same
//! as clicking it in the file browser.

use std::time::SystemTime;

use iced::{Alignment, Background, Border, Color, Element, Length, Theme};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input, Column, Space};

use crate::app::Message;
use crate::docs::{self, DateFilter, Doc, DocsState, SortBy};
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::ui::{buttons, with_tip};

/// Widget id for the Docs search field, so the Ctrl+Shift+S chord can drop the
/// caret straight into it rather than leaving the user to click.
pub const DOCS_SEARCH_INPUT: &str = "docs-search-input";

pub fn view<'a>(docs: &'a DocsState, folder: Color) -> Element<'a, Message> {
    // One `now` for the whole render: filtering and every age label then agree,
    // instead of each row asking the clock and disagreeing at a boundary.
    let now = SystemTime::now();
    let results = docs.results(now);

    let search = text_input("Search documents by name…", &docs.query)
        .id(text_input::Id::new(DOCS_SEARCH_INPUT))
        .on_input(Message::DocsQueryChanged)
        .size(14)
        .width(Length::Fill);

    let date_picker = pick_list(DateFilter::ALL, Some(docs.date), Message::DocsDateChanged)
        .text_size(buttons::TEXT_SIZE)
        .padding(buttons::PADDING);
    let sort_picker = pick_list(SortBy::ALL, Some(docs.sort), Message::DocsSortChanged)
        .text_size(buttons::TEXT_SIZE)
        .padding(buttons::PADDING);

    let refresh = with_tip(
        button(text("\u{21BB}").font(ICON_FONT).size(13)) // ↻
            .on_press(Message::DocsRefresh)
            .padding(buttons::PADDING)
            .style(buttons::secondary),
        "Rescan _specs and _docs",
    );

    // The controls row wraps a lot into a narrow dock, so "Clear" only appears
    // when there is actually something to clear.
    let mut controls = row![date_picker, sort_picker]
        .spacing(6)
        .align_y(Alignment::Center);
    if docs.is_filtered() {
        controls = controls.push(
            button(text("Clear").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::DocsClearFilters)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        );
    }
    controls = controls
        .push(Space::new(Length::Fill, Length::Shrink))
        .push(text(count_label(results.len(), docs.docs.len())).font(UI_FONT).size(11))
        .push(refresh);

    let mut body = column![
        row![search].spacing(6).align_y(Alignment::Center),
        controls,
    ]
    .spacing(8);

    // Say so rather than silently listing a prefix — an unexplained missing
    // file is worse than a partial listing the user knows is partial.
    if docs.truncated {
        body = body.push(
            text("Too many files to index — showing the first ones found.")
                .font(UI_FONT)
                .size(11)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(theme.palette().danger),
                }),
        );
    }

    body.push(scrollable(results_list(docs, &results, now, folder)).height(Length::Fill))
        .height(Length::Fill)
        .into()
}

/// "12 documents" when nothing is filtered, "3 of 12" when something is — so
/// the number always says whether the list is the whole picture.
fn count_label(shown: usize, total: usize) -> String {
    if shown == total {
        match total {
            1 => "1 document".to_string(),
            n => format!("{n} documents"),
        }
    } else {
        format!("{shown} of {total}")
    }
}

fn results_list<'a>(
    docs:    &'a DocsState,
    results: &[&'a Doc],
    now:     SystemTime,
    folder:  Color,
) -> Element<'a, Message> {
    if results.is_empty() {
        return empty_state(docs);
    }

    // Group under a folder heading per doc root, in DOC_DIRS order. Headings
    // are dropped entirely when only one root is in play — a lone "_docs"
    // header over every row is pure noise in a narrow panel.
    let multi_root = docs::DOC_DIRS.iter()
        .filter(|d| results.iter().any(|r| r.root == **d))
        .count() > 1;

    let mut list = Column::new().spacing(2).width(Length::Fill);
    for dir in docs::DOC_DIRS {
        let group: Vec<&&Doc> = results.iter().filter(|d| d.root == *dir).collect();
        if group.is_empty() {
            continue;
        }
        if multi_root {
            list = list.push(group_header(dir, group.len(), folder));
        }
        for d in group {
            list = list.push(doc_row(d, now));
        }
    }

    // Right padding keeps rows clear of the overlaid scrollbar.
    container(list)
        .padding(iced::Padding::ZERO.right(10.0))
        .width(Length::Fill)
        .into()
}

/// One result: the file name over its project-relative path, with the age on
/// the right. Clicking opens it in the editor.
fn doc_row<'a>(doc: &Doc, now: SystemTime) -> Element<'a, Message> {
    let name = text(doc.name.clone()).font(UI_FONT).size(buttons::TEXT_SIZE);
    // The relative path is the disambiguator when two folders hold a `README.md`,
    // so it rides under every row — muted, since it's reference not label.
    let path = container(text(doc.rel.clone()).font(UI_FONT).size(10))
        .style(|theme: &Theme| container::Style {
            text_color: Some(Color { a: 0.65, ..theme.palette().text }),
            ..Default::default()
        });
    let age = container(text(docs::age_label(doc.modified, now)).font(UI_FONT).size(10))
        .style(|theme: &Theme| container::Style {
            text_color: Some(Color { a: 0.65, ..theme.palette().text }),
            ..Default::default()
        });

    let content = row![
        // 🗎 — the same document glyph the notes list uses, so a row here reads
        // as a file rather than a folder.
        text("\u{1F5CE}").font(ICON_FONT).size(buttons::TEXT_SIZE),
        column![name, path].spacing(1).width(Length::Fill),
        age,
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    button(content)
        .on_press(Message::OpenFile(doc.path.clone()))
        .padding(buttons::PADDING)
        .style(buttons::note_item(false))
        .width(Length::Fill)
        .into()
}

/// Folder heading above each doc root's results, styled like the notes list's
/// category headers so the two tabs read as siblings.
fn group_header<'a>(dir: &str, count: usize, folder: Color) -> Element<'a, Message> {
    let header = row![
        text("\u{1F5C1}").font(ICON_FONT).size(12), // 🗁
        text(dir.to_string()).font(UI_FONT).size(11),
        Space::new(Length::Fill, Length::Shrink),
        text(count.to_string()).font(UI_FONT).size(10),
    ]
    .spacing(5)
    .align_y(Alignment::Center);

    container(header)
        .padding([5, 7])
        .width(Length::Fill)
        .style(move |theme: &Theme| container::Style {
            background: Some(Background::Color(Color { a: 0.16, ..folder })),
            border: Border { color: Color { a: 0.45, ..folder }, width: 1.0, radius: 4.0.into() },
            text_color: Some(theme.palette().text),
            ..Default::default()
        })
        .into()
}

/// What to show when the list comes back empty — which has three quite
/// different causes, and saying which one it is saves the user a hunt.
fn empty_state<'a>(docs: &'a DocsState) -> Element<'a, Message> {
    let msg = if !docs.scanned {
        "Loading…".to_string()
    } else if docs.roots.is_empty() {
        format!(
            "This project has no {} folder. Create one and click ↻ to index it.",
            docs::DOC_DIRS.join(" or "),
        )
    } else if docs.is_filtered() {
        "No documents match this search.".to_string()
    } else {
        format!("{} is empty.", docs.roots.join(" and "))
    };

    container(text(msg).font(UI_FONT).size(12))
        .padding(16)
        .width(Length::Fill)
        .into()
}
