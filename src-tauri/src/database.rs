//! Built-in `database` connector: PostgreSQL, MySQL and SQLite behind one API.
//!
//! Reads and writes are separate types. `Database` can only read: every query
//! runs inside a read-only transaction (or a read-only SQLite file), so the
//! database itself rejects writes whatever SQL a model sends. `WritableDatabase`
//! is opened only for `execute`, which the host gates behind the user's approval.

// The tools are wired to the MCP host in F2; until then only tests and the wizard call them.
#![cfg_attr(not(test), allow(dead_code))]

use std::str::FromStr;
use std::time::Duration;

use futures_util::{Stream, TryStreamExt};
use serde::Serialize;
use serde_json::Value;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlRow};
use sqlx::pool::PoolOptions;
use sqlx::postgres::{PgConnectOptions, PgPool, PgRow};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqliteRow};
use sqlx::types::chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::types::{Decimal, JsonValue, Uuid};
use sqlx::{AssertSqlSafe, Column, Executor, Row, TypeInfo};

use crate::catalog::{TestReport, Values};
use crate::error::{Error, Result};

/// Rows returned by one query; more are reported as `truncated`.
pub const ROW_LIMIT: usize = 500;
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Postgres,
    Mysql,
    Sqlite,
}

impl FromStr for Driver {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "mysql" | "mariadb" => Ok(Self::Mysql),
            "sqlite" => Ok(Self::Sqlite),
            other => Err(Error::Database(format!(
                "unsupported database type `{other}`"
            ))),
        }
    }
}

impl Driver {
    fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Sqlite => "SQLite",
        }
    }

    /// Statements that wrap one read in a read-only transaction, and the one that ends it.
    fn read_guard(self) -> (&'static [&'static str], Option<&'static str>) {
        match self {
            Self::Postgres => (&["BEGIN READ ONLY"], Some("ROLLBACK")),
            // Session-level too: MySQL DDL commits implicitly and would escape a plain transaction.
            Self::Mysql => (
                &[
                    "SET SESSION TRANSACTION READ ONLY",
                    "START TRANSACTION READ ONLY",
                ],
                Some("ROLLBACK"),
            ),
            // The file itself is opened read-only.
            Self::Sqlite => (&[], None),
        }
    }

    fn version_sql(self) -> &'static str {
        match self {
            Self::Postgres => "SELECT current_setting('server_version')",
            Self::Mysql => "SELECT VERSION()",
            Self::Sqlite => "SELECT sqlite_version()",
        }
    }

    fn tables_sql(self) -> &'static str {
        match self {
            Self::Postgres => {
                "SELECT table_name::text FROM information_schema.tables \
                 WHERE table_schema = current_schema() ORDER BY 1"
            }
            Self::Mysql => {
                "SELECT CAST(table_name AS CHAR) FROM information_schema.tables \
                 WHERE table_schema = DATABASE() ORDER BY 1"
            }
            Self::Sqlite => {
                "SELECT name FROM sqlite_schema \
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY 1"
            }
        }
    }

    fn columns_sql(self) -> &'static str {
        match self {
            Self::Postgres => {
                "SELECT column_name::text, data_type::text, is_nullable = 'YES' \
                 FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = $1 ORDER BY ordinal_position"
            }
            Self::Mysql => {
                "SELECT CAST(column_name AS CHAR), CAST(column_type AS CHAR), is_nullable = 'YES' \
                 FROM information_schema.columns \
                 WHERE table_schema = DATABASE() AND table_name = ? ORDER BY ordinal_position"
            }
            Self::Sqlite => {
                // Primary keys are never null even when not declared NOT NULL.
                "SELECT name, type, \"notnull\" = 0 AND pk = 0 FROM pragma_table_info(?) ORDER BY cid"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

enum Pool {
    Postgres(PgPool),
    Mysql(MySqlPool),
    Sqlite(SqlitePool),
}

/// Runs the same body against whichever concrete pool is in use.
macro_rules! with_pool {
    ($pool:expr, $p:ident => $body:expr) => {
        match $pool {
            Pool::Postgres($p) => $body,
            Pool::Mysql($p) => $body,
            Pool::Sqlite($p) => $body,
        }
    };
}

fn pool_options<DB: sqlx::Database>() -> PoolOptions<DB> {
    PoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(10))
}

async fn connect(driver: Driver, url: &str, read_only: bool) -> Result<Pool> {
    let url = url.trim();
    Ok(match driver {
        Driver::Postgres => {
            let mut opts = PgConnectOptions::from_str(url)?;
            if read_only {
                opts = opts.options([("default_transaction_read_only", "on")]);
            }
            Pool::Postgres(pool_options().connect_with(opts).await?)
        }
        Driver::Mysql => Pool::Mysql(
            pool_options()
                .connect_with(MySqlConnectOptions::from_str(url)?)
                .await?,
        ),
        Driver::Sqlite => {
            let mut opts = sqlite_options(url)?;
            if read_only {
                opts = opts.read_only(true).pragma("query_only", "ON");
            }
            Pool::Sqlite(pool_options().connect_with(opts).await?)
        }
    })
}

/// Accepts `sqlite://…` URLs as well as plain file paths.
fn sqlite_options(url: &str) -> Result<SqliteConnectOptions> {
    if url.starts_with("sqlite:") {
        return Ok(SqliteConnectOptions::from_str(url)?);
    }
    if !std::path::Path::new(url).is_file() {
        return Err(Error::Database(format!("file not found: {url}")));
    }
    Ok(SqliteConnectOptions::new().filename(url))
}

pub struct Database {
    driver: Driver,
    pool: Pool,
}

impl Database {
    pub async fn open(driver: Driver, url: &str) -> Result<Self> {
        Ok(Self {
            driver,
            pool: connect(driver, url, true).await?,
        })
    }

    pub async fn query(&self, sql: &str) -> Result<QueryResult> {
        self.fetch(sql, &[], ROW_LIMIT).await
    }

    pub async fn list_tables(&self) -> Result<Vec<String>> {
        let r = self
            .fetch(self.driver.tables_sql(), &[], usize::MAX)
            .await?;
        Ok(r.rows.iter().filter_map(|row| text(row.first())).collect())
    }

    pub async fn describe_table(&self, table: &str) -> Result<Vec<ColumnInfo>> {
        let r = self
            .fetch(self.driver.columns_sql(), &[table], usize::MAX)
            .await?;
        Ok(r.rows
            .iter()
            .filter_map(|row| {
                Some(ColumnInfo {
                    name: text(row.first())?,
                    data_type: text(row.get(1)).unwrap_or_default(),
                    nullable: truthy(row.get(2)),
                })
            })
            .collect())
    }

    pub async fn report(&self) -> Result<TestReport> {
        let version = self.fetch(self.driver.version_sql(), &[], 1).await?;
        let version = version.rows.first().and_then(|r| text(r.first()));
        Ok(TestReport {
            title: format!(
                "{} {}",
                self.driver.label(),
                version.as_deref().unwrap_or("")
            )
            .trim_end()
            .to_owned(),
            items: self.list_tables().await?,
        })
    }

    async fn fetch(&self, sql: &str, binds: &[&str], limit: usize) -> Result<QueryResult> {
        let (begin, end) = self.driver.read_guard();
        let work = async {
            with_pool!(&self.pool, p => {
                let mut conn = p.acquire().await?;
                for stmt in begin {
                    conn.execute(sqlx::raw_sql(*stmt)).await?;
                }
                // Arbitrary SQL is the point of this tool; the read-only guard is what keeps it safe.
                let mut q = sqlx::query(AssertSqlSafe(sql));
                for b in binds {
                    q = q.bind(*b);
                }
                let result = collect(q.fetch(&mut *conn), limit).await;
                if let Some(stmt) = end {
                    conn.execute(sqlx::raw_sql(stmt)).await?;
                }
                result
            })
        };
        tokio::time::timeout(QUERY_TIMEOUT, work)
            .await
            .map_err(|_| Error::Database(format!("query took longer than {QUERY_TIMEOUT:?}")))?
    }
}

/// Write access, used only for the `execute` tool. Accepts several statements at once.
pub struct WritableDatabase(Pool);

impl WritableDatabase {
    pub async fn open(driver: Driver, url: &str) -> Result<Self> {
        Ok(Self(connect(driver, url, false).await?))
    }

    pub async fn execute(&self, sql: &str) -> Result<u64> {
        with_pool!(&self.0, p => Ok(sqlx::raw_sql(AssertSqlSafe(sql.to_owned())).execute(p).await?.rows_affected()))
    }
}

/// Connection test for the setup wizard. Field keys match `catalog/connectors/database.toml`.
pub async fn test(values: &Values) -> Result<TestReport> {
    let field = |key: &str| {
        values
            .get(key)
            .and_then(|v| v.as_text())
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| Error::Database(format!("missing `{key}`")))
    };
    Database::open(field("driver")?.parse()?, field("url")?)
        .await?
        .report()
        .await
}

async fn collect<R: Row + RowJson>(
    rows: impl Stream<Item = sqlx::Result<R>>,
    limit: usize,
) -> Result<QueryResult> {
    let mut rows = std::pin::pin!(rows);
    let mut out = QueryResult {
        columns: Vec::new(),
        rows: Vec::new(),
        truncated: false,
    };
    while let Some(row) = rows.try_next().await? {
        if out.rows.len() == limit {
            out.truncated = true;
            break;
        }
        if out.columns.is_empty() {
            out.columns = row.columns().iter().map(|c| c.name().to_owned()).collect();
        }
        out.rows.push((0..row.len()).map(|i| row.cell(i)).collect());
    }
    Ok(out)
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64() != Some(0),
        _ => false,
    }
}

/// Converts one cell to JSON. Each driver lists the Rust types to try, in order.
trait RowJson {
    fn cell(&self, i: usize) -> Value;
}

/// Returns the first successful decode; `try_get` rejects types incompatible with the column.
macro_rules! decode_first {
    ($row:expr, $i:expr; $($t:ty => $to_json:expr),+ $(,)?) => {{
        $(
            if let Ok(v) = $row.try_get::<Option<$t>, _>($i) {
                return v.map($to_json).unwrap_or(Value::Null);
            }
        )+
        Value::String(format!("<{}>", $row.column($i).type_info().name()))
    }};
}

/// MySQL and SQLite store booleans as integers; only the declared type tells them apart.
macro_rules! declared_bool {
    ($row:expr, $i:expr) => {
        if $row.column($i).type_info().name() == "BOOLEAN"
            && let Ok(v) = $row.try_get::<Option<bool>, _>($i)
        {
            return v.map(Value::Bool).unwrap_or(Value::Null);
        }
    };
}

fn decimal(d: Decimal) -> Value {
    d.to_string()
        .parse::<f64>()
        .map(Value::from)
        .unwrap_or_else(|_| Value::String(d.to_string()))
}

fn bytes(b: Vec<u8>) -> Value {
    Value::String(format!("<{} bytes>", b.len()))
}

fn display<T: ToString>(v: T) -> Value {
    Value::String(v.to_string())
}

impl RowJson for PgRow {
    fn cell(&self, i: usize) -> Value {
        decode_first!(self, i;
            bool => Value::Bool,
            i16 => Value::from,
            i32 => Value::from,
            i64 => Value::from,
            f32 => Value::from,
            f64 => Value::from,
            Decimal => decimal,
            NaiveDate => display,
            NaiveDateTime => display,
            DateTime<Utc> => |d: DateTime<Utc>| Value::String(d.to_rfc3339()),
            NaiveTime => display,
            Uuid => display,
            JsonValue => |v| v,
            String => Value::String,
            Vec<u8> => bytes,
        )
    }
}

impl RowJson for MySqlRow {
    fn cell(&self, i: usize) -> Value {
        declared_bool!(self, i);
        decode_first!(self, i;
            i64 => Value::from,
            u64 => Value::from,
            f64 => Value::from,
            f32 => Value::from,
            Decimal => decimal,
            NaiveDate => display,
            NaiveDateTime => display,
            DateTime<Utc> => |d: DateTime<Utc>| Value::String(d.to_rfc3339()),
            NaiveTime => display,
            JsonValue => |v| v,
            String => Value::String,
            Vec<u8> => bytes,
        )
    }
}

impl RowJson for SqliteRow {
    fn cell(&self, i: usize) -> Value {
        declared_bool!(self, i);
        decode_first!(self, i;
            i64 => Value::from,
            f64 => Value::from,
            String => Value::String,
            Vec<u8> => bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: &str = include_str!("../tests/databases/sales.sql");

    /// Seeds the database, then runs every check through the read-only API.
    async fn exercise(driver: Driver, url: &str) {
        WritableDatabase::open(driver, url)
            .await
            .unwrap()
            .execute(SEED)
            .await
            .unwrap();
        let db = Database::open(driver, url).await.unwrap();

        let tables = db.list_tables().await.unwrap();
        assert_eq!(tables, ["customers", "orders", "products"], "{driver:?}");

        let cols = db.describe_table("products").await.unwrap();
        let names: Vec<_> = cols.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["id", "name", "category", "price", "active"]);
        assert!(cols.iter().all(|c| !c.nullable));

        let r = db
            .query("SELECT name, price, active FROM products WHERE id = 1")
            .await
            .unwrap();
        assert_eq!(r.columns, ["name", "price", "active"]);
        assert_eq!(
            r.rows,
            [vec![
                Value::from("Laptop Pro 14"),
                Value::from(1299.99),
                Value::Bool(true)
            ]]
        );

        let r = db
            .query("SELECT ordered_at FROM orders WHERE id = 1")
            .await
            .unwrap();
        assert_eq!(r.rows[0][0], Value::from("2026-01-12"), "{driver:?}");

        let r = db
            .query("SELECT COUNT(*) AS n, SUM(quantity) AS units FROM orders")
            .await
            .unwrap();
        assert_eq!(r.rows[0][0], Value::from(18), "{driver:?}");

        for write in [
            "INSERT INTO customers (id, name, email, region, created_at) VALUES (99, 'x', 'x', 'x', '2026-01-01')",
            "UPDATE products SET price = 0",
            "DELETE FROM orders",
            "DROP TABLE orders",
            "CREATE TABLE intruder (id INTEGER)",
        ] {
            assert!(
                db.query(write).await.is_err(),
                "{driver:?} allowed a write through query: {write}"
            );
        }
        // Switching the session to read-write in one call must not unlock the next one.
        for escape in [
            "SET SESSION TRANSACTION READ WRITE",
            "SET default_transaction_read_only = off",
            "PRAGMA query_only = OFF",
            "COMMIT",
        ] {
            let _ = db.query(escape).await;
            assert!(
                db.query("DELETE FROM orders").await.is_err(),
                "{driver:?} became writable after `{escape}`"
            );
        }

        let r = db.query("SELECT COUNT(*) FROM orders").await.unwrap();
        assert_eq!(
            r.rows[0][0],
            Value::from(18),
            "{driver:?} data changed after rejected writes"
        );

        let r = db.fetch("SELECT id FROM orders", &[], 5).await.unwrap();
        assert_eq!(r.rows.len(), 5);
        assert!(r.truncated);

        let report = db.report().await.unwrap();
        assert!(report.title.starts_with(driver.label()), "{}", report.title);
        assert_eq!(report.items.len(), 3);
    }

    #[tokio::test]
    async fn sqlite_end_to_end() {
        let path = std::env::temp_dir().join(format!("mcforge-test-{}.db", std::process::id()));
        std::fs::File::create(&path).unwrap();
        exercise(Driver::Sqlite, path.to_str().unwrap()).await;
        std::fs::remove_file(path).ok();
    }

    /// Runs against a real server when `MCFORGE_TEST_<NAME>_URL` is set (see tests/databases/compose.yml).
    async fn against_server(driver: Driver, var: &str) {
        match std::env::var(var) {
            Ok(url) => exercise(driver, &url).await,
            Err(_) => eprintln!("skipped: set {var} to test {}", driver.label()),
        }
    }

    #[tokio::test]
    async fn postgres_end_to_end() {
        against_server(Driver::Postgres, "MCFORGE_TEST_POSTGRES_URL").await;
    }

    #[tokio::test]
    async fn mysql_end_to_end() {
        against_server(Driver::Mysql, "MCFORGE_TEST_MYSQL_URL").await;
    }

    #[test]
    fn parses_driver_names() {
        assert_eq!("PostgreSQL".parse::<Driver>().unwrap(), Driver::Postgres);
        assert!("oracle".parse::<Driver>().is_err());
    }

    #[tokio::test]
    async fn missing_sqlite_file_is_a_clear_error() {
        let err = Database::open(Driver::Sqlite, "/nope/missing.db")
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("file not found"), "{err}");
    }
}
