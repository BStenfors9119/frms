//! Database backend for the side panel.
//!
//! User-facing engines: PostgreSQL, MySQL, MariaDB. MariaDB shares MySQL's
//! wire protocol, so internally there are still only two backends. The
//! `DbProvider` enum (Self-hosted / GCP Cloud SQL / AWS RDS) is informational
//! — managed cloud instances are reached over the same protocols.
//!
//! No TLS yet, so managed cloud instances must allow non-SSL connections.

use std::sync::Arc;

use mysql_async::prelude::Queryable;
use mysql_async::{Pool as MyPool, Value as MyValue};
use tokio_postgres::types::Type;
use tokio_postgres::{Client as PgClient, NoTls, Row as PgRow};

// ── engine ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DbEngine {
    #[default]
    Postgres,
    Mysql,
    MariaDb,
}

impl DbEngine {
    pub const ALL: [DbEngine; 3] = [DbEngine::Postgres, DbEngine::Mysql, DbEngine::MariaDb];

    pub fn default_port(self) -> u16 {
        match self {
            DbEngine::Postgres                 => 5432,
            DbEngine::Mysql | DbEngine::MariaDb => 3306,
        }
    }

    /// Quote an identifier following the engine's rules.
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            DbEngine::Postgres                 => format!("\"{ident}\""),
            DbEngine::Mysql | DbEngine::MariaDb => format!("`{ident}`"),
        }
    }
}

impl std::fmt::Display for DbEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DbEngine::Postgres => "PostgreSQL",
            DbEngine::Mysql    => "MySQL",
            DbEngine::MariaDb  => "MariaDB",
        })
    }
}

// ── provider ──────────────────────────────────────────────────────────────────

/// Where the database is hosted. Informational — the wire protocol is
/// determined by `DbEngine`, not by the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DbProvider {
    #[default]
    SelfHosted,
    GcpCloudSql,
    AwsRds,
}

impl DbProvider {
    pub const ALL: [DbProvider; 3] =
        [DbProvider::SelfHosted, DbProvider::GcpCloudSql, DbProvider::AwsRds];
}

impl std::fmt::Display for DbProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DbProvider::SelfHosted  => "Self-hosted",
            DbProvider::GcpCloudSql => "GCP Cloud SQL",
            DbProvider::AwsRds      => "AWS RDS",
        })
    }
}

// ── connection config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DbConfig {
    pub engine:   DbEngine,
    pub provider: DbProvider,
    pub host:     String,
    pub port:     u16,
    pub user:     String,
    pub password: String,
    pub database: String,
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            engine:   DbEngine::Postgres,
            provider: DbProvider::SelfHosted,
            host:     String::from("localhost"),
            port:     5432,
            user:     String::from("postgres"),
            password: String::new(),
            database: String::from("postgres"),
        }
    }
}

// ── live client handle ────────────────────────────────────────────────────────

/// Cloneable wrapper carried inside `Message`. Both `Arc<PgClient>` and
/// `MyPool` are cheap to clone.
#[derive(Clone)]
pub enum DbClient {
    Postgres(Arc<PgClient>),
    Mysql(MyPool),
}

impl DbClient {
    /// The wire-protocol engine of the live connection. Note that a MariaDB
    /// connection reports as `Mysql` here — once connected we no longer need
    /// to distinguish them, since they share the protocol.
    pub fn engine(&self) -> DbEngine {
        match self {
            DbClient::Postgres(_) => DbEngine::Postgres,
            DbClient::Mysql(_)    => DbEngine::Mysql,
        }
    }
}

impl std::fmt::Debug for DbClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DbClient::Postgres(_) => "<postgres::Client>",
            DbClient::Mysql(_)    => "<mysql::Pool>",
        })
    }
}

// ── data shapes ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct TableRef {
    pub schema: String,
    pub name:   String,
}

impl TableRef {
    pub fn key(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }

    pub fn quoted(&self, engine: DbEngine) -> String {
        format!("{}.{}", engine.quote_ident(&self.schema), engine.quote_ident(&self.name))
    }
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows:    Vec<Vec<String>>,
}

// ── public ops (engine-dispatched) ────────────────────────────────────────────

pub async fn connect_and_list(cfg: DbConfig) -> Result<(DbClient, Vec<TableRef>), String> {
    match cfg.engine {
        DbEngine::Postgres                  => connect_pg(cfg).await,
        DbEngine::Mysql | DbEngine::MariaDb => connect_mysql(cfg).await,
    }
}

pub async fn list_columns(client: DbClient, table: TableRef) -> Result<Vec<String>, String> {
    match client {
        DbClient::Postgres(c) => list_columns_pg(c, table).await,
        DbClient::Mysql(p)    => list_columns_mysql(p, table).await,
    }
}

pub async fn run_query(client: DbClient, sql: String) -> Result<QueryResult, String> {
    match client {
        DbClient::Postgres(c) => run_query_pg(c, sql).await,
        DbClient::Mysql(p)    => run_query_mysql(p, sql).await,
    }
}

// ── postgres backend ──────────────────────────────────────────────────────────

async fn connect_pg(cfg: DbConfig) -> Result<(DbClient, Vec<TableRef>), String> {
    let mut config = tokio_postgres::Config::new();
    config.host(&cfg.host);
    config.port(cfg.port);
    config.user(&cfg.user);
    if !cfg.password.is_empty() { config.password(&cfg.password); }
    if !cfg.database.is_empty() { config.dbname(&cfg.database); }

    let (client, connection) = config
        .connect(NoTls)
        .await
        .map_err(|e| format!("connect: {e}"))?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("[db/pg] connection driver exited: {e}");
        }
    });

    let tables = list_tables_pg(&client).await?;
    Ok((DbClient::Postgres(Arc::new(client)), tables))
}

async fn list_tables_pg(client: &PgClient) -> Result<Vec<TableRef>, String> {
    let rows = client
        .query(
            "SELECT table_schema, table_name \
             FROM information_schema.tables \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
               AND table_type = 'BASE TABLE' \
             ORDER BY table_schema, table_name",
            &[],
        )
        .await
        .map_err(|e| format!("list tables: {e}"))?;

    Ok(rows.into_iter()
        .map(|r| TableRef { schema: r.get(0), name: r.get(1) })
        .collect())
}

async fn list_columns_pg(client: Arc<PgClient>, table: TableRef) -> Result<Vec<String>, String> {
    let rows = client
        .query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = $1 AND table_name = $2 \
             ORDER BY ordinal_position",
            &[&table.schema, &table.name],
        )
        .await
        .map_err(|e| format!("list columns: {e}"))?;
    Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
}

async fn run_query_pg(client: Arc<PgClient>, sql: String) -> Result<QueryResult, String> {
    let rows = client
        .query(&sql, &[])
        .await
        .map_err(|e| format!("query: {e}"))?;

    let columns = if let Some(first) = rows.first() {
        first.columns().iter().map(|c| c.name().to_string()).collect()
    } else {
        Vec::new()
    };

    let rendered: Vec<Vec<String>> = rows
        .iter()
        .map(|r| (0..r.len()).map(|i| pg_cell_to_string(r, i)).collect())
        .collect();

    Ok(QueryResult { columns, rows: rendered })
}

fn pg_cell_to_string(row: &PgRow, idx: usize) -> String {
    let col_type = row.columns()[idx].type_().clone();

    macro_rules! try_get {
        ($t:ty) => {
            match row.try_get::<_, Option<$t>>(idx) {
                Ok(Some(v)) => return v.to_string(),
                Ok(None)    => return "NULL".to_string(),
                Err(_)      => {}
            }
        };
    }

    match col_type {
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => try_get!(String),
        Type::INT2  => try_get!(i16),
        Type::INT4  => try_get!(i32),
        Type::INT8  => try_get!(i64),
        Type::FLOAT4 => try_get!(f32),
        Type::FLOAT8 => try_get!(f64),
        Type::BOOL   => try_get!(bool),
        _ => {}
    }

    if let Ok(v) = row.try_get::<_, Option<String>>(idx) {
        return v.unwrap_or_else(|| "NULL".to_string());
    }
    format!("<{}>", col_type.name())
}

// ── mysql / mariadb backend ───────────────────────────────────────────────────

async fn connect_mysql(cfg: DbConfig) -> Result<(DbClient, Vec<TableRef>), String> {
    let mut builder = mysql_async::OptsBuilder::default()
        .ip_or_hostname(cfg.host.clone())
        .tcp_port(cfg.port)
        .user(Some(cfg.user.clone()));
    if !cfg.password.is_empty() { builder = builder.pass(Some(cfg.password.clone())); }
    if !cfg.database.is_empty() { builder = builder.db_name(Some(cfg.database.clone())); }

    let opts: mysql_async::Opts = builder.into();
    let pool = MyPool::new(opts);

    // Validate the connection before returning the pool.
    let mut conn = pool.get_conn().await.map_err(|e| format!("connect: {e}"))?;
    let tables = list_tables_mysql_with(&mut conn).await?;
    drop(conn);

    Ok((DbClient::Mysql(pool), tables))
}

async fn list_tables_mysql_with(conn: &mut mysql_async::Conn) -> Result<Vec<TableRef>, String> {
    let rows: Vec<(String, String)> = conn
        .query(
            "SELECT TABLE_SCHEMA, TABLE_NAME FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys') \
               AND TABLE_TYPE = 'BASE TABLE' \
             ORDER BY TABLE_SCHEMA, TABLE_NAME",
        )
        .await
        .map_err(|e| format!("list tables: {e}"))?;
    Ok(rows.into_iter().map(|(schema, name)| TableRef { schema, name }).collect())
}

async fn list_columns_mysql(pool: MyPool, table: TableRef) -> Result<Vec<String>, String> {
    let mut conn = pool.get_conn().await.map_err(|e| format!("conn: {e}"))?;
    let sql = format!(
        "SELECT COLUMN_NAME FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA = '{}' AND TABLE_NAME = '{}' \
         ORDER BY ORDINAL_POSITION",
        escape_mysql_string(&table.schema),
        escape_mysql_string(&table.name),
    );
    let cols: Vec<String> = conn
        .query(&sql)
        .await
        .map_err(|e| format!("list columns: {e}"))?;
    Ok(cols)
}

async fn run_query_mysql(pool: MyPool, sql: String) -> Result<QueryResult, String> {
    let mut conn = pool.get_conn().await.map_err(|e| format!("conn: {e}"))?;
    let mut result = conn.query_iter(&sql).await.map_err(|e| format!("query: {e}"))?;

    let columns = result
        .columns()
        .as_ref()
        .map(|cols| cols.iter().map(|c| c.name_str().to_string()).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut rendered: Vec<Vec<String>> = Vec::new();
    while let Some(set) = result.next().await.map_err(|e| format!("rows: {e}"))? {
        let row: mysql_async::Row = set;
        let len = row.len();
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let v: MyValue = row.as_ref(i).cloned().unwrap_or(MyValue::NULL);
            out.push(mysql_value_to_string(&v));
        }
        rendered.push(out);
    }

    Ok(QueryResult { columns, rows: rendered })
}

fn mysql_value_to_string(v: &MyValue) -> String {
    match v {
        MyValue::NULL          => "NULL".into(),
        MyValue::Bytes(b)      => String::from_utf8_lossy(b).into_owned(),
        MyValue::Int(n)        => n.to_string(),
        MyValue::UInt(n)       => n.to_string(),
        MyValue::Float(f)      => f.to_string(),
        MyValue::Double(f)     => f.to_string(),
        MyValue::Date(y, mo, d, h, mi, s, _us) => {
            format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
        }
        MyValue::Time(neg, days, h, mi, s, _us) => {
            let sign = if *neg { "-" } else { "" };
            format!("{sign}{days}d {h:02}:{mi:02}:{s:02}")
        }
    }
}

/// Minimal escaping for identifiers/strings in literal-substituted MySQL SQL.
/// Used by the columns lookup which doesn't go through prepared statements.
fn escape_mysql_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            _    => out.push(c),
        }
    }
    out
}
