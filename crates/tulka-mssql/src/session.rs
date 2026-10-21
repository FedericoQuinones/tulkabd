//! A SQL Server session over the vendored Tiberius fork.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::TryStreamExt;
use tiberius::error::Error as TdsError;
use tiberius::{AuthMethod, ColumnType, Config, EncryptionLevel, QueryItem, Row, SqlBrowser};
use tokio::net::TcpStream;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};
use tulka_core::{
    CancelHandle, Column, ConnectParams, DatabaseSnapshot, DriverError, ExecEvent, MessageKind,
    ServerInfo, Session, TableStructure, Value,
};

use crate::batches::{split_batches, Batch};
use crate::convert::{to_value, type_name};

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

pub struct MssqlSession {
    /// `None` once the connection is lost.
    client: Option<Client>,
    cancel: tiberius::CancelHandle,
    /// Set by [`MssqlCancel`] and cleared when `execute` starts; stops the
    /// remaining batches of a script.
    cancel_requested: Arc<AtomicBool>,
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
        TdsError::Io { message, .. } => DriverError::Unreachable(format!("{target}: {message}")),
        _ => DriverError::Unreachable(format!(
            "{target}: SQL Server Browser (UDP 1434) did not return the instance's port. \
             Check the instance name and that the SQL Server Browser service is running, \
             or enter the instance's TCP port."
        )),
    }
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

/// Quotes a name as a bracketed identifier.
fn quote_ident(name: &str) -> String {
    format!("[{}]", name.replace(']', "]]"))
}

impl MssqlSession {
    /// Runs a statement and returns the rows of its first result set.
    async fn query_rows(&mut self, sql: &str) -> Result<Vec<Row>, DriverError> {
        let client = self.client.as_mut().ok_or_else(closed)?;
        let result = async { client.simple_query(sql).await?.into_first_result().await }.await;
        result.map_err(|e| self.fail(request_error(e, |_| None)))
    }

    /// Runs a statement that returns one non-null string.
    async fn query_string(&mut self, sql: &str) -> Result<String, DriverError> {
        let rows = self.query_rows(sql).await?;
        rows.first()
            .and_then(|row| row.get::<&str, _>(0))
            .map(str::to_string)
            .ok_or_else(|| DriverError::Other(format!("no value returned by `{sql}`")))
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
            single_database: false,
        })
    }

    async fn snapshot(&mut self, _database: &str) -> Result<DatabaseSnapshot, DriverError> {
        Err(DriverError::Other("not implemented".into()))
    }

    async fn structure(
        &mut self,
        _database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<TableStructure, DriverError> {
        Err(DriverError::Other("not implemented".into()))
    }

    async fn definition(
        &mut self,
        _database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<String, DriverError> {
        Err(DriverError::Other("not implemented".into()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
