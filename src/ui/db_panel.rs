//! Database plugin body — rendered inside the right-side plugin panel.

use iced::widget::{
    button, column, combo_box, container, mouse_area, pick_list, row, scrollable, text, text_editor,
    text_input, Column, Row, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Theme};

use crate::app::Message;
use crate::db::{DbEngine, DbProvider, QueryResult, RoutineRef};
use crate::db_panel::{ColRef, ConnState, DbPanel, Filter, FilterOp};
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::ui::buttons;

pub const FIELD_HOST:     &str = "db-host";
pub const FIELD_PORT:     &str = "db-port";
pub const FIELD_DATABASE: &str = "db-database";
pub const FIELD_USER:     &str = "db-user";
pub const FIELD_PASSWORD: &str = "db-password";

// ── disconnected: connection form ─────────────────────────────────────────────

fn connection_form(panel: &DbPanel) -> Element<'_, Message> {
    let cfg = &panel.config;

    let engine_field = labeled_field(
        "Engine",
        pick_list(&DbEngine::ALL[..], Some(cfg.engine), Message::DbEngineChanged)
            .text_size(12)
            .font(UI_FONT)
            .width(Length::Fill)
            .into(),
    );
    let provider_field = labeled_field(
        "Provider",
        pick_list(&DbProvider::ALL[..], Some(cfg.provider), Message::DbProviderChanged)
            .text_size(12)
            .font(UI_FONT)
            .width(Length::Fill)
            .into(),
    );
    let host_field = labeled_field(
        "Host",
        combo_box(&panel.host_combo, "localhost", Some(&cfg.host), Message::DbHostEdited)
            .on_input(Message::DbHostEdited)
            .font(UI_FONT)
            .size(12.0)
            .width(Length::Fill)
            .into(),
    );
    let port_field = labeled_field(
        "Port",
        combo_box(&panel.port_combo, "5432", Some(&panel.port_str), Message::DbPortEdited)
            .on_input(Message::DbPortEdited)
            .font(UI_FONT)
            .size(12.0)
            .width(Length::Fill)
            .into(),
    );
    let database_field = labeled_field(
        "Database",
        combo_box(&panel.database_combo, "postgres", Some(&cfg.database), Message::DbDatabaseEdited)
            .on_input(Message::DbDatabaseEdited)
            .font(UI_FONT)
            .size(12.0)
            .width(Length::Fill)
            .into(),
    );
    let user_field = labeled_field(
        "User",
        combo_box(&panel.user_combo, "postgres", Some(&cfg.user), Message::DbUserEdited)
            .on_input(Message::DbUserEdited)
            .font(UI_FONT)
            .size(12.0)
            .width(Length::Fill)
            .into(),
    );
    let password_field = labeled_field(
        "Password",
        text_input("", &cfg.password)
            .id(text_input::Id::new(FIELD_PASSWORD))
            .on_input(Message::DbPasswordEdited)
            .on_submit(Message::DbFieldSubmitted(FIELD_PASSWORD))
            .secure(true)
            .size(12)
            .width(Length::Fill)
            .into(),
    );
    // Connect sits in the 4th cell of the second row, bottom-aligned with the
    // inputs via an empty spacer label.
    let connect_field = labeled_field(
        "",
        button(text("Connect").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbConnect)
            .padding(buttons::PADDING)
            .style(buttons::primary)
            .width(Length::Fill)
            .into(),
    );

    let row1 = row![engine_field, provider_field, host_field, port_field]
        .spacing(8)
        .width(Length::Fill);
    let row2 = row![database_field, user_field, password_field, connect_field]
        .spacing(8)
        .width(Length::Fill);

    let mut col = Column::new().spacing(8).width(Length::Fill).push(row1).push(row2);

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

/// A label stacked over its control, sized to share a row equally with sibling
/// fields — the building block of the horizontal connection form.
fn labeled_field<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    column![field_label(label), control]
        .spacing(2)
        .width(Length::Fill)
        .into()
}

// ── connected: builder grid (2 rows × 3 cols) + results ───────────────────────

/// Compact connected banner shown inside the (collapsible) Connection section.
fn connected_view(panel: &DbPanel) -> Element<'_, Message> {
    row![
        text(format!(
            "Connected: {}@{}:{}/{}",
            panel.config.user, panel.config.host, panel.config.port, panel.config.database
        ))
        .font(UI_FONT)
        .size(11),
        Space::with_width(Length::Fill),
        button(text("Disconnect").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbDisconnect)
            .padding(buttons::PADDING)
            .style(buttons::secondary),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn section_title(label: &str) -> Element<'static, Message> {
    container(text(label.to_string()).font(UI_FONT).size(12))
        .padding([4, 0])
        .into()
}

/// The full database tool, rendered in the per-project Database center tab:
/// a collapsible Connection section on top, then the builder grid (Row 1:
/// tables | fields | query) over the results table (Row 2).
pub fn query_tab(panel: &DbPanel) -> Element<'_, Message> {
    let mut body = Column::new()
        .spacing(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .push(connection_section(panel));

    if matches!(panel.conn_state, ConnState::Connected) {
        body = body.push(builder_and_results(panel));
    }

    container(body)
        .padding([8, 10])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(query_bar_bg)
        .into()
}

/// Collapsible "Connection" section: a clickable header showing the current
/// connection status, with the connection form (disconnected) or a compact
/// connected banner tucked beneath it when expanded.
fn connection_section(panel: &DbPanel) -> Element<'_, Message> {
    let collapsed = panel.form_collapsed;
    let chevron = if collapsed { "\u{25B8}" } else { "\u{25BE}" }; // ▸ / ▾

    let status = match &panel.conn_state {
        ConnState::Disconnected => "Disconnected".to_string(),
        ConnState::Connecting   => "Connecting…".to_string(),
        ConnState::Connected    => format!(
            "{}@{}:{}/{}",
            panel.config.user, panel.config.host, panel.config.port, panel.config.database
        ),
        ConnState::Failed(_)    => "Connection failed".to_string(),
    };
    let status_color = match &panel.conn_state {
        ConnState::Connected => Color::from_rgb8(74, 222, 128),
        ConnState::Failed(_) => Color::from_rgb8(239, 68, 68),
        _                    => Color::from_rgb8(160, 160, 172),
    };

    let header = button(
        row![
            text(chevron).font(ICON_FONT).size(11),
            text("Connection").font(UI_FONT).size(12),
            text(status).font(UI_FONT).size(11).color(status_color),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .on_press(Message::DbFormToggleCollapsed)
    .padding(buttons::PADDING)
    .style(buttons::secondary)
    .width(Length::Fill);

    let mut col = Column::new().spacing(8).width(Length::Fill).push(header);

    if !collapsed {
        let inner: Element<'_, Message> = match &panel.conn_state {
            ConnState::Disconnected | ConnState::Failed(_) => connection_form(panel),
            ConnState::Connecting => container(text("Connecting…").size(13)).padding(8).into(),
            ConnState::Connected  => connected_view(panel),
        };
        col = col.push(container(inner).padding([0, 4]));
    }

    col.into()
}

/// Builder grid (Row 1, collapsible) stacked over the results table (Row 2).
/// Row 1 has two modes: the visual query builder, or a free-hand SQL editor that
/// runs any statement verbatim — switched with the Builder / SQL segmented tabs.
fn builder_and_results(panel: &DbPanel) -> Element<'_, Message> {
    let collapsed = panel.row1_collapsed;
    let chevron = if collapsed { "\u{25B8}" } else { "\u{25BE}" }; // ▸ / ▾

    let collapse_btn = button(text(chevron).font(ICON_FONT).size(11))
        .on_press(Message::DbRow1ToggleCollapsed)
        .padding(buttons::PADDING)
        .style(buttons::secondary);

    let mode_builder = button(text("Builder").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::DbSetSqlMode(false))
        .padding(buttons::PADDING)
        .style(buttons::toggle(!panel.sql_mode));
    let mode_sql = button(text("SQL").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::DbSetSqlMode(true))
        .padding(buttons::PADDING)
        .style(buttons::toggle(panel.sql_mode));

    let toggle = row![collapse_btn, mode_builder, mode_sql]
        .spacing(6)
        .align_y(Alignment::Center);

    let mut col = Column::new()
        .spacing(8)
        .width(Length::Fill)
        .height(Length::Fill)
        .push(toggle);

    if !collapsed {
        let row1: Element<'_, Message> = if panel.sql_mode {
            sql_editor_row(panel)
        } else {
            builder_row(panel)
        };
        col = col.push(
            container(row1)
                .width(Length::Fill)
                .height(Length::FillPortion(2)),
        );
    }

    // Row 2 spans the full width; it takes all remaining height (and the whole
    // area when the builder is collapsed).
    let results_height = if collapsed { Length::Fill } else { Length::FillPortion(3) };
    col = col.push(
        container(results_section(panel))
            .width(Length::Fill)
            .height(results_height),
    );

    col.into()
}

/// Row 1 — five columns: tables | fields | filters (WHERE) | group by | query.
fn builder_row(panel: &DbPanel) -> Element<'_, Message> {
    row![
        col_panel("Tables",   tables_list(panel)),
        col_panel("Fields",   fields_list(panel)),
        col_panel("Filters",  filters_col(panel)),
        col_panel("Group by", group_col(panel)),
        col_panel("Query",    query_col(panel)),
    ]
    .spacing(8)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Row 1 (SQL mode) — a free-hand editor for any statement. The user types or
/// pastes SQL of any kind (SELECT, INSERT, UPDATE, DELETE, DDL …) and Run sends
/// it to the connection verbatim; the outcome lands in the shared results row
/// below (a table for row-returning statements, an "N rows affected" line
/// otherwise).
fn sql_editor_row(panel: &DbPanel) -> Element<'_, Message> {
    let run = button(
        text(if panel.query_running { "Running…" } else { "Run" })
            .font(UI_FONT)
            .size(buttons::TEXT_SIZE),
    )
    .on_press_maybe((!panel.query_running).then_some(Message::DbRunSql))
    .padding(buttons::PADDING)
    .style(buttons::primary);

    let hint = text(
        "Type or paste any statement — SELECT, INSERT, UPDATE, DELETE, DDL. \
         Runs as-is against the connection.",
    )
    .font(UI_FONT)
    .size(11)
    .color(Color::from_rgb8(150, 150, 165));

    let actions = row![run, hint]
        .spacing(10)
        .align_y(Alignment::Center);

    let editor = text_editor(&panel.sql_input)
        .on_action(Message::DbSqlAction)
        .font(Font::MONOSPACE)
        .size(12)
        .height(Length::Fill);

    let inner = column![actions, editor]
        .spacing(6)
        .width(Length::Fill)
        .height(Length::Fill);

    container(inner)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(6)
        .style(panel_box)
        .into()
}

/// One bordered column of the builder grid: a title over its (already
/// scroll-managed) body, each taking an equal third of the width.
fn col_panel<'a>(title: &str, body: Element<'a, Message>) -> Element<'a, Message> {
    container(
        column![section_title(title), body]
            .spacing(4)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::FillPortion(1))
    .height(Length::Fill)
    .padding(6)
    .style(panel_box)
    .into()
}

/// Col 1 — the table list. The first table picked is the base of the `FROM`;
/// each additional table is joined on. With no foreign keys to drive an
/// automatic join, every table stays selectable and the join condition is set
/// in the "Joins" section below (pre-filled from a foreign key when one exists).
fn tables_list(panel: &DbPanel) -> Element<'_, Message> {
    // Case-insensitive substring filter shared by the Tables and Objects groups.
    let needle = panel.object_filter.trim().to_ascii_lowercase();
    let matches = |hay: &str| needle.is_empty() || hay.to_ascii_lowercase().contains(&needle);

    let search = text_input("Search tables / objects…", &panel.object_filter)
        .on_input(Message::DbObjectFilterChanged)
        .font(UI_FONT)
        .size(11)
        .width(Length::Fill);

    let mut col = Column::new().spacing(2).width(Length::Fill);

    // ── Tables (collapsible) ────────────────────────────────────────────────
    col = col.push(group_header(
        panel.tables_collapsed,
        "Tables",
        Message::DbTablesToggleCollapsed,
    ));
    if !panel.tables_collapsed {
        let shown: Vec<_> = panel.tables.iter().filter(|t| matches(&t.key())).collect();
        if shown.is_empty() {
            let msg = if panel.tables.is_empty() { "(no tables)" } else { "(no matches)" };
            col = col.push(container(text(msg).font(UI_FONT).size(11)).padding([1, 8]));
        } else {
            for t in shown {
                let key      = t.key();
                let selected = panel.selected_tables.iter().any(|k| *k == key);
                let marker   = if selected { "[x]" } else { "[ ]" };

                col = col.push(
                    button(text(format!("{marker} {key}")).font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::DbTableToggled(key.clone()))
                        .padding(buttons::PADDING)
                        .style(if selected { buttons::primary } else { buttons::secondary })
                        .width(Length::Fill),
                );
            }
        }
    }

    // Joins — one ON editor per non-base table (everything after the first).
    if panel.selected_tables.len() > 1 {
        col = col
            .push(Space::with_height(8))
            .push(
                text("Joins")
                    .font(UI_FONT)
                    .size(11)
                    .color(Color::from_rgb8(150, 150, 165)),
            );

        for idx in 1..panel.selected_tables.len() {
            col = col.push(join_cond_row(panel, idx));
        }
    }

    // ── Objects (collapsible) — stored procedures and functions. Selecting one
    // opens its source in the results row (rather than toggling it into the
    // query like a table).
    if !panel.routines.is_empty() {
        col = col.push(Space::with_height(8)).push(group_header(
            panel.objects_collapsed,
            "Objects",
            Message::DbObjectsToggleCollapsed,
        ));

        if !panel.objects_collapsed {
            let shown: Vec<_> = panel
                .routines
                .iter()
                .filter(|r| matches(&r.key()) || matches(r.kind.tag()))
                .collect();
            if shown.is_empty() {
                col = col.push(container(text("(no matches)").font(UI_FONT).size(11)).padding([1, 8]));
            } else {
                for r in shown {
                    let selected = panel.selected_routine.as_ref() == Some(r);
                    col = col.push(
                        button(
                            text(format!("{}  {}", r.kind.tag(), r.key()))
                                .font(UI_FONT)
                                .size(buttons::TEXT_SIZE),
                        )
                        .on_press(Message::DbRoutineSelected(r.clone()))
                        .padding(buttons::PADDING)
                        .style(if selected { buttons::primary } else { buttons::secondary })
                        .width(Length::Fill),
                    );
                }
            }
        }
    }

    column![search, scrollable(col).width(Length::Fill).height(Length::Fill)]
        .spacing(6)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A clickable group header (Tables / Objects) in the table column: a chevron
/// reflecting the collapsed state beside the label, toggling on press.
fn group_header(collapsed: bool, label: &str, msg: Message) -> Element<'static, Message> {
    let chevron = if collapsed { "\u{25B8}" } else { "\u{25BE}" }; // ▸ / ▾
    button(
        row![
            text(chevron).font(ICON_FONT).size(11),
            text(label.to_string()).font(UI_FONT).size(11),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(msg)
    .padding(buttons::PADDING)
    .style(buttons::secondary)
    .width(Length::Fill)
    .into()
}

/// One JOIN ON editor for the table at `idx` in `selected_tables` (idx >= 1).
/// Picks the left table (any table included before this one), the left column,
/// and this table's column to match on. Pre-filled from a foreign key when one
/// links them; otherwise blank for the user to complete. While incomplete the
/// generated SQL falls back to a CROSS JOIN, which the Query preview shows.
fn join_cond_row(panel: &DbPanel, idx: usize) -> Element<'_, Message> {
    let joined_key = panel.selected_tables[idx].clone();
    let short = joined_key.rsplit('.').next().unwrap_or(&joined_key).to_string();
    let cond = panel.joins.get(&joined_key).cloned().unwrap_or_default();

    // Left side may be any table included before this one.
    let left_tables: Vec<String> = panel.selected_tables[..idx].to_vec();
    let left_sel = (!cond.left_table.is_empty()).then(|| cond.left_table.clone());

    let jk = joined_key.clone();
    let left_table_pick = pick_list(left_tables, left_sel, move |t| {
        Message::DbJoinLeftTableChanged(jk.clone(), t)
    })
    .placeholder("left table")
    .text_size(11)
    .font(UI_FONT)
    .width(Length::Fill);

    // Columns of the chosen left table (empty until one is picked / loaded).
    let left_cols: Vec<String> = panel
        .columns_by_table
        .get(&cond.left_table)
        .cloned()
        .unwrap_or_default();
    let left_col_sel = (!cond.left_col.is_empty()).then(|| cond.left_col.clone());
    let jk = joined_key.clone();
    let left_col_pick = pick_list(left_cols, left_col_sel, move |c| {
        Message::DbJoinLeftColChanged(jk.clone(), c)
    })
    .placeholder("left column")
    .text_size(11)
    .font(UI_FONT)
    .width(Length::Fill);

    // Columns of this (joined) table.
    let right_cols: Vec<String> = panel
        .columns_by_table
        .get(&joined_key)
        .cloned()
        .unwrap_or_default();
    let right_col_sel = (!cond.right_col.is_empty()).then(|| cond.right_col.clone());
    let jk = joined_key.clone();
    let right_col_pick = pick_list(right_cols, right_col_sel, move |c| {
        Message::DbJoinRightColChanged(jk.clone(), c)
    })
    .placeholder(format!("{short} column"))
    .text_size(11)
    .font(UI_FONT)
    .width(Length::Fill);

    let inner = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(text(format!("join {short} on")).font(UI_FONT).size(11))
        .push(left_table_pick)
        .push(left_col_pick)
        .push(
            row![text("=").font(UI_FONT).size(12), right_col_pick]
                .spacing(6)
                .align_y(Alignment::Center),
        );

    container(inner)
        .padding([4, 6])
        .width(Length::Fill)
        .style(field_row_bg)
        .into()
}

/// Col 2 — fields of the selected tables, each a checkbox toggling membership
/// in the result set.
fn fields_list(panel: &DbPanel) -> Element<'_, Message> {
    if panel.selected_tables.is_empty() {
        return text("Select a table to list its fields.")
            .font(UI_FONT)
            .size(11)
            .into();
    }

    // "All fields" toggles every available column on/off at once.
    let all_selected = panel.all_fields_selected();
    let all_marker   = if all_selected { "[x]" } else { "[ ]" };
    let all_btn = button(
        text(format!("{all_marker} All fields")).font(UI_FONT).size(buttons::TEXT_SIZE),
    )
    .on_press(Message::DbFieldsSelectAll(!all_selected))
    .padding(buttons::PADDING)
    .style(if all_selected { buttons::primary } else { buttons::secondary })
    .width(Length::Fill);

    let mut col = Column::new().spacing(2).width(Length::Fill);
    for table_key in &panel.selected_tables {
        col = col.push(
            text(table_key.clone()).font(UI_FONT).size(11).color(Color::from_rgb8(150, 150, 165)),
        );

        if panel.loading_columns.contains(table_key) {
            col = col.push(container(text("loading…").size(11)).padding([1, 12]));
            continue;
        }
        let Some(cols) = panel.columns_by_table.get(table_key) else {
            col = col.push(container(text("(no columns)").size(11)).padding([1, 12]));
            continue;
        };

        for c in cols {
            let is_selected = panel.is_col_selected(table_key, c);
            let marker = if is_selected { "[x]" } else { "[ ]" };
            col = col.push(
                button(text(format!("{marker} {c}")).font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::DbColumnToggled(table_key.clone(), c.clone()))
                    .padding(buttons::PADDING)
                    .style(if is_selected { buttons::primary } else { buttons::secondary })
                    .width(Length::Fill),
            );
        }
    }

    column![all_btn, scrollable(col).width(Length::Fill).height(Length::Fill)]
        .spacing(4)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Col 3 — WHERE-clause builder. Each row picks a column and operator, plus a
/// value for value-taking operators; rows after the first carry an AND/OR
/// toggle. "+ Add filter" appends a row pre-filled with the first available
/// column, so the generated SQL updates the moment a value is typed.
fn filters_col(panel: &DbPanel) -> Element<'_, Message> {
    let avail = panel.available_cols();

    let add_btn = button(text("+ Add filter").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press_maybe((!avail.is_empty()).then_some(Message::DbFilterAdd))
        .padding(buttons::PADDING)
        .style(buttons::primary)
        .width(Length::Fill);

    let mut col = Column::new().spacing(6).width(Length::Fill);
    if panel.filters.is_empty() {
        col = col.push(
            text("No filters — every row is returned. Add one to build a WHERE clause.")
                .font(UI_FONT)
                .size(11),
        );
    } else {
        for (i, f) in panel.filters.iter().enumerate() {
            col = col.push(filter_row(i, f, &avail));
        }
    }

    // Row cap. Blank = no LIMIT (all rows); a number caps the result set. Pinned
    // below the filter list so it's always reachable regardless of filter count.
    let limit = row![
        text("Limit").font(UI_FONT).size(11),
        text_input("all rows", &panel.limit_input)
            .on_input(Message::DbLimitChanged)
            .font(UI_FONT)
            .size(11)
            .width(Length::Fixed(90.0)),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    column![
        add_btn,
        scrollable(col).width(Length::Fill).height(Length::Fill),
        limit,
    ]
    .spacing(6)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// One WHERE-clause condition: an AND/OR connector (or a "where" label on the
/// first row) and remove button up top, then the column / operator pickers and
/// the value input stacked beneath — kept vertical to fit the narrow column.
fn filter_row<'a>(i: usize, f: &'a Filter, avail: &[ColRef]) -> Element<'a, Message> {
    let connector: Element<'_, Message> = if i == 0 {
        text("where").font(UI_FONT).size(11).into()
    } else {
        button(text(f.conj.sql()).font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbFilterConjToggled(i))
            .padding(buttons::PADDING)
            .style(buttons::secondary)
            .into()
    };

    let remove = button(text("\u{2715}").font(ICON_FONT).size(11))
        .on_press(Message::DbFilterRemoved(i))
        .padding([2, 6])
        .style(buttons::danger);

    let selected_col = (!f.column.is_empty()).then(|| ColRef {
        table:  f.table.clone(),
        column: f.column.clone(),
    });
    let column_pick = pick_list(
        avail.to_vec(),
        selected_col,
        move |c| Message::DbFilterColumnChanged(i, c),
    )
    .placeholder("column")
    .text_size(11)
    .font(UI_FONT)
    .width(Length::Fill);

    let op_pick = pick_list(
        &FilterOp::ALL[..],
        Some(f.op),
        move |op| Message::DbFilterOpChanged(i, op),
    )
    .text_size(11)
    .font(UI_FONT)
    .width(Length::Fill);

    let mut inner = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(
            row![connector, Space::with_width(Length::Fill), remove]
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .push(column_pick)
        .push(op_pick);

    // IS NULL / IS NOT NULL stand alone — no value to enter.
    if f.op.takes_value() {
        inner = inner.push(
            text_input("value", &f.value)
                .on_input(move |s| Message::DbFilterValueChanged(i, s))
                .font(UI_FONT)
                .size(11)
                .width(Length::Fill),
        );
    }

    container(inner)
        .padding([4, 6])
        .width(Length::Fill)
        .style(field_row_bg)
        .into()
}

/// Col 4 — GROUP BY builder. Lists the fields of the selected tables (mirroring
/// the Fields column) as toggles; checked columns are added to the GROUP BY in
/// the order they were picked, so the generated SQL collapses rows on them.
fn group_col(panel: &DbPanel) -> Element<'_, Message> {
    if panel.selected_tables.is_empty() {
        return text("Select a table to group on its fields.")
            .font(UI_FONT)
            .size(11)
            .into();
    }

    let mut col = Column::new().spacing(2).width(Length::Fill);
    for table_key in &panel.selected_tables {
        col = col.push(
            text(table_key.clone()).font(UI_FONT).size(11).color(Color::from_rgb8(150, 150, 165)),
        );

        if panel.loading_columns.contains(table_key) {
            col = col.push(container(text("loading…").size(11)).padding([1, 12]));
            continue;
        }
        let Some(cols) = panel.columns_by_table.get(table_key) else {
            col = col.push(container(text("(no columns)").size(11)).padding([1, 12]));
            continue;
        };

        for c in cols {
            let is_grouped = panel.is_grouped(table_key, c);
            let marker = if is_grouped { "[x]" } else { "[ ]" };
            col = col.push(
                button(text(format!("{marker} {c}")).font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::DbGroupToggled(table_key.clone(), c.clone()))
                    .padding(buttons::PADDING)
                    .style(if is_grouped { buttons::primary } else { buttons::secondary })
                    .width(Length::Fill),
            );
        }
    }

    scrollable(col).width(Length::Fill).height(Length::Fill).into()
}

/// Col 5 — the live generated SQL, a reorderable list of the picked fields, and
/// the Run / Reset actions.
fn query_col(panel: &DbPanel) -> Element<'_, Message> {
    let sql = panel.build_select().unwrap_or_else(|| "—".to_string());

    let sql_box = container(text(sql).font(Font::MONOSPACE).size(11))
        .padding(6)
        .width(Length::Fill)
        .style(sql_box_bg);

    let actions = row![
        button(
            text(if panel.query_running { "Running…" } else { "Run" })
                .font(UI_FONT)
                .size(buttons::TEXT_SIZE),
        )
        .on_press_maybe((!panel.query_running).then_some(Message::DbRunQuery))
        .padding(buttons::PADDING)
        .style(buttons::primary),
        button(text("Reset").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::DbResetTables)
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let fields = selected_fields_list(panel);

    column![
        actions,
        sql_box,
        text("Result columns (drag to reorder):").font(UI_FONT).size(11),
        fields,
    ]
    .spacing(6)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The ordered list of picked result columns. Each row is draggable by mouse to
/// reorder it (grab anywhere on the row and drag over another); the ✕ removes
/// it. Empty selection means "all columns" (`*`).
fn selected_fields_list(panel: &DbPanel) -> Element<'_, Message> {
    if panel.selected_cols.is_empty() {
        return container(
            text("No fields picked — all columns of the selected tables are returned.")
                .font(UI_FONT)
                .size(11),
        )
        .padding([2, 0])
        .into();
    }

    let mut col = Column::new().spacing(2).width(Length::Fill);
    for (i, c) in panel.selected_cols.iter().enumerate() {
        let short_table = c.table.rsplit('.').next().unwrap_or(&c.table);
        let label = format!("{short_table}.{}", c.column);
        let dragging = panel.dragging_field == Some(i);

        let remove = button(text("\u{2715}").font(ICON_FONT).size(11))
            .on_press(Message::DbFieldRemoved(i))
            .padding([2, 6])
            .style(buttons::danger);

        let row_inner = row![
            text("\u{2195}").font(ICON_FONT).size(12), // ↕ drag handle
            text(label).font(UI_FONT).size(11).width(Length::Fill),
            remove,
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let styled = container(row_inner)
            .padding([3, 4])
            .width(Length::Fill)
            .style(if dragging { field_row_drag_bg } else { field_row_bg });

        // The whole row is a drag target: press to pick it up, drag over any
        // other row to move it there (handled live in `update`), release to drop.
        let area = mouse_area(styled)
            .interaction(iced::mouse::Interaction::Grab)
            .on_press(Message::DbFieldDragStart(i))
            .on_enter(Message::DbFieldDragOver(i))
            .on_release(Message::DbFieldDragEnd);

        col = col.push(area);
    }

    scrollable(col).width(Length::Fill).height(Length::Fill).into()
}

/// Row 2 — the results table. Spans all three builder columns. When a stored
/// procedure/function is selected from the Objects list, its source takes over
/// this row instead.
fn results_section(panel: &DbPanel) -> Element<'_, Message> {
    if let Some(routine) = &panel.selected_routine {
        return routine_source_view(panel, routine);
    }

    let count = panel.query_result.as_ref().map(|r| r.rows.len());
    let title = match count {
        Some(n) => format!("Results ({n})"),
        None    => "Results".to_string(),
    };

    let body: Element<'_, Message> = if let Some(err) = &panel.query_error {
        container(
            text(format!("Query error: {err}"))
                .size(11)
                .color(Color::from_rgb8(239, 68, 68)),
        )
        .padding(6)
        .into()
    } else if let Some(status) = &panel.statement_status {
        // A non-row-returning statement (UPDATE/DELETE/DDL) ran from the SQL
        // pane — report what it changed instead of an empty table.
        container(
            text(status.clone())
                .font(UI_FONT)
                .size(12)
                .color(Color::from_rgb8(74, 222, 128)),
        )
        .padding(8)
        .into()
    } else if let Some(result) = &panel.query_result {
        if result.columns.is_empty() {
            container(text("Query returned no columns.").font(UI_FONT).size(11))
                .padding(6)
                .into()
        } else {
            results_table(result)
        }
    } else {
        container(text("Run the query to see results here.").font(UI_FONT).size(12))
            .padding(12)
            .into()
    };

    // Refresh re-runs the last query so the list reflects current data. Enabled
    // whenever a query exists (or can be built) and none is already in flight.
    let can_refresh =
        !panel.query_running && (!panel.query.is_empty() || panel.build_select().is_some());
    let refresh = button(
        text(if panel.query_running { "Refreshing…" } else { "Refresh" })
            .font(UI_FONT)
            .size(buttons::TEXT_SIZE),
    )
    .on_press_maybe(can_refresh.then_some(Message::DbRefreshResults))
    .padding(buttons::PADDING)
    .style(buttons::secondary);

    let header = row![
        section_title(&title),
        Space::with_width(Length::Fill),
        refresh,
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    container(
        column![header, body]
            .spacing(4)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(6)
    .style(panel_box)
    .into()
}

/// Source view for a selected stored procedure/function: a header naming the
/// routine with a ✕ to dismiss it, over the fetched DDL in a monospace box.
fn routine_source_view<'a>(panel: &'a DbPanel, routine: &'a RoutineRef) -> Element<'a, Message> {
    let title = format!("{} {}", routine.kind.tag(), routine.key());

    let close = button(text("\u{2715}").font(ICON_FONT).size(11))
        .on_press(Message::DbRoutineClosed)
        .padding([2, 6])
        .style(buttons::secondary);

    let header = row![
        text(format!("Source — {title}")).font(UI_FONT).size(12).width(Length::Fill),
        close,
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let body: Element<'_, Message> = if panel.routine_loading {
        container(text("Loading source…").font(UI_FONT).size(11)).padding(6).into()
    } else if let Some(code) = &panel.routine_code {
        scrollable(
            container(text(code.clone()).font(Font::MONOSPACE).size(11))
                .padding(6)
                .width(Length::Fill)
                .style(sql_box_bg),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        container(text("No source available.").font(UI_FONT).size(11)).padding(6).into()
    };

    container(
        column![header, body]
            .spacing(4)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .padding(6)
    .style(panel_box)
    .into()
}

/// Fixed width of each results column. Columns no longer shrink to share the
/// pane — they keep this width so a wide result set overflows the pane and is
/// reached by scrolling horizontally rather than being squeezed unreadable.
const RESULT_COL_WIDTH: f32 = 180.0;

/// Render the result set as a grid that scrolls in both directions. Every column
/// is a fixed width, so when the columns together exceed the pane the table
/// overflows and a horizontal scrollbar appears; each cell carries a hairline
/// border so rows and columns read as a grid. Cells in ID columns are copyable —
/// click the value (or its Copy button) to put it on the clipboard.
fn results_table(result: &QueryResult) -> Element<'static, Message> {
    let id_cols: Vec<bool> = result.columns.iter().map(|c| is_id_column(c)).collect();

    let mut header = Row::new().spacing(0);
    for c in &result.columns {
        header = header.push(
            container(text(c.clone()).font(UI_FONT).size(11))
                .width(Length::Fixed(RESULT_COL_WIDTH))
                .padding([3, 6])
                .style(header_cell_box),
        );
    }
    // Shrink width so the grid takes only the columns' combined width — letting
    // it overflow the pane (and thus scroll) instead of being compressed to fit.
    let mut grid = Column::new().spacing(0).width(Length::Shrink).push(header);

    for row_vals in &result.rows {
        let mut r = Row::new().spacing(0).align_y(Alignment::Center);
        for (i, v) in row_vals.iter().enumerate() {
            r = r.push(result_cell(v, id_cols.get(i).copied().unwrap_or(false)));
        }
        grid = grid.push(r);
    }

    scrollable(grid)
        .direction(scrollable::Direction::Both {
            vertical:   scrollable::Scrollbar::new(),
            horizontal: scrollable::Scrollbar::new(),
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One results cell, a fixed width matching its column header (so cells line up
/// into a grid that scrolls horizontally) and bordered to form grid lines.
/// ID-column cells get a clickable (copy-on-press) value plus an explicit Copy
/// button; ordinary cells are plain text.
fn result_cell(value: &str, is_id: bool) -> Element<'static, Message> {
    let display: String = if value.chars().count() > 40 {
        format!("{}…", value.chars().take(40).collect::<String>())
    } else {
        value.to_string()
    };

    let inner: Element<'static, Message> = if is_id && value != "NULL" {
        let label = mouse_area(text(display).font(Font::MONOSPACE).size(11))
            .interaction(iced::mouse::Interaction::Pointer)
            .on_press(Message::DbCopyValue(value.to_string()));
        let copy = button(text("Copy").font(UI_FONT).size(9))
            .on_press(Message::DbCopyValue(value.to_string()))
            .padding([1, 5])
            .style(buttons::secondary);
        row![label, Space::with_width(Length::Fill), copy]
            .spacing(4)
            .align_y(Alignment::Center)
            .into()
    } else {
        text(display).font(Font::MONOSPACE).size(11).into()
    };

    container(inner)
        .width(Length::Fixed(RESULT_COL_WIDTH))
        .padding([3, 6])
        .style(cell_box)
        .into()
}

/// A column whose values are identifiers worth copying: exactly `id`, or any
/// `…_id` (case-insensitive). Kept deliberately narrow to avoid flagging
/// columns like `valid` or `width`.
fn is_id_column(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "id" || n.ends_with("_id")
}

// ── styling ───────────────────────────────────────────────────────────────────

fn query_bar_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(32, 32, 42))),
        border: Border { color: Color::from_rgb8(55, 55, 68), width: 0.0, radius: 0.0.into() },
        ..Default::default()
    }
}

/// Bordered box used for each builder column and the results pane.
fn panel_box(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(28, 28, 38))),
        border: Border { color: Color::from_rgb8(55, 55, 68), width: 1.0, radius: 4.0.into() },
        ..Default::default()
    }
}

/// Slightly darker inset behind the generated SQL.
fn sql_box_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(22, 22, 30))),
        border: Border { color: Color::from_rgb8(55, 55, 68), width: 1.0, radius: 4.0.into() },
        ..Default::default()
    }
}

/// Header cell of the results table — filled, with grid-line borders.
fn header_cell_box(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(40, 40, 52))),
        border: Border { color: Color::from_rgb8(70, 70, 86), width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

/// Data cell of the results table — transparent fill, hairline grid borders.
fn cell_box(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border { color: Color::from_rgb8(55, 55, 68), width: 1.0, radius: 0.0.into() },
        ..Default::default()
    }
}

/// A picked-field row in the reorder list (idle).
fn field_row_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(34, 34, 44))),
        border: Border { color: Color::from_rgb8(55, 55, 68), width: 1.0, radius: 3.0.into() },
        ..Default::default()
    }
}

/// A picked-field row while it is being dragged — highlighted so the user can
/// see which field they have grabbed.
fn field_row_drag_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(58, 58, 78))),
        border: Border { color: Color::from_rgb8(110, 110, 140), width: 1.0, radius: 3.0.into() },
        ..Default::default()
    }
}

