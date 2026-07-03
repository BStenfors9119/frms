use iced::{Alignment, Background, Border, Color, Element, Length, Pixels, Theme};
use iced::widget::{button, column, container, pick_list, progress_bar, row, text};
use iced::widget::text::LineHeight;

use crate::app::Message;
use crate::fonts::{ICON_FONT, RESET_FONT, UI_FONT};
use crate::stats::{ClaudeStats, ProcStats, RefreshInterval};
use crate::theme::ThemeColors;
use crate::ui::buttons;

pub fn view(
    plugin_visible:   bool,
    stats:            &ClaudeStats,
    procs:            &ProcStats,
    refresh_interval: RefreshInterval,
    collapsed:        bool,
    colors:           ThemeColors,
) -> Element<'static, Message> {
    // Header chrome reads as a distinct strip — surface tinted toward the
    // active text color for contrast in both light and dark modes.
    let header_bg = mix(colors.surface, colors.text, 0.10);
    let accent    = colors.tertiary;

    let collapse_btn = button(
        text(if collapsed { "▾" } else { "▴" })
            .font(ICON_FONT)
            .size(22)
            .line_height(LineHeight::Absolute(Pixels(16.0))),
    )
    .on_press(Message::HeaderToggleCollapse)
    .padding(buttons::PADDING)
    .style(buttons::secondary);

    if collapsed {
        return container(
            row![
                iced::widget::Space::with_width(Length::Fill),
                collapse_btn,
            ]
            .align_y(Alignment::Center)
            .padding([2, 12]),
        )
        .width(Length::Fill)
        .style(move |_theme: &Theme| container::Style {
            background: Some(Background::Color(header_bg)),
            border:     Border { color: accent, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        })
        .into();
    }

    // ── left column: usage bars + their action row ────────────────────────────
    let session_bar = usage_bar(
        format!("Session (5h)  {:.0}%", stats.five_hour_pct),
        stats.five_hour_resets.clone(),
        stats.five_hour_fraction(),
        bar_color(stats.five_hour_fraction()),
    );
    let week_bar = usage_bar(
        format!("Week (7d)  {:.0}%", stats.seven_day_pct),
        stats.seven_day_resets.clone(),
        stats.seven_day_fraction(),
        bar_color(stats.seven_day_fraction()),
    );

    let refresh_btn = button(text("Refresh").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::StatsRefresh)
        .padding(buttons::PADDING)
        .style(buttons::secondary);

    let interval_picker = pick_list(
        &RefreshInterval::ALL[..],
        Some(refresh_interval),
        Message::StatsRefreshIntervalChanged,
    )
    .text_size(12)
    .font(UI_FONT);

    let left_col = column![
        row![session_bar, week_bar].spacing(16),
        proc_tracker(procs),
        row![refresh_btn, interval_picker]
            .spacing(8)
            .align_y(Alignment::Center),
    ]
    .spacing(8)
    .width(Length::Fill);

    // ── right column: action buttons ──────────────────────────────────────────
    // One consolidated toolbox button — opens the right-docked panel whose
    // own tab control switches between DB / Profile / Notes / Terminals.
    let plugin_btn = button(
        text("\u{1F6E0}") // 🛠 hammer & wrench — the toolbox panel toggle
            .font(ICON_FONT)
            .size(22)
            .line_height(LineHeight::Absolute(Pixels(16.0))),
    )
    .on_press(Message::PluginPanelToggled)
    .padding(buttons::PADDING)
    .style(buttons::toggle(plugin_visible));

    let quit_btn = button(text("Quit").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::Quit)
        .padding(buttons::PADDING)
        .style(buttons::danger);

    let right_col = row![
        plugin_btn,
        quit_btn,
        collapse_btn,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    container(
        row![left_col, right_col]
            .align_y(Alignment::Center)
            .padding([8, 16])
            .spacing(20),
    )
    .width(Length::Fill)
    .style(move |_theme: &Theme| container::Style {
        background: Some(Background::Color(header_bg)),
        border:     Border { color: accent, width: 1.0, radius: 0.0.into() },
        ..Default::default()
    })
    .into()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn usage_bar(
    label:    String,
    resets:   Option<String>,
    fraction: f32,
    color:    Color,
) -> Element<'static, Message> {
    // The ↺ glyph lives in its own micro-font (RESET_FONT) — Liberation Sans
    // tofus on it — so the reset time renders as separate text widgets.
    let mut label_row = row![text(label).font(UI_FONT).size(13)]
        .spacing(6)
        .align_y(Alignment::Center);
    if let Some(r) = resets {
        label_row = label_row
            .push(text("↺").font(RESET_FONT).size(13))
            .push(text(r).font(UI_FONT).size(13));
    }
    column![
        label_row,
        progress_bar(0.0..=1.0, fraction)
            .width(Length::Fill)
            .height(Length::Fixed(14.0))
            .style(move |_theme: &Theme| progress_bar::Style {
                background: Background::Color(Color::from_rgb8(45, 45, 55)),
                bar:        Background::Color(color),
                border:     Border { color: Color::TRANSPARENT, width: 0.0, radius: 4.0.into() },
            }),
    ]
    .width(Length::Fill)
    .spacing(3)
    .into()
}

/// Compact summary of every `claude` process on the machine — count, total
/// resident memory (with its share of system RAM), and aggregate CPU — plus a
/// bar showing the memory share, matching the usage bars above it.
fn proc_tracker(procs: &ProcStats) -> Element<'static, Message> {
    let frac  = procs.mem_fraction();
    let label = format!(
        "Claude procs  {}   ·   Mem {} ({:.0}%)   ·   CPU {:.0}%",
        procs.count(),
        fmt_mem(procs.total_rss_kb()),
        frac * 100.0,
        procs.total_cpu_pct(),
    );

    column![
        text(label).font(UI_FONT).size(13),
        progress_bar(0.0..=1.0, frac)
            .width(Length::Fill)
            .height(Length::Fixed(14.0))
            .style(move |_theme: &Theme| progress_bar::Style {
                background: Background::Color(Color::from_rgb8(45, 45, 55)),
                bar:        Background::Color(bar_color(frac)),
                border:     Border { color: Color::TRANSPARENT, width: 0.0, radius: 4.0.into() },
            }),
    ]
    .width(Length::Fill)
    .spacing(3)
    .into()
}

/// Render a KiB count as a friendly "512 MB" / "1.2 GB" string.
fn fmt_mem(kb: u64) -> String {
    let mb = kb as f64 / 1024.0;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

fn bar_color(fraction: f32) -> Color {
    if fraction < 0.6 {
        Color::from_rgb8(56, 189, 96)
    } else if fraction < 0.85 {
        Color::from_rgb8(234, 179, 8)
    } else {
        Color::from_rgb8(239, 68, 68)
    }
}

/// Linear blend `t` of the way from `a` to `b`. `t = 0.0` → `a`; `t = 1.0` → `b`.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: 1.0,
    }
}
