//! Lightweight read-only Markdown preview.
//!
//! Block structure (headings, lists, quotes, fenced code, rules) maps to a
//! column of widgets; inline markdown (`**bold**`, `*italic*`, `~~strike~~`,
//! `` `code` ``, `[links](url)`) is parsed into styled `rich_text` spans so
//! the markers actually style their text instead of being stripped.

use iced::font::{Style, Weight};
use iced::widget::{column, container, horizontal_rule, rich_text, scrollable, span, text};
use iced::{Color, Element, Font, Length};

use crate::app::Message;
use crate::fonts::{MONO_FONT, UI_FONT};

// ── markdown ──────────────────────────────────────────────────────────────────

pub fn markdown(source: &str) -> Element<'static, Message> {
    let mut col = column![].spacing(6).width(Length::Fill);
    let mut in_code_block = false;
    let mut code_buf      = String::new();

    for raw_line in source.lines() {
        let line = raw_line;

        // Fenced code block toggle (``` or ~~~).
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if in_code_block {
                col = col.push(code_block(std::mem::take(&mut code_buf)));
                in_code_block = false;
            } else {
                in_code_block = true;
                code_buf.clear();
            }
            continue;
        }

        if in_code_block {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        let t = line.trim_end();

        if t.is_empty() {
            // Paragraph break — a small empty spacer keeps the column readable.
            col = col.push(text(" ").size(6));
            continue;
        }

        // Headings — inline markers still apply, on a bold base style.
        const BOLD: InlineStyle = InlineStyle { bold: true, italic: false, strike: false };
        if let Some(rest) = t.strip_prefix("# ") {
            col = col.push(rich_line("", rest, BOLD, 24.0));
            continue;
        }
        if let Some(rest) = t.strip_prefix("## ") {
            col = col.push(rich_line("", rest, BOLD, 20.0));
            continue;
        }
        if let Some(rest) = t.strip_prefix("### ") {
            col = col.push(rich_line("", rest, BOLD, 16.0));
            continue;
        }
        if let Some(rest) = t.strip_prefix("#### ") {
            col = col.push(rich_line("", rest, BOLD, 14.0));
            continue;
        }

        // Blockquote.
        if let Some(rest) = t.strip_prefix("> ") {
            col = col.push(rich_line("  ", rest, InlineStyle::default(), 13.0));
            continue;
        }

        // Unordered list.
        if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            col = col.push(rich_line("• ", rest, InlineStyle::default(), 13.0));
            continue;
        }

        // Ordered list — keep the numeral.
        if let Some((num, rest)) = split_ordered_list(t) {
            col = col.push(rich_line(&format!("{num}. "), rest, InlineStyle::default(), 13.0));
            continue;
        }

        // Horizontal rule.
        if t == "---" || t == "***" || t == "___" {
            col = col.push(horizontal_rule(1));
            continue;
        }

        // Plain paragraph.
        col = col.push(rich_line("", t, InlineStyle::default(), 13.0));
    }

    if in_code_block && !code_buf.is_empty() {
        col = col.push(code_block(code_buf));
    }

    scroll(col.into())
}

fn code_block(body: String) -> Element<'static, Message> {
    container(text(body).font(Font::MONOSPACE).size(12))
        .padding(8)
        .width(Length::Fill)
        .style(container::bordered_box)
        .into()
}

// ── inline parsing ────────────────────────────────────────────────────────────

/// Inline style state carried through (possibly nested) markdown markers.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct InlineStyle {
    bold:   bool,
    italic: bool,
    strike: bool,
}

/// One styled fragment of a line, produced by `parse_inline`.
#[derive(Debug, PartialEq, Eq)]
enum Piece {
    /// Plain (possibly styled) text.
    Text(String, InlineStyle),
    /// `` `inline code` `` — rendered monospace on a subtle box, verbatim.
    Code(String),
    /// `[label](url)` link. Not clickable — the url is shown dimmed after
    /// the label so the target stays visible.
    Link { label: String, url: String },
}

/// Parse one line of inline markdown into styled pieces, starting from
/// `base` (headings pass a bold base so nested markers compose with it).
fn parse_inline(s: &str, base: InlineStyle) -> Vec<Piece> {
    let mut out = Vec::new();
    collect(s, base, &mut out);
    out
}

fn collect(s: &str, style: InlineStyle, out: &mut Vec<Piece>) {
    let mut buf = String::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];

        // Inline code — verbatim, no nested markers inside.
        if let Some(after_tick) = rest.strip_prefix('`') {
            if let Some(end) = after_tick.find('`') {
                flush(&mut buf, style, out);
                out.push(Piece::Code(after_tick[..end].to_string()));
                i += end + 2;
                continue;
            }
        }

        // Emphasis markers. Only commit when a matching closer exists ahead —
        // a lone `*` in "2 * 3" stays literal. Underscores additionally
        // require a word boundary so snake_case identifiers don't italicize.
        if let Some((delim, styled)) = match_delim(rest, style) {
            let word_internal = delim == "_"
                && buf.chars().last().is_some_and(|c| !c.is_whitespace());
            if !word_internal {
                if let Some(close) = find_closer(&rest[delim.len()..], delim) {
                    flush(&mut buf, style, out);
                    let inner = &rest[delim.len()..delim.len() + close];
                    collect(inner, styled, out);
                    i += 2 * delim.len() + close;
                    continue;
                }
            }
        }

        // [label](url)
        if rest.starts_with('[') {
            if let Some(cb) = rest.find(']') {
                let after = &rest[cb + 1..];
                if after.starts_with('(') {
                    if let Some(cp) = after.find(')') {
                        flush(&mut buf, style, out);
                        out.push(Piece::Link {
                            label: rest[1..cb].to_string(),
                            url:   after[1..cp].to_string(),
                        });
                        i += cb + 1 + cp + 1;
                        continue;
                    }
                }
            }
        }

        // Literal character.
        let c = rest.chars().next().unwrap();
        buf.push(c);
        i += c.len_utf8();
    }
    flush(&mut buf, style, out);
}

fn flush(buf: &mut String, style: InlineStyle, out: &mut Vec<Piece>) {
    if !buf.is_empty() {
        out.push(Piece::Text(std::mem::take(buf), style));
    }
}

/// Which emphasis delimiter starts at `rest`, and the style it toggles on.
/// Two-character delimiters are tried first so `**`/`~~` win over `*`/`_`.
fn match_delim(rest: &str, st: InlineStyle) -> Option<(&'static str, InlineStyle)> {
    if rest.starts_with("**") {
        Some(("**", InlineStyle { bold: true, ..st }))
    } else if rest.starts_with("~~") {
        Some(("~~", InlineStyle { strike: true, ..st }))
    } else if rest.starts_with('*') {
        Some(("*", InlineStyle { italic: true, ..st }))
    } else if rest.starts_with('_') {
        Some(("_", InlineStyle { italic: true, ..st }))
    } else {
        None
    }
}

/// Find the closing `delim` in `s` (the text just after an opener), enforcing
/// the usual emphasis rules: content is non-empty and has no whitespace
/// immediately inside either marker.
fn find_closer(s: &str, delim: &str) -> Option<usize> {
    if s.chars().next()?.is_whitespace() {
        return None;
    }
    let mut from = 0;
    while let Some(rel) = s[from..].find(delim) {
        let pos = from + rel;
        if pos > 0 && s[..pos].chars().last().is_some_and(|c| !c.is_whitespace()) {
            return Some(pos);
        }
        from = pos + delim.len();
        if from >= s.len() {
            break;
        }
    }
    None
}

// ── rendering ─────────────────────────────────────────────────────────────────

/// Render one line of inline markdown as a `rich_text` widget. `prefix` is a
/// literal lead-in (list bullet, quote indent) exempt from inline parsing.
fn rich_line(
    prefix: &str,
    md:     &str,
    base:   InlineStyle,
    size:   f32,
) -> Element<'static, Message> {
    let mut spans = Vec::new();
    if !prefix.is_empty() {
        spans.push(span(prefix.to_string()).font(styled_font(base)));
    }
    spans.extend(piece_spans(parse_inline(md, base)));
    rich_text(spans).size(size).width(Length::Fill).into()
}

fn piece_spans(pieces: Vec<Piece>) -> Vec<iced::widget::text::Span<'static, Message>> {
    let mut out = Vec::new();
    for p in pieces {
        match p {
            Piece::Text(content, st) => {
                out.push(
                    span(content)
                        .font(styled_font(st))
                        .strikethrough(st.strike),
                );
            }
            Piece::Code(code) => {
                out.push(
                    span(code)
                        .font(MONO_FONT)
                        .background(Color::from_rgba8(255, 255, 255, 0.08))
                        .padding([0.0, 3.0]),
                );
            }
            Piece::Link { label, url } => {
                out.push(
                    span(label)
                        .color(Color::from_rgb8(110, 160, 240))
                        .underline(true),
                );
                if !url.is_empty() {
                    out.push(
                        span(format!(" ({url})"))
                            .color(Color::from_rgba8(170, 170, 180, 0.8))
                            .size(11.0),
                    );
                }
            }
        }
    }
    out
}

/// The bundled Liberation Sans face matching an inline style. All four
/// weight/style combinations ship in `fonts.rs`, so bold and italic render
/// with real faces rather than falling back to Regular.
fn styled_font(st: InlineStyle) -> Font {
    Font {
        weight: if st.bold   { Weight::Bold }  else { Weight::Normal },
        style:  if st.italic { Style::Italic } else { Style::Normal },
        ..UI_FONT
    }
}

fn split_ordered_list(s: &str) -> Option<(u32, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
    if i == 0 || i + 1 >= bytes.len() { return None; }
    if bytes[i] != b'.' || bytes[i + 1] != b' ' { return None; }
    let num: u32 = s[..i].parse().ok()?;
    Some((num, &s[i + 2..]))
}

// ── shared helpers ────────────────────────────────────────────────────────────

fn scroll(content: Element<'static, Message>) -> Element<'static, Message> {
    scrollable(
        container(content)
            .padding(12)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(s: &str, bold: bool, italic: bool, strike: bool) -> Piece {
        Piece::Text(s.into(), InlineStyle { bold, italic, strike })
    }

    fn plain(s: &str) -> Piece {
        txt(s, false, false, false)
    }

    #[test]
    fn bold_styles_marked_run() {
        assert_eq!(
            parse_inline("a **b** c", InlineStyle::default()),
            vec![plain("a "), txt("b", true, false, false), plain(" c")],
        );
    }

    #[test]
    fn italic_and_strikethrough() {
        assert_eq!(
            parse_inline("*it* and ~~gone~~", InlineStyle::default()),
            vec![
                txt("it", false, true, false),
                plain(" and "),
                txt("gone", false, false, true),
            ],
        );
    }

    #[test]
    fn nested_markers_compose() {
        assert_eq!(
            parse_inline("**bold ~~struck~~**", InlineStyle::default()),
            vec![txt("bold ", true, false, false), txt("struck", true, false, true)],
        );
    }

    #[test]
    fn inline_code_is_verbatim() {
        assert_eq!(
            parse_inline("run `a * b` now", InlineStyle::default()),
            vec![plain("run "), Piece::Code("a * b".into()), plain(" now")],
        );
    }

    #[test]
    fn lone_asterisk_stays_literal() {
        assert_eq!(
            parse_inline("2 * 3 = 6", InlineStyle::default()),
            vec![plain("2 * 3 = 6")],
        );
    }

    #[test]
    fn snake_case_is_not_italic() {
        assert_eq!(
            parse_inline("use foo_bar_baz here", InlineStyle::default()),
            vec![plain("use foo_bar_baz here")],
        );
    }

    #[test]
    fn link_splits_label_and_url() {
        assert_eq!(
            parse_inline("see [docs](https://x) ok", InlineStyle::default()),
            vec![
                plain("see "),
                Piece::Link { label: "docs".into(), url: "https://x".into() },
                plain(" ok"),
            ],
        );
    }

    #[test]
    fn heading_base_style_composes_with_inline() {
        assert_eq!(
            parse_inline("title *em*", InlineStyle { bold: true, ..Default::default() }),
            vec![txt("title ", true, false, false), txt("em", true, true, false)],
        );
    }
}
