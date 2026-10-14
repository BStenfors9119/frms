//! Receivers plugin UI — a hierarchical TSR receiver inventory.
//!
//! Left column: a tree of containers (clients/locations) and the receivers
//! inside them, with controls to add/remove nodes. Right column: the selected
//! receiver's credential form plus Connect (interactive SSH) and Copy actions,
//! or — when a container is selected — its add/delete controls and the
//! database/CSV import flow. Mirrors the two-column shape of the Notes and
//! Terminals plugin bodies.

use iced::{Alignment, Element, Length};
use iced::widget::{
    button, column, container, pick_list, row, scrollable, text, text_input, Column, Space,
};

use crate::app::Message;
use crate::fonts::{ICON_FONT, UI_FONT};
use crate::receivers::{
    node_sort_key, ImportFilterOp, ImportSource, MapField, ReceiversState,
};
use crate::ui::buttons;
use crate::ui::plugin_panel::ReceiverCtx;
use crate::ui::terminal as terminal_ui;

/// Sentinel shown in mapping pick-lists for "no column mapped".
const NONE_OPTION: &str = "(none)";

pub fn view<'a>(ctx: &ReceiverCtx<'a>) -> Element<'a, Message> {
    row![
        container(tree(ctx.state))
            .width(Length::Fixed(200.0))
            .height(Length::Fill),
        container(detail(ctx))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, 8]),
    ]
    .spacing(8)
    .height(Length::Fill)
    .into()
}

// ── left: the tree ──────────────────────────────────────────────────────────

fn tree(rs: &ReceiversState) -> Element<'_, Message> {
    let add_root = button(text("+ Client").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::ReceiverContainerAdd(None))
        .padding(buttons::PADDING)
        .style(buttons::primary)
        .width(Length::Fill);

    let mut rows: Vec<Element<'_, Message>> = Vec::new();
    // Clients first by leading number (ascending), then alphabetically.
    let mut roots = rs.root_containers();
    roots.sort_by_key(|c| node_sort_key(&c.name));
    for c in roots {
        push_container(&mut rows, rs, c.id, 0);
    }
    if rows.is_empty() {
        rows.push(text("(no receivers yet)").size(11).into());
    }

    let list = Column::with_children(rows).spacing(2).width(Length::Fill);

    column![
        add_root,
        scrollable(list).height(Length::Fill),
    ]
    .spacing(6)
    .height(Length::Fill)
    .into()
}

/// Append a container row and, when expanded, its child containers and
/// receivers (depth-first). `depth` drives indentation.
fn push_container<'a>(
    out:   &mut Vec<Element<'a, Message>>,
    rs:    &'a ReceiversState,
    id:    u64,
    depth: u16,
) {
    let Some(c) = rs.container_by_id(id) else { return; };
    let expanded = rs.is_expanded(id);
    let selected = rs.selected_container == Some(id) && rs.selected_receiver.is_none();

    let caret = if expanded { "\u{25BE}" } else { "\u{25B8}" }; // ▾ / ▸
    // A roomy, fixed-size hit target — the bare glyph was painfully small to
    // click. Centered so the larger box still reads as a tidy chevron.
    let caret_btn = button(
        container(text(caret).font(ICON_FONT).size(buttons::TEXT_SIZE + 4))
            .center_x(Length::Fill)
            .center_y(Length::Fill)
    )
        .on_press(Message::ReceiverContainerToggle(id))
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .padding(0)
        .style(buttons::secondary);
    // 🗁 open folder for an expanded container, 🗀 closed for collapsed.
    let folder = if expanded { "\u{1F5C1}" } else { "\u{1F5C0}" };
    let name_btn = button(
        row![
            text(folder).font(ICON_FONT).size(buttons::TEXT_SIZE),
            text(c.name.clone()).font(UI_FONT).size(buttons::TEXT_SIZE),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
    )
        .on_press(Message::ReceiverContainerSelect(id))
        .padding(buttons::PADDING)
        .style(buttons::toggle(selected))
        .width(Length::Fill);

    out.push(
        row![
            Space::with_width(Length::Fixed(depth as f32 * 12.0)),
            caret_btn,
            name_btn,
        ]
        .spacing(2)
        .align_y(Alignment::Center)
        .into(),
    );

    if !expanded {
        return;
    }

    // Child containers first, then receivers — same numeric-aware ordering.
    let mut kids = rs.child_containers(id);
    kids.sort_by_key(|c| node_sort_key(&c.name));
    for kid in kids {
        push_container(out, rs, kid.id, depth + 1);
    }
    let mut recvs = rs.receivers_in(id);
    recvs.sort_by_key(|r| node_sort_key(&r.name));
    for r in recvs {
        let selected = rs.selected_receiver == Some(r.id);
        // A ● at the end of the row flags this receiver's SSH state: green when
        // a session is live, amber when one exists but its ssh process exited
        // (still shown so the failure output stays readable). No dot when there
        // is no session. Colors mirror the detail pane's connected/disconnected
        // labels; the glyph comes from ICON_FONT (UI_FONT is Latin-only).
        let mut content = row![
            // 🖧 networked-device glyph stands in for a receiver.
            text("\u{1F5A7}").font(ICON_FONT).size(buttons::TEXT_SIZE),
            text(r.name.clone()).font(UI_FONT).size(buttons::TEXT_SIZE),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        if let Some(ssh) = rs.ssh.iter().find(|s| s.receiver == r.id) {
            let dot = if ssh.exited {
                iced::Color::from_rgb8(210, 140, 110)
            } else {
                iced::Color::from_rgb8(120, 200, 120)
            };
            content = content
                .push(Space::with_width(Length::Fill))
                .push(
                    text("\u{25CF}") // ●
                        .font(ICON_FONT)
                        .size(buttons::TEXT_SIZE)
                        .color(dot),
                );
        }
        out.push(
            row![
                Space::with_width(Length::Fixed((depth + 1) as f32 * 12.0 + 8.0)),
                button(content)
                    .on_press(Message::ReceiverSelect(r.id))
                    .padding(buttons::PADDING)
                    .style(buttons::toggle(selected))
                    .width(Length::Fill),
            ]
            .spacing(2)
            .align_y(Alignment::Center)
            .into(),
        );
    }
}

// ── right: detail pane ──────────────────────────────────────────────────────

fn detail<'a>(ctx: &ReceiverCtx<'a>) -> Element<'a, Message> {
    let rs = ctx.state;
    let body: Element<'a, Message> = if rs.selected_receiver.is_some() {
        receiver_detail(ctx)
    } else if let Some(id) = rs.selected_container {
        container_detail(ctx, id)
    } else {
        container(
            text("Select a client/location to add receivers, or a receiver to edit it.")
                .size(12),
        )
        .padding(16)
        .into()
    };

    match &rs.status {
        Some(result) => {
            let (msg, ok) = match result {
                Ok(m)  => (m.clone(), true),
                Err(m) => (m.clone(), false),
            };
            let line = text(msg).size(11).color(if ok {
                iced::Color::from_rgb8(120, 200, 120)
            } else {
                iced::Color::from_rgb8(220, 120, 120)
            });
            column![body, line].spacing(6).height(Length::Fill).into()
        }
        None => body,
    }
}

fn container_detail<'a>(ctx: &ReceiverCtx<'a>, id: u64) -> Element<'a, Message> {
    let rs = ctx.state;
    let Some(c) = rs.container_by_id(id) else {
        return text("Container not found.").size(12).into();
    };

    let header = row![
        text_input("Name", &c.name)
            .on_input(move |s| Message::ReceiverContainerRenamed(id, s))
            .size(14)
            .width(Length::Fill),
        button(text("Delete").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::ReceiverContainerDelete(id))
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let actions = row![
        button(text("+ Receiver").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::ReceiverAdd(id))
            .padding(buttons::PADDING)
            .style(buttons::primary),
        button(text("+ Sub-location").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::ReceiverContainerAdd(Some(id)))
            .padding(buttons::PADDING)
            .style(buttons::secondary),
    ]
    .spacing(6);

    // Bulk SSH over the whole branch (this container plus its sub-locations),
    // so "connect this client" is one click instead of one per receiver. Both
    // buttons are inert with nothing to act on, hence `on_press_maybe`.
    let under = rs.receivers_under(id);
    // An exited pane counts as connectable (Connect All retries it) *and* as
    // something to clear, so the two counts can overlap.
    let idle = under.iter().filter(|r| !rs.is_connected(r.id)).count();
    let live = under.iter()
        .filter(|r| rs.ssh.iter().any(|s| s.receiver == r.id))
        .count();
    let bulk_ssh = row![
        button(
            text(format!("Connect All ({idle})")).font(UI_FONT).size(buttons::TEXT_SIZE)
        )
            .on_press_maybe((idle > 0).then_some(Message::ReceiverConnectAll(id)))
            .padding(buttons::PADDING)
            .style(buttons::primary),
        button(
            text(format!("Disconnect All ({live})")).font(UI_FONT).size(buttons::TEXT_SIZE)
        )
            .on_press_maybe((live > 0).then_some(Message::ReceiverDisconnectAll(id)))
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6);

    // Reparent this container. Its own subtree is excluded (moving a branch
    // inside itself would detach it), and "(top level)" promotes it to a client.
    let move_self = row![
        text("Move to").size(12).width(Length::Fixed(90.0)),
        pick_list(
            rs.container_choices(Some(id), true),
            Some(rs.choice_for(c.parent)),
            move |choice| Message::ReceiverContainerMoveTo(id, choice.id),
        )
        .text_size(12)
        .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut col = column![header, actions, bulk_ssh, move_self].spacing(12);

    // Bulk move: the common cleanup is receivers parked at a location's root
    // that belong in one of its sub-locations, so offer it only when this
    // container directly holds any.
    let held = rs.receivers_in(id).len();
    if held > 0 {
        col = col.push(
            row![
                text(format!(
                    "Move {held} receiver{} to",
                    if held == 1 { "" } else { "s" },
                ))
                .size(12)
                .width(Length::Fixed(150.0)),
                pick_list(
                    rs.container_choices(None, false)
                        .into_iter()
                        .filter(|choice| choice.id != Some(id))
                        .collect::<Vec<_>>(),
                    // Always unselected: this is an action, not a stored field.
                    None::<crate::receivers::ContainerChoice>,
                    move |choice| Message::ReceiverMoveAll(id, choice.id.unwrap_or(id)),
                )
                .placeholder("pick a destination")
                .text_size(12)
                .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        );
    }

    col.push(import_section(ctx))
        .height(Length::Fill)
        .into()
}

/// The DB/CSV import controls shown under a selected container.
fn import_section<'a>(ctx: &ReceiverCtx<'a>) -> Element<'a, Message> {
    let rs = ctx.state;
    let Some(draft) = &rs.import else {
        // No import in progress — offer the two entry points.
        return column![
            text("Import receivers").size(13),
            row![
                button(text("From database").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::ReceiverImportDbStart)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
                button(text("From CSV").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::ReceiverImportCsvStart)
                    .padding(buttons::PADDING)
                    .style(buttons::secondary),
            ]
            .spacing(6),
            if ctx.db_connected {
                text("Database connected.").size(10)
                    .color(iced::Color::from_rgb8(120, 180, 120))
            } else {
                text("No database connected — open the Database tool to connect.")
                    .size(10)
                    .color(iced::Color::from_rgb8(180, 160, 120))
            },
        ]
        .spacing(8)
        .into();
    };

    // Source-specific picker (table list or CSV path) at the top.
    let source_row: Element<'a, Message> = match draft.source {
        ImportSource::Database => {
            let keys: Vec<String> = ctx.db_tables.iter().map(|t| t.key()).collect();
            let selected = if draft.origin.is_empty() { None } else { Some(draft.origin.clone()) };
            row![
                text("Table:").size(12).width(Length::Fixed(60.0)),
                pick_list(keys, selected, Message::ReceiverImportTablePicked)
                    .placeholder("pick a table")
                    .text_size(12)
                    .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into()
        }
        ImportSource::Csv => row![
            text("File:").size(12).width(Length::Fixed(60.0)),
            text_input("/path/to/receivers.csv", &draft.csv_path)
                .on_input(Message::ReceiverImportCsvPathEdited)
                .size(12)
                .width(Length::Fill),
            button(text("Load").font(UI_FONT).size(buttons::TEXT_SIZE))
                .on_press(Message::ReceiverImportCsvLoad)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
    };

    let mut col = column![
        text("Import receivers").size(13),
        source_row,
    ]
    .spacing(8);

    if draft.loading {
        col = col.push(text("Loading columns…").size(11));
    }
    if let Some(err) = &draft.error {
        col = col.push(
            text(err.clone()).size(11).color(iced::Color::from_rgb8(220, 120, 120)),
        );
    }

    // Column mapping once the columns are known.
    if !draft.columns.is_empty() {
        // A pick-list of the available columns, prefixed with "(none)".
        let col_options = || {
            let mut opts: Vec<String> = vec![NONE_OPTION.to_string()];
            opts.extend(draft.columns.iter().cloned());
            opts
        };
        let selected_of = |current: &Option<String>| {
            Some(current.clone().unwrap_or_else(|| NONE_OPTION.to_string()))
        };

        // Name / Host: column mapping only.
        let map_row = |label: &'a str, field: MapField, current: &Option<String>| {
            row![
                text(label).size(12).width(Length::Fixed(90.0)),
                pick_list(col_options(), selected_of(current), move |s| {
                    let col = if s == NONE_OPTION { None } else { Some(s) };
                    Message::ReceiverImportFieldMapped(field, col)
                })
                .text_size(12)
                .width(Length::Fill),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        };

        // Username / Password / Port: a column OR a literal value typed once and
        // applied to every imported receiver (column wins when both are set).
        let lit_row = |label: &'a str, field: MapField, current: &Option<String>,
                       literal: &str, placeholder: &'a str, secure: bool| {
            row![
                text(label).size(12).width(Length::Fixed(90.0)),
                pick_list(col_options(), selected_of(current), move |s| {
                    let col = if s == NONE_OPTION { None } else { Some(s) };
                    Message::ReceiverImportFieldMapped(field, col)
                })
                .text_size(12)
                .width(Length::FillPortion(1)),
                text_input(placeholder, literal)
                    .on_input(move |s| Message::ReceiverImportLiteralEdited(field, s))
                    .secure(secure)
                    .size(12)
                    .width(Length::FillPortion(1)),
            ]
            .spacing(6)
            .align_y(Alignment::Center)
        };

        col = col
            .push(text("Map columns (host required; or type a value to apply to all):").size(11))
            .push(map_row("Name", MapField::Name, &draft.map.name))
            .push(map_row("Host / URL", MapField::Host, &draft.map.host))
            .push(lit_row("Username", MapField::Username, &draft.map.username,
                          &draft.user_value, "value for all", false))
            .push(lit_row("Password", MapField::Password, &draft.map.password,
                          &draft.pass_value, "value for all", true))
            .push(lit_row("Port", MapField::Port, &draft.map.port,
                          &draft.port_value, "22", false));

        // Optional row filter: only import rows matching a column comparison.
        let mut filter_opts: Vec<String> = vec![NONE_OPTION.to_string()];
        filter_opts.extend(draft.columns.iter().cloned());
        let filter_sel = Some(draft.filter_column.clone().unwrap_or_else(|| NONE_OPTION.to_string()));
        let filter_row = row![
            text("Only where").size(12).width(Length::Fixed(90.0)),
            pick_list(filter_opts, filter_sel, |s| {
                let col = if s == NONE_OPTION { None } else { Some(s) };
                Message::ReceiverImportFilterColumn(col)
            })
            .placeholder("column")
            .text_size(12)
            .width(Length::FillPortion(2)),
            pick_list(ImportFilterOp::ALL.to_vec(), Some(draft.filter_op),
                      Message::ReceiverImportFilterOp)
                .text_size(12)
                .width(Length::FillPortion(2)),
            text_input("value", &draft.filter_value)
                .on_input(Message::ReceiverImportFilterValue)
                .size(12)
                .width(Length::FillPortion(2)),
        ]
        .spacing(6)
        .align_y(Alignment::Center);
        col = col.push(filter_row);

        let confirm = button(text("Import").font(UI_FONT).size(buttons::TEXT_SIZE))
            .padding(buttons::PADDING)
            .style(buttons::primary)
            .on_press_maybe(draft.is_ready().then_some(Message::ReceiverImportConfirm));
        col = col.push(row![confirm, cancel_btn()].spacing(6));
    } else {
        col = col.push(cancel_btn());
    }

    scrollable(col).height(Length::Fill).into()
}

fn cancel_btn<'a>() -> Element<'a, Message> {
    button(text("Cancel").font(UI_FONT).size(buttons::TEXT_SIZE))
        .on_press(Message::ReceiverImportCancel)
        .padding(buttons::PADDING)
        .style(buttons::secondary)
        .into()
}

fn receiver_detail<'a>(ctx: &ReceiverCtx<'a>) -> Element<'a, Message> {
    let rs = ctx.state;
    let Some(id) = rs.selected_receiver else {
        return text("No receiver selected.").size(12).into();
    };

    let field = |label: &'a str, value: &str, secure: bool,
                 on_input: fn(String) -> Message| {
        let input = text_input(label, value)
            .on_input(on_input)
            .secure(secure)
            .size(13)
            .width(Length::Fill);
        row![
            text(label).size(12).width(Length::Fixed(80.0)),
            input,
        ]
        .spacing(6)
        .align_y(Alignment::Center)
    };

    // Which container holds this receiver — changing the pick moves it there.
    let location = row![
        text("Location").size(12).width(Length::Fixed(80.0)),
        pick_list(
            rs.container_choices(None, false),
            rs.receiver_by_id(id).map(|r| rs.choice_for(Some(r.container))),
            move |choice| Message::ReceiverMoveTo(id, choice.id.unwrap_or_default()),
        )
        .placeholder("pick a location")
        .text_size(13)
        .width(Length::Fill),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let form = column![
        field("Name",     &rs.name_input,     false, Message::ReceiverNameEdited),
        field("Host",     &rs.host_input,     false, Message::ReceiverHostEdited),
        field("Username", &rs.user_input,     false, Message::ReceiverUserEdited),
        field("Password", &rs.password_input, true,  Message::ReceiverPasswordEdited),
        field("Port",     &rs.port_input,     false, Message::ReceiverPortEdited),
        location,
    ]
    .spacing(6);

    // Opens a file picker to choose any local file to scp to this receiver.
    let copy_btn = button(text("Copy file…").font(UI_FONT).size(buttons::TEXT_SIZE))
        .padding(buttons::PADDING)
        .style(buttons::secondary)
        .on_press(Message::ReceiverCopyPick);

    let actions = row![
        button(text("Connect (SSH)").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::ReceiverConnect(id))
            .padding(buttons::PADDING)
            .style(buttons::primary),
        copy_btn,
        button(text("Delete").font(UI_FONT).size(buttons::TEXT_SIZE))
            .on_press(Message::ReceiverDelete(id))
            .padding(buttons::PADDING)
            .style(buttons::danger),
    ]
    .spacing(6);

    // The credential form + connect actions can be collapsed once an SSH pane
    // is live, handing the reclaimed height to the terminal below.
    let has_ssh = rs.ssh.iter().any(|s| s.receiver == id);
    let collapsed = has_ssh && rs.connection_collapsed;

    let mut col = column![].spacing(12);
    if collapsed {
        col = col.push(
            button(
                // ▸ / ▾ come from ICON_FONT: Liberation Sans is Latin-only and
                // renders them as tofu boxes.
                row![
                    text("\u{25B8}").font(ICON_FONT).size(buttons::TEXT_SIZE),
                    text("Connection details").font(UI_FONT).size(buttons::TEXT_SIZE),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
            )
            .on_press(Message::ReceiverToggleConnectionForm)
            .padding(buttons::PADDING)
            .style(buttons::secondary),
        );
    } else {
        col = col.push(form).push(actions);
        if has_ssh {
            col = col.push(
                button(
                    row![
                        text("\u{25BE}").font(ICON_FONT).size(buttons::TEXT_SIZE),
                        text("Hide connection details").font(UI_FONT).size(buttons::TEXT_SIZE),
                    ]
                    .spacing(4)
                    .align_y(Alignment::Center),
                )
                .on_press(Message::ReceiverToggleConnectionForm)
                .padding(buttons::PADDING)
                .style(buttons::secondary),
            );
        }
    }

    // Embedded SSH terminal, shown under the receiver it belongs to. The pane
    // persists after the session ends so its final output stays readable.
    if let Some(ssh) = rs.ssh.iter().find(|s| s.receiver == id) {
        let status = if ssh.exited {
            text("disconnected").size(11).color(iced::Color::from_rgb8(210, 140, 110))
        } else {
            text("connected").size(11).color(iced::Color::from_rgb8(120, 200, 120))
        };
        // While live: Disconnect. After exit: Reconnect to retry + Close to
        // dismiss the dead pane.
        let mut header = row![
            text(format!("SSH: {}", ssh.name)).size(12),
            status,
            Space::with_width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if ssh.exited {
            header = header
                .push(
                    button(text("Reconnect").font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::ReceiverConnect(id))
                        .padding(buttons::PADDING)
                        .style(buttons::primary),
                )
                .push(
                    button(text("Close").font(UI_FONT).size(buttons::TEXT_SIZE))
                        .on_press(Message::ReceiverSshDisconnect(id))
                        .padding(buttons::PADDING)
                        .style(buttons::secondary),
                );
        } else {
            header = header.push(
                button(text("Disconnect").font(UI_FONT).size(buttons::TEXT_SIZE))
                    .on_press(Message::ReceiverSshDisconnect(id))
                    .padding(buttons::PADDING)
                    .style(buttons::danger),
            );
        }

        let term = terminal_ui::view(terminal_ui::View {
            id:         ssh.id,
            grid:       ssh.pane.grid.clone(),
            master:     ssh.pane.master(),
            focused:    ssh.focused,
            is_shell:   true,
            font_scale: ctx.font_scale,
            selection:  ssh.pane.selection,
            transcript: false,
            find:       None,
        });
        col = col.push(header).push(
            container(term)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(container::bordered_box),
        );
    }

    col.height(Length::Fill).into()
}
