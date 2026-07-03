//! Per-session UI state for the database browser side panel.

use std::collections::{HashMap, HashSet};

use iced::widget::{combo_box, text_editor};

use crate::db::{DbClient, DbConfig, DbEngine, ForeignKey, QueryResult, RoutineRef, TableRef};
use crate::db_history::DbHistory;

#[derive(Debug, Clone)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

/// One column picked for the query result set. The query builder keeps these
/// in an explicit, user-reorderable order (Col 3 of the builder grid), which
/// is why selections live in an ordered `Vec` rather than a per-table set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColRef {
    /// Owning table key — `schema.name`.
    pub table:  String,
    pub column: String,
}

impl std::fmt::Display for ColRef {
    /// Shown in the WHERE-clause column picker — uses the short table name so
    /// the dropdown stays readable in the narrow Filters column.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let short = self.table.rsplit('.').next().unwrap_or(&self.table);
        write!(f, "{short}.{}", self.column)
    }
}

/// Comparison used by a single WHERE-clause filter row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Like,
    IsNull,
    IsNotNull,
}

impl FilterOp {
    pub const ALL: [FilterOp; 9] = [
        FilterOp::Eq,
        FilterOp::Ne,
        FilterOp::Lt,
        FilterOp::Lte,
        FilterOp::Gt,
        FilterOp::Gte,
        FilterOp::Like,
        FilterOp::IsNull,
        FilterOp::IsNotNull,
    ];

    /// Whether the operator compares against a value. `IS NULL` / `IS NOT NULL`
    /// stand alone, so their filter rows hide the value input and emit no literal.
    pub fn takes_value(self) -> bool {
        !matches!(self, FilterOp::IsNull | FilterOp::IsNotNull)
    }

    /// The SQL operator/keyword emitted into the query.
    pub fn sql(self) -> &'static str {
        match self {
            FilterOp::Eq        => "=",
            FilterOp::Ne        => "<>",
            FilterOp::Lt        => "<",
            FilterOp::Lte       => "<=",
            FilterOp::Gt        => ">",
            FilterOp::Gte       => ">=",
            FilterOp::Like      => "LIKE",
            FilterOp::IsNull    => "IS NULL",
            FilterOp::IsNotNull => "IS NOT NULL",
        }
    }
}

impl std::fmt::Display for FilterOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.sql())
    }
}

/// How a filter row joins to the one above it. The first filter ignores this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conj {
    And,
    Or,
}

impl Conj {
    pub fn sql(self) -> &'static str {
        match self {
            Conj::And => "AND",
            Conj::Or  => "OR",
        }
    }

    pub fn toggled(self) -> Conj {
        match self {
            Conj::And => Conj::Or,
            Conj::Or  => Conj::And,
        }
    }
}

/// One WHERE-clause condition assembled in the Filters column of the builder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Owning table key — `schema.name`.
    pub table:  String,
    pub column: String,
    pub op:     FilterOp,
    /// Right-hand value for value-taking operators; ignored for IS [NOT] NULL.
    pub value:  String,
    /// How this row joins to the previous one (AND/OR); the first row ignores it.
    pub conj:   Conj,
}

/// A manual `JOIN … ON …` condition for one joined (non-base) table:
/// `left_table.left_col = <this table>.right_col`. Pre-filled from a foreign key
/// when one links the tables, but fully user-editable so schemas without FK
/// constraints can still be joined. `left_table` is always a table included
/// before this one in `selected_tables`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinCond {
    pub left_table: String,
    pub left_col:   String,
    pub right_col:  String,
}

impl JoinCond {
    /// True once all three parts are filled — i.e. it can emit a real `ON`.
    pub fn is_complete(&self) -> bool {
        !self.left_table.is_empty() && !self.left_col.is_empty() && !self.right_col.is_empty()
    }
}

pub struct DbPanel {
    /// True while keyboard focus is "in" the DB form. Drives whether Tab
    /// cycles through form fields or is sent to the active terminal.
    pub focus_active:      bool,
    /// When true the connection form (the collapsible section at the top of the
    /// Database center tab) is collapsed to just its header, freeing room for
    /// the query/results below once a connection is established.
    pub form_collapsed:    bool,
    /// Form values for the connection.
    pub config:            DbConfig,
    /// Current connection state.
    pub conn_state:        ConnState,
    /// Live client handle once connected.
    pub client:            Option<DbClient>,
    /// All tables visible to the connected user.
    pub tables:            Vec<TableRef>,
    /// Foreign-key relationships across the database — used to pre-fill a JOIN
    /// condition when one links a newly added table to the current selection.
    pub foreign_keys:      Vec<ForeignKey>,
    /// Stored procedures and functions visible to the user, listed under the
    /// Tables column so their source can be opened for viewing.
    pub routines:          Vec<RoutineRef>,
    /// The routine whose source is currently being viewed, if any. While set,
    /// the results row shows the routine's code instead of query output.
    pub selected_routine:  Option<RoutineRef>,
    /// Fetched source (DDL) of `selected_routine`, once it has loaded.
    pub routine_code:      Option<String>,
    /// True while a routine's source is being fetched.
    pub routine_loading:   bool,
    /// Tables chosen in Col 1, in selection order. The first is the base table
    /// of the `FROM`; the rest are joined on. Order matters for the builder.
    pub selected_tables:   Vec<String>,
    /// Cached column names per table key.
    pub columns_by_table:  HashMap<String, Vec<String>>,
    /// Tables whose columns are still loading.
    pub loading_columns:   HashSet<String>,
    /// User-picked result columns, in display order (Col 3, reorderable).
    pub selected_cols:     Vec<ColRef>,
    /// WHERE-clause conditions, applied in order and joined by each filter's
    /// AND/OR connector. Empty means no WHERE clause.
    pub filters:           Vec<Filter>,
    /// Columns to GROUP BY, in selection order. Empty means no GROUP BY clause.
    pub group_by:          Vec<ColRef>,
    /// Optional row cap for generated SELECTs, entered in the Filters column.
    /// Empty means no `LIMIT` (every matching row); a positive number appends
    /// `LIMIT n`. Kept digits-only on input so the clause is always valid.
    pub limit_input:       String,
    /// Manual JOIN conditions keyed by joined table key. A non-base table with
    /// no entry (or an incomplete one) falls back to its foreign key, or a
    /// CROSS JOIN when no key links it.
    pub joins:             HashMap<String, JoinCond>,
    /// Index of the result column currently being mouse-dragged to reorder it,
    /// if any. `None` when no drag is in progress.
    pub dragging_field:    Option<usize>,
    /// When true the builder row (Row 1) is collapsed so the results table
    /// (Row 2) takes the full height.
    pub row1_collapsed:    bool,
    /// SQL of the most recently run query (generated from the builder).
    pub query:             String,
    /// Last query result (for display).
    pub query_result:      Option<QueryResult>,
    /// Last query error message.
    pub query_error:       Option<String>,
    /// True while a query is in flight.
    pub query_running:     bool,
    /// Status line for a statement that returned no rows (e.g. "3 rows
    /// affected"), set when a raw-SQL command completes. Shown in the results
    /// row in place of a table; cleared whenever a row-returning query runs.
    pub statement_status:  Option<String>,
    /// When true the builder row (Row 1) shows the free-hand SQL editor instead
    /// of the visual query builder. Lets the user run any statement directly.
    pub sql_mode:          bool,
    /// Free-hand SQL buffer backing the raw editor — typed or pasted verbatim
    /// and run as-is against the connection.
    pub sql_input:         text_editor::Content,
    /// Filter text applied to the Tables/Objects list so the user can jump to a
    /// table or routine without scrolling. Case-insensitive substring match.
    pub object_filter:     String,
    /// Collapsed state of the Tables group in Col 1 (header click toggles it).
    pub tables_collapsed:  bool,
    /// Collapsed state of the Objects (routines) group in Col 1.
    pub objects_collapsed: bool,
    /// Persisted per-field input history for the connection form. Loaded at
    /// panel creation and rewritten every time we commit new entries.
    pub history:           DbHistory,
    /// String mirror of `config.port` so the port combo-box (which is typed
    /// over `String`) has a stable reference to show when unfocused.
    pub port_str:          String,
    /// Combo-box state for each history-enabled field. Drives the
    /// click-to-open dropdown of previously entered values. Passwords are
    /// intentionally not included.
    pub host_combo:        combo_box::State<String>,
    pub port_combo:        combo_box::State<String>,
    pub database_combo:    combo_box::State<String>,
    pub user_combo:        combo_box::State<String>,
}

impl Default for DbPanel {
    fn default() -> Self {
        let history  = DbHistory::load();
        let config   = DbConfig::default();
        let port_str = config.port.to_string();
        Self {
            focus_active:     false,
            form_collapsed:   false,
            host_combo:       combo_box::State::new(history.host.clone()),
            port_combo:       combo_box::State::new(history.port.clone()),
            database_combo:   combo_box::State::new(history.database.clone()),
            user_combo:       combo_box::State::new(history.user.clone()),
            config,
            conn_state:       ConnState::Disconnected,
            client:           None,
            tables:           Vec::new(),
            foreign_keys:     Vec::new(),
            routines:         Vec::new(),
            selected_routine: None,
            routine_code:     None,
            routine_loading:  false,
            selected_tables:  Vec::new(),
            columns_by_table: HashMap::new(),
            loading_columns:  HashSet::new(),
            selected_cols:    Vec::new(),
            filters:          Vec::new(),
            group_by:         Vec::new(),
            limit_input:      String::new(),
            joins:            HashMap::new(),
            dragging_field:   None,
            row1_collapsed:   false,
            query:            String::new(),
            query_result:     None,
            query_error:      None,
            query_running:    false,
            statement_status: None,
            sql_mode:         false,
            sql_input:        text_editor::Content::new(),
            object_filter:    String::new(),
            tables_collapsed: false,
            objects_collapsed: false,
            history,
            port_str,
        }
    }
}

impl DbPanel {
    /// Promote the current form values into the persisted per-field history
    /// and rebuild the combo-box option lists so newly-added entries appear
    /// at the top of each dropdown on the next click.
    pub fn commit_history(&mut self) {
        let port_s = self.config.port.to_string();
        self.history.commit(
            &self.config.host,
            &port_s,
            &self.config.database,
            &self.config.user,
        );
        self.history.save();
        self.host_combo     = combo_box::State::new(self.history.host.clone());
        self.port_combo     = combo_box::State::new(self.history.port.clone());
        self.database_combo = combo_box::State::new(self.history.database.clone());
        self.user_combo     = combo_box::State::new(self.history.user.clone());
    }

    /// Engine of the live connection, falling back to the form's configured
    /// engine when not yet connected.
    pub fn engine(&self) -> DbEngine {
        self.client
            .as_ref()
            .map(DbClient::engine)
            .unwrap_or(self.config.engine)
    }

    pub fn table_by_key(&self, key: &str) -> Option<&TableRef> {
        self.tables.iter().find(|t| t.key() == key)
    }

    pub fn is_col_selected(&self, table: &str, column: &str) -> bool {
        self.selected_cols.iter().any(|c| c.table == table && c.column == column)
    }

    pub fn is_grouped(&self, table: &str, column: &str) -> bool {
        self.group_by.iter().any(|c| c.table == table && c.column == column)
    }

    /// Every (table, column) available from the currently selected tables, in
    /// table-then-column order. Backs the "All fields" toggle.
    pub fn available_cols(&self) -> Vec<ColRef> {
        let mut out = Vec::new();
        for table in &self.selected_tables {
            if let Some(cols) = self.columns_by_table.get(table) {
                for c in cols {
                    out.push(ColRef { table: table.clone(), column: c.clone() });
                }
            }
        }
        out
    }

    /// True when every available field is already in the result set (and there
    /// is at least one) — i.e. the "All fields" box should read as checked.
    pub fn all_fields_selected(&self) -> bool {
        let avail = self.available_cols();
        !avail.is_empty()
            && avail.iter().all(|c| self.is_col_selected(&c.table, &c.column))
    }

    /// A default `JoinCond` for `new_key` derived from a foreign key that links
    /// it (in either direction) to a table already in `included`. `right_col` is
    /// always the column on `new_key`; `left_table`/`left_col` name the included
    /// side. Returns `None` when no key connects them, in which case the user
    /// fills the condition in manually. Backs the FK pre-fill on table add.
    pub fn fk_join_default(&self, new_key: &str, included: &[String]) -> Option<JoinCond> {
        for fk in &self.foreign_keys {
            let from = fk.from_table.key();
            let to   = fk.to_table.key();
            if from == new_key && included.iter().any(|t| *t == to) {
                return Some(JoinCond {
                    left_table: to,
                    left_col:   fk.to_column.clone(),
                    right_col:  fk.from_column.clone(),
                });
            }
            if to == new_key && included.iter().any(|t| *t == from) {
                return Some(JoinCond {
                    left_table: from,
                    left_col:   fk.from_column.clone(),
                    right_col:  fk.to_column.clone(),
                });
            }
        }
        None
    }

    /// The `ON …` predicate joining `new_key` to an already-included table, if
    /// a foreign key links them. Returns the fully-qualified equality.
    fn join_on(&self, new_key: &str, included: &[String], engine: DbEngine) -> Option<String> {
        for fk in &self.foreign_keys {
            let from = fk.from_table.key();
            let to   = fk.to_table.key();
            if from == new_key && included.iter().any(|t| *t == to) {
                return Some(format!(
                    "{}.{} = {}.{}",
                    fk.from_table.quoted(engine), engine.quote_ident(&fk.from_column),
                    fk.to_table.quoted(engine),   engine.quote_ident(&fk.to_column),
                ));
            }
            if to == new_key && included.iter().any(|t| *t == from) {
                return Some(format!(
                    "{}.{} = {}.{}",
                    fk.to_table.quoted(engine),   engine.quote_ident(&fk.to_column),
                    fk.from_table.quoted(engine), engine.quote_ident(&fk.from_column),
                ));
            }
        }
        None
    }

    /// The `ON` predicate for joining `key` to the already-`included` tables.
    /// Prefers the user's manual `JoinCond` (when complete and its left side is
    /// included); otherwise falls back to a foreign-key match. `None` means no
    /// condition is available, so the caller emits a `CROSS JOIN`.
    fn join_clause_for(&self, key: &str, included: &[String], engine: DbEngine) -> Option<String> {
        if let Some(j) = self.joins.get(key) {
            if j.is_complete() && included.iter().any(|t| *t == j.left_table) {
                let left_t  = self.table_by_key(&j.left_table)?;
                let right_t = self.table_by_key(key)?;
                return Some(format!(
                    "{}.{} = {}.{}",
                    left_t.quoted(engine),  engine.quote_ident(&j.left_col),
                    right_t.quoted(engine), engine.quote_ident(&j.right_col),
                ));
            }
        }
        self.join_on(key, included, engine)
    }

    /// Build a `SELECT` from the chosen tables and columns. The base table
    /// opens the `FROM`; each subsequent table is `JOIN`ed on its foreign-key
    /// relationship to the tables already included (falling back to a
    /// `CROSS JOIN` only if no key is found). Columns follow the user's
    /// explicit order; with none picked, every selected table contributes `*`.
    pub fn build_select(&self) -> Option<String> {
        let (base, rest) = self.selected_tables.split_first()?;
        let engine = self.engine();
        let base_t = self.table_by_key(base)?;

        let select = if self.selected_cols.is_empty() {
            self.selected_tables
                .iter()
                .filter_map(|k| self.table_by_key(k))
                .map(|t| format!("{}.*", t.quoted(engine)))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            self.selected_cols
                .iter()
                .filter_map(|c| {
                    let t = self.table_by_key(&c.table)?;
                    Some(format!("{}.{}", t.quoted(engine), engine.quote_ident(&c.column)))
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        let mut included: Vec<String> = vec![base.clone()];
        let mut from = base_t.quoted(engine);
        for key in rest {
            let Some(t) = self.table_by_key(key) else { continue };
            match self.join_clause_for(key, &included, engine) {
                Some(on) => from.push_str(&format!(" JOIN {} ON {on}", t.quoted(engine))),
                None     => from.push_str(&format!(" CROSS JOIN {}", t.quoted(engine))),
            }
            included.push(key.clone());
        }

        Some(format!(
            "SELECT {select} FROM {from}{}{}{}",
            self.where_clause(engine),
            self.group_by_clause(engine),
            self.limit_clause(),
        ))
    }

    /// ` LIMIT n` when the user has set a positive row cap in the Filters
    /// column, else empty — no limit, every matching row is returned. (There is
    /// deliberately no default cap: unlimited unless the user opts in.)
    fn limit_clause(&self) -> String {
        match self.limit_input.trim().parse::<u64>() {
            Ok(n) if n > 0 => format!(" LIMIT {n}"),
            _ => String::new(),
        }
    }

    /// Render the ` GROUP BY …` clause from the chosen grouping columns, or an
    /// empty string when none apply. Columns whose table isn't currently
    /// selected are skipped so an in-progress selection never emits broken SQL.
    fn group_by_clause(&self, engine: DbEngine) -> String {
        let mut cols: Vec<String> = Vec::new();
        for g in &self.group_by {
            if !self.selected_tables.iter().any(|t| *t == g.table) { continue; }
            let Some(t) = self.table_by_key(&g.table) else { continue };
            cols.push(format!("{}.{}", t.quoted(engine), engine.quote_ident(&g.column)));
        }
        if cols.is_empty() {
            String::new()
        } else {
            format!(" GROUP BY {}", cols.join(", "))
        }
    }

    /// Render the ` WHERE …` clause from the current filters, or an empty
    /// string when none apply. Filters are skipped when they reference a table
    /// that isn't selected, have no column, or take a value but have none —
    /// so an in-progress row never produces broken SQL. The AND/OR connector
    /// of the first *emitted* predicate is ignored.
    fn where_clause(&self, engine: DbEngine) -> String {
        let mut predicates: Vec<(Conj, String)> = Vec::new();
        for f in &self.filters {
            if f.column.is_empty() { continue; }
            if !self.selected_tables.iter().any(|t| *t == f.table) { continue; }
            let Some(t) = self.table_by_key(&f.table) else { continue };
            if f.op.takes_value() && f.value.trim().is_empty() { continue; }

            let col = format!("{}.{}", t.quoted(engine), engine.quote_ident(&f.column));
            let pred = if f.op.takes_value() {
                format!("{col} {} {}", f.op.sql(), engine.quote_literal(&f.value))
            } else {
                format!("{col} {}", f.op.sql())
            };
            predicates.push((f.conj, pred));
        }

        if predicates.is_empty() {
            return String::new();
        }

        let mut clause = String::from(" WHERE ");
        for (i, (conj, pred)) in predicates.iter().enumerate() {
            if i == 0 {
                clause.push_str(pred);
            } else {
                clause.push_str(&format!(" {} {pred}", conj.sql()));
            }
        }
        clause
    }
}
