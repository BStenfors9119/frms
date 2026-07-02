//! Per-field input history for the database connection form. Persisted as
//! JSON at `$HOME/.frms/db_history.json` so previously typed hosts, ports,
//! databases, and users survive across restarts and show up as a dropdown
//! under each field. Passwords are intentionally not stored.

use std::path::PathBuf;

use serde_json::{json, Value};

const MAX_PER_FIELD: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct DbHistory {
    pub host:     Vec<String>,
    pub port:     Vec<String>,
    pub database: Vec<String>,
    pub user:     Vec<String>,
}

fn history_path() -> Option<PathBuf> {
    Some(crate::port::dirs::config()?.join("db_history.json"))
}

impl DbHistory {
    pub fn load() -> Self {
        let Some(path)  = history_path()                 else { return Self::default(); };
        let Ok(bytes)   = std::fs::read(&path)           else { return Self::default(); };
        let Ok(v)       = serde_json::from_slice::<Value>(&bytes) else { return Self::default(); };

        Self {
            host:     read_list(&v, "host"),
            port:     read_list(&v, "port"),
            database: read_list(&v, "database"),
            user:     read_list(&v, "user"),
        }
    }

    pub fn save(&self) {
        let Some(path) = history_path() else { return; };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let v = json!({
            "host":     self.host,
            "port":     self.port,
            "database": self.database,
            "user":     self.user,
        });
        if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
            let _ = std::fs::write(&path, bytes);
        }
    }

    /// Promote the given values to the front of their respective histories.
    /// Empty values are skipped; duplicates are moved rather than repeated.
    pub fn commit(&mut self, host: &str, port: &str, database: &str, user: &str) {
        push_front_unique(&mut self.host,     host);
        push_front_unique(&mut self.port,     port);
        push_front_unique(&mut self.database, database);
        push_front_unique(&mut self.user,     user);
    }
}

fn read_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn push_front_unique(vec: &mut Vec<String>, value: &str) {
    let v = value.trim();
    if v.is_empty() { return; }
    vec.retain(|x| x != v);
    vec.insert(0, v.to_string());
    vec.truncate(MAX_PER_FIELD);
}
