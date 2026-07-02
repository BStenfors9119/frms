//! TSR receiver inventory exposed through the Receivers plugin tab.
//!
//! A global, hierarchical inventory: parent containers hold child containers
//! and/or receivers (e.g. `Client A → Location A → receiver 1, 2`). Each
//! receiver carries the SSH host, username, password, and port needed to open
//! an interactive shell or `scp` files to it.
//!
//! Persistence mirrors `notes.rs`: hand-rolled JSON on top of
//! `serde_json::Value` so unknown fields round-trip and a corrupt file falls
//! back to defaults. Because the stored password is needed to drive
//! `sshpass`, the file is written with `0600` permissions — there is no
//! keyring backend available given the project's no-C-deps constraint.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::terminal::{TerminalId, TerminalPane};

/// Default SSH port used when a receiver doesn't specify one.
pub const DEFAULT_PORT: u16 = 22;

// ── data model ──────────────────────────────────────────────────────────────

/// A node in the inventory tree that groups receivers and/or child
/// containers. `parent` is `None` for a root (top-level) container.
#[derive(Debug, Clone)]
pub struct Container {
    pub id:     u64,
    pub name:   String,
    pub parent: Option<u64>,
}

/// A single TSR receiver reachable over SSH. `container` names the owning
/// container.
#[derive(Debug, Clone)]
pub struct Receiver {
    pub id:        u64,
    pub name:      String,
    pub host:      String,
    pub username:  String,
    pub password:  String,
    pub port:      u16,
    pub container: u64,
}

impl Receiver {
    /// `user@host`, the SSH destination.
    pub fn destination(&self) -> String {
        format!("{}@{}", self.username, self.host)
    }

    /// Program, arguments, and environment for an interactive SSH session via
    /// `sshpass`. The password is passed through the `SSHPASS` env var (not the
    /// argument list, which is world-readable via `/proc`). `accept-new` adds
    /// unknown host keys without an interactive prompt that the PTY user would
    /// otherwise have to answer blind.
    pub fn ssh_command(&self) -> (String, Vec<String>, Vec<(String, String)>) {
        let args = vec![
            "-e".to_string(),
            "ssh".to_string(),
            "-p".to_string(),
            self.port.to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            self.destination(),
        ];
        let envs = vec![("SSHPASS".to_string(), self.password.clone())];
        ("sshpass".to_string(), args, envs)
    }
}

// ── import drafting ─────────────────────────────────────────────────────────

/// Where an in-progress import is reading rows from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportSource {
    Database,
    Csv,
}

/// Which receiver field each available column maps onto. `host` is required;
/// the rest fall back to sensible defaults (name → host, port → 22, empty
/// username/password) when left unmapped.
#[derive(Debug, Clone, Default)]
pub struct FieldMap {
    pub name:     Option<String>,
    pub host:     Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub port:     Option<String>,
}

/// Which receiver field a mapping pick-list controls. Drives the UI and the
/// `FieldMap` setter so one message covers all five fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapField {
    Name,
    Host,
    Username,
    Password,
    Port,
}

/// Operator for the optional "only import rows where …" import filter. Kept
/// deliberately small (three string-friendly comparisons) so it applies
/// uniformly to both SQL (pushed into a `WHERE`) and CSV (matched per row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportFilterOp {
    #[default]
    Equals,
    NotEquals,
    Contains,
}

impl ImportFilterOp {
    pub const ALL: [ImportFilterOp; 3] =
        [ImportFilterOp::Equals, ImportFilterOp::NotEquals, ImportFilterOp::Contains];

    /// SQL operator emitted for a database import.
    fn sql(self) -> &'static str {
        match self {
            ImportFilterOp::Equals    => "=",
            ImportFilterOp::NotEquals => "<>",
            ImportFilterOp::Contains  => "LIKE",
        }
    }

    /// Whether `cell` satisfies the filter against `value` (CSV row matching;
    /// case-insensitive to match typical SQL collations).
    fn matches(self, cell: &str, value: &str) -> bool {
        match self {
            ImportFilterOp::Equals    => cell.eq_ignore_ascii_case(value),
            ImportFilterOp::NotEquals => !cell.eq_ignore_ascii_case(value),
            ImportFilterOp::Contains  => cell.to_lowercase().contains(&value.to_lowercase()),
        }
    }
}

impl std::fmt::Display for ImportFilterOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImportFilterOp::Equals    => "equals",
            ImportFilterOp::NotEquals => "not equals",
            ImportFilterOp::Contains  => "contains",
        })
    }
}

/// An in-progress import: a chosen source, the columns available to map, and
/// the user's current field mapping. Receivers are created under
/// `target_container` when the user confirms.
#[derive(Debug, Clone)]
pub struct ImportDraft {
    pub source:           ImportSource,
    /// Table key (`schema.name`) for a DB import; CSV file path for a CSV import.
    pub origin:           String,
    /// CSV path input (only used while `source == Csv`).
    pub csv_path:         String,
    /// Column names available to map. Empty until loaded (DB) or parsed (CSV).
    pub columns:          Vec<String>,
    pub map:              FieldMap,
    /// Literal username/password to apply to every imported receiver when the
    /// corresponding column is left unmapped. `port_value` defaults to "22".
    pub user_value:       String,
    pub pass_value:       String,
    pub port_value:       String,
    /// Optional row filter: only rows where `filter_column <op> filter_value`
    /// are imported. Inert when `filter_column` is `None` or the value is blank.
    pub filter_column:    Option<String>,
    pub filter_op:        ImportFilterOp,
    pub filter_value:     String,
    /// True while DB columns are being fetched.
    pub loading:          bool,
    pub error:            Option<String>,
    pub target_container: u64,
}

impl ImportDraft {
    fn new(source: ImportSource, target_container: u64) -> Self {
        ImportDraft {
            source,
            origin:           String::new(),
            csv_path:         String::new(),
            columns:          Vec::new(),
            map:              FieldMap::default(),
            user_value:       String::new(),
            pass_value:       String::new(),
            port_value:       DEFAULT_PORT.to_string(),
            filter_column:    None,
            filter_op:        ImportFilterOp::default(),
            filter_value:     String::new(),
            loading:          false,
            error:            None,
            target_container,
        }
    }

    fn set(&mut self, field: MapField, column: Option<String>) {
        match field {
            MapField::Name     => self.map.name     = column,
            MapField::Host     => self.map.host     = column,
            MapField::Username => self.map.username = column,
            MapField::Password => self.map.password = column,
            MapField::Port     => self.map.port     = column,
        }
    }

    /// True once a host column is mapped — the minimum needed to create
    /// receivers.
    pub fn is_ready(&self) -> bool {
        self.map.host.is_some() && !self.columns.is_empty()
    }

    /// The ` WHERE …` clause for a database import, or an empty string when no
    /// filter is set. The value is quoted via the engine's rules; `Contains`
    /// becomes a `LIKE '%value%'`.
    pub fn db_where(&self, engine: crate::db::DbEngine) -> String {
        let Some(col) = self.filter_column.as_ref() else { return String::new(); };
        let val = self.filter_value.trim();
        if val.is_empty() { return String::new(); }
        let lit = match self.filter_op {
            ImportFilterOp::Contains => engine.quote_literal(&format!("%{val}%")),
            _                        => engine.quote_literal(val),
        };
        format!(" WHERE {} {} {}", engine.quote_ident(col), self.filter_op.sql(), lit)
    }
}

// ── embedded SSH terminal ───────────────────────────────────────────────────

/// A live SSH session shown inside the Receivers panel. One may exist per
/// receiver; reconnecting the *same* receiver replaces its session, but
/// connecting a different receiver leaves the others running so the user can
/// switch between connected clients without losing any pane.
pub struct SshTerminal {
    pub id:       TerminalId,
    pub receiver: u64,
    pub name:     String,
    pub pane:     TerminalPane,
    /// True when keyboard input should route to this terminal.
    pub focused:  bool,
    /// Set once the ssh process exits. The pane stays visible (its grid still
    /// holds the final output — e.g. a "Permission denied" or "Connection
    /// refused" line) so the user can read why, rather than the pane vanishing.
    pub exited:   bool,
}

// ── state ───────────────────────────────────────────────────────────────────

pub struct ReceiversState {
    pub containers: Vec<Container>,
    pub receivers:  Vec<Receiver>,
    /// Expanded container ids in the tree.
    pub expanded:   HashSet<u64>,
    pub selected_container: Option<u64>,
    pub selected_receiver:  Option<u64>,
    // Editable mirrors of the selected receiver's fields.
    pub name_input:     String,
    pub host_input:     String,
    pub user_input:     String,
    pub password_input: String,
    pub port_input:     String,
    /// Active import flow, if any.
    pub import:  Option<ImportDraft>,
    /// Live SSH sessions, at most one per receiver. Connecting a new receiver
    /// appends rather than replacing, so panes for other clients stay alive.
    pub ssh:     Vec<SshTerminal>,
    /// Last copy/import outcome line shown in the panel.
    pub status:  Option<Result<String, String>>,
    next_id:     u64,
    /// File this inventory persists to, captured once at `load()`. `None`
    /// disables saving entirely — tests construct state with `None` (or a temp
    /// path) so a `cargo test` run can never overwrite the user's real
    /// `~/.frms/receivers.json`.
    persist_path: Option<PathBuf>,
}

impl ReceiversState {
    pub fn load() -> Self {
        let (containers, receivers) = read_from_disk().unwrap_or_default();
        let next_id = containers.iter().map(|c| c.id)
            .chain(receivers.iter().map(|r| r.id))
            .max()
            .unwrap_or(0)
            + 1;
        // Expand all root containers by default so the tree isn't a wall of
        // collapsed rows on first open.
        let expanded = containers.iter()
            .filter(|c| c.parent.is_none())
            .map(|c| c.id)
            .collect();
        Self {
            containers,
            receivers,
            expanded,
            selected_container: None,
            selected_receiver:  None,
            name_input:     String::new(),
            host_input:     String::new(),
            user_input:     String::new(),
            password_input: String::new(),
            port_input:     String::new(),
            import:  None,
            ssh:     Vec::new(),
            status:  None,
            next_id,
            persist_path: receivers_path(),
        }
    }

    pub fn save(&self) {
        let Some(path) = self.persist_path.clone() else { return; };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let doc = to_value(&self.containers, &self.receivers);
        if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
            if std::fs::write(&path, bytes).is_ok() {
                // Credentials live here in plaintext; keep them owner-only.
                crate::port::fs::restrict_to_owner(&path);
            }
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ── tree queries ────────────────────────────────────────────────────────

    pub fn root_containers(&self) -> Vec<&Container> {
        self.containers.iter().filter(|c| c.parent.is_none()).collect()
    }

    pub fn child_containers(&self, parent: u64) -> Vec<&Container> {
        self.containers.iter().filter(|c| c.parent == Some(parent)).collect()
    }

    pub fn receivers_in(&self, container: u64) -> Vec<&Receiver> {
        self.receivers.iter().filter(|r| r.container == container).collect()
    }

    pub fn container_by_id(&self, id: u64) -> Option<&Container> {
        self.containers.iter().find(|c| c.id == id)
    }

    pub fn receiver_by_id(&self, id: u64) -> Option<&Receiver> {
        self.receivers.iter().find(|r| r.id == id)
    }

    pub fn selected_receiver(&self) -> Option<&Receiver> {
        self.selected_receiver.and_then(|id| self.receiver_by_id(id))
    }

    pub fn is_expanded(&self, id: u64) -> bool {
        self.expanded.contains(&id)
    }

    pub fn toggle_expanded(&mut self, id: u64) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
    }

    // ── mutations ───────────────────────────────────────────────────────────

    /// Create a container under `parent` (`None` = root), select it, expand it,
    /// and return its id.
    pub fn add_container(&mut self, parent: Option<u64>) -> u64 {
        let id = self.alloc_id();
        let name = if parent.is_some() { "New location" } else { "New client" };
        self.containers.push(Container { id, name: name.to_string(), parent });
        if let Some(p) = parent { self.expanded.insert(p); }
        self.expanded.insert(id);
        self.selected_container = Some(id);
        self.selected_receiver  = None;
        self.save();
        id
    }

    /// Create a receiver under `container`, select it (filling the edit form),
    /// and return its id.
    pub fn add_receiver(&mut self, container: u64) -> u64 {
        let id = self.alloc_id();
        let r = Receiver {
            id,
            name:      "New receiver".to_string(),
            host:      String::new(),
            username:  String::new(),
            password:  String::new(),
            port:      DEFAULT_PORT,
            container,
        };
        self.receivers.push(r);
        self.expanded.insert(container);
        self.select_receiver(id);
        self.save();
        id
    }

    /// Delete a container and everything beneath it (descendant containers and
    /// their receivers).
    pub fn delete_container(&mut self, id: u64) {
        // Gather the subtree of container ids (id plus all transitive children).
        let mut doomed = vec![id];
        let mut i = 0;
        while i < doomed.len() {
            let parent = doomed[i];
            for c in &self.containers {
                if c.parent == Some(parent) {
                    doomed.push(c.id);
                }
            }
            i += 1;
        }
        self.receivers.retain(|r| !doomed.contains(&r.container));
        self.containers.retain(|c| !doomed.contains(&c.id));
        for d in &doomed { self.expanded.remove(d); }
        if self.selected_container.map(|s| doomed.contains(&s)).unwrap_or(false) {
            self.selected_container = None;
        }
        if self.selected_receiver().is_none() {
            self.clear_receiver_selection();
        }
        self.save();
    }

    pub fn delete_receiver(&mut self, id: u64) {
        self.receivers.retain(|r| r.id != id);
        if self.selected_receiver == Some(id) {
            self.clear_receiver_selection();
        }
        self.save();
    }

    pub fn select_container(&mut self, id: u64) {
        self.selected_container = Some(id);
        self.selected_receiver  = None;
    }

    pub fn rename_container(&mut self, id: u64, name: String) {
        if let Some(c) = self.containers.iter_mut().find(|c| c.id == id) {
            c.name = name;
            self.save();
        }
    }

    pub fn select_receiver(&mut self, id: u64) {
        if let Some(r) = self.receivers.iter().find(|r| r.id == id) {
            self.selected_receiver  = Some(id);
            self.selected_container = Some(r.container);
            self.name_input     = r.name.clone();
            self.host_input     = r.host.clone();
            self.user_input     = r.username.clone();
            self.password_input = r.password.clone();
            self.port_input     = r.port.to_string();
        }
    }

    fn clear_receiver_selection(&mut self) {
        self.selected_receiver = None;
        self.name_input.clear();
        self.host_input.clear();
        self.user_input.clear();
        self.password_input.clear();
        self.port_input.clear();
    }

    /// Apply the current edit-form mirrors to the selected receiver and persist.
    /// `port_input` is parsed leniently: a blank or invalid value keeps the
    /// default port rather than rejecting the edit.
    pub fn commit_edits(&mut self) {
        let Some(id) = self.selected_receiver else { return; };
        let port = self.port_input.trim().parse::<u16>().unwrap_or(DEFAULT_PORT);
        if let Some(r) = self.receivers.iter_mut().find(|r| r.id == id) {
            r.name     = self.name_input.clone();
            r.host     = self.host_input.clone();
            r.username = self.user_input.clone();
            r.password = self.password_input.clone();
            r.port     = port;
        }
        self.save();
    }

    // ── SSH terminals ─────────────────────────────────────────────────────────

    pub fn ssh_mut(&mut self, id: TerminalId) -> Option<&mut SshTerminal> {
        self.ssh.iter_mut().find(|t| t.id == id)
    }

    /// True when some live SSH session owns terminal `id`.
    pub fn has_ssh(&self, id: TerminalId) -> bool {
        self.ssh.iter().any(|t| t.id == id)
    }

    /// The SSH terminal that currently holds keyboard focus, if any. At most
    /// one is ever focused (see [`focus_ssh_for`] / [`unfocus_ssh`]).
    pub fn focused_ssh_mut(&mut self) -> Option<&mut SshTerminal> {
        self.ssh.iter_mut().find(|t| t.focused)
    }

    /// Drop keyboard focus from every SSH terminal (another pane took it).
    pub fn unfocus_ssh(&mut self) {
        for t in &mut self.ssh {
            t.focused = false;
        }
    }

    /// Make `receiver`'s SSH session (if it has one) the sole focused terminal,
    /// so the pane shown for the selected receiver is always the keyboard
    /// target. Receivers without a session simply clear SSH focus.
    pub fn focus_ssh_for(&mut self, receiver: u64) {
        for t in &mut self.ssh {
            t.focused = t.receiver == receiver;
        }
    }

    /// Install `term` as the live session for its receiver, replacing any prior
    /// session to the *same* receiver and dropping focus from the rest. Sessions
    /// to other receivers are left running.
    pub fn set_ssh(&mut self, term: SshTerminal) {
        self.ssh.retain(|t| t.receiver != term.receiver);
        for t in &mut self.ssh {
            t.focused = false;
        }
        self.ssh.push(term);
    }

    /// Tear down the SSH session for `receiver` (Disconnect / Close), leaving
    /// other clients' sessions untouched.
    pub fn remove_ssh_for(&mut self, receiver: u64) {
        self.ssh.retain(|t| t.receiver != receiver);
    }

    // ── import flow ─────────────────────────────────────────────────────────

    /// Begin a database import targeting `container`. The table is chosen next
    /// (via [`set_import_table`]); columns are filled in via
    /// [`set_import_columns`] once fetched.
    pub fn begin_db_import(&mut self, container: u64) {
        self.import = Some(ImportDraft::new(ImportSource::Database, container));
    }

    /// Record the chosen DB table and mark columns as loading. Returns the
    /// target container so the caller can kick off the column fetch.
    pub fn set_import_table(&mut self, table_key: String) {
        if let Some(d) = &mut self.import {
            d.origin  = table_key;
            d.columns.clear();
            d.map     = FieldMap::default();
            // Columns are about to change, so any column-based filter is stale.
            d.filter_column = None;
            d.loading = true;
            d.error   = None;
        }
    }

    /// Begin a CSV import targeting `container`.
    pub fn begin_csv_import(&mut self, container: u64) {
        self.import = Some(ImportDraft::new(ImportSource::Csv, container));
    }

    pub fn cancel_import(&mut self) {
        self.import = None;
    }

    pub fn set_import_columns(&mut self, columns: Vec<String>) {
        if let Some(d) = &mut self.import {
            // Pre-map any column whose name obviously matches a field so the
            // common case needs no manual mapping.
            d.map = guess_map(&columns);
            d.columns = columns;
            d.loading = false;
        }
    }

    pub fn set_import_error(&mut self, err: String) {
        if let Some(d) = &mut self.import {
            d.error = Some(err);
            d.loading = false;
        }
    }

    pub fn set_import_field(&mut self, field: MapField, column: Option<String>) {
        if let Some(d) = &mut self.import {
            d.set(field, column);
        }
    }

    /// Set the literal username/password/port value applied when the matching
    /// column is unmapped. Ignored for `Name`/`Host` (which have no literal).
    pub fn set_import_literal(&mut self, field: MapField, value: String) {
        if let Some(d) = &mut self.import {
            match field {
                MapField::Username => d.user_value = value,
                MapField::Password => d.pass_value = value,
                MapField::Port     => d.port_value = value,
                _                  => {}
            }
        }
    }

    pub fn set_import_filter_column(&mut self, column: Option<String>) {
        if let Some(d) = &mut self.import { d.filter_column = column; }
    }

    pub fn set_import_filter_op(&mut self, op: ImportFilterOp) {
        if let Some(d) = &mut self.import { d.filter_op = op; }
    }

    pub fn set_import_filter_value(&mut self, value: String) {
        if let Some(d) = &mut self.import { d.filter_value = value; }
    }

    pub fn set_csv_path(&mut self, path: String) {
        if let Some(d) = &mut self.import {
            d.csv_path = path;
        }
    }

    /// Create receivers from `rows` (ordered to match `import.columns`) using
    /// the current field mapping, under the draft's target container. Returns
    /// the number created. Clears the import on success.
    pub fn create_from_rows(&mut self, rows: Vec<Vec<String>>) -> usize {
        let Some(d) = self.import.clone() else { return 0; };
        let idx = |col: &Option<String>| -> Option<usize> {
            col.as_ref().and_then(|c| d.columns.iter().position(|x| x == c))
        };
        let name_i = idx(&d.map.name);
        let host_i = idx(&d.map.host);
        let user_i = idx(&d.map.username);
        let pass_i = idx(&d.map.password);
        let port_i = idx(&d.map.port);
        let filter_i = d.filter_column.as_ref()
            .and_then(|c| d.columns.iter().position(|x| x == c));
        let filter_val = d.filter_value.trim();

        let get = |row: &[String], i: Option<usize>| -> Option<String> {
            i.and_then(|i| row.get(i)).map(|s| s.trim().to_string())
        };

        // Literal fallbacks applied when the column for a field is unmapped.
        let user_value = d.user_value.trim();
        let port_default = d.port_value.trim().parse::<u16>().unwrap_or(DEFAULT_PORT);

        let mut created = 0;
        let target = d.target_container;
        let mut new_receivers = Vec::new();
        for row in &rows {
            // Apply the optional row filter (also covers DB rows, which are
            // already filtered server-side — re-checking is a harmless no-op).
            if let Some(fi) = filter_i {
                if !filter_val.is_empty() {
                    let cell = row.get(fi).map(String::as_str).unwrap_or("");
                    if !d.filter_op.matches(cell, filter_val) { continue; }
                }
            }
            let host = match get(row, host_i) {
                Some(h) if !h.is_empty() => h,
                _ => continue, // skip rows without a host
            };
            let id = self.alloc_id();
            let name = get(row, name_i).filter(|s| !s.is_empty()).unwrap_or_else(|| host.clone());
            // A mapped column wins; otherwise fall back to the typed literal.
            let username = get(row, user_i)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| user_value.to_string());
            let password = get(row, pass_i)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| d.pass_value.clone());
            let port = get(row, port_i)
                .and_then(|s| s.parse::<u16>().ok())
                .unwrap_or(port_default);
            new_receivers.push(Receiver {
                id, name, host, username, password, port,
                container: target,
            });
            created += 1;
        }
        self.receivers.extend(new_receivers);
        if created > 0 {
            self.expanded.insert(target);
            self.save();
        }
        self.import = None;
        created
    }
}

/// Guess a field mapping from column names so an obvious schema needs no manual
/// mapping. Case-insensitive substring matches; the host match is intentionally
/// strict so a `username` column isn't mistaken for `host`.
fn guess_map(columns: &[String]) -> FieldMap {
    let find = |needles: &[&str]| -> Option<String> {
        columns.iter()
            .find(|c| {
                let lc = c.to_ascii_lowercase();
                needles.iter().any(|n| lc == *n || lc.contains(n))
            })
            .cloned()
    };
    FieldMap {
        name:     find(&["name", "label", "receiver"]),
        host:     find(&["host", "url", "address", "ip", "hostname"]),
        username: find(&["username", "user", "login"]),
        password: find(&["password", "pass", "pwd", "secret"]),
        port:     find(&["port"]),
    }
}

/// Parse a simple CSV into `(header, rows)`. Naive: splits on commas and does
/// not handle quoted fields containing commas — adequate for the flat
/// host/user/password exports this targets. Returns `None` when empty.
pub fn parse_csv(text: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines.next()?
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    if header.is_empty() {
        return None;
    }
    let rows: Vec<Vec<String>> = lines
        .map(|l| l.split(',').map(|s| s.trim().to_string()).collect())
        .collect();
    Some((header, rows))
}

fn receivers_path() -> Option<PathBuf> {
    Some(crate::port::dirs::config()?.join("receivers.json"))
}

/// Serialize the inventory to the on-disk JSON shape. Split out so the
/// save/load round-trip (credentials included) can be unit-tested without
/// touching `$HOME`.
fn to_value(containers: &[Container], receivers: &[Receiver]) -> Value {
    let containers: Vec<Value> = containers.iter().map(|c| json!({
        "id":     c.id,
        "name":   c.name,
        "parent": c.parent,
    })).collect();
    let receivers: Vec<Value> = receivers.iter().map(|r| json!({
        "id":        r.id,
        "name":      r.name,
        "host":      r.host,
        "username":  r.username,
        "password":  r.password,
        "port":      r.port,
        "container": r.container,
    })).collect();
    json!({ "containers": containers, "receivers": receivers })
}

/// Parse the on-disk JSON shape back into the inventory. Unknown fields are
/// ignored and missing optional fields fall back to defaults.
fn from_value(v: &Value) -> (Vec<Container>, Vec<Receiver>) {
    let containers = v.get("containers")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|c| Some(Container {
            id:     c.get("id")?.as_u64()?,
            name:   c.get("name")?.as_str()?.to_string(),
            parent: c.get("parent").and_then(Value::as_u64),
        })).collect())
        .unwrap_or_default();

    let receivers = v.get("receivers")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(|r| Some(Receiver {
            id:        r.get("id")?.as_u64()?,
            name:      r.get("name")?.as_str()?.to_string(),
            host:      r.get("host").and_then(Value::as_str).unwrap_or_default().to_string(),
            username:  r.get("username").and_then(Value::as_str).unwrap_or_default().to_string(),
            password:  r.get("password").and_then(Value::as_str).unwrap_or_default().to_string(),
            port:      r.get("port").and_then(Value::as_u64).unwrap_or(DEFAULT_PORT as u64) as u16,
            container: r.get("container")?.as_u64()?,
        })).collect())
        .unwrap_or_default();

    (containers, receivers)
}

fn read_from_disk() -> Option<(Vec<Container>, Vec<Receiver>)> {
    let path  = receivers_path()?;
    let bytes = std::fs::read(&path).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some(from_value(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_state() -> ReceiversState {
        ReceiversState {
            containers: Vec::new(),
            receivers:  Vec::new(),
            expanded:   HashSet::new(),
            selected_container: None,
            selected_receiver:  None,
            name_input:     String::new(),
            host_input:     String::new(),
            user_input:     String::new(),
            password_input: String::new(),
            port_input:     String::new(),
            import:  None,
            ssh:     Vec::new(),
            status:  None,
            next_id: 1,
            // No persistence: keeps mutating tests from clobbering the real
            // ~/.frms/receivers.json. The disk path is exercised separately by
            // `save_writes_to_path_and_reloads`.
            persist_path: None,
        }
    }

    #[test]
    fn parse_csv_reads_header_and_rows() {
        let (header, rows) = parse_csv("name, host, user\nr1, 10.0.0.1, root\n\nr2,10.0.0.2,admin")
            .expect("parses");
        assert_eq!(header, vec!["name", "host", "user"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["r1", "10.0.0.1", "root"]);
        assert_eq!(rows[1], vec!["r2", "10.0.0.2", "admin"]);
    }

    #[test]
    fn parse_csv_empty_is_none() {
        assert!(parse_csv("   \n\n").is_none());
    }

    #[test]
    fn delete_container_removes_subtree() {
        let mut s = empty_state();
        let client = s.add_container(None);
        let loc    = s.add_container(Some(client));
        let r1     = s.add_receiver(loc);
        let other  = s.add_container(None);
        s.delete_container(client);
        assert!(s.container_by_id(client).is_none());
        assert!(s.container_by_id(loc).is_none());
        assert!(s.receiver_by_id(r1).is_none());
        // An unrelated root container survives.
        assert!(s.container_by_id(other).is_some());
    }

    #[test]
    fn guess_map_matches_common_columns() {
        let cols = vec!["Name".into(), "IP_Address".into(), "Login".into(), "Secret".into()];
        let m = guess_map(&cols);
        assert_eq!(m.name.as_deref(), Some("Name"));
        assert_eq!(m.host.as_deref(), Some("IP_Address"));
        assert_eq!(m.username.as_deref(), Some("Login"));
        assert_eq!(m.password.as_deref(), Some("Secret"));
    }

    #[test]
    fn create_from_rows_applies_mapping_and_skips_hostless() {
        let mut s = empty_state();
        let c = s.add_container(None);
        s.begin_csv_import(c);
        s.set_import_columns(vec!["name".into(), "host".into(), "user".into(), "port".into()]);
        // host auto-mapped by guess; ensure explicit too.
        s.set_import_field(MapField::Host, Some("host".into()));
        s.set_import_field(MapField::Name, Some("name".into()));
        s.set_import_field(MapField::Username, Some("user".into()));
        s.set_import_field(MapField::Port, Some("port".into()));
        let rows = vec![
            vec!["alpha".into(), "10.0.0.1".into(), "root".into(), "2200".into()],
            vec!["nohost".into(), "".into(), "x".into(), "22".into()], // skipped
        ];
        let n = s.create_from_rows(rows);
        assert_eq!(n, 1);
        let recvs = s.receivers_in(c);
        assert_eq!(recvs.len(), 1);
        assert_eq!(recvs[0].host, "10.0.0.1");
        assert_eq!(recvs[0].port, 2200);
        assert!(s.import.is_none());
    }

    #[test]
    fn import_literals_fill_in_for_unmapped_columns() {
        let mut s = empty_state();
        let c = s.add_container(None);
        s.begin_csv_import(c);
        s.set_import_columns(vec!["host".into()]);
        s.set_import_field(MapField::Host, Some("host".into()));
        // No username/password/port columns — supply literals instead.
        s.set_import_literal(MapField::Username, "root".into());
        s.set_import_literal(MapField::Password, "hunter2".into());
        s.set_import_literal(MapField::Port, "2222".into());
        let n = s.create_from_rows(vec![vec!["10.0.0.5".into()]]);
        assert_eq!(n, 1);
        let r = s.receivers_in(c)[0];
        assert_eq!(r.username, "root");
        assert_eq!(r.password, "hunter2");
        assert_eq!(r.port, 2222);
    }

    #[test]
    fn import_filter_skips_non_matching_rows() {
        let mut s = empty_state();
        let c = s.add_container(None);
        s.begin_csv_import(c);
        s.set_import_columns(vec!["host".into(), "status".into()]);
        s.set_import_field(MapField::Host, Some("host".into()));
        s.set_import_filter_column(Some("status".into()));
        s.set_import_filter_op(ImportFilterOp::Equals);
        s.set_import_filter_value("active".into());
        let rows = vec![
            vec!["10.0.0.1".into(), "active".into()],
            vec!["10.0.0.2".into(), "retired".into()],
        ];
        let n = s.create_from_rows(rows);
        assert_eq!(n, 1);
        assert_eq!(s.receivers_in(c)[0].host, "10.0.0.1");
    }

    #[test]
    fn db_where_quotes_column_and_value() {
        use crate::db::DbEngine;
        let mut s = empty_state();
        let c = s.add_container(None);
        s.begin_db_import(c);
        s.set_import_columns(vec!["region".into()]);
        s.set_import_filter_column(Some("region".into()));
        s.set_import_filter_op(ImportFilterOp::Contains);
        s.set_import_filter_value("east".into());
        let draft = s.import.as_ref().unwrap();
        assert_eq!(draft.db_where(DbEngine::Postgres), " WHERE \"region\" LIKE '%east%'");
        // No filter → empty clause.
        s.set_import_filter_column(None);
        assert_eq!(s.import.as_ref().unwrap().db_where(DbEngine::Postgres), "");
    }

    #[test]
    fn persistence_round_trips_tree_and_credentials() {
        // Build a Client → Location → receiver tree with credentials.
        let mut s = empty_state();
        let client = s.add_container(None);
        let loc    = s.add_container(Some(client));
        let rid    = s.add_receiver(loc);
        s.select_receiver(rid);
        s.host_input     = "10.0.0.9".into();
        s.user_input     = "tsruser".into();
        s.password_input = "s3cr3t".into();
        s.port_input     = "2022".into();
        s.name_input     = "Receiver 1".into();
        s.commit_edits();

        // Serialize exactly as save() does, then reload as load() does.
        let doc = to_value(&s.containers, &s.receivers);
        let (containers, receivers) = from_value(&doc);

        assert_eq!(containers.len(), 2);
        assert_eq!(receivers.len(), 1);
        let r = &receivers[0];
        assert_eq!(r.name, "Receiver 1");
        assert_eq!(r.host, "10.0.0.9");
        assert_eq!(r.username, "tsruser");
        assert_eq!(r.password, "s3cr3t"); // credentials survive the restart
        assert_eq!(r.port, 2022);
        assert_eq!(r.container, loc);
        // The location's parent link back to the client is preserved.
        let loc_c = containers.iter().find(|c| c.id == loc).unwrap();
        assert_eq!(loc_c.parent, Some(client));
    }

    #[test]
    fn save_writes_to_path_and_reloads() {
        // Drive the real on-disk save() path (fs write + 0600), isolated to a
        // temp file so it never touches the user's ~/.frms/receivers.json.
        let dir  = std::env::temp_dir().join(format!("frms_recv_test_{}", std::process::id()));
        let _    = std::fs::create_dir_all(&dir);
        let file = dir.join("receivers.json");

        let mut s = empty_state();
        s.persist_path = Some(file.clone());
        let client = s.add_container(None);          // each mutation calls save()
        let loc    = s.add_container(Some(client));
        let rid    = s.add_receiver(loc);
        s.select_receiver(rid);
        s.host_input     = "10.1.2.3".into();
        s.user_input     = "root".into();
        s.password_input = "pw".into();
        s.port_input     = "2200".into();
        s.commit_edits();

        // Read it back exactly as load() does and confirm the tree survives.
        let bytes = std::fs::read(&file).expect("save() wrote the file");
        let v: Value = serde_json::from_slice(&bytes).expect("valid json");
        let (containers, receivers) = from_value(&v);
        assert_eq!(containers.len(), 2);
        assert_eq!(receivers.len(), 1);
        assert_eq!(receivers[0].host, "10.1.2.3");
        assert_eq!(receivers[0].password, "pw");
        assert_eq!(receivers[0].port, 2200);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fake_ssh(id: TerminalId, receiver: u64, focused: bool) -> SshTerminal {
        // A pane that points at /bin/true — spawns and exits immediately, but we
        // only exercise the bookkeeping (focus, per-receiver replace), never the
        // PTY, so the child lifecycle is irrelevant here.
        let pane = TerminalPane::spawn(id, "true", None).expect("spawn true");
        SshTerminal { id, receiver, name: "t".into(), pane, focused, exited: false }
    }

    #[test]
    fn set_ssh_keeps_other_receivers_and_steals_focus() {
        let mut s = empty_state();
        // Two different receivers connect; both sessions must survive.
        s.set_ssh(fake_ssh(1, 100, true));
        s.set_ssh(fake_ssh(2, 200, true));
        assert_eq!(s.ssh.len(), 2);
        // Only the most recent connection holds focus.
        assert!(!s.ssh.iter().find(|t| t.id == 1).unwrap().focused);
        assert!(s.ssh.iter().find(|t| t.id == 2).unwrap().focused);

        // Reconnecting receiver 100 replaces *its* session (new id 3) but leaves
        // receiver 200's session running.
        s.set_ssh(fake_ssh(3, 100, true));
        assert_eq!(s.ssh.len(), 2);
        assert!(s.has_ssh(3));
        assert!(!s.has_ssh(1));
        assert!(s.has_ssh(2));
    }

    #[test]
    fn focus_ssh_for_follows_selected_receiver() {
        let mut s = empty_state();
        s.set_ssh(fake_ssh(1, 100, false));
        s.set_ssh(fake_ssh(2, 200, false));
        // Selecting receiver 100 routes keys to its session alone.
        s.focus_ssh_for(100);
        assert_eq!(s.focused_ssh_mut().map(|t| t.id), Some(1));
        // Selecting a receiver with no session clears SSH focus entirely.
        s.focus_ssh_for(999);
        assert!(s.focused_ssh_mut().is_none());
    }

    #[test]
    fn remove_ssh_for_drops_only_that_receiver() {
        let mut s = empty_state();
        s.set_ssh(fake_ssh(1, 100, true));
        s.set_ssh(fake_ssh(2, 200, false));
        s.remove_ssh_for(100);
        assert!(!s.has_ssh(1));
        assert!(s.has_ssh(2));
    }

    #[test]
    fn ssh_command_keeps_password_in_env() {
        let r = Receiver {
            id: 1, name: "r".into(), host: "h".into(), username: "u".into(),
            password: "secret".into(), port: 2222, container: 0,
        };
        let (prog, args, envs) = r.ssh_command();
        assert_eq!(prog, "sshpass");
        assert!(args.contains(&"u@h".to_string()));
        assert!(args.contains(&"2222".to_string()));
        // Password must not appear in the argument list.
        assert!(!args.iter().any(|a| a.contains("secret")));
        assert_eq!(envs, vec![("SSHPASS".to_string(), "secret".to_string())]);
    }
}
