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
use tokio_postgres::{Client as PgClient, NoTls, Row as PgRow, SimpleQueryMessage};

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

    /// Quote a value as a single-quoted string literal, escaping per the
    /// engine's rules. Used by the query builder's WHERE clause, which is
    /// substituted into the SQL text rather than bound as a parameter.
    pub fn quote_literal(self, value: &str) -> String {
        let mut out = String::with_capacity(value.len() + 2);
        out.push('\'');
        for c in value.chars() {
            match c {
                '\'' => out.push_str("''"),
                // MySQL/MariaDB treat backslash as an escape character inside
                // string literals; double it so the value is taken verbatim.
                // Postgres (standard_conforming_strings) keeps it literal.
                '\\' if matches!(self, DbEngine::Mysql | DbEngine::MariaDb) => out.push_str("\\\\"),
                _ => out.push(c),
            }
        }
        out.push('\'');
        out
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

/// Whether a stored routine is a `PROCEDURE` or a `FUNCTION`. Drives the
/// keyword used to fetch its source on MySQL/MariaDB (`SHOW CREATE …`) and the
/// little tag shown beside it in the Objects list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutineKind {
    Procedure,
    Function,
}

impl RoutineKind {
    /// Map an `information_schema` `routine_type` ("PROCEDURE"/"FUNCTION") onto
    /// the enum, defaulting to `Function` for anything unexpected.
    fn parse(s: &str) -> RoutineKind {
        if s.eq_ignore_ascii_case("PROCEDURE") {
            RoutineKind::Procedure
        } else {
            RoutineKind::Function
        }
    }

    /// Short tag shown beside the routine name in the Objects list.
    pub fn tag(self) -> &'static str {
        match self {
            RoutineKind::Procedure => "proc",
            RoutineKind::Function  => "func",
        }
    }
}

/// A stored procedure or function visible to the connected user. The `kind`
/// distinguishes the two so the source can be fetched with the right keyword.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct RoutineRef {
    pub schema: String,
    pub name:   String,
    pub kind:   RoutineKind,
}

impl RoutineRef {
    pub fn key(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows:    Vec<Vec<String>>,
}

/// The result of a free-hand statement run from the raw SQL pane. Row-returning
/// statements (SELECT, or anything with RETURNING) yield a `Rows` result set;
/// statements that only mutate (INSERT/UPDATE/DELETE/DDL) yield the server's
/// affected-row count so the UI can report "N rows affected".
#[derive(Debug, Clone)]
pub enum StatementOutcome {
    Rows(QueryResult),
    Affected(u64),
}

/// A foreign-key relationship: `from_table.from_column` references
/// `to_table.to_column`. Used to gate which tables are joinable and to build
/// the `JOIN … ON …` clauses in the query builder.
#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub from_table:  TableRef,
    pub from_column: String,
    pub to_table:    TableRef,
    pub to_column:   String,
}

// ── public ops (engine-dispatched) ────────────────────────────────────────────

pub async fn connect_and_list(
    cfg: DbConfig,
) -> Result<(DbClient, Vec<TableRef>, Vec<ForeignKey>, Vec<RoutineRef>), String> {
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

/// Fetch the source (DDL) of a stored procedure or function for display.
pub async fn routine_definition(
    client: DbClient,
    routine: RoutineRef,
) -> Result<String, String> {
    match client {
        DbClient::Postgres(c) => routine_definition_pg(c, routine).await,
        DbClient::Mysql(p)    => routine_definition_mysql(p, routine).await,
    }
}

pub async fn run_query(client: DbClient, sql: String) -> Result<QueryResult, String> {
    match client {
        DbClient::Postgres(c) => run_query_pg(c, sql).await,
        DbClient::Mysql(p)    => run_query_mysql(p, sql).await,
    }
}

/// Run an arbitrary, user-authored statement (SELECT, UPDATE, DELETE, DDL, …)
/// verbatim. Unlike `run_query` — which only ever runs the builder's SELECT —
/// this reports either a result set or an affected-row count, so the raw SQL
/// pane can execute commands that return no rows.
pub async fn run_statement(client: DbClient, sql: String) -> Result<StatementOutcome, String> {
    match client {
        DbClient::Postgres(c) => run_statement_pg(c, sql).await,
        DbClient::Mysql(p)    => run_statement_mysql(p, sql).await,
    }
}

// ── postgres backend ──────────────────────────────────────────────────────────

async fn connect_pg(
    cfg: DbConfig,
) -> Result<(DbClient, Vec<TableRef>, Vec<ForeignKey>, Vec<RoutineRef>), String> {
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

    let tables   = list_tables_pg(&client).await?;
    let fks      = list_foreign_keys_pg(&client).await?;
    let routines = list_routines_pg(&client).await?;
    Ok((DbClient::Postgres(Arc::new(client)), tables, fks, routines))
}

async fn list_routines_pg(client: &PgClient) -> Result<Vec<RoutineRef>, String> {
    let rows = client
        .query(
            "SELECT routine_schema, routine_name, routine_type \
             FROM information_schema.routines \
             WHERE routine_schema NOT IN ('pg_catalog', 'information_schema') \
               AND routine_type IN ('PROCEDURE', 'FUNCTION') \
             ORDER BY routine_schema, routine_name",
            &[],
        )
        .await
        .map_err(|e| format!("list routines: {e}"))?;

    Ok(rows.into_iter()
        .map(|r| RoutineRef {
            schema: r.get(0),
            name:   r.get(1),
            kind:   RoutineKind::parse(&r.get::<_, String>(2)),
        })
        .collect())
}

async fn routine_definition_pg(
    client: Arc<PgClient>,
    routine: RoutineRef,
) -> Result<String, String> {
    // `pg_get_functiondef` reconstructs the full `CREATE …` for both functions
    // and procedures. A name may be overloaded, so several rows can come back;
    // show each definition separated by a blank line.
    let rows = client
        .query(
            "SELECT pg_get_functiondef(p.oid) \
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE n.nspname = $1 AND p.proname = $2 \
             ORDER BY p.oid",
            &[&routine.schema, &routine.name],
        )
        .await
        .map_err(|e| format!("routine definition: {e}"))?;

    if rows.is_empty() {
        return Err("routine definition not found".to_string());
    }
    Ok(rows.iter()
        .map(|r| r.get::<_, String>(0))
        .collect::<Vec<_>>()
        .join("\n\n"))
}

async fn list_foreign_keys_pg(client: &PgClient) -> Result<Vec<ForeignKey>, String> {
    let rows = client
        .query(
            "SELECT tc.table_schema, tc.table_name, kcu.column_name, \
                    ccu.table_schema, ccu.table_name, ccu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
               ON tc.constraint_name = kcu.constraint_name \
              AND tc.table_schema = kcu.table_schema \
             JOIN information_schema.constraint_column_usage ccu \
               ON ccu.constraint_name = tc.constraint_name \
              AND ccu.table_schema = tc.table_schema \
             WHERE tc.constraint_type = 'FOREIGN KEY' \
               AND tc.table_schema NOT IN ('pg_catalog', 'information_schema')",
            &[],
        )
        .await
        .map_err(|e| format!("list foreign keys: {e}"))?;

    Ok(rows.into_iter()
        .map(|r| ForeignKey {
            from_table:  TableRef { schema: r.get(0), name: r.get(1) },
            from_column: r.get(2),
            to_table:    TableRef { schema: r.get(3), name: r.get(4) },
            to_column:   r.get(5),
        })
        .collect())
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

/// Run a free-hand statement on Postgres via `simple_query`, which executes the
/// text as-is (no parameter binding) and streams back per-statement messages.
/// Row-returning statements produce a `Rows` outcome with all values rendered as
/// text; everything else reports the affected-row count of the final command.
async fn run_statement_pg(client: Arc<PgClient>, sql: String) -> Result<StatementOutcome, String> {
    let messages = client
        .simple_query(&sql)
        .await
        .map_err(|e| format!("query: {e}"))?;

    let mut columns:  Vec<String>      = Vec::new();
    let mut rows:     Vec<Vec<String>> = Vec::new();
    let mut got_rows = false;
    let mut affected: u64 = 0;

    for msg in messages {
        match msg {
            SimpleQueryMessage::RowDescription(cols) => {
                columns  = cols.iter().map(|c| c.name().to_string()).collect();
                got_rows = true;
            }
            SimpleQueryMessage::Row(row) => {
                got_rows = true;
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                rows.push(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_string).unwrap_or_else(|| "NULL".to_string()))
                        .collect(),
                );
            }
            SimpleQueryMessage::CommandComplete(n) => affected = n,
            _ => {}
        }
    }

    if got_rows {
        Ok(StatementOutcome::Rows(QueryResult { columns, rows }))
    } else {
        Ok(StatementOutcome::Affected(affected))
    }
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

async fn connect_mysql(
    cfg: DbConfig,
) -> Result<(DbClient, Vec<TableRef>, Vec<ForeignKey>, Vec<RoutineRef>), String> {
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
    let tables   = list_tables_mysql_with(&mut conn).await?;
    let fks      = list_foreign_keys_mysql_with(&mut conn).await?;
    let routines = list_routines_mysql_with(&mut conn).await?;
    drop(conn);

    Ok((DbClient::Mysql(pool), tables, fks, routines))
}

async fn list_routines_mysql_with(
    conn: &mut mysql_async::Conn,
) -> Result<Vec<RoutineRef>, String> {
    let rows: Vec<(String, String, String)> = conn
        .query(
            "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE FROM information_schema.ROUTINES \
             WHERE ROUTINE_SCHEMA NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys') \
             ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME",
        )
        .await
        .map_err(|e| format!("list routines: {e}"))?;

    Ok(rows.into_iter()
        .map(|(schema, name, kind)| RoutineRef { schema, name, kind: RoutineKind::parse(&kind) })
        .collect())
}

async fn routine_definition_mysql(
    pool: MyPool,
    routine: RoutineRef,
) -> Result<String, String> {
    let mut conn = pool.get_conn().await.map_err(|e| format!("conn: {e}"))?;
    let keyword = match routine.kind {
        RoutineKind::Procedure => "PROCEDURE",
        RoutineKind::Function  => "FUNCTION",
    };
    let sql = format!(
        "SHOW CREATE {keyword} `{}`.`{}`",
        escape_mysql_ident(&routine.schema),
        escape_mysql_ident(&routine.name),
    );
    let row: mysql_async::Row = conn
        .query_first(&sql)
        .await
        .map_err(|e| format!("routine definition: {e}"))?
        .ok_or_else(|| "routine definition not found".to_string())?;

    // SHOW CREATE PROCEDURE/FUNCTION returns the DDL in its third column
    // ("Create Procedure" / "Create Function"); it is NULL without privileges.
    row.get::<Option<String>, _>(2)
        .flatten()
        .ok_or_else(|| "routine definition unavailable (insufficient privileges?)".to_string())
}

async fn list_foreign_keys_mysql_with(
    conn: &mut mysql_async::Conn,
) -> Result<Vec<ForeignKey>, String> {
    let rows: Vec<(String, String, String, String, String, String)> = conn
        .query(
            "SELECT TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, \
                    REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
             FROM information_schema.KEY_COLUMN_USAGE \
             WHERE REFERENCED_TABLE_NAME IS NOT NULL \
               AND TABLE_SCHEMA NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')",
        )
        .await
        .map_err(|e| format!("list foreign keys: {e}"))?;

    Ok(rows.into_iter()
        .map(|(fs, ft, fc, ts, tn, tc)| ForeignKey {
            from_table:  TableRef { schema: fs, name: ft },
            from_column: fc,
            to_table:    TableRef { schema: ts, name: tn },
            to_column:   tc,
        })
        .collect())
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

/// Run a free-hand statement on MySQL/MariaDB. A statement that exposes columns
/// is collected into a `Rows` outcome; one that does not (INSERT/UPDATE/DELETE/
/// DDL) reports the server's affected-row count.
async fn run_statement_mysql(pool: MyPool, sql: String) -> Result<StatementOutcome, String> {
    let mut conn = pool.get_conn().await.map_err(|e| format!("conn: {e}"))?;
    let mut result = conn.query_iter(&sql).await.map_err(|e| format!("query: {e}"))?;

    let columns: Vec<String> = result
        .columns()
        .as_ref()
        .map(|cols| cols.iter().map(|c| c.name_str().to_string()).collect())
        .unwrap_or_default();

    // No columns ⇒ a non-row-returning statement; report what it changed.
    if columns.is_empty() {
        let affected = result.affected_rows();
        drop(result);
        return Ok(StatementOutcome::Affected(affected));
    }

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

    Ok(StatementOutcome::Rows(QueryResult { columns, rows: rendered }))
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

/// Escape an identifier for use inside backticks — a literal backtick is
/// doubled. Used by the `SHOW CREATE …` routine lookup, which interpolates the
/// schema/name rather than binding them.
fn escape_mysql_ident(s: &str) -> String {
    s.replace('`', "``")
}
