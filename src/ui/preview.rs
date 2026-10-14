//! Lightweight read-only Markdown preview.
//!
//! Block structure (headings, lists, quotes, fenced code, rules) maps to a
//! column of widgets; inline markdown (`**bold**`, `*italic*`, `~~strike~~`,
//! `` `code` ``, `[links](url)`) is parsed into styled `rich_text` spans so
//! the markers actually style their text instead of being stripped.

use iced::alignment::Horizontal;
use iced::font::{Style, Weight};
use iced::widget::text::Span;
use iced::widget::{column, container, horizontal_rule, rich_text, row, scrollable, span, text};
use iced::{Color, Element, Font, Length};

use crate::app::Message;
use crate::fonts::{MONO_FONT, UI_FONT};

// ── markdown ──────────────────────────────────────────────────────────────────

pub fn markdown(source: &str) -> Element<'static, Message> {
    let mut col = column![].spacing(6).width(Length::Fill);
    let mut in_code_block = false;
    let mut code_buf      = String::new();

    let lines: Vec<&str> = source.lines().collect();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        idx += 1;

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

        // Embedded raw HTML block (a <table>, <div>, …). Markdown allows raw
        // HTML; hand the whole block to the native HTML renderer so its tags,
        // inline CSS and colours render instead of showing up as literal text.
        if let Some(root) = html_block_start(trimmed) {
            let start = idx - 1;
            let mut end = start;
            let mut depth = 0i32;
            while end < lines.len() {
                depth += tag_delta(lines[end], &root);
                end += 1;
                if depth <= 0 {
                    break;
                }
            }
            col = col.push(html_body(&lines[start..end].join("\n")));
            idx = end;
            continue;
        }

        // GitHub-flavoured table — a header row followed by a delimiter row
        // (`| --- | :--: |`). Consume the whole block, then continue past it.
        if line.contains('|') {
            if let Some(aligns) = lines.get(idx).and_then(|l| separator_row(l)) {
                let header = split_table_row(line);
                let mut rows = Vec::new();
                idx += 1; // skip the delimiter row
                while idx < lines.len()
                    && lines[idx].contains('|')
                    && !lines[idx].trim().is_empty()
                {
                    rows.push(split_table_row(lines[idx]));
                    idx += 1;
                }
                col = col.push(table(header, aligns, rows));
                continue;
            }
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

// ── tables ──────────────────────────────────────────────────────────────────

/// Per-column horizontal alignment, from the delimiter row's colons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColAlign { Left, Center, Right }

/// Split a table row into trimmed cells, tolerating optional leading/trailing
/// pipes (`| a | b |` and `a | b` both yield two cells).
fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// A GFM delimiter row (`|---|:--:|---:|`) → the columns' alignments, or
/// `None` if the line isn't one. Each cell must be dashes with optional
/// bounding colons.
fn separator_row(line: &str) -> Option<Vec<ColAlign>> {
    if !line.contains('-') || !line.contains('|') {
        return None;
    }
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in &cells {
        let c = cell.trim();
        let left  = c.starts_with(':');
        let right = c.ends_with(':');
        let dashes = &c[left as usize..c.len().saturating_sub(right as usize)];
        if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true)  => ColAlign::Center,
            (false, true) => ColAlign::Right,
            _             => ColAlign::Left,
        });
    }
    Some(aligns)
}

fn table(
    header: Vec<String>,
    aligns: Vec<ColAlign>,
    rows:   Vec<Vec<String>>,
) -> Element<'static, Message> {
    const HEADER: InlineStyle = InlineStyle { bold: true, italic: false, strike: false };

    let mut col = column![table_row(header, &aligns, HEADER)]
        .spacing(4)
        .width(Length::Fill);
    col = col.push(horizontal_rule(1));
    for r in rows {
        col = col.push(table_row(r, &aligns, InlineStyle::default()));
    }

    container(col)
        .padding(8)
        .width(Length::Fill)
        .style(container::bordered_box)
        .into()
}

fn table_row(
    cells:  Vec<String>,
    aligns: &[ColAlign],
    base:   InlineStyle,
) -> Element<'static, Message> {
    let mut r = row![].spacing(8).width(Length::Fill);
    for (i, cell) in cells.into_iter().enumerate() {
        let horiz = match aligns.get(i).copied().unwrap_or(ColAlign::Left) {
            ColAlign::Left   => Horizontal::Left,
            ColAlign::Center => Horizontal::Center,
            ColAlign::Right  => Horizontal::Right,
        };
        r = r.push(
            container(rich_line("", &cell, base, 13.0))
                .width(Length::FillPortion(1))
                .align_x(horiz),
        );
    }
    r.into()
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
                let font = styled_font(st);
                // Hex / rgb() / hsl() colour codes in the text get a swatch chip
                // rendered right after them.
                emit_colored(
                    &content,
                    move |t| span(t).font(font).strikethrough(st.strike),
                    13.0,
                    &mut out,
                );
            }
            Piece::Code(code) => {
                emit_colored(
                    &code,
                    |t| {
                        span(t)
                            .font(MONO_FONT)
                            .background(Color::from_rgba8(255, 255, 255, 0.08))
                            .padding([0.0, 3.0])
                    },
                    13.0,
                    &mut out,
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

// ── colour swatches ───────────────────────────────────────────────────────────
//
// Any `#rgb`/`#rrggbb`/`#rrggbbaa`, `rgb()/rgba()`, or `hsl()/hsla()` literal in
// rendered text is followed by a small filled chip of that colour, so a markdown
// note or an HTML file full of colour values reads at a glance. Named CSS colours
// are resolved too, but only from an explicit CSS `color:` declaration — matching
// bare words like "red" in prose would be far too noisy.

const CODE_BG:    Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };
const LINK_COLOR: Color = Color { r: 0.43, g: 0.63, b: 0.94, a: 1.0 };

/// A filled chip carrying `c` as its background — the visible swatch.
fn swatch_span(c: Color, size: f32) -> Span<'static, Message> {
    span("  ").size(size).background(c).padding([0.0, 2.0])
}

/// Split `content` into spans built by `mk`, inserting a [`swatch_span`] after
/// every colour literal found in it.
fn emit_colored<F>(content: &str, mk: F, swatch_size: f32, out: &mut Vec<Span<'static, Message>>)
where
    F: Fn(String) -> Span<'static, Message>,
{
    let mut s = content;
    while let Some((start, len, col)) = scan_color_token(s) {
        if start > 0 {
            out.push(mk(s[..start].to_string()));
        }
        out.push(mk(s[start..start + len].to_string()));
        out.push(swatch_span(col, swatch_size));
        s = &s[start + len..];
    }
    if !s.is_empty() {
        out.push(mk(s.to_string()));
    }
}

/// Find the first colour literal in `s`: its byte offset, length, and value.
/// Only `#hex` and functional `rgb()/hsl()` forms are scanned here (never bare
/// names), so ordinary prose is left alone.
fn scan_color_token(s: &str) -> Option<(usize, usize, Color)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'#' {
            if let Some(len) = hex_len(&s[i..]) {
                if let Some(c) = parse_color(&s[i..i + len]) {
                    return Some((i, len, c));
                }
            }
        } else if matches!(b, b'r' | b'R' | b'h' | b'H') {
            // Functional notation only at a word boundary, so "worgb" doesn't hit.
            let prev_alnum = i > 0 && bytes[i - 1].is_ascii_alphanumeric();
            if !prev_alnum {
                if let Some(len) = func_len(&s[i..]) {
                    if let Some(c) = parse_color(&s[i..i + len]) {
                        return Some((i, len, c));
                    }
                }
            }
        }
        i += 1;
    }
    None
}

/// Length (including the `#`) of a hex colour at the start of `s`, or `None`.
/// Only exact 3/4/6/8-digit runs count, so `#deadbeef99` isn't a partial match.
fn hex_len(s: &str) -> Option<usize> {
    let n = s[1..].chars().take_while(|c| c.is_ascii_hexdigit()).count();
    matches!(n, 3 | 4 | 6 | 8).then_some(1 + n)
}

/// Length of a functional `rgb()/rgba()/hsl()/hsla()` colour at the start of `s`.
fn func_len(s: &str) -> Option<usize> {
    // `chars().take` keeps this on UTF-8 boundaries — a raw byte slice would
    // panic on a multi-byte char straddling the cut.
    let head: String = s.chars().take(5).collect::<String>().to_ascii_lowercase();
    let ok = head.starts_with("rgba(")
        || head.starts_with("rgb(")
        || head.starts_with("hsla(")
        || head.starts_with("hsl(");
    if !ok {
        return None;
    }
    // `find` returns a byte index on a boundary; ignore an unclosed paren.
    let end = s.find(')')?;
    if end > 64 {
        return None;
    }
    Some(end + 1)
}

/// Parse any single CSS colour value — `#hex`, `rgb()/rgba()`, `hsl()/hsla()`,
/// or a named colour — into an iced [`Color`].
fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("rgb") {
        return parse_rgb(&lower);
    }
    if lower.starts_with("hsl") {
        return parse_hsl(&lower);
    }
    named_color(&lower)
}

fn parse_hex(h: &str) -> Option<Color> {
    let hex: String = match h.chars().count() {
        3 | 4 => h.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => h.to_string(),
        _ => return None,
    };
    let get = |i: usize| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok();
    let r = get(0)?;
    let g = get(1)?;
    let b = get(2)?;
    let a = if hex.len() == 8 { get(3)? as f32 / 255.0 } else { 1.0 };
    Some(Color::from_rgba8(r, g, b, a))
}

/// Split the body of a functional notation (`rgb(a b c / d)`) into its parts,
/// tolerating comma-, slash-, or space-separated forms.
fn func_parts(s: &str) -> Option<Vec<String>> {
    let inner = &s[s.find('(')? + 1..s.rfind(')')?];
    Some(
        inner
            .split(|c| c == ',' || c == '/' || c == ' ')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// A channel value or alpha, honouring a trailing `%`. `full` is what 100% means
/// (255.0 for rgb channels, 1.0 for alpha).
fn css_number(p: &str, full: f32) -> Option<f32> {
    if let Some(pct) = p.strip_suffix('%') {
        Some(pct.trim().parse::<f32>().ok()? / 100.0 * full)
    } else {
        p.parse::<f32>().ok()
    }
}

fn parse_rgb(s: &str) -> Option<Color> {
    let parts = func_parts(s)?;
    if parts.len() < 3 {
        return None;
    }
    let r = css_number(&parts[0], 255.0)?;
    let g = css_number(&parts[1], 255.0)?;
    let b = css_number(&parts[2], 255.0)?;
    let a = parts.get(3).and_then(|p| css_number(p, 1.0)).unwrap_or(1.0);
    Some(Color::from_rgba8(
        r.round().clamp(0.0, 255.0) as u8,
        g.round().clamp(0.0, 255.0) as u8,
        b.round().clamp(0.0, 255.0) as u8,
        a.clamp(0.0, 1.0),
    ))
}

fn parse_hsl(s: &str) -> Option<Color> {
    let parts = func_parts(s)?;
    if parts.len() < 3 {
        return None;
    }
    let h = parts[0].trim_end_matches("deg").parse::<f32>().ok()?;
    let sat = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let lum = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
    let a = parts.get(3).and_then(|p| css_number(p, 1.0)).unwrap_or(1.0);
    let (r, g, b) = hsl_to_rgb(h, sat.clamp(0.0, 1.0), lum.clamp(0.0, 1.0));
    Some(Color { r, g, b, a: a.clamp(0.0, 1.0) })
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (l, l, l);
    }
    let h = (((h % 360.0) + 360.0) % 360.0) / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0))
}

/// The common CSS named colours. Not exhaustive, but covers the basic + widely
/// used extended set; anything unknown just isn't treated as a colour.
fn named_color(name: &str) -> Option<Color> {
    let rgb = match name {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "lime" => (0, 255, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "cyan" | "aqua" => (0, 255, 255),
        "magenta" | "fuchsia" => (255, 0, 255),
        "silver" => (192, 192, 192),
        "gray" | "grey" => (128, 128, 128),
        "maroon" => (128, 0, 0),
        "olive" => (128, 128, 0),
        "purple" => (128, 0, 128),
        "teal" => (0, 128, 128),
        "navy" => (0, 0, 128),
        "orange" => (255, 165, 0),
        "gold" => (255, 215, 0),
        "pink" => (255, 192, 203),
        "brown" => (165, 42, 42),
        "coral" => (255, 127, 80),
        "salmon" => (250, 128, 114),
        "crimson" => (220, 20, 60),
        "indigo" => (75, 0, 130),
        "violet" => (238, 130, 238),
        "turquoise" => (64, 224, 208),
        "tan" => (210, 180, 140),
        "beige" => (245, 245, 220),
        "ivory" => (255, 255, 240),
        "khaki" => (240, 230, 140),
        "lavender" => (230, 230, 250),
        "plum" => (221, 160, 221),
        "orchid" => (218, 112, 214),
        "chocolate" => (210, 105, 30),
        "tomato" => (255, 99, 71),
        "slategray" | "slategrey" => (112, 128, 144),
        "darkgray" | "darkgrey" => (169, 169, 169),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "transparent" => return Some(Color::TRANSPARENT),
        _ => return None,
    };
    Some(Color::from_rgb8(rgb.0, rgb.1, rgb.2))
}

// ── HTML ──────────────────────────────────────────────────────────────────────
//
// A small native HTML renderer: no browser, no screenshot. It tokenises a
// forgiving subset of HTML and honours inline `style="…"` CSS (colour,
// background, font weight/style, text-decoration, font-size) plus the deprecated
// `color`/`bgcolor` attributes, emitting the same styled `rich_text` spans the
// markdown path uses — so colour swatches work here too. `<style>` blocks are
// parsed into a small stylesheet (simple selectors) and `<table>`s render as
// grids; `<script>` is dropped and unknown tags are transparent to their content.

/// If `line` (left-trimmed) begins a block-level raw-HTML element, return its
/// lowercase tag name — the root whose matching close ends the block.
fn html_block_start(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('<')?;
    if rest.starts_with('/') || rest.starts_with('!') {
        return None;
    }
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    const BLOCK: &[&str] = &[
        "table", "thead", "tbody", "div", "style", "section", "article",
        "details", "figure", "blockquote", "pre", "ul", "ol", "dl", "form",
        "header", "footer", "nav", "aside", "h1", "h2", "h3", "h4", "h5", "h6",
    ];
    BLOCK.contains(&name.as_str()).then_some(name)
}

/// Net open/close balance of `root` on one line: `+1` per `<root…>`, `-1` per
/// `</root>`. Used to find where an embedded HTML block ends.
fn tag_delta(line: &str, root: &str) -> i32 {
    let lower = line.to_ascii_lowercase();
    let opens = lower.matches(&format!("<{root}")).count() as i32;
    let closes = lower.matches(&format!("</{root}")).count() as i32;
    opens - closes
}

pub fn html(source: &str) -> Element<'static, Message> {
    scroll(html_body(source))
}

/// The rendered HTML as a plain column — no surrounding scrollable, so it can be
/// embedded inside markdown (which supplies its own scroll) without nesting two
/// Fill-height scrollables, which iced rejects.
fn html_body(source: &str) -> Element<'static, Message> {
    let mut b = Builder::default();
    for tok in tokenize(source) {
        match tok {
            Tok::Text(t) => b.push_text(&t),
            Tok::Style(css) => b.css.extend(parse_css(&css)),
            Tok::Close(name) => b.close(&name.to_ascii_lowercase()),
            Tok::Open(name, attrs) => {
                let name = name.to_ascii_lowercase();
                if is_void(&name) {
                    b.void(&name);
                } else {
                    b.open(&name, &attrs);
                }
            }
            Tok::SelfClose(name, attrs) => {
                let name = name.to_ascii_lowercase();
                if is_void(&name) {
                    b.void(&name);
                } else {
                    b.open(&name, &attrs);
                    b.close(&name);
                }
            }
        }
    }
    b.finish()
}

/// One computed style frame — pushed on every open tag, popped on close, so both
/// inline style and block layout unwind exactly with the element that set them.
#[derive(Clone)]
struct Frame {
    bold:       bool,
    italic:     bool,
    underline:  bool,
    strike:     bool,
    mono:       bool,
    pre:        bool,
    color:      Option<Color>,
    background: Option<Color>,
    size:       Option<f32>,
    block_size: f32,
    indent:     usize,
    /// `text-align`, used for table cells.
    align:      Horizontal,
}

impl Default for Frame {
    fn default() -> Self {
        Frame {
            bold: false, italic: false, underline: false, strike: false,
            mono: false, pre: false, color: None, background: None,
            size: None, block_size: 13.0, indent: 0, align: Horizontal::Left,
        }
    }
}

/// One captured table cell — its inline content already rendered to spans,
/// plus the box styling needed to lay it out as a grid cell.
struct Cell {
    spans:      Vec<Span<'static, Message>>,
    background: Option<Color>,
    align:      Horizontal,
    rowspan:    usize,
}

/// State for the `<table>` currently being captured. While one is open, inline
/// content is routed into the current cell instead of the block flow.
#[derive(Default)]
struct TableCtx {
    rows:     Vec<Vec<Cell>>,
    cur_row:  Vec<Cell>,
    cur_cell: Option<Cell>,
}

#[derive(Default)]
struct Builder {
    blocks: Vec<Element<'static, Message>>,
    line:   Vec<Span<'static, Message>>,
    stack:  Vec<Frame>,
    /// One entry per open `<ul>`/`<ol>`; `Some(n)` is an ordered list's running
    /// counter, `None` an unordered one.
    list:   Vec<Option<u32>>,
    /// Rules from every `<style>` block seen so far.
    css:    Vec<Rule>,
    /// The `<table>` being captured, if any.
    table:  Option<TableCtx>,
}

/// Rendering caps. A real saved web page can hold tens of thousands of
/// elements, and iced re-shapes every `rich_text` block on each redraw, so an
/// uncapped tree turns Preview into a frozen window. Past these limits the
/// preview truncates with a note rather than locking the app up.
const MAX_BLOCKS: usize = 4000;
const MAX_LINE_SPANS: usize = 6000;

impl Builder {
    fn cur(&self) -> Frame {
        self.stack.last().cloned().unwrap_or_default()
    }

    /// Turn the pending inline run into a block, indented per the current frame.
    fn flush_line(&mut self) {
        // Inside a table, inline content lives in cells, not the block flow.
        if self.table.is_some() {
            self.line.clear();
            return;
        }
        if self.line.is_empty() {
            return;
        }
        if self.blocks.len() >= MAX_BLOCKS {
            self.line.clear();
            return;
        }
        let f = self.cur();
        let spans = std::mem::take(&mut self.line);
        let rt = rich_text(spans).size(f.block_size).width(Length::Fill);
        let el: Element<'static, Message> = if f.indent > 0 {
            container(rt)
                .padding(iced::Padding { top: 0.0, right: 0.0, bottom: 0.0, left: f.indent as f32 * 16.0 })
                .width(Length::Fill)
                .into()
        } else {
            rt.into()
        };
        self.blocks.push(el);
        if self.blocks.len() == MAX_BLOCKS {
            self.blocks
                .push(text("… preview truncated (document too large) …").size(11).into());
        }
    }

    fn open(&mut self, name: &str, attrs: &[(String, String)]) {
        if is_block(name) {
            self.flush_line();
        }
        let classes: Vec<&str> = attrs
            .iter()
            .find(|(k, _)| k == "class")
            .map(|(_, v)| v.split_whitespace().collect())
            .unwrap_or_default();
        let id = attrs.iter().find(|(k, _)| k == "id").map(|(_, v)| v.as_str());

        let mut f = self.cur();
        apply_tag(&mut f, name);
        // Stylesheet rules first; the inline `style` attribute overrides them.
        let sheet = self.matched_css(name, &classes, id);
        if !sheet.is_empty() {
            apply_style_attr(&mut f, &sheet);
        }
        for (k, v) in attrs {
            apply_attr(&mut f, k, v);
        }
        match name {
            "ul" => self.list.push(None),
            "ol" => {
                let start = attrs
                    .iter()
                    .find(|(k, _)| k == "start")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(1);
                self.list.push(Some(start));
            }
            "table" => self.table = Some(TableCtx::default()),
            "tr" => {
                if let Some(t) = self.table.as_mut() {
                    t.cur_row.clear();
                }
            }
            "td" | "th" => {
                let rowspan = attrs
                    .iter()
                    .find(|(k, _)| k == "rowspan")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(1);
                let (background, align) = (f.background, f.align);
                if let Some(t) = self.table.as_mut() {
                    t.cur_cell = Some(Cell { spans: Vec::new(), background, align, rowspan });
                }
            }
            _ => {}
        }
        self.stack.push(f);
        if name == "li" {
            self.li_prefix();
        }
    }

    fn close(&mut self, name: &str) {
        match name {
            "td" | "th" => {
                if let Some(t) = self.table.as_mut() {
                    if let Some(cell) = t.cur_cell.take() {
                        t.cur_row.push(cell);
                    }
                }
            }
            "tr" => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.cur_row);
                    if !row.is_empty() {
                        t.rows.push(row);
                    }
                }
            }
            "table" => {
                if let Some(t) = self.table.take() {
                    if self.blocks.len() < MAX_BLOCKS {
                        self.blocks.push(render_table(t));
                    }
                }
            }
            _ => {}
        }
        if is_block(name) {
            self.flush_line();
        }
        if name == "ul" || name == "ol" {
            self.list.pop();
        }
        self.stack.pop();
    }

    /// Declarations from every stylesheet rule this element matches, in order.
    fn matched_css(&self, tag: &str, classes: &[&str], id: Option<&str>) -> String {
        let mut out = String::new();
        for rule in &self.css {
            if rule.selectors.iter().any(|s| s.matches(tag, classes, id)) {
                out.push_str(&rule.decls);
                if !out.ends_with(';') {
                    out.push(';');
                }
            }
        }
        out
    }

    /// Route inline spans to the current table cell, or the block line.
    fn push_spans(&mut self, spans: Vec<Span<'static, Message>>) {
        if let Some(t) = self.table.as_mut() {
            if let Some(cell) = t.cur_cell.as_mut() {
                cell.spans.extend(spans);
            }
            // Between cells (whitespace) — dropped.
            return;
        }
        self.line.extend(spans);
    }

    /// Length of whatever inline run new spans would land in.
    fn line_len(&self) -> usize {
        match self.table.as_ref().and_then(|t| t.cur_cell.as_ref()) {
            Some(cell) => cell.spans.len(),
            None => self.line.len(),
        }
    }

    fn void(&mut self, name: &str) {
        match name {
            "br" => self.flush_line(),
            "hr" => {
                self.flush_line();
                self.blocks.push(horizontal_rule(1).into());
            }
            _ => {}
        }
    }

    /// Push a bullet or numeral for a list item, using the innermost open list.
    fn li_prefix(&mut self) {
        let f = self.cur();
        let prefix = match self.list.last_mut() {
            Some(Some(n)) => {
                let s = format!("{n}. ");
                *n += 1;
                s
            }
            _ => "• ".to_string(),
        };
        self.push_spans(vec![span(prefix).font(css_font(&f))]);
    }

    fn push_text(&mut self, raw: &str) {
        let f = self.cur();
        let decoded = decode_entities(raw);
        if f.pre {
            // Preformatted: keep whitespace, break the line on newlines.
            let mut first = true;
            for segment in decoded.split('\n') {
                if !first {
                    self.flush_line();
                }
                first = false;
                self.emit_run(&f, segment);
            }
            return;
        }
        let mut collapsed = collapse_ws(&decoded);
        if self.line.is_empty() {
            collapsed = collapsed.trim_start().to_string();
        }
        if collapsed.is_empty() {
            return;
        }
        self.emit_run(&f, &collapsed);
    }

    /// Emit one styled text run (with colour swatches) into the current line or
    /// table cell.
    fn emit_run(&mut self, f: &Frame, text: &str) {
        if text.is_empty() || self.line_len() >= MAX_LINE_SPANS {
            return;
        }
        let font = css_font(f);
        let color = f.color;
        let background = f.background;
        let strike = f.strike;
        let underline = f.underline;
        let size = f.size;
        let swsize = f.size.unwrap_or(f.block_size);
        let mut buf = Vec::new();
        emit_colored(
            text,
            move |t| {
                let mut sp = span(t).font(font).strikethrough(strike).underline(underline);
                if let Some(c) = color {
                    sp = sp.color(c);
                }
                if let Some(bg) = background {
                    sp = sp.background(bg);
                }
                if let Some(sz) = size {
                    sp = sp.size(sz);
                }
                sp
            },
            swsize,
            &mut buf,
        );
        self.push_spans(buf);
    }

    fn finish(mut self) -> Element<'static, Message> {
        self.flush_line();
        let mut col = column![].spacing(6).width(Length::Fill);
        for el in self.blocks {
            col = col.push(el);
        }
        col.into()
    }
}

/// The bundled face matching a frame's weight / slant / family.
fn css_font(f: &Frame) -> Font {
    let base = if f.mono { MONO_FONT } else { UI_FONT };
    Font {
        weight: if f.bold { Weight::Bold } else { Weight::Normal },
        style:  if f.italic { Style::Italic } else { Style::Normal },
        ..base
    }
}

/// One resolved grid cell after rowspans are expanded.
struct GridCell {
    spans:      Vec<Span<'static, Message>>,
    background: Option<Color>,
    align:      Horizontal,
}

/// Lay a captured table out as a bordered grid of equal-width columns. Rowspans
/// are expanded into filler cells that carry the spanning cell's background, so
/// columns stay aligned and the colour block runs unbroken down the rows.
fn render_table(t: TableCtx) -> Element<'static, Message> {
    // (remaining rows, background) for each column still covered by a rowspan.
    let mut carry: Vec<Option<(usize, Option<Color>)>> = Vec::new();
    let mut grid: Vec<Vec<GridCell>> = Vec::new();

    for row in t.rows {
        let mut out: Vec<GridCell> = Vec::new();
        let mut cells = row.into_iter();
        let mut ci = 0;
        loop {
            if ci >= carry.len() {
                carry.push(None);
            }
            if let Some((rem, bg)) = carry[ci] {
                out.push(GridCell { spans: vec![span(" ")], background: bg, align: Horizontal::Left });
                carry[ci] = (rem > 1).then_some((rem - 1, bg));
                ci += 1;
                continue;
            }
            let Some(cell) = cells.next() else { break };
            let mut spans = cell.spans;
            if spans.is_empty() {
                spans.push(span(" "));
            }
            if cell.rowspan > 1 {
                carry[ci] = Some((cell.rowspan - 1, cell.background));
            }
            out.push(GridCell { spans, background: cell.background, align: cell.align });
            ci += 1;
        }
        grid.push(out);
    }

    let mut col = column![].spacing(0).width(Length::Fill);
    for grow in grid {
        let mut r = row![].spacing(0).width(Length::Fill);
        for gc in grow {
            let bg = gc.background;
            r = r.push(
                container(rich_text(gc.spans).size(13))
                    .width(Length::FillPortion(1))
                    .padding([3.0, 8.0])
                    .align_x(gc.align)
                    .style(move |_: &iced::Theme| container::Style {
                        background: bg.map(iced::Background::from),
                        border: iced::Border {
                            color:  Color::from_rgba8(120, 120, 130, 0.5),
                            width:  1.0,
                            radius: 0.0.into(),
                        },
                        ..container::Style::default()
                    }),
            );
        }
        col = col.push(r);
    }

    container(col).width(Length::Fill).into()
}

/// Apply a tag's intrinsic styling / layout to `f`.
fn apply_tag(f: &mut Frame, name: &str) {
    match name {
        "b" | "strong"                    => f.bold = true,
        "i" | "em" | "cite" | "var" | "dfn" => f.italic = true,
        "u" | "ins"                       => f.underline = true,
        "th"                              => f.bold = true,
        "s" | "strike" | "del"            => f.strike = true,
        "kbd" | "samp" | "tt"             => f.mono = true,
        "code" => {
            f.mono = true;
            if f.background.is_none() {
                f.background = Some(CODE_BG);
            }
        }
        "a" => {
            f.underline = true;
            if f.color.is_none() {
                f.color = Some(LINK_COLOR);
            }
        }
        "mark" => {
            if f.background.is_none() {
                f.background = Some(Color::from_rgba8(250, 204, 21, 0.35));
            }
        }
        "small" => f.size = Some(f.size.unwrap_or(f.block_size) * 0.85),
        "h1" => { f.block_size = 28.0; f.bold = true; }
        "h2" => { f.block_size = 24.0; f.bold = true; }
        "h3" => { f.block_size = 20.0; f.bold = true; }
        "h4" => { f.block_size = 16.0; f.bold = true; }
        "h5" => { f.block_size = 14.0; f.bold = true; }
        "h6" => { f.block_size = 13.0; f.bold = true; }
        "pre" => { f.mono = true; f.pre = true; }
        "blockquote" => {
            f.indent += 1;
            if f.color.is_none() {
                f.color = Some(Color::from_rgba8(170, 170, 180, 0.9));
            }
        }
        "ul" | "ol" => f.indent += 1,
        _ => {}
    }
}

fn apply_attr(f: &mut Frame, k: &str, v: &str) {
    match k {
        "style"   => apply_style_attr(f, v),
        "color"   => if let Some(c) = parse_color(v) { f.color = Some(c); },
        "bgcolor" => if let Some(c) = parse_color(v) { f.background = Some(c); },
        _ => {}
    }
}

/// Apply the declarations of an inline `style="…"` attribute to `f`.
fn apply_style_attr(f: &mut Frame, decl: &str) {
    let base = f.block_size;
    for part in decl.split(';') {
        let mut kv = part.splitn(2, ':');
        let key = kv.next().unwrap_or("").trim().to_ascii_lowercase();
        let val = kv.next().unwrap_or("").trim();
        if val.is_empty() {
            continue;
        }
        match key.as_str() {
            "color" => if let Some(c) = parse_color(val) { f.color = Some(c); },
            "background" | "background-color" => {
                if let Some(c) = parse_color(val) {
                    f.background = Some(c);
                }
            }
            "font-weight" => {
                let bold = val == "bold"
                    || val == "bolder"
                    || val.parse::<u32>().map(|n| n >= 600).unwrap_or(false);
                if bold {
                    f.bold = true;
                } else if val == "normal" {
                    f.bold = false;
                }
            }
            "font-style" => f.italic = val == "italic" || val == "oblique",
            "text-decoration" | "text-decoration-line" => {
                if val.contains("underline") {
                    f.underline = true;
                }
                if val.contains("line-through") {
                    f.strike = true;
                }
                if val.contains("none") {
                    f.underline = false;
                    f.strike = false;
                }
            }
            "font-size" => if let Some(px) = parse_size(val, base) { f.size = Some(px); },
            "font-family" => {
                if val.to_ascii_lowercase().contains("mono") {
                    f.mono = true;
                }
            }
            "text-align" => {
                f.align = match val {
                    "right"  => Horizontal::Right,
                    "center" => Horizontal::Center,
                    _        => Horizontal::Left,
                };
            }
            _ => {}
        }
    }
}

fn parse_size(v: &str, base: f32) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    if let Some(px) = v.strip_suffix("px") {
        px.trim().parse().ok()
    } else if let Some(em) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        em.trim().parse::<f32>().ok().map(|n| n * base)
    } else if let Some(p) = v.strip_suffix('%') {
        p.trim().parse::<f32>().ok().map(|n| n / 100.0 * base)
    } else {
        match v.as_str() {
            "small"    => Some(11.0),
            "medium"   => Some(13.0),
            "large"    => Some(18.0),
            "x-large"  => Some(22.0),
            "xx-large" => Some(28.0),
            _ => v.parse().ok(),
        }
    }
}

// ── stylesheet (`<style>` blocks) ─────────────────────────────────────────────
//
// A deliberately small CSS engine: it parses rule sets and matches simple
// selectors (`tag`, `.class`, `#id`, and compounds like `td.foo`), applying the
// same properties as inline `style`. Combinators are approximated by matching
// the selector's rightmost compound (so `table td` styles any `td`), and
// at-rules like `@media` are skipped. Enough for hand- and AI-written HTML.

/// A parsed rule set: its selectors and the raw declaration text they carry.
struct Rule {
    selectors: Vec<Simple>,
    decls:     String,
}

/// One simple/compound selector: an optional tag plus any classes and an id.
#[derive(Default)]
struct Simple {
    tag:     Option<String>,
    classes: Vec<String>,
    id:      Option<String>,
}

impl Simple {
    fn matches(&self, tag: &str, classes: &[&str], id: Option<&str>) -> bool {
        if let Some(t) = &self.tag {
            if t != "*" && !t.eq_ignore_ascii_case(tag) {
                return false;
            }
        }
        for c in &self.classes {
            if !classes.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                return false;
            }
        }
        if let Some(sid) = &self.id {
            if id != Some(sid.as_str()) {
                return false;
            }
        }
        // An empty selector matches nothing.
        self.tag.is_some() || !self.classes.is_empty() || self.id.is_some()
    }
}

fn parse_css(body: &str) -> Vec<Rule> {
    let body = strip_css_comments(body);
    let mut rules = Vec::new();
    let mut rest = body.as_str();
    while let Some(open) = rest.find('{') {
        let sel_str = rest[..open].trim();
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        let decls = after[..close].trim();
        // Skip at-rules (@media, @font-face, …) — nested blocks aren't handled.
        if !sel_str.starts_with('@') && !decls.is_empty() {
            let selectors: Vec<Simple> = sel_str.split(',').filter_map(parse_simple).collect();
            if !selectors.is_empty() {
                rules.push(Rule { selectors, decls: decls.to_string() });
            }
        }
        rest = &after[close + 1..];
    }
    rules
}

fn strip_css_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Parse one selector into its rightmost compound (`tag`/`.class`/`#id`).
fn parse_simple(sel: &str) -> Option<Simple> {
    // Keep only the rightmost compound — ancestor/combinator parts are ignored.
    let compound = sel
        .rsplit(|c: char| c.is_whitespace() || c == '>' || c == '+' || c == '~')
        .next()?
        .split(':') // drop pseudo-classes/elements
        .next()?
        .trim();
    if compound.is_empty() {
        return None;
    }
    if compound == "*" {
        return Some(Simple { tag: Some("*".into()), ..Simple::default() });
    }
    let mut simple = Simple::default();
    let mut buf = String::new();
    let mut kind = 't'; // 't' tag, 'c' class, 'i' id
    let commit = |kind: char, buf: &mut String, s: &mut Simple| {
        if buf.is_empty() {
            return;
        }
        let v = std::mem::take(buf);
        match kind {
            'c' => s.classes.push(v),
            'i' => s.id = Some(v),
            _ => s.tag = Some(v),
        }
    };
    for ch in compound.chars() {
        match ch {
            '.' => { commit(kind, &mut buf, &mut simple); kind = 'c'; }
            '#' => { commit(kind, &mut buf, &mut simple); kind = 'i'; }
            _ => buf.push(ch),
        }
    }
    commit(kind, &mut buf, &mut simple);
    (simple.tag.is_some() || !simple.classes.is_empty() || simple.id.is_some()).then_some(simple)
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav"
            | "aside" | "figure" | "figcaption" | "h1" | "h2" | "h3" | "h4" | "h5"
            | "h6" | "ul" | "ol" | "li" | "blockquote" | "pre" | "table" | "tr"
            | "thead" | "tbody"
    )
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "br" | "hr" | "img" | "input" | "meta" | "link" | "col" | "area" | "base"
            | "source" | "track" | "wbr" | "embed"
    )
}

/// Collapse every run of whitespace (including newlines) to a single space.
fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out
}

/// Decode the handful of HTML entities that actually turn up in text, plus
/// numeric (`&#NN;` / `&#xHH;`) references.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some(semi) = after[..after.len().min(12)].find(';') {
            let ent = &after[1..semi];
            let decoded = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some('\u{00a0}'),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "mdash" => Some('—'),
                "ndash" => Some('–'),
                "hellip" => Some('…'),
                _ => ent
                    .strip_prefix('#')
                    .and_then(|n| {
                        if let Some(hex) = n.strip_prefix(['x', 'X']) {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            n.parse().ok()
                        }
                    })
                    .and_then(char::from_u32),
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        // Not a recognised entity — keep the literal ampersand and move on.
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

// ── HTML tokeniser ────────────────────────────────────────────────────────────

enum Tok {
    Open(String, Vec<(String, String)>),
    SelfClose(String, Vec<(String, String)>),
    Close(String),
    Text(String),
    /// The body of a `<style>` element — parsed into a stylesheet, not shown.
    Style(String),
}

/// Byte index of the `>` that closes the tag opening at `start` (`<`), skipping
/// any `>` inside a quoted attribute value. `None` if the tag is unterminated.
fn find_tag_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut j = start + 1;
    let mut quote = 0u8;
    while j < b.len() {
        let c = b[j];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return Some(j);
        }
        j += 1;
    }
    None
}

fn tokenize(s: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let n = s.len();
    while i < n {
        if s[i..].starts_with('<') {
            if s[i..].starts_with("<!--") {
                flush_text(&mut text, &mut out);
                i += s[i..].find("-->").map(|e| e + 3).unwrap_or(n - i);
                continue;
            }
            if s[i..].starts_with("<!") {
                flush_text(&mut text, &mut out);
                i += s[i..].find('>').map(|e| e + 1).unwrap_or(n - i);
                continue;
            }
            // Scan for the tag's closing '>', skipping any inside quoted
            // attribute values so `title="a > b"` can't cut the tag short and
            // spill the rest as literal text.
            if let Some(gt_abs) = find_tag_end(s, i) {
                let inner = &s[i + 1..gt_abs];
                // A stray "<" (e.g. "a < b") with no plausible tag — keep literal.
                if inner.is_empty()
                    || !inner
                        .trim_start_matches('/')
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
                {
                    text.push('<');
                    i += 1;
                    continue;
                }
                flush_text(&mut text, &mut out);
                let (tok, raw) = parse_tag(inner);
                // Raw-text elements: capture <style> CSS as a token, drop <script>.
                if let Some(name) = raw {
                    let close = format!("</{name}>");
                    let body_start = gt_abs + 1;
                    let body = &s[body_start..];
                    let (css, advance) = match body.to_ascii_lowercase().find(&close) {
                        Some(ce) => (&body[..ce], body_start + ce + close.len()),
                        None => (body, n),
                    };
                    if name == "style" {
                        out.push(Tok::Style(css.to_string()));
                    }
                    i = advance;
                    continue;
                }
                out.push(tok);
                i = gt_abs + 1;
                continue;
            }
            // No closing '>': treat the rest as text.
            text.push('<');
            i += 1;
        } else {
            let c = s[i..].chars().next().unwrap();
            text.push(c);
            i += c.len_utf8();
        }
    }
    flush_text(&mut text, &mut out);
    out
}

fn flush_text(text: &mut String, out: &mut Vec<Tok>) {
    if !text.is_empty() {
        out.push(Tok::Text(std::mem::take(text)));
    }
}

/// Parse a tag's inner text (between `<` and `>`). The second return is the
/// lowercase name only when it's a raw-text element (`script`/`style`) whose
/// body the tokeniser must then skip.
fn parse_tag(inner: &str) -> (Tok, Option<String>) {
    let inner = inner.trim();
    if let Some(rest) = inner.strip_prefix('/') {
        let name = rest.trim().split_whitespace().next().unwrap_or("").to_string();
        return (Tok::Close(name), None);
    }
    let self_close = inner.ends_with('/');
    let inner = inner.trim_end_matches('/').trim_end();
    let mut it = inner.splitn(2, char::is_whitespace);
    let name = it.next().unwrap_or("").to_string();
    let attrs = parse_attrs(it.next().unwrap_or(""));
    let lname = name.to_ascii_lowercase();
    if self_close {
        (Tok::SelfClose(name, attrs), None)
    } else {
        let raw = (lname == "script" || lname == "style").then_some(lname);
        (Tok::Open(name, attrs), raw)
    }
}

fn parse_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let start = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let key = s[start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                val = s[vs..i].to_string();
                if i < b.len() {
                    i += 1;
                }
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                val = s[vs..i].to_string();
            }
        }
        if !key.is_empty() {
            out.push((key, decode_entities(&val)));
        }
    }
    out
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
    fn separator_row_reads_alignments() {
        assert_eq!(
            separator_row("| :--- | :--: | ---: |"),
            Some(vec![ColAlign::Left, ColAlign::Center, ColAlign::Right]),
        );
        assert_eq!(separator_row("|---|---|"), Some(vec![ColAlign::Left, ColAlign::Left]));
    }

    #[test]
    fn non_separator_rows_rejected() {
        assert_eq!(separator_row("| a | b |"), None);
        assert_eq!(separator_row("just text"), None);
    }

    #[test]
    fn row_split_tolerates_bounding_pipes() {
        assert_eq!(split_table_row("| a | b | c |"), vec!["a", "b", "c"]);
        assert_eq!(split_table_row("a | b"), vec!["a", "b"]);
    }

    #[test]
    fn heading_base_style_composes_with_inline() {
        assert_eq!(
            parse_inline("title *em*", InlineStyle { bold: true, ..Default::default() }),
            vec![txt("title ", true, false, false), txt("em", true, true, false)],
        );
    }

    // ── colour parsing ──────────────────────────────────────────────────────

    fn rgb(c: Color) -> (u8, u8, u8) {
        (
            (c.r * 255.0).round() as u8,
            (c.g * 255.0).round() as u8,
            (c.b * 255.0).round() as u8,
        )
    }

    #[test]
    fn hex_colours_parse_all_lengths() {
        assert_eq!(rgb(parse_color("#f00").unwrap()), (255, 0, 0));
        assert_eq!(rgb(parse_color("#00ff00").unwrap()), (0, 255, 0));
        let a = parse_color("#0000ff80").unwrap();
        assert_eq!(rgb(a), (0, 0, 255));
        assert!((a.a - 128.0 / 255.0).abs() < 0.01);
        // A short hex expands each nibble: #abc -> #aabbcc.
        assert_eq!(rgb(parse_color("#abc").unwrap()), (0xaa, 0xbb, 0xcc));
    }

    #[test]
    fn functional_and_named_colours_parse() {
        assert_eq!(rgb(parse_color("rgb(255, 128, 0)").unwrap()), (255, 128, 0));
        assert_eq!(rgb(parse_color("rgba(0 0 0 / 50%)").unwrap()), (0, 0, 0));
        assert_eq!(rgb(parse_color("hsl(120, 100%, 50%)").unwrap()), (0, 255, 0));
        assert!(parse_color("rebeccapurple").is_none()); // not in the table
        assert_eq!(rgb(parse_color("teal").unwrap()), (0, 128, 128));
    }

    #[test]
    fn invalid_hex_lengths_are_rejected() {
        assert!(hex_len("#12").is_none());
        assert!(hex_len("#12345").is_none());
        assert!(hex_len("#1234567").is_none());
        assert_eq!(hex_len("#123"), Some(4));
        assert_eq!(hex_len("#abcdef"), Some(7));
    }

    #[test]
    fn scanner_finds_first_colour_only() {
        let (start, len, col) = scan_color_token("bg is #ff0000 today").unwrap();
        assert_eq!(&"bg is #ff0000 today"[start..start + len], "#ff0000");
        assert_eq!(rgb(col), (255, 0, 0));
        // A non-hex hash run is left alone.
        assert!(scan_color_token("issue #12 and #zzz").is_none());
    }

    #[test]
    fn scanner_ignores_functional_mid_word() {
        // "worgb(...)" shouldn't trigger — rgb must start at a word boundary.
        assert!(scan_color_token("worgb(1,2,3)").is_none());
        assert!(scan_color_token("use rgb(1,2,3)").is_some());
    }

    // ── HTML ────────────────────────────────────────────────────────────────

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt; &#65; &#x42;"), "a & b <c> A B");
        assert_eq!(decode_entities("no entities"), "no entities");
        // An unrecognised & is kept literally.
        assert_eq!(decode_entities("Tom & Jerry"), "Tom & Jerry");
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(collapse_ws("a  \n  b\t c"), "a b c");
    }

    #[test]
    fn attrs_parse_quoted_and_bare() {
        let a = parse_attrs(r#"style="color: red" href='/x' checked"#);
        assert_eq!(a[0], ("style".into(), "color: red".into()));
        assert_eq!(a[1], ("href".into(), "/x".into()));
        assert_eq!(a[2], ("checked".into(), String::new()));
    }

    #[test]
    fn style_attr_sets_colour_and_weight() {
        let mut f = Frame::default();
        apply_style_attr(&mut f, "color:#ff0000; font-weight:bold; text-decoration:underline");
        assert_eq!(rgb(f.color.unwrap()), (255, 0, 0));
        assert!(f.bold);
        assert!(f.underline);
    }

    #[test]
    fn tokenizer_splits_tags_and_text() {
        let toks = tokenize("<p>hi <b>there</b></p>");
        assert_eq!(toks.len(), 6);
        assert!(matches!(&toks[0], Tok::Open(n, _) if n == "p"));
        assert!(matches!(&toks[1], Tok::Text(t) if t == "hi "));
        assert!(matches!(&toks[2], Tok::Open(n, _) if n == "b"));
        assert!(matches!(&toks[3], Tok::Text(t) if t == "there"));
        assert!(matches!(&toks[4], Tok::Close(n) if n == "b"));
        assert!(matches!(&toks[5], Tok::Close(n) if n == "p"));
    }

    #[test]
    fn renders_full_html_document_without_hanging() {
        // Multi-byte chars near where the colour scanner slices used to panic.
        let doc = r#"<!doctype html>
<html><head><title>t</title><style>.a{color:red}</style></head>
<body>
  <h1 style="color:#e33">Héllo — wörld</h1>
  <p>rgb(1,2,3) and hsl(200 50% 40%) and #abc inline.</p>
  <ul><li>one</li><li>two</li></ul>
  <blockquote>quoted &amp; <b>bold</b></blockquote>
  <pre>fn main() {}</pre>
  <hr>
  <a href="/x">link</a>
</body></html>"#;
        let _ = html(doc); // must return; a hang or panic fails the test
        let _ = markdown("colors: #ff0000, rgb(0,255,0), `#0000ff`, teal\n\n## Ünïcödé §");
    }

    #[test]
    fn func_len_survives_multibyte_after_prefix() {
        // "rgb(" then a multi-byte char — the old byte-slice boundary bug.
        assert!(scan_color_token("rgb(café)").is_none()); // not a valid colour, no panic
        assert!(scan_color_token("«rgb(1,2,3)»").is_some());
    }

    #[test]
    fn scanner_survives_emoji_after_r_or_h() {
        // The freeze: an emoji / accented char landing on the byte offset the old
        // `func_len` sliced at, right after an r/h. These must not panic.
        for s in ["r🎨🎨🎨", "h😀😀😀", "hℹ️ note", "raðar", "Header 🎨 #zzz"] {
            let _ = scan_color_token(s);
            let _ = html(s);
            let _ = markdown(s);
        }
    }

    #[test]
    fn html_output_contains_no_tags() {
        let doc = "<!doctype html><html><body>\n<h1 class=\"t\">Hi</h1>\n<p>a <b>b</b> c</p>\n</body></html>";
        let visible: String = tokenize(doc)
            .into_iter()
            .filter_map(|t| match t {
                Tok::Text(s) => Some(decode_entities(&s)),
                _ => None,
            })
            .collect();
        assert!(!visible.contains('<'), "tags leaked into visible text: {visible:?}");
        assert!(visible.contains("Hi") && visible.contains('b'));
    }

    #[test]
    fn tag_end_skips_quoted_angle_brackets() {
        // The '>' inside the title attribute must not end the tag early.
        let s = r#"<a title="a > b">x</a>"#;
        let end = find_tag_end(s, 0).unwrap();
        assert_eq!(&s[..=end], r#"<a title="a > b">"#);
        // And the whole thing tokenises to a single open/text/close, no leak.
        let visible: String = tokenize(s)
            .into_iter()
            .filter_map(|t| match t {
                Tok::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(visible, "x");
    }

    #[test]
    fn style_block_becomes_a_token_not_text() {
        let toks = tokenize("<style>td{color:red}</style><p>hi</p>");
        assert!(matches!(&toks[0], Tok::Style(css) if css.contains("color:red")));
        // The CSS never appears as visible text.
        assert!(!toks.iter().any(|t| matches!(t, Tok::Text(s) if s.contains("color"))));
    }

    #[test]
    fn selectors_parse_and_match() {
        let td = parse_simple("td").unwrap();
        assert!(td.matches("td", &[], None));
        assert!(!td.matches("th", &[], None));

        let cls = parse_simple(".hot").unwrap();
        assert!(cls.matches("span", &["hot"], None));
        assert!(!cls.matches("span", &["cold"], None));

        let compound = parse_simple("td.num").unwrap();
        assert!(compound.matches("td", &["num"], None));
        assert!(!compound.matches("td", &["other"], None));

        // Descendant combinator: only the rightmost compound is used.
        let desc = parse_simple("table td").unwrap();
        assert!(desc.matches("td", &[], None));

        let id = parse_simple("#total").unwrap();
        assert!(id.matches("div", &[], Some("total")));
        assert!(!id.matches("div", &[], Some("other")));
    }

    #[test]
    fn stylesheet_parses_rules_comments_and_at_rules() {
        let css = parse_css(
            "/* c */ td, th { color: #333; } .hot{color:red}\n@media x { p{color:blue} }",
        );
        // Two real rules; the @media block is skipped.
        assert_eq!(css.len(), 2);
        assert_eq!(css[0].selectors.len(), 2); // td, th
    }

    #[test]
    fn stylesheet_colour_reaches_a_matching_element() {
        // A td rule sets the colour; matched_css must surface it for a <td>.
        let mut b = Builder::default();
        b.css = parse_css("td { color: #ff0000; }");
        let decls = b.matched_css("td", &[], None);
        let mut f = Frame::default();
        apply_style_attr(&mut f, &decls);
        assert_eq!(rgb(f.color.unwrap()), (255, 0, 0));
        // A non-matching element gets nothing.
        assert!(b.matched_css("p", &[], None).is_empty());
    }

    #[test]
    fn table_and_style_document_renders() {
        let doc = r#"<style>td{color:#4af} th{color:red}</style>
<table>
  <tr><th>Name</th><th>Hex</th></tr>
  <tr><td>Sky</td><td>#4af</td></tr>
</table>"#;
        let _ = html(doc); // exercises table capture + stylesheet application
    }

    #[test]
    fn script_and_style_bodies_are_dropped() {
        let toks = tokenize("a<script>var x=1<2;</script>b<style>.c{}</style>c");
        let text: String = toks
            .iter()
            .filter_map(|t| match t {
                Tok::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "abc");
    }
}
