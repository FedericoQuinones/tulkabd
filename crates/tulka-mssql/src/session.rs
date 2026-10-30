//! A SQL Server session over the vendored Tiberius fork.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use tiberius::error::Error as TdsError;
use tiberius::{
    AuthMethod, ColumnType, Config, EncryptionLevel, QueryItem, Row, SqlBrowser, ToSql,
};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use tulka_core::{
    quote_ident, CancelHandle, Column, ColumnInfo, ConnectParams, DatabaseSnapshot, DriverError,
    ExecEvent, MessageKind, ReadOnlyReason, ReadOnlyViolation, ServerInfo, Session, TableStructure,
    Value,
};

use crate::batches::{split_batches, Batch};
use crate::convert::{to_value, type_name};
use crate::meta;
use crate::readonly;

type Client = tiberius::Client<Compat<TcpStream>>;

/// Rows are sent to the UI in chunks of at most this many rows...
const ROWS_PER_EVENT: usize = 500;
/// ...or as soon as the oldest buffered row is this old.
const FLUSH_INTERVAL: Duration = Duration::from_millis(50);
/// How often a pending cancel is re-sent while a request is being written.
const CANCEL_RETRY: Duration = Duration::from_millis(100);

/// Login error numbers that mean the credentials were rejected: login
/// failed, untrusted domain, password expired or must change, account locked.
const AUTH_ERRORS: [u32; 6] = [18456, 18452, 18470, 18486, 18487, 18488];

/// Errors of this severity or higher close the connection.
const FATAL_SEVERITY: u8 = 20;

/// The batch that opens the transaction a read-only script runs in. Rows are
/// capped one past `max_rows`, so the caller can tell a result was cut.
fn read_only_preamble(max_rows: u32) -> String {
    format!(
        "SET NOCOUNT ON; SET XACT_ABORT ON; SET IMPLICIT_TRANSACTIONS OFF; \
         SET QUOTED_IDENTIFIER ON; SET ANSI_NULLS ON; \
         SET TRANSACTION ISOLATION LEVEL READ COMMITTED; SET LOCK_TIMEOUT 5000; \
         SET DEADLOCK_PRIORITY LOW; SET ROWCOUNT {}; BEGIN TRANSACTION;",
        max_rows.saturating_add(1)
    )
}

/// Ends a read-only script: reports how many transactions were open, then
/// rolls back whatever is left.
const READ_ONLY_END: &str =
    "DECLARE @n int = @@TRANCOUNT; IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION; SELECT @n;";

/// The error for a script the read-only check refused.
fn refusal(v: &ReadOnlyViolation) -> DriverError {
    let why = match v.reason {
        ReadOnlyReason::Empty => "there is nothing to run".to_string(),
        ReadOnlyReason::NotASelect => "only queries are allowed".to_string(),
        ReadOnlyReason::Forbidden => {
            "it contains a statement or word that can change data or state".to_string()
        }
        ReadOnlyReason::LinkedServer => "it reaches through a linked server".to_string(),
        ReadOnlyReason::Unparsed => "it could not be read with certainty".to_string(),
        ReadOnlyReason::Unsupported => "this driver cannot check it".to_string(),
    };
    let at = match &v.word {
        Some(word) => format!(" (`{word}` at line {})", v.line + 1),
        None if v.reason == ReadOnlyReason::Empty => String::new(),
        None => format!(" (line {})", v.line + 1),
    };
    DriverError::Other(format!("refused: not a read-only query: {why}{at}"))
}

pub struct MssqlSession {
    /// `None` once the connection is lost.
    client: Option<Client>,
    cancel: tiberius::CancelHandle,
    /// Set by [`MssqlCancel`] and cleared when `execute` starts; stops the
    /// remaining batches of a script.
    cancel_requested: Arc<AtomicBool>,
    /// Whether the server hosts a single database (Azure SQL Database),
    /// learned on first use.
    single_database: Option<bool>,
}

/// The default SQL Server port. With a named instance it means "ask SQL
/// Server Browser for the instance's port".
const DEFAULT_PORT: u16 = 1433;

/// Where to connect, parsed from the Server field and the port.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    host: String,
    /// The instance of `host\instance` when its port must be looked up with
    /// SQL Server Browser (UDP 1434).
    instance: Option<String>,
    /// The TCP port when it is known.
    port: Option<u16>,
}

impl Target {
    /// Parses `host` or `host\instance`. A named instance is resolved through
    /// SQL Server Browser when `port` is the default 1433; any other port is
    /// used directly, as ADO.NET does for `host\instance,port`.
    fn parse(server: &str, port: u16) -> Result<Target, DriverError> {
        let server = server.trim();
        let Some((host, instance)) = server.split_once('\\') else {
            return Ok(Target {
                host: server.to_string(),
                instance: None,
                port: Some(port),
            });
        };
        let (host, instance) = (host.trim(), instance.trim());
        if host.is_empty() || instance.is_empty() || instance.contains('\\') {
            return Err(DriverError::Other(format!(
                "invalid server name `{server}`: use host or host\\instance"
            )));
        }
        let browse = port == DEFAULT_PORT;
        Ok(Target {
            host: host.to_string(),
            instance: browse.then(|| instance.to_string()),
            port: (!browse).then_some(port),
        })
    }

    /// `host:port`, or `host\instance` while the port is unknown.
    fn describe(&self) -> String {
        match (&self.instance, self.port) {
            (Some(instance), _) => format!("{}\\{instance}", self.host),
            (None, Some(port)) => format!("{}:{port}", self.host),
            (None, None) => self.host.clone(),
        }
    }
}

/// Opens a session. `connect_timeout_s` bounds the SQL Browser lookup, the
/// TCP connection and the login together; queries run without a timeout.
pub async fn connect(p: &ConnectParams) -> Result<MssqlSession, DriverError> {
    let target = Target::parse(&p.server, p.port)?;
    let timeout =
        (p.connect_timeout_s > 0).then(|| Duration::from_secs(p.connect_timeout_s.into()));
    let client = match timeout {
        Some(limit) => tokio::time::timeout(limit, open(p, &target, timeout))
            .await
            .map_err(|_| {
                DriverError::Unreachable(format!(
                    "{} did not answer within {} s",
                    target.describe(),
                    p.connect_timeout_s
                ))
            })??,
        None => open(p, &target, None).await?,
    };
    let cancel = client.cancel_handle();
    Ok(MssqlSession {
        client: Some(client),
        cancel,
        cancel_requested: Arc::new(AtomicBool::new(false)),
        single_database: None,
    })
}

fn config(p: &ConnectParams, target: &Target, timeout: Option<Duration>) -> Config {
    let mut config = Config::new();
    config.host(&target.host);
    if let Some(port) = target.port {
        config.port(port);
    }
    if let Some(instance) = &target.instance {
        // Without a port, Tiberius asks SQL Browser on UDP 1434.
        config.instance_name(instance);
    }
    if let Some(db) = p.database.as_deref().filter(|db| !db.is_empty()) {
        config.database(db);
    }
    config.application_name(&p.application_name);
    config.authentication(AuthMethod::sql_server(&p.user, &p.password));
    config.encryption(if p.encrypt {
        EncryptionLevel::Required
    } else {
        // SQL Server still encrypts the login packet.
        EncryptionLevel::Off
    });
    if p.trust_server_certificate {
        config.trust_cert();
    }
    config.handshake_timeout(timeout);
    // Tiberius defaults to a 30 s per-round-trip timeout that leaves the
    // connection unusable when it fires. Queries must have no timeout.
    config.command_timeout(None);
    // Show malformed UTF-16 (lone surrogates) with replacement characters
    // instead of failing the whole result.
    config.lossy_utf16_decoding(true);
    config
}

async fn open(
    p: &ConnectParams,
    target: &Target,
    timeout: Option<Duration>,
) -> Result<Client, DriverError> {
    let mut config = config(p, target, timeout);
    let mut target_name = target.describe();
    let mut named = target.instance.is_some();
    // Azure SQL can redirect the login to another node, once.
    for _ in 0..2 {
        let tcp = if named {
            // Looks the port up with SQL Browser, then connects to it.
            TcpStream::connect_named(&config)
                .await
                .map_err(|e| browser_error(&target_name, e))?
        } else {
            let addr = config.get_addr();
            TcpStream::connect(&addr)
                .await
                .map_err(|e| DriverError::Unreachable(format!("{addr}: {e}")))?
        };
        // Latency matters more than throughput for an interactive client.
        let _ = tcp.set_nodelay(true);
        match Client::connect(config.clone(), tcp.compat_write()).await {
            Ok(client) => return Ok(client),
            Err(TdsError::Routing { host, port }) => {
                // The redirect names a host and port; no instance lookup.
                let routed = Target {
                    host,
                    instance: None,
                    port: Some(port),
                };
                config = self::config(p, &routed, timeout);
                target_name = routed.describe();
                named = false;
            }
            Err(e) => return Err(login_error(e)),
        }
    }
    Err(DriverError::Other(
        "the server redirected the login more than once".into(),
    ))
}

/// Maps a failed SQL Browser lookup or connection to a named instance.
fn browser_error(target: &str, e: TdsError) -> DriverError {
    match e {
        // Windows reports a closed UDP 1434 as a reset connection: the Browser
        // did not answer, so the hint applies there too.
        TdsError::Io {
            kind: std::io::ErrorKind::ConnectionReset,
            ..
        } => browser_hint(target),
        TdsError::Io { message, .. } => DriverError::Unreachable(format!("{target}: {message}")),
        _ => browser_hint(target),
    }
}

fn browser_hint(target: &str) -> DriverError {
    DriverError::Unreachable(format!(
        "{target}: SQL Server Browser (UDP 1434) did not return the instance's port. \
         Check the instance name and that the SQL Server Browser service is running, \
         or enter the instance's TCP port."
    ))
}

fn login_error(e: TdsError) -> DriverError {
    match e {
        TdsError::Server(te) if AUTH_ERRORS.contains(&te.code()) => {
            DriverError::AuthFailed(te.message().to_string())
        }
        TdsError::Server(te) => DriverError::Other(te.message().to_string()),
        TdsError::Tls(msg) => DriverError::Tls(msg),
        TdsError::Io { message, .. } => DriverError::Unreachable(message),
        other => DriverError::Other(other.to_string()),
    }
}

/// Maps an error of a running request. `line_of` turns a server line number
/// (1-based within the batch) into an editor line.
fn request_error(e: TdsError, line_of: impl Fn(u32) -> Option<u32>) -> DriverError {
    match e {
        TdsError::Cancelled => DriverError::Cancelled,
        TdsError::Server(te) if te.class() >= FATAL_SEVERITY => {
            DriverError::Disconnected(te.message().to_string())
        }
        TdsError::Server(te) => DriverError::Query {
            message: te.message().to_string(),
            line: line_of(te.line()),
        },
        TdsError::Io { message, .. } => DriverError::Disconnected(message),
        other => DriverError::Other(other.to_string()),
    }
}

fn closed() -> DriverError {
    DriverError::Disconnected("the connection is closed".into())
}

impl MssqlSession {
    /// Runs a statement and returns the rows of its first result set.
    async fn query_rows(&mut self, sql: &str) -> Result<Vec<Row>, DriverError> {
        let client = self.client.as_mut().ok_or_else(closed)?;
        let result = async { client.simple_query(sql).await?.into_first_result().await }.await;
        result.map_err(|e| self.fail(request_error(e, |_| None)))
    }

    /// Runs a statement with `@P1`, `@P2`... parameters and returns the rows of
    /// its first result set.
    async fn query_params(
        &mut self,
        sql: &str,
        params: &[&dyn ToSql],
    ) -> Result<Vec<Row>, DriverError> {
        let client = self.client.as_mut().ok_or_else(closed)?;
        let result = async { client.query(sql, params).await?.into_first_result().await }.await;
        result.map_err(|e| self.fail(request_error(e, |_| None)))
    }

    /// Whether the server hosts a single database (`EngineEdition` 5).
    async fn is_single_database(&mut self) -> Result<bool, DriverError> {
        if let Some(single) = self.single_database {
            return Ok(single);
        }
        let rows = self
            .query_rows("SELECT CAST(SERVERPROPERTY('EngineEdition') AS int)")
            .await?;
        let single = rows.first().and_then(|r| r.get::<i32, _>(0)) == Some(5);
        self.single_database = Some(single);
        Ok(single)
    }

    /// Checks that `database` can be read and returns the prefix of its
    /// catalog views. On a single-database server the current database is
    /// the only one, and `database` is ignored.
    async fn catalog(&mut self, database: &str) -> Result<(String, String), DriverError> {
        let single = self.is_single_database().await?;
        let database = if single {
            self.query_string("SELECT DB_NAME()").await?
        } else {
            database.to_string()
        };
        let rows = self.query_params(meta::ACCESS_SQL, &[&database]).await?;
        let row = rows.first();
        let access = row.and_then(|r| r.get::<i32, _>(0));
        let status = row.and_then(|r| r.get::<&str, _>(1));
        if let Some(e) = meta::access_error(&database, access, status) {
            return Err(e);
        }
        Ok((meta::catalog_prefix(&database, single), database))
    }

    /// Runs a statement that returns one non-null string.
    async fn query_string(&mut self, sql: &str) -> Result<String, DriverError> {
        let rows = self.query_rows(sql).await?;
        rows.first()
            .and_then(|row| row.get::<&str, _>(0))
            .map(str::to_string)
            .ok_or_else(|| DriverError::Other(format!("no value returned by `{sql}`")))
    }

    /// Runs `sql` inside a transaction that is rolled back whatever happens,
    /// without checking that it only reads. Callers must have checked it.
    ///
    /// The session must not be used afterwards: row limits stay in force, and
    /// it is closed if the script left the transaction in an unexpected state.
    async fn run_rolled_back(
        &mut self,
        sql: &str,
        max_rows: u32,
        tx: Sender<ExecEvent>,
    ) -> Result<(), DriverError> {
        // Cleared once, before the preamble: a cancel from here on stops the
        // script, even one that lands between the preamble and its first batch.
        self.cancel_requested.store(false, Ordering::SeqCst);
        if let Err(e) = self.query_rows(&read_only_preamble(max_rows)).await {
            // Unknown whether the transaction opened: do not keep the session.
            self.client = None;
            return Err(e);
        }
        let result = self.run_events(sql, tx).await;
        if matches!(result, Err(DriverError::Disconnected(_))) {
            // The server rolls back when the connection goes away.
            return result;
        }
        let open = match self.query_rows(READ_ONLY_END).await {
            Ok(rows) => rows.first().and_then(|r| r.get::<i32, _>(0)),
            Err(e) => {
                self.client = None;
                return result.and(Err(e));
            }
        };
        // One transaction is open after a script that ran to the end. After a
        // failure or a cancel, XACT_ABORT or the Attention may have ended it.
        let expected = if result.is_ok() {
            open == Some(1)
        } else {
            matches!(open, Some(0 | 1))
        };
        if !expected {
            self.client = None;
            return Err(DriverError::Other(
                "the query changed the transaction state; the session was closed".into(),
            ));
        }
        result
    }

    /// [`Session::execute_read_only`] without the read-only check, to test
    /// that the rollback undoes a write that got through.
    #[cfg(test)]
    async fn execute_in_rolled_back_transaction(
        &mut self,
        sql: &str,
        max_rows: u32,
        tx: Sender<ExecEvent>,
    ) -> Result<(), DriverError> {
        self.run_rolled_back(sql, max_rows, tx).await
    }

    /// Runs the script and reports how it ended, leaving the cancel flag as it is.
    async fn run_events(&mut self, sql: &str, tx: Sender<ExecEvent>) -> Result<(), DriverError> {
        let mut out = Emitter {
            tx,
            receiver_gone: false,
            cancel: self.cancel.clone(),
            cancel_requested: self.cancel_requested.clone(),
        };

        let result = match self.client.as_mut() {
            None => Err(closed()),
            Some(client) => run_script(client, sql, &mut out).await,
        };
        let Err(e) = result else { return Ok(()) };

        match &e {
            DriverError::Query { message, line } => {
                out.send(ExecEvent::Message {
                    kind: MessageKind::Error,
                    text: message.clone(),
                    line: *line,
                })
                .await;
            }
            DriverError::Disconnected(reason) => {
                out.send(ExecEvent::Disconnected {
                    reason: reason.clone(),
                })
                .await;
            }
            DriverError::Other(text) => {
                out.send(ExecEvent::Message {
                    kind: MessageKind::Error,
                    text: text.clone(),
                    line: None,
                })
                .await;
            }
            _ => {}
        }
        Err(self.fail(e))
    }

    /// Drops the connection when `e` means it was lost.
    fn fail(&mut self, e: DriverError) -> DriverError {
        if matches!(e, DriverError::Disconnected(_)) {
            self.client = None;
        }
        e
    }
}

#[async_trait]
impl Session for MssqlSession {
    async fn server_info(&mut self) -> Result<ServerInfo, DriverError> {
        let single_database = self.is_single_database().await?;
        let rows = self.query_rows("SELECT @@VERSION, SUSER_SNAME()").await?;
        let row = rows
            .first()
            .ok_or_else(|| DriverError::Other("no server information returned".into()))?;
        // The first line of @@VERSION names the product and build.
        let version = row
            .get::<&str, _>(0)
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let user = row.get::<&str, _>(1).unwrap_or_default().to_string();
        Ok(ServerInfo {
            version,
            user,
            single_database,
        })
    }

    async fn snapshot(&mut self, database: &str) -> Result<DatabaseSnapshot, DriverError> {
        let (prefix, _) = self.catalog(database).await?;
        let objects: Vec<_> = self
            .query_rows(&meta::objects_sql(&prefix))
            .await?
            .iter()
            .filter_map(meta::object_from_row)
            .collect();
        let index_of: HashMap<(&str, &str), usize> = objects
            .iter()
            .enumerate()
            .map(|(i, o)| ((o.schema.as_str(), o.name.as_str()), i))
            .collect();

        let mut columns: Vec<(usize, Vec<ColumnInfo>)> = Vec::new();
        for row in self.query_rows(&meta::columns_sql(&prefix, false)).await? {
            let Some(((schema, name), column)) = meta::column_from_row(&row) else {
                continue;
            };
            let Some(&index) = index_of.get(&(schema.as_str(), name.as_str())) else {
                continue;
            };
            // Rows arrive grouped by object.
            match columns.last_mut().filter(|(i, _)| *i == index) {
                Some((_, list)) => list.push(column),
                None => columns.push((index, vec![column])),
            }
        }
        Ok(DatabaseSnapshot { objects, columns })
    }

    async fn structure(
        &mut self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<TableStructure, DriverError> {
        let (prefix, _) = self.catalog(database).await?;
        let params: [&dyn ToSql; 2] = [&name, &schema];
        let columns = self
            .query_params(&meta::columns_sql(&prefix, true), &params)
            .await?
            .iter()
            .filter_map(|row| meta::column_from_row(row).map(|(_, c)| c))
            .collect();
        let foreign_keys = self
            .query_params(&meta::foreign_keys_sql(&prefix), &params)
            .await?;
        let indexes = self
            .query_params(&meta::indexes_sql(&prefix), &params)
            .await?;
        Ok(TableStructure {
            columns,
            foreign_keys: meta::foreign_keys_from_rows(&foreign_keys),
            indexes: meta::indexes_from_rows(&indexes),
        })
    }

    async fn definition(
        &mut self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<String, DriverError> {
        let (prefix, database) = self.catalog(database).await?;
        let rows = self
            .query_params(&meta::definition_sql(&prefix), &[&name, &schema])
            .await?;
        let Some(row) = rows.first() else {
            return Err(DriverError::Other(format!(
                "`{schema}`.`{name}` has no definition in `{database}`"
            )));
        };
        match row.get::<&str, _>(0) {
            Some(text) => Ok(text.to_string()),
            // The server hides the text from logins without VIEW DEFINITION.
            None => Err(DriverError::NoAccess(format!(
                "no permission to view the definition of `{schema}`.`{name}` in `{database}`"
            ))),
        }
    }

    async fn list_databases(&mut self) -> Result<Vec<String>, DriverError> {
        let rows = self
            .query_rows("SELECT name FROM sys.databases ORDER BY name")
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get::<&str, _>(0).map(str::to_string))
            .collect())
    }

    async fn use_database(&mut self, name: &str) -> Result<(), DriverError> {
        self.query_rows(&format!("USE {}", quote_ident(name)))
            .await
            .map(|_| ())
    }

    async fn current_database(&mut self) -> Result<String, DriverError> {
        self.query_string("SELECT DB_NAME()").await
    }

    async fn execute(&mut self, sql: &str, tx: Sender<ExecEvent>) -> Result<(), DriverError> {
        self.cancel_requested.store(false, Ordering::SeqCst);
        self.run_events(sql, tx).await
    }

    async fn execute_read_only(
        &mut self,
        sql: &str,
        max_rows: u32,
        tx: Sender<ExecEvent>,
    ) -> Result<(), DriverError> {
        // The text that was checked is the text that runs: `execute` splits
        // it into batches with the same `split_batches` the check used.
        if let Some(v) = readonly::violation(sql) {
            return Err(refusal(&v));
        }
        self.run_rolled_back(sql, max_rows, tx).await
    }

    async fn transaction_count(&mut self) -> Result<u32, DriverError> {
        let rows = self.query_rows("SELECT @@TRANCOUNT").await?;
        // No answer is not "no transaction": report it rather than let a close go ahead.
        let count = rows
            .first()
            .and_then(|r| r.get::<i32, _>(0))
            .ok_or_else(|| {
                DriverError::Other("no value returned by `SELECT @@TRANCOUNT`".into())
            })?;
        Ok(u32::try_from(count).unwrap_or(0))
    }

    async fn end_transactions(&mut self, commit: bool) -> Result<(), DriverError> {
        // A commit has to be repeated for nested transactions; a rollback
        // ends all of them at once and fails if none is open.
        let sql = if commit {
            "WHILE @@TRANCOUNT > 0 COMMIT TRANSACTION"
        } else {
            "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION"
        };
        self.query_rows(sql).await.map(|_| ())
    }

    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(MssqlCancel {
            inner: self.cancel.clone(),
            requested: self.cancel_requested.clone(),
        })
    }
}

struct MssqlCancel {
    inner: tiberius::CancelHandle,
    requested: Arc<AtomicBool>,
}

#[async_trait]
impl CancelHandle for MssqlCancel {
    async fn cancel(&self) -> Result<(), DriverError> {
        self.requested.store(true, Ordering::SeqCst);
        // Fails only when the session is gone, and then nothing is running.
        let _ = self.inner.cancel().await;
        Ok(())
    }
}

/// Sends events to the UI. If the receiver goes away, the running request is
/// cancelled instead of streaming its results into the void.
struct Emitter {
    tx: Sender<ExecEvent>,
    receiver_gone: bool,
    cancel: tiberius::CancelHandle,
    cancel_requested: Arc<AtomicBool>,
}

impl Emitter {
    async fn send(&mut self, event: ExecEvent) {
        if self.receiver_gone {
            return;
        }
        if self.tx.send(event).await.is_err() {
            self.receiver_gone = true;
            self.cancel_requested.store(true, Ordering::SeqCst);
            let _ = self.cancel.cancel().await;
        }
    }

    fn cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }
}

/// Runs every batch of `sql` in order, stopping at the first failure.
async fn run_script(client: &mut Client, sql: &str, out: &mut Emitter) -> Result<(), DriverError> {
    let mut next_result = 0u32;
    for (n, batch) in split_batches(sql).iter().enumerate() {
        if out.cancel_requested() {
            return Err(DriverError::Cancelled);
        }
        run_batch(client, batch, &mut next_result, out).await?;
        out.send(ExecEvent::BatchDone { batch: n as u32 }).await;
    }
    Ok(())
}

/// The result set being streamed.
struct ResultSet {
    index: u32,
    types: Vec<ColumnType>,
}

/// Rows waiting to be sent.
struct RowBuffer {
    rows: Vec<Vec<Value>>,
    /// When the buffered rows must be sent at the latest.
    deadline: Instant,
}

impl RowBuffer {
    async fn flush(&mut self, index: u32, out: &mut Emitter) {
        if !self.rows.is_empty() {
            let rows = std::mem::take(&mut self.rows);
            out.send(ExecEvent::Rows { index, rows }).await;
        }
    }
}

async fn run_batch(
    client: &mut Client,
    batch: &Batch,
    next_result: &mut u32,
    out: &mut Emitter,
) -> Result<(), DriverError> {
    let line_of = |line: u32| (line > 0).then(|| batch.start_line + line - 1);

    let stream = client.simple_query(batch.text.as_str());
    let mut stream = resend_cancel_while(stream, out)
        .await
        .map_err(|e| request_error(e, line_of))?;

    let mut current: Option<ResultSet> = None;
    let mut buffer = RowBuffer {
        rows: Vec::new(),
        deadline: Instant::now(),
    };
    let index_of = |current: &Option<ResultSet>| current.as_ref().map_or(0, |r| r.index);

    loop {
        let item = if buffer.rows.is_empty() {
            stream.try_next().await
        } else {
            tokio::select! {
                biased;
                item = stream.try_next() => item,
                _ = tokio::time::sleep_until(buffer.deadline) => {
                    buffer.flush(index_of(&current), out).await;
                    continue;
                }
            }
        };

        match item {
            Ok(None) => break,
            Ok(Some(QueryItem::Metadata(meta))) => {
                buffer.flush(index_of(&current), out).await;
                let index = *next_result;
                *next_result += 1;
                let columns = meta
                    .columns()
                    .iter()
                    .map(|c| Column {
                        name: c.name().to_string(),
                        type_name: type_name(c.column_type()).to_string(),
                    })
                    .collect();
                current = Some(ResultSet {
                    index,
                    types: meta.columns().iter().map(|c| c.column_type()).collect(),
                });
                out.send(ExecEvent::ResultStart { index, columns }).await;
            }
            Ok(Some(QueryItem::Row(row))) => {
                let types = current.as_ref().map(|r| r.types.as_slice()).unwrap_or(&[]);
                let values = row
                    .into_iter()
                    .enumerate()
                    .map(|(i, data)| {
                        to_value(data, types.get(i).copied().unwrap_or(ColumnType::Null))
                    })
                    .collect();
                if buffer.rows.is_empty() {
                    buffer.deadline = Instant::now() + FLUSH_INTERVAL;
                }
                buffer.rows.push(values);
                if buffer.rows.len() >= ROWS_PER_EVENT || Instant::now() >= buffer.deadline {
                    buffer.flush(index_of(&current), out).await;
                }
            }
            Ok(Some(QueryItem::Info(info))) => {
                buffer.flush(index_of(&current), out).await;
                out.send(ExecEvent::Message {
                    kind: MessageKind::Info,
                    text: info.message,
                    line: line_of(info.line),
                })
                .await;
            }
            Ok(Some(QueryItem::Done(done))) => {
                // `None` is a statement without a row count (DDL, SET, PRINT...).
                if let Some(count) = done.rows_affected {
                    buffer.flush(index_of(&current), out).await;
                    out.send(ExecEvent::RowsAffected { count }).await;
                }
            }
            Err(e) => {
                buffer.flush(index_of(&current), out).await;
                return Err(request_error(e, line_of));
            }
        }
    }
    buffer.flush(index_of(&current), out).await;
    Ok(())
}

/// Awaits `request` (a request being sent), re-sending a requested cancel
/// periodically. Tiberius discards a cancel that arrives before the request
/// is written, so a Stop pressed in that window would otherwise be lost
/// until the batch finished on its own.
async fn resend_cancel_while<F: Future>(request: F, out: &Emitter) -> F::Output {
    tokio::pin!(request);
    loop {
        tokio::select! {
            biased;
            result = &mut request => return result,
            _ = tokio::time::sleep(CANCEL_RETRY) => {
                if out.cancel_requested() {
                    let _ = out.cancel.cancel().await;
                }
            }
        }
    }
}

// A path on a module nested inside `tests` would not resolve, so it lives here.
#[cfg(test)]
#[path = "../tests/support/params.rs"]
mod params;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reset_browser_lookup_gets_the_browser_hint() {
        let io = |kind, message: &str| TdsError::Io {
            kind,
            message: message.into(),
        };
        let reset = io(
            std::io::ErrorKind::ConnectionReset,
            "forcibly closed (os error 10054)",
        );
        assert!(matches!(
            browser_error("db01\\NOPE", reset),
            DriverError::Unreachable(m) if m.contains("SQL Server Browser") && m.starts_with("db01\\NOPE")
        ));
        let dns = io(std::io::ErrorKind::Other, "no such host");
        assert!(matches!(
            browser_error("db01\\NOPE", dns),
            DriverError::Unreachable(m) if m == "db01\\NOPE: no such host"
        ));
    }

    fn target(host: &str, instance: Option<&str>, port: Option<u16>) -> Target {
        Target {
            host: host.into(),
            instance: instance.map(Into::into),
            port,
        }
    }

    #[test]
    fn plain_host_uses_the_port() {
        assert_eq!(
            Target::parse("db01", 1433).unwrap(),
            target("db01", None, Some(1433))
        );
        assert_eq!(
            Target::parse(" 10.0.0.5 ", 14333).unwrap(),
            target("10.0.0.5", None, Some(14333))
        );
    }

    #[test]
    fn named_instance_on_the_default_port_asks_sql_browser() {
        let t = Target::parse("db01\\SQLEXPRESS", 1433).unwrap();
        assert_eq!(t, target("db01", Some("SQLEXPRESS"), None));
        assert_eq!(t.describe(), "db01\\SQLEXPRESS");
        assert_eq!(
            Target::parse(" db01 \\ SQLEXPRESS ", 1433).unwrap(),
            target("db01", Some("SQLEXPRESS"), None)
        );
    }

    #[test]
    fn named_instance_with_another_port_connects_to_it() {
        let t = Target::parse("db01\\SQLEXPRESS", 50123).unwrap();
        assert_eq!(t, target("db01", None, Some(50123)));
        assert_eq!(t.describe(), "db01:50123");
    }

    #[test]
    fn malformed_instance_names_are_rejected() {
        for server in ["db01\\", "\\SQLEXPRESS", "\\", "db01\\a\\b"] {
            assert!(
                matches!(Target::parse(server, 1433), Err(DriverError::Other(_))),
                "{server}"
            );
        }
    }

    #[test]
    fn config_sets_the_instance_only_for_a_browser_lookup() {
        let p = ConnectParams::default();
        let named = config(&p, &Target::parse("db01\\SQLEXPRESS", 1433).unwrap(), None);
        // Tiberius sends the SQL Browser request to UDP 1434.
        assert_eq!(named.get_addr(), "db01:1434");
        let direct = config(&p, &Target::parse("db01\\SQLEXPRESS", 50123).unwrap(), None);
        assert_eq!(direct.get_addr(), "db01:50123");
    }

    #[test]
    fn preamble_text_is_exact() {
        assert_eq!(
            read_only_preamble(500),
            "SET NOCOUNT ON; SET XACT_ABORT ON; SET IMPLICIT_TRANSACTIONS OFF; \
             SET QUOTED_IDENTIFIER ON; SET ANSI_NULLS ON; \
             SET TRANSACTION ISOLATION LEVEL READ COMMITTED; SET LOCK_TIMEOUT 5000; \
             SET DEADLOCK_PRIORITY LOW; SET ROWCOUNT 501; BEGIN TRANSACTION;"
        );
        assert!(read_only_preamble(u32::MAX).contains("SET ROWCOUNT 4294967295;"));
    }

    use super::params;

    #[tokio::test]
    async fn rollback_undoes_a_write_that_slipped_through() {
        let Some(admin) = params::test_params() else {
            return;
        };
        params::with_temp_db(admin, |p| async move {
            let mut s = connect(&p).await.unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::channel(16);
            let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let result = s
                .execute_in_rolled_back_transaction(
                    "INSERT INTO dbo.w VALUES (4); SELECT 1",
                    10,
                    tx,
                )
                .await;
            drain.await.unwrap();
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(params::count_w(&p).await, 3);
        })
        .await;
    }

    #[tokio::test]
    async fn commit_inside_is_detected_and_closes_the_session() {
        let Some(admin) = params::test_params() else {
            return;
        };
        params::with_temp_db(admin, |p| async move {
            let mut s = connect(&p).await.unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::channel(16);
            let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let result = s
                .execute_in_rolled_back_transaction("SELECT 1; COMMIT", 10, tx)
                .await;
            drain.await.unwrap();
            assert!(
                matches!(&result, Err(DriverError::Other(m)) if m.contains("transaction state")),
                "{result:?}"
            );
            let (next, _) = params::collect(&mut s, "SELECT 1").await;
            assert!(
                matches!(next, Err(DriverError::Disconnected(_))),
                "{next:?}"
            );
        })
        .await;
    }
}
