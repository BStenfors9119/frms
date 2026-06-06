//! Database plugin body — rendered inside the right-side plugin panel.

use iced::widget::{
    button, column, combo_box, container, pick_list, row, scrollable, text, text_input, Column,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::app::Message;
use crate::db::{DbEngine, DbProvider};
use crate::db_panel::{ConnState, DbPanel};
use crate::fonts::UI_FONT;
use crate::ui::buttons;

pub const FIELD_HOST:     &str = "db-host";
pub const FIELD_PORT:     &str = "db-port";
pub const FIELD_DATABASE: &str = "db-database";
pub const FIELD_USER:     &str = "db-user";
pub const FIELD_PASSWORD: &str = "db-password";

/// DB plugin body (no chrome — caller wraps it in the plugin panel).
pub fn body(panel: &DbPanel) -> Element<'_, Message> {
    let content: Element<'_, Message> = match &panel.conn_state {
        ConnState::Disconnected | ConnState::Failed(_) => connection_form(panel),
        ConnState::Connecting => container(text("Connecting…").size(13))
            .padding(8)
            .into(),
        ConnState::Connected => connected_view(panel),
    };

    column![content]
        .spacing(10)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ── disconnected: connection form ─────────────────────────────────────────────

fn connection_form(panel: &DbPanel) -> Element<'_, Message> {
    let cfg = &panel.config;

    let engine_picker = pick_list(
        &DbEngine::ALL[..],
        Some(cfg.engine),
        Message::DbEngineChanged,
    )
    .text_size(12)
    .font(UI_FONT);

    let provider_picker = pick_list(
        &DbProvider::ALL[..],
        Some(cfg.provider),
        Message::DbProviderChanged,
    )
    .text_size(12)
    .font(UI_FONT);

    let mut col = Column::new()
        .spacing(6)
        .width(Length::Fill)
        .push(field_label("Engine"))
        .push(engine_picker)
        .push(field_label("Provider"))
        .push(provider_picker)
        .push(field_label("Host"))
        .push(
            combo_box(&panel.host_combo, "localhost", Some(&cfg.host), Message::DbHostEdited)
                .on_input(Message::DbHostEdited)
                .font(UI_FONT)
                .size(12.0),
        )
        .push(field_label("Port"))
        .push(
            combo_box(&panel.port_combo, "5432", Some(&panel.port_str), Message::DbPortEdited)
                .on_input(Message::DbPortEdited)
                .font(UI_FONT)
                .size(12.0),
        )
        .push(field_label("Database"))
        .push(
            combo_box(&panel.database_combo, "postgres", Some(&cfg.database), Message::DbDatabaseEdited)
                .on_input(Message::DbDatabaseEdited)
                .font(UI_FONT)
                .size(12.0),
        )
        .push(field_label("User"))
        .push(
            combo_box(&panel.user_combo, "postgres", Some(&cfg.user), Message::DbUserEdited)
                .on_input(Message::DbUserEdited)
                .font(UI_FONT)
                .size(12.0),
        )
        .push(field_label("Password"))
        .push(
            text_input("", &cfg.password)
                .id(text_input::Id::new(FIELD_PASSWORD))
                .on_input(Message::DbPasswordEdited)
                .on_submit(Message::DbFieldSubmitted(FIELD_PASSWORD))
                .secure(true)
                .size(12),
        )
        .push(
            button(text("Connect").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::DbConnect)
                .padding(buttons::PADDING)
                .style(buttons::primary),
        );

    if let ConnState::Failed(err) = &panel.conn_state {
        col = col.push(
            text(format!("Error: {err}"))
                .size(11)
                .color(Color::from_rgb8(239, 68, 68)),
        );
    }

    col.into()
}

fn field_label(label: &str) -> Element<'static, Message> {
    text(label.to_string()).font(UI_FONT).size(11).into()
}

// ── connected: tables / columns / query / results ─────────────────────────────

fn connected_view(panel: &DbPanel) -> Element<'_, Message> {
    let connected_bar = row![
        text(format!(
            "Connected: {}@{}:{}/{}",
            panel.config.user, panel.config.host, panel.config.port, panel.config.database
        ))
        .size(11),
        button(text("Disconnect").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbDisconnect)
            .padding(buttons::PADDING)
            .style(buttons::secondary),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let tables_section = section_title("Tables");
    let mut tables_col = Column::new().spacing(2).width(Length::Fill);

    if panel.tables.is_empty() {
        tables_col = tables_col.push(text("(no tables)").size(11));
    } else {
        for t in &panel.tables {
            let key      = t.key();
            let selected = panel.selected_tables.contains(&key);
            let marker   = if selected { "[x]" } else { "[ ]" };
            let label    = format!("{marker} {key}");

            let toggle = button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::DbTableToggled(key.clone()))
                .padding(buttons::PADDING)
                .style(if selected { buttons::primary } else { buttons::secondary })
                .width(Length::Fill);

            tables_col = tables_col.push(toggle);

            if selected {
                tables_col = tables_col.push(columns_for(panel, &key));
            }
        }
    }

    let scroll_inner = column![
        connected_bar,
        tables_section,
        tables_col,
    ]
    .spacing(10)
    .width(Length::Fill);

    scrollable(scroll_inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn columns_for<'a>(panel: &'a DbPanel, table_key: &str) -> Element<'a, Message> {
    if panel.loading_columns.contains(table_key) {
        return container(text("loading columns…").size(11))
            .padding([2, 18])
            .into();
    }

    let cols = match panel.columns_by_table.get(table_key) {
        Some(c) => c,
        None    => return container(text("(no columns cached)").size(11))
            .padding([2, 18])
            .into(),
    };

    let selected = panel.selected_columns.get(table_key);
    let mut col_col = Column::new().spacing(1).padding([2, 18]).width(Length::Fill);

    for c in cols {
        let is_selected = selected.map(|s| s.contains(c)).unwrap_or(false);
        let marker = if is_selected { "[x]" } else { "[ ]" };
        let label  = format!("{marker} {c}");
        col_col = col_col.push(
            button(text(label).font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::DbColumnToggled(table_key.to_string(), c.clone()))
                .padding(buttons::PADDING)
                .style(if is_selected { buttons::primary } else { buttons::secondary })
                .width(Length::Fill),
        );
    }
    col_col.into()
}

fn section_title(label: &str) -> Element<'static, Message> {
    container(text(label.to_string()).font(UI_FONT).size(12))
        .padding([4, 0])
        .into()
}

pub fn query_tab(panel: &DbPanel) -> Element<'_, Message> {
    if !matches!(panel.conn_state, ConnState::Connected) {
        return container(
            text("Connect to a database using the panel on the right to run queries.")
                .font(UI_FONT)
                .size(12),
        )
        .padding(16)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(query_bar_bg)
        .into();
    }

    let primary_actions = row![
        button(text("Build SELECT").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbBuildQuery)
            .padding(buttons::PADDING)
            .style(buttons::secondary),
        button(
            text(if panel.query_running { "Running…" } else { "Run Query" })
                .font(UI_FONT)
                .size(buttons::TEXT_SIZE)
        )
        .on_press_maybe(if panel.query_running { None } else { Some(Message::DbRunQuery) })
        .padding(buttons::PADDING)
        .style(buttons::primary),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let reset_actions = row![
        button(text("Reset Query").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbResetQuery)
            .padding(buttons::PADDING)
            .style(buttons::danger),
        button(text("Reset Tables").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbResetTables)
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let query_row = row![
        text_input("SELECT … FROM …", &panel.query)
            .on_input(Message::DbQueryEdited)
            .size(12)
            .width(Length::Fill),
        primary_actions,
        reset_actions,
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut body = Column::new()
        .push(query_row)
        .spacing(8)
        .width(Length::Fill);
    if let Some(results) = results_body(panel) {
        body = body.push(results);
    }

    container(body)
        .padding([8, 10])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(query_bar_bg)
        .into()
}

fn results_body(panel: &DbPanel) -> Option<Element<'_, Message>> {
    use iced::widget::Row;
    use iced::Font;

    if let Some(err) = &panel.query_error {
        return Some(
            container(
                text(format!("Query error: {err}"))
                    .size(11)
                    .color(Color::from_rgb8(239, 68, 68)),
            )
            .padding(6)
            .into(),
        );
    }

    let result = panel.query_result.as_ref()?;

    if result.columns.is_empty() {
        return Some(container(text("Query returned no columns.").size(11)).padding(6).into());
    }

    let mut header_row = Row::new().spacing(6);
    for c in &result.columns {
        header_row = header_row.push(
            container(text(c.clone()).font(UI_FONT).size(11))
                .width(Length::Fixed(120.0)),
        );
    }
    let mut grid = Column::new().spacing(2).push(header_row);

    for row_vals in &result.rows {
        let mut r = Row::new().spacing(6);
        for v in row_vals {
            let display = if v.len() > 60 { format!("{}…", &v[..60]) } else { v.clone() };
            r = r.push(
                container(text(display).font(Font::MONOSPACE).size(11))
                    .width(Length::Fixed(120.0)),
            );
        }
        grid = grid.push(r);
    }

    Some(
        column![
            section_title(&format!("Results ({})", result.rows.len())),
            scrollable(grid).width(Length::Fill).height(Length::Fixed(280.0)),
        ]
        .spacing(4)
        .into(),
    )
}

// ── styling ───────────────────────────────────────────────────────────────────

fn query_bar_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(32, 32, 42))),
        border: Border { color: Color::from_rgb8(55, 55, 68), width: 0.0, radius: 0.0.into() },
        ..Default::default()
    }
}

