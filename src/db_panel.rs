//! Per-session UI state for the database browser side panel.

use std::collections::{HashMap, HashSet};

use iced::widget::combo_box;

use crate::db::{DbClient, DbConfig, QueryResult, TableRef};
use crate::db_history::DbHistory;

#[derive(Debug, Clone)]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Failed(String),
}

pub struct DbPanel {
    /// True while keyboard focus is "in" the DB form. Drives whether Tab
    /// cycles through form fields or is sent to the active terminal.
    pub focus_active:      bool,
    /// Form values for the connection.
    pub config:            DbConfig,
    /// Current connection state.
    pub conn_state:        ConnState,
    /// Live client handle once connected.
    pub client:            Option<DbClient>,
    /// All tables visible to the connected user.
    pub tables:            Vec<TableRef>,
    /// Tables the user has expanded — populated columns appear under each.
    pub selected_tables:   HashSet<String>,
    /// Cached column names per table key.
    pub columns_by_table:  HashMap<String, Vec<String>>,
    /// Tables whose columns are still loading.
    pub loading_columns:   HashSet<String>,
    /// User-picked columns: table key → set of column names.
    pub selected_columns:  HashMap<String, HashSet<String>>,
    /// Editable SQL — generated on demand, runnable via the Run button.
    pub query:             String,
    /// Last query result (for display).
    pub query_result:      Option<QueryResult>,
    /// Last query error message.
    pub query_error:       Option<String>,
    /// True while a query is in flight.
    pub query_running:     bool,
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
            host_combo:       combo_box::State::new(history.host.clone()),
            port_combo:       combo_box::State::new(history.port.clone()),
            database_combo:   combo_box::State::new(history.database.clone()),
            user_combo:       combo_box::State::new(history.user.clone()),
            config,
            conn_state:       ConnState::Disconnected,
            client:           None,
            tables:           Vec::new(),
            selected_tables:  HashSet::new(),
            columns_by_table: HashMap::new(),
            loading_columns:  HashSet::new(),
            selected_columns: HashMap::new(),
            query:            String::new(),
            query_result:     None,
            query_error:      None,
            query_running:    false,
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

    /// Build a `SELECT` from the user's selected tables and columns. Joins
    /// multiple tables with commas (CROSS JOIN) — the user can edit the
    /// generated text to add real join conditions before running.
    pub fn build_select(&self) -> Option<String> {
        if self.selected_tables.is_empty() { return None; }

        let engine = self
            .client
            .as_ref()
            .map(DbClient::engine)
            .unwrap_or(self.config.engine);

        let chosen: Vec<&TableRef> = self
            .tables
            .iter()
            .filter(|t| self.selected_tables.contains(&t.key()))
            .collect();
        if chosen.is_empty() { return None; }

        let mut select_parts: Vec<String> = Vec::new();
        for t in &chosen {
            let key  = t.key();
            let cols = self.selected_columns.get(&key);
            let qual = t.quoted(engine);
            match cols {
                Some(cols) if !cols.is_empty() => {
                    let ordered = self
                        .columns_by_table
                        .get(&key)
                        .map(|all| {
                            all.iter()
                                .filter(|c| cols.contains(*c))
                                .cloned()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_else(|| cols.iter().cloned().collect());
                    for c in ordered {
                        select_parts.push(format!("{qual}.{}", engine.quote_ident(&c)));
                    }
                }
                _ => {
                    select_parts.push(format!("{qual}.*"));
                }
            }
        }

        let from_parts: Vec<String> = chosen.iter().map(|t| t.quoted(engine)).collect();
        let select = select_parts.join(", ");
        let from   = from_parts.join(", ");
        Some(format!("SELECT {select} FROM {from} LIMIT 100"))
    }
}
