//! Command logic against a scripted fake driver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use std::thread::ThreadId;
use tempfile::TempDir;

use tokio::sync::mpsc::{self, Sender, UnboundedReceiver};
use tokio::sync::Notify;
use tulka_app::*;
use tulka_core::{
    estimate_tokens, system_decimal_separator, to_ai, to_excel_tsv, CancelHandle, Column,
    ColumnInfo, ConfigStore, ConnectParams, ConnectionConfig, DatabaseScope, DatabaseSnapshot,
    DbObject, Driver, DriverError, Environment, ExecEvent, MemoryStore, MessageKind, ObjectKind,
    SecretStore, ServerInfo, Session, TableStructure, UnavailableStore, Value,
};

const PASSWORD: &str = "s3cret-pw";
const TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- fake driver

struct FakeDriver;

#[async_trait]
impl Driver for FakeDriver {
    fn id(&self) -> &'static str {
        "fake"
    }

    async fn connect(&self, p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        if p.password != PASSWORD {
            return Err(DriverError::AuthFailed(format!(
                "Login failed for user '{}'.",
                p.user
            )));
        }
        Ok(Box::new(FakeSession {
            user: p.user.clone(),
            database: p.database.clone().unwrap_or_else(|| "master".into()),
            cancel: Arc::new(FakeCancel::default()),
            dropped: None,
        }))
    }
}

/// Like [`FakeDriver`], but `connect` signals `entered` and then waits for
/// `release`; `dropped` is set when one of its sessions is dropped.
#[derive(Default)]
struct GatedDriver {
    entered: Notify,
    release: Notify,
    dropped: Arc<AtomicBool>,
}

#[async_trait]
impl Driver for GatedDriver {
    fn id(&self) -> &'static str {
        "gated"
    }

    async fn connect(&self, p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(Box::new(FakeSession {
            user: p.user.clone(),
            database: "master".into(),
            cancel: Arc::new(FakeCancel::default()),
            dropped: Some(self.dropped.clone()),
        }))
    }
}

struct FakeSession {
    user: String,
    database: String,
    cancel: Arc<FakeCancel>,
    dropped: Option<Arc<AtomicBool>>,
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        if let Some(flag) = &self.dropped {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

#[derive(Default)]
struct FakeCancel {
    requested: AtomicBool,
}

#[async_trait]
impl CancelHandle for FakeCancel {
    async fn cancel(&self) -> Result<(), DriverError> {
        self.requested.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn fake_columns() -> Vec<Column> {
    ["id", "amount", "name"]
        .iter()
        .map(|n| Column {
            name: n.to_string(),
            type_name: "fake".into(),
        })
        .collect()
}

fn fake_row(i: usize) -> Vec<Value> {
    vec![
        Value::Int(i as i64),
        Value::Decimal(format!("{i}.50")),
        Value::Text(format!("row\t{i}")),
    ]
}

/// A row with a text and a binary cell, both over [`MAX_CELL_BYTES`].
fn big_row() -> Vec<Value> {
    vec![
        Value::Text("x".repeat(MAX_CELL_BYTES + 100)),
        Value::Bytes(vec![0xAB; MAX_CELL_BYTES + 100]),
    ]
}

impl FakeSession {
    /// Sends a new result of `n` rows of [`big_row`].
    async fn big_rows(&self, index: &mut u32, n: usize, tx: &Sender<ExecEvent>) {
        let i = *index;
        *index += 1;
        let columns = ["doc", "blob"]
            .iter()
            .map(|n| Column {
                name: n.to_string(),
                type_name: "fake".into(),
            })
            .collect();
        let _ = tx.send(ExecEvent::ResultStart { index: i, columns }).await;
        let rows = (0..n).map(|_| big_row()).collect();
        let _ = tx.send(ExecEvent::Rows { index: i, rows }).await;
    }

    /// Sends `n` rows of a new result in chunks of `chunk`.
    async fn rows(
        &self,
        index: &mut u32,
        n: usize,
        chunk: usize,
        tx: &Sender<ExecEvent>,
    ) -> Result<(), DriverError> {
        let i = *index;
        *index += 1;
        let _ = tx
            .send(ExecEvent::ResultStart {
                index: i,
                columns: fake_columns(),
            })
            .await;
        let mut sent = 0;
        while sent < n {
            let end = (sent + chunk).min(n);
            let rows = (sent..end).map(fake_row).collect();
            let _ = tx.send(ExecEvent::Rows { index: i, rows }).await;
            sent = end;
        }
        Ok(())
    }
}

fn fake_column() -> ColumnInfo {
    ColumnInfo {
        name: "id".into(),
        type_name: "int".into(),
        nullable: false,
        primary_key: true,
        default: None,
    }
}

#[async_trait]
impl Session for FakeSession {
    async fn server_info(&mut self) -> Result<ServerInfo, DriverError> {
        Ok(ServerInfo {
            version: "Fake SQL 1".into(),
            user: self.user.clone(),
            single_database: false,
        })
    }

    async fn snapshot(&mut self, _database: &str) -> Result<DatabaseSnapshot, DriverError> {
        Ok(DatabaseSnapshot {
            objects: vec![DbObject {
                schema: "dbo".into(),
                name: "Pedidos".into(),
                kind: ObjectKind::Table,
            }],
            columns: vec![(0, vec![fake_column()])],
        })
    }

    async fn structure(
        &mut self,
        _database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<TableStructure, DriverError> {
        Ok(TableStructure {
            columns: vec![fake_column()],
            foreign_keys: vec![],
            indexes: vec![],
        })
    }

    async fn definition(
        &mut self,
        _database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<String, DriverError> {
        Ok("CREATE VIEW dbo.V AS SELECT 1".into())
    }

    async fn list_databases(&mut self) -> Result<Vec<String>, DriverError> {
        Ok(["a", "b", "c", "master"].map(String::from).to_vec())
    }

    async fn use_database(&mut self, name: &str) -> Result<(), DriverError> {
        if name == "missing" {
            return Err(DriverError::Query {
                message: format!("Database '{name}' does not exist."),
                line: None,
            });
        }
        self.database = name.into();
        Ok(())
    }

    async fn current_database(&mut self) -> Result<String, DriverError> {
        Ok(self.database.clone())
    }

    /// Statements separated by `;`: `rows N`, `trickle N` (one row per
    /// event), `big N` (rows of [`big_row`]), `wait` (until cancelled),
    /// `error`, `disconnect`, `affected N`, `panic`.
    async fn execute(&mut self, sql: &str, tx: Sender<ExecEvent>) -> Result<(), DriverError> {
        self.cancel.requested.store(false, Ordering::SeqCst);
        let mut index = 0;
        for stmt in sql.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            let (cmd, arg) = stmt.split_once(' ').unwrap_or((stmt, "0"));
            let n: usize = arg.parse().unwrap();
            match cmd {
                "rows" => self.rows(&mut index, n, 500, &tx).await?,
                "trickle" => self.rows(&mut index, n, 1, &tx).await?,
                "big" => self.big_rows(&mut index, n, &tx).await,
                "affected" => {
                    let _ = tx.send(ExecEvent::RowsAffected { count: n as u64 }).await;
                }
                "wait" => loop {
                    if self.cancel.requested.load(Ordering::SeqCst) {
                        return Err(DriverError::Cancelled);
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                },
                "panic" => panic!("fake driver panic"),
                "error" => {
                    let _ = tx
                        .send(ExecEvent::Message {
                            kind: MessageKind::Error,
                            text: "Divide by zero error encountered.".into(),
                            line: Some(1),
                        })
                        .await;
                    return Err(DriverError::Query {
                        message: "Divide by zero error encountered.".into(),
                        line: Some(1),
                    });
                }
                "disconnect" => {
                    let reason = "connection reset by peer".to_string();
                    let _ = tx
                        .send(ExecEvent::Disconnected {
                            reason: reason.clone(),
                        })
                        .await;
                    return Err(DriverError::Disconnected(reason));
                }
                other => panic!("unknown fake statement {other}"),
            }
        }
        let _ = tx.send(ExecEvent::BatchDone { batch: 0 }).await;
        Ok(())
    }

    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(CancelRef(self.cancel.clone()))
    }
}

struct CancelRef(Arc<FakeCancel>);

#[async_trait]
impl CancelHandle for CancelRef {
    async fn cancel(&self) -> Result<(), DriverError> {
        self.0.cancel().await
    }
}

#[derive(Default)]
struct FakeClipboard {
    texts: Mutex<Vec<String>>,
    threads: Mutex<Vec<ThreadId>>,
}

impl Clipboard for FakeClipboard {
    fn write_text(&self, text: String) -> Result<(), String> {
        self.texts.lock().unwrap().push(text);
        self.threads
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        Ok(())
    }
}

// ------------------------------------------------------------------- helpers

struct Fixture {
    state: Arc<AppState>,
    secrets: Arc<dyn SecretStore>,
    _dir: TempDir,
}

fn fixture(secrets: Arc<dyn SecretStore>) -> Fixture {
    fixture_with(secrets, vec![])
}

/// A fixture with [`FakeDriver`] plus `extra` drivers.
fn fixture_with(secrets: Arc<dyn SecretStore>, mut extra: Vec<Arc<dyn Driver>>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let config = ConfigStore::at(dir.path().join("config.json"));
    extra.push(Arc::new(FakeDriver));
    let state = AppState::new(config, secrets.clone(), extra).unwrap();
    Fixture {
        state: Arc::new(state),
        secrets,
        _dir: dir,
    }
}

fn new_conn() -> ConnectionConfig {
    let mut c = ConnectionConfig::new("Fake", "fake-host", 1433, "sa", Environment::Local);
    c.driver = "fake".into();
    c
}

/// A fixture with one connection whose password is saved in a MemoryStore.
async fn connected(tab: &str) -> (Fixture, ConnectionConfig) {
    let f = fixture(Arc::new(MemoryStore::default()));
    let saved = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    open_session(&f.state, tab.into(), saved.id.clone(), None, None)
        .await
        .unwrap();
    (f, saved)
}

type Events = Arc<Mutex<Vec<UiEvent>>>;

fn recorder() -> (impl Fn(UiEvent) + Send + Sync + 'static, Events) {
    let events: Events = Arc::default();
    let sink = events.clone();
    (move |e| sink.lock().unwrap().push(e), events)
}

fn streamer() -> (
    impl Fn(UiEvent) + Send + Sync + 'static,
    UnboundedReceiver<UiEvent>,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        move |e| {
            let _ = tx.send(e);
        },
        rx,
    )
}

async fn run(state: &AppState, tab: &str, sql: &str) -> (Result<RunSummary, String>, Vec<UiEvent>) {
    let (sink, events) = recorder();
    let result = tokio::time::timeout(TIMEOUT, run_query(state, tab.into(), sql.into(), sink))
        .await
        .expect("run_query did not finish");
    let events = events.lock().unwrap().clone();
    (result, events)
}

/// (total, truncated) of every RowCount of result `index`, in order.
fn row_counts(events: &[UiEvent], index: u32) -> Vec<(usize, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            UiEvent::RowCount {
                index: i,
                total,
                truncated,
            } if *i == index => Some((*total, *truncated)),
            _ => None,
        })
        .collect()
}

/// Waits for a RowCount of result 0 with `total` rows.
async fn wait_for_total(rx: &mut UnboundedReceiver<UiEvent>, total: usize) {
    tokio::time::timeout(TIMEOUT, async {
        while let Some(e) = rx.recv().await {
            if matches!(e, UiEvent::RowCount { index: 0, total: t, .. } if t == total) {
                return;
            }
        }
        panic!("event stream ended");
    })
    .await
    .expect("rows never arrived");
}

// --------------------------------------------------------------- connections

#[tokio::test]
async fn unavailable_keyring_saves_without_password_then_requires_it() {
    let f = fixture(Arc::new(UnavailableStore));
    let result = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap();
    assert!(result.keyring_unavailable);
    assert!(!result.connection.password_saved);
    let listed = list_connections(&f.state).await;
    assert_eq!(listed, vec![result.connection.clone()]);
    assert!(!listed[0].password_saved);

    let err = open_session(&f.state, "t1".into(), result.connection.id, None, None)
        .await
        .unwrap_err();
    assert_eq!(err, PASSWORD_REQUIRED);
}

#[tokio::test]
async fn saving_without_password_keeps_the_saved_one() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let first = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap();
    assert!(first.connection.password_saved && !first.keyring_unavailable);

    // Rename without retyping; the UI may send a stale passwordSaved.
    let mut renamed = first.connection.clone();
    renamed.name = "Renamed".into();
    renamed.password_saved = false;
    let second = save_connection(&f.state, renamed, None).await.unwrap();
    assert!(second.connection.password_saved);
    assert_eq!(second.connection.name, "Renamed");
    assert_eq!(
        f.secrets.get(&first.connection.id).unwrap().as_deref(),
        Some(PASSWORD)
    );
    let listed = list_connections(&f.state).await;
    assert_eq!(listed, vec![second.connection.clone()]);

    // It is persisted: a fresh state over the same file sees it.
    let reloaded = AppState::new(
        ConfigStore::at(f._dir.path().join("config.json")),
        f.secrets.clone(),
        vec![],
    )
    .unwrap();
    assert_eq!(list_connections(&reloaded).await, listed);

    // A new connection saved without a password has none.
    let mut fresh = new_conn();
    fresh.password_saved = true;
    let third = save_connection(&f.state, fresh, None).await.unwrap();
    assert!(!third.connection.password_saved);
}

#[tokio::test]
async fn delete_connection_removes_it_and_its_secret() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let saved = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    delete_connection(&f.state, saved.id.clone()).await.unwrap();
    assert!(list_connections(&f.state).await.is_empty());
    assert_eq!(f.secrets.get(&saved.id).unwrap(), None);
}

#[tokio::test]
async fn test_connection_uses_given_or_saved_password() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let info = test_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap();
    assert_eq!(info.version, "Fake SQL 1");
    assert_eq!(
        test_connection(&f.state, new_conn(), None)
            .await
            .unwrap_err(),
        PASSWORD_REQUIRED
    );
    let saved = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    assert!(test_connection(&f.state, saved, None).await.is_ok());

    let err = test_connection(&f.state, new_conn(), Some("wrong-pw".into()))
        .await
        .unwrap_err();
    assert!(err.starts_with("authentication failed"), "{err}");
    assert!(!err.contains("wrong-pw"));
}

// ------------------------------------------------------------------ sessions

#[tokio::test]
async fn open_session_reports_server_and_databases() {
    let (f, saved) = connected("t1").await;
    let info = open_session(&f.state, "t1".into(), saved.id.clone(), None, None)
        .await
        .unwrap();
    assert_eq!(info.server.user, "sa");
    assert_eq!(info.databases, ["a", "b", "c", "master"]);
    assert_eq!(info.current_database, "master");

    let info = open_session(&f.state, "t1".into(), saved.id, Some("c".into()), None)
        .await
        .unwrap();
    assert_eq!(info.current_database, "c");
}

#[tokio::test]
async fn selected_databases_are_filtered_and_the_first_is_used() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let mut conn = new_conn();
    conn.databases = DatabaseScope::Selected {
        names: vec!["c".into(), "b".into(), "gone".into()],
    };
    let saved = save_connection(&f.state, conn, None)
        .await
        .unwrap()
        .connection;
    let info = open_session(&f.state, "t1".into(), saved.id, None, Some(PASSWORD.into()))
        .await
        .unwrap();
    assert_eq!(info.databases, ["b", "c"]);
    assert_eq!(info.current_database, "b");
}

#[tokio::test]
async fn wrong_password_is_a_clear_error() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let saved = save_connection(&f.state, new_conn(), None)
        .await
        .unwrap()
        .connection;
    let err = open_session(
        &f.state,
        "t1".into(),
        saved.id,
        None,
        Some("nope-pw".into()),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "authentication failed: Login failed for user 'sa'.");
}

#[tokio::test]
async fn use_database_switches_and_reports_errors() {
    let (f, _) = connected("t1").await;
    use_database(&f.state, "t1".into(), "b".into())
        .await
        .unwrap();
    let err = use_database(&f.state, "t1".into(), "missing".into())
        .await
        .unwrap_err();
    assert_eq!(err, "Database 'missing' does not exist.");
    // The session is still usable.
    let (result, _) = run(&f.state, "t1", "affected 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
    assert_eq!(
        use_database(&f.state, "nope".into(), "b".into())
            .await
            .unwrap_err(),
        "no session is open on this tab"
    );
}

// ------------------------------------------------------------------- queries

#[tokio::test]
async fn run_query_counts_rows_and_fills_buffers() {
    let (f, _) = connected("t1").await;
    let (result, events) = run(&f.state, "t1", "rows 1234; affected 7").await;
    let summary = result.unwrap();
    assert_eq!(summary.outcome, RunOutcome::Ok);
    assert!(
        summary.finished_at.ends_with('Z'),
        "{}",
        summary.finished_at
    );
    chrono::DateTime::parse_from_rfc3339(&summary.finished_at).unwrap();

    assert_eq!(
        events[0],
        UiEvent::ResultStart {
            index: 0,
            columns: fake_columns()
        }
    );
    let counts = row_counts(&events, 0);
    assert_eq!(counts.last(), Some(&(1234, false)));
    assert!(counts.len() <= 3, "{counts:?}");
    assert!(counts.windows(2).all(|w| w[0].0 < w[1].0));
    // The final count comes before later events.
    let last_count = events
        .iter()
        .rposition(|e| matches!(e, UiEvent::RowCount { .. }))
        .unwrap();
    let affected = events
        .iter()
        .position(|e| *e == UiEvent::RowsAffected { count: 7 })
        .unwrap();
    assert!(last_count < affected);
    assert_eq!(events.last(), Some(&UiEvent::BatchDone { batch: 0 }));
    assert!(!events.iter().any(|e| matches!(e, UiEvent::Message { .. })));

    let rows = fetch_rows(&f.state, "t1".into(), 0, 0, 5000).await.unwrap();
    assert_eq!(rows.len(), MAX_FETCH_ROWS);
    assert_eq!(rows[999], fake_row(999));
    let rows = fetch_rows(&f.state, "t1".into(), 0, 1000, 1000)
        .await
        .unwrap();
    assert_eq!(rows.len(), 234);
    assert_eq!(rows[233], fake_row(1233));
    assert!(fetch_rows(&f.state, "t1".into(), 0, 5000, 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        fetch_rows(&f.state, "t1".into(), 1, 0, 10)
            .await
            .unwrap_err(),
        "result 1 does not exist"
    );
}

#[tokio::test]
async fn row_counts_are_throttled() {
    let (f, _) = connected("t1").await;
    let (result, events) = run(&f.state, "t1", "trickle 3000").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
    let counts = row_counts(&events, 0);
    assert_eq!(counts.last(), Some(&(3000, false)));
    // One per 500 rows at most, plus time-based ones on a slow machine.
    assert!(counts.len() < 60, "{} RowCount events", counts.len());
}

#[tokio::test]
async fn every_result_gets_a_final_count_and_runs_clear_buffers() {
    let (f, _) = connected("t1").await;
    let (_, events) = run(&f.state, "t1", "rows 5; rows 0; rows 3").await;
    assert_eq!(row_counts(&events, 0).last(), Some(&(5, false)));
    assert_eq!(row_counts(&events, 1), vec![(0, false)]);
    assert_eq!(row_counts(&events, 2).last(), Some(&(3, false)));
    assert_eq!(
        fetch_rows(&f.state, "t1".into(), 2, 0, 10)
            .await
            .unwrap()
            .len(),
        3
    );

    let (_, _) = run(&f.state, "t1", "rows 2").await;
    assert_eq!(
        fetch_rows(&f.state, "t1".into(), 0, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(fetch_rows(&f.state, "t1".into(), 2, 0, 10).await.is_err());
}

#[tokio::test]
async fn results_over_the_cap_are_truncated_and_drained() {
    let (f, _) = connected("t1").await;
    let (result, events) = run(&f.state, "t1", "rows 120000; affected 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
    let counts = row_counts(&events, 0);
    assert_eq!(counts.last(), Some(&(MAX_ROWS, true)));
    // Draining past the cap does not repeat the same count.
    assert_eq!(counts.iter().filter(|c| c.1).count(), 1);
    // The stream was read to its end.
    assert!(events.contains(&UiEvent::RowsAffected { count: 1 }));
    let tail = fetch_rows(&f.state, "t1".into(), 0, MAX_ROWS - 1, 10)
        .await
        .unwrap();
    assert_eq!(tail, vec![fake_row(MAX_ROWS - 1)]);
}

#[tokio::test]
async fn query_errors_end_with_outcome_error() {
    let (f, _) = connected("t1").await;
    let (result, events) = run(&f.state, "t1", "rows 1; error").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Error);
    assert!(events.contains(&UiEvent::Message {
        kind: MessageKind::Error,
        text: "Divide by zero error encountered.".into(),
        line: Some(1),
    }));
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn disconnect_blocks_the_tab_until_open_session() {
    let (f, saved) = connected("t1").await;
    let (result, events) = run(&f.state, "t1", "rows 3; disconnect").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Disconnected);
    assert!(events.contains(&UiEvent::Disconnected {
        reason: "connection reset by peer".into()
    }));

    let (result, events) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap_err(), DISCONNECTED);
    assert!(events.is_empty());
    assert_eq!(
        use_database(&f.state, "t1".into(), "b".into())
            .await
            .unwrap_err(),
        DISCONNECTED
    );
    // The rows received before the disconnect can still be read.
    assert_eq!(
        fetch_rows(&f.state, "t1".into(), 0, 0, 10)
            .await
            .unwrap()
            .len(),
        3
    );

    open_session(&f.state, "t1".into(), saved.id, None, None)
        .await
        .unwrap();
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn cancel_stops_a_run_that_streams_rows_meanwhile() {
    let (f, _) = connected("t1").await;
    let (sink, mut rx) = streamer();
    let state = f.state.clone();
    // Spawned: the run_query future must be Send for the Tauri layer.
    let running =
        tokio::spawn(
            async move { run_query(&state, "t1".into(), "rows 700; wait".into(), sink).await },
        );
    wait_for_total(&mut rx, 700).await;

    // Rows are readable while the query is still running...
    let rows = fetch_rows(&f.state, "t1".into(), 0, 0, 1000).await.unwrap();
    assert_eq!(rows.len(), 700);
    // ...and a second run on the same tab is refused, not queued.
    let (busy, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(busy.unwrap_err(), BUSY);

    cancel_query(&f.state, "t1".into()).await.unwrap();
    let summary = tokio::time::timeout(TIMEOUT, running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(summary.outcome, RunOutcome::Cancelled);

    // The session is back and works; cancelling while idle is a no-op.
    cancel_query(&f.state, "t1".into()).await.unwrap();
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn close_session_cancels_a_running_query() {
    let (f, _) = connected("t1").await;
    let (sink, mut rx) = streamer();
    let state = f.state.clone();
    let running =
        tokio::spawn(
            async move { run_query(&state, "t1".into(), "rows 1; wait".into(), sink).await },
        );
    wait_for_total(&mut rx, 1).await;

    close_session(&f.state, "t1".into()).await.unwrap();
    let summary = tokio::time::timeout(TIMEOUT, running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(summary.outcome, RunOutcome::Cancelled);
    assert!(fetch_rows(&f.state, "t1".into(), 0, 0, 10).await.is_err());
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap_err(), "no session is open on this tab");
    close_session(&f.state, "t1".into()).await.unwrap();
}

#[tokio::test]
async fn reopening_a_running_tab_does_not_mix_runs() {
    let (f, saved) = connected("t1").await;
    let (sink, mut rx) = streamer();
    let state = f.state.clone();
    let running =
        tokio::spawn(
            async move { run_query(&state, "t1".into(), "rows 4; wait".into(), sink).await },
        );
    wait_for_total(&mut rx, 4).await;

    open_session(&f.state, "t1".into(), saved.id, None, None)
        .await
        .unwrap();
    let summary = tokio::time::timeout(TIMEOUT, running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(summary.outcome, RunOutcome::Cancelled);
    // The new session is idle and has no results of the old run.
    assert!(fetch_rows(&f.state, "t1".into(), 0, 0, 10).await.is_err());
    let (result, _) = run(&f.state, "t1", "rows 2").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

// ---------------------------------------------------------------------- copy

#[tokio::test]
async fn copy_results_in_excel_and_ai_formats() {
    let (f, _) = connected("t1").await;
    let (result, _) = run(&f.state, "t1", "rows 4").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
    let all_rows: Vec<_> = (0..4).map(fake_row).collect();
    let clipboard = Arc::new(FakeClipboard::default());

    let out = copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Excel,
        Selection::All,
        clipboard.clone(),
    )
    .await
    .unwrap();
    assert_eq!(out, CopyOutcome { rows: 4, tokens: 0 });
    let expected = to_excel_tsv(&fake_columns(), &all_rows, system_decimal_separator());
    assert_eq!(clipboard.texts.lock().unwrap().last(), Some(&expected));
    // Formatting and the clipboard write run off the runtime thread.
    let threads = clipboard.threads.lock().unwrap().clone();
    assert!(!threads.contains(&std::thread::current().id()));

    let out = copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Ai,
        Selection::Rows { from: 1, to: 2 },
        clipboard.clone(),
    )
    .await
    .unwrap();
    let expected = to_ai(&fake_columns(), &all_rows[1..=2]);
    assert_eq!(
        out,
        CopyOutcome {
            rows: 2,
            tokens: estimate_tokens(&expected)
        }
    );
    assert!(out.tokens > 0);
    assert_eq!(clipboard.texts.lock().unwrap().last(), Some(&expected));

    let out = copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Ai,
        Selection::Cells {
            rows: (3, 3),
            cols: (0, 1),
        },
        clipboard.clone(),
    )
    .await
    .unwrap();
    assert_eq!(out.rows, 1);
    let expected = to_ai(&fake_columns()[0..2], &[all_rows[3][0..2].to_vec()]);
    assert_eq!(clipboard.texts.lock().unwrap().last(), Some(&expected));

    assert!(copy_results(
        &f.state,
        "t1".into(),
        5,
        CopyMode::Excel,
        Selection::All,
        clipboard.clone()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn large_cells_are_capped_with_one_warning_and_trimmed_for_display() {
    let (f, _) = connected("t1").await;
    let (result, events) = run(&f.state, "t1", "big 2; rows 1; big 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
    let warnings: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, UiEvent::Message { .. }))
        .collect();
    assert_eq!(
        warnings,
        vec![&UiEvent::Message {
            kind: MessageKind::Warning,
            text: CELLS_CAPPED_WARNING.into(),
            line: None,
        }]
    );

    // The grid gets the display form.
    let rows = fetch_rows(&f.state, "t1".into(), 0, 0, 10).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        vec![
            Value::Text(format!("{}…", "x".repeat(DISPLAY_TEXT_CHARS))),
            Value::Bytes(vec![0xAB; DISPLAY_BYTES + 1]),
        ]
    );
    // Small values are sent as they are.
    let rows = fetch_rows(&f.state, "t1".into(), 1, 0, 10).await.unwrap();
    assert_eq!(rows, vec![fake_row(0)]);

    // Copy uses the stored value, capped at MAX_CELL_BYTES.
    let clipboard = Arc::new(FakeClipboard::default());
    let doc = Selection::Cells {
        rows: (0, 0),
        cols: (0, 0),
    };
    copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Excel,
        doc,
        clipboard.clone(),
    )
    .await
    .unwrap();
    let doc_column = Column {
        name: "doc".into(),
        type_name: "fake".into(),
    };
    let expected = to_excel_tsv(
        &[doc_column],
        &[vec![Value::Text("x".repeat(MAX_CELL_BYTES))]],
        system_decimal_separator(),
    );
    assert_eq!(clipboard.texts.lock().unwrap().last(), Some(&expected));

    // A run without large values has no warning.
    let (_, events) = run(&f.state, "t1", "rows 3").await;
    assert!(!events.iter().any(|e| matches!(e, UiEvent::Message { .. })));
}

fn sort_by(column: usize, descending: bool) -> ViewSpec {
    ViewSpec {
        sort: Some(SortSpec { column, descending }),
        ..ViewSpec::default()
    }
}

#[tokio::test]
async fn result_view_orders_fetch_and_copy() {
    let (f, _) = connected("t1").await;
    let (result, _) = run(&f.state, "t1", "rows 4").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
    let visible = set_result_view(&f.state, "t1".into(), 0, sort_by(0, true))
        .await
        .unwrap();
    assert_eq!(visible, 4);

    let rows = fetch_rows(&f.state, "t1".into(), 0, 1, 2).await.unwrap();
    assert_eq!(rows, vec![fake_row(2), fake_row(1)]);

    let clipboard = Arc::new(FakeClipboard::default());
    let out = copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Excel,
        Selection::All,
        clipboard.clone(),
    )
    .await
    .unwrap();
    assert_eq!(out.rows, 4);
    let reversed: Vec<_> = (0..4).rev().map(fake_row).collect();
    let expected = to_excel_tsv(&fake_columns(), &reversed, system_decimal_separator());
    assert_eq!(clipboard.texts.lock().unwrap().last(), Some(&expected));

    // A filter copies only the visible rows.
    let filter = ViewSpec {
        text: Some("ROW\t2".into()),
        ..ViewSpec::default()
    };
    assert_eq!(
        set_result_view(&f.state, "t1".into(), 0, filter)
            .await
            .unwrap(),
        1
    );
    let out = copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Ai,
        Selection::All,
        clipboard.clone(),
    )
    .await
    .unwrap();
    assert_eq!(out.rows, 1);
    let expected = to_ai(&fake_columns(), &[fake_row(2)]);
    assert_eq!(clipboard.texts.lock().unwrap().last(), Some(&expected));

    // An empty spec gives back the original order.
    assert_eq!(
        set_result_view(&f.state, "t1".into(), 0, ViewSpec::default())
            .await
            .unwrap(),
        4
    );
    let rows = fetch_rows(&f.state, "t1".into(), 0, 0, 10).await.unwrap();
    assert_eq!(rows, (0..4).map(fake_row).collect::<Vec<_>>());

    // A new run starts without a view.
    set_result_view(&f.state, "t1".into(), 0, sort_by(0, true))
        .await
        .unwrap();
    run(&f.state, "t1", "rows 3").await.0.unwrap();
    let rows = fetch_rows(&f.state, "t1".into(), 0, 0, 10).await.unwrap();
    assert_eq!(rows, (0..3).map(fake_row).collect::<Vec<_>>());
}

#[tokio::test]
async fn result_view_rejects_unknown_columns_and_results() {
    let (f, _) = connected("t1").await;
    run(&f.state, "t1", "rows 2").await.0.unwrap();
    assert!(set_result_view(&f.state, "t1".into(), 0, sort_by(3, false))
        .await
        .is_err());
    let filter = ViewSpec {
        columns: vec![ColumnFilter::IsNull { column: 9 }],
        ..ViewSpec::default()
    };
    assert!(set_result_view(&f.state, "t1".into(), 0, filter)
        .await
        .is_err());
    assert!(set_result_view(&f.state, "t1".into(), 1, sort_by(0, false))
        .await
        .is_err());
    assert!(
        set_result_view(&f.state, "nope".into(), 0, sort_by(0, false))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn result_view_is_busy_while_rows_arrive() {
    let (f, _) = connected("t1").await;
    let (sink, mut rx) = streamer();
    let state = f.state.clone();
    let running =
        tokio::spawn(
            async move { run_query(&state, "t1".into(), "rows 700; wait".into(), sink).await },
        );
    wait_for_total(&mut rx, 700).await;
    assert_eq!(
        set_result_view(&f.state, "t1".into(), 0, sort_by(0, true)).await,
        Err(BUSY.to_string())
    );
    cancel_query(&f.state, "t1".into()).await.unwrap();
    tokio::time::timeout(TIMEOUT, running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        set_result_view(&f.state, "t1".into(), 0, sort_by(0, true)).await,
        Ok(700)
    );
}

#[tokio::test]
async fn copy_works_while_the_result_is_streaming() {
    let (f, _) = connected("t1").await;
    let (sink, mut rx) = streamer();
    let state = f.state.clone();
    let running =
        tokio::spawn(
            async move { run_query(&state, "t1".into(), "rows 700; wait".into(), sink).await },
        );
    wait_for_total(&mut rx, 700).await;
    let clipboard = Arc::new(FakeClipboard::default());
    let out = copy_results(
        &f.state,
        "t1".into(),
        0,
        CopyMode::Excel,
        Selection::All,
        clipboard.clone(),
    )
    .await
    .unwrap();
    assert_eq!(out.rows, 700);
    cancel_query(&f.state, "t1".into()).await.unwrap();
    let summary = tokio::time::timeout(TIMEOUT, running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(summary.outcome, RunOutcome::Cancelled);
}

// --------------------------------------------- runs that never check back in

/// After a run that did not end normally, the tab reports "disconnected"
/// (not "busy") and a new open_session recovers it.
async fn assert_disconnected_then_recovers(f: &Fixture, saved: &ConnectionConfig) {
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap_err(), DISCONNECTED);
    assert_eq!(
        use_database(&f.state, "t1".into(), "b".into())
            .await
            .unwrap_err(),
        DISCONNECTED
    );
    // Nothing left to cancel.
    cancel_query(&f.state, "t1".into()).await.unwrap();
    open_session(&f.state, "t1".into(), saved.id.clone(), None, None)
        .await
        .unwrap();
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn a_panicking_driver_leaves_the_tab_disconnected_not_busy() {
    let (f, saved) = connected("t1").await;
    let state = f.state.clone();
    let joined = tokio::spawn(async move {
        run_query(&state, "t1".into(), "rows 1; panic".into(), |_| {}).await
    })
    .await;
    assert!(joined.unwrap_err().is_panic());
    assert_disconnected_then_recovers(&f, &saved).await;
}

#[tokio::test]
async fn a_panicking_event_sink_leaves_the_tab_disconnected_not_busy() {
    let (f, saved) = connected("t1").await;
    let state = f.state.clone();
    let joined = tokio::spawn(async move {
        run_query(&state, "t1".into(), "rows 1".into(), |_| {
            panic!("sink panic")
        })
        .await
    })
    .await;
    assert!(joined.unwrap_err().is_panic());
    assert_disconnected_then_recovers(&f, &saved).await;
}

#[tokio::test]
async fn a_dropped_run_leaves_the_tab_disconnected_not_busy() {
    let (f, saved) = connected("t1").await;
    let dropped = tokio::time::timeout(
        Duration::from_millis(100),
        run_query(&f.state, "t1".into(), "rows 1; wait".into(), |_| {}),
    )
    .await;
    assert!(dropped.is_err(), "the run should still be waiting");
    assert_disconnected_then_recovers(&f, &saved).await;
}

// ------------------------------------------------- close during open_session

async fn gated_fixture() -> (Fixture, Arc<GatedDriver>, ConnectionConfig) {
    let gated = Arc::new(GatedDriver::default());
    let f = fixture_with(Arc::new(MemoryStore::default()), vec![gated.clone()]);
    let mut conn = new_conn();
    conn.driver = "gated".into();
    let saved = save_connection(&f.state, conn, Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    (f, gated, saved)
}

#[tokio::test]
async fn closing_a_tab_while_it_connects_closes_the_new_session() {
    let (f, gated, saved) = gated_fixture().await;
    let state = f.state.clone();
    let id = saved.id.clone();
    let opening =
        tokio::spawn(async move { open_session(&state, "t1".into(), id, None, None).await });
    tokio::time::timeout(TIMEOUT, gated.entered.notified())
        .await
        .unwrap();

    close_session(&f.state, "t1".into()).await.unwrap();
    gated.release.notify_one();
    let err = tokio::time::timeout(TIMEOUT, opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(err, CLOSED);
    assert!(gated.dropped.load(Ordering::SeqCst), "session not closed");
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap_err(), "no session is open on this tab");

    // Opening the tab again later works.
    gated.release.notify_one();
    open_session(&f.state, "t1".into(), saved.id, None, None)
        .await
        .unwrap();
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn a_slow_open_does_not_replace_a_newer_one() {
    let (f, gated, saved) = gated_fixture().await;
    let state = f.state.clone();
    let id = saved.id.clone();
    let slow = tokio::spawn(async move { open_session(&state, "t1".into(), id, None, None).await });
    tokio::time::timeout(TIMEOUT, gated.entered.notified())
        .await
        .unwrap();

    // A newer open_session on the same tab, with an instant driver.
    let fast = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    open_session(&f.state, "t1".into(), fast.id, None, None)
        .await
        .unwrap();
    gated.release.notify_one();
    let err = tokio::time::timeout(TIMEOUT, slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(err, CLOSED);
    assert!(gated.dropped.load(Ordering::SeqCst));
    // The newer session is the one in the tab.
    let (result, _) = run(&f.state, "t1", "rows 1").await;
    assert_eq!(result.unwrap().outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn test_connection_uses_the_stored_password_flag() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let saved = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    // The UI may send a stale passwordSaved = false.
    let mut stale = saved.clone();
    stale.password_saved = false;
    let info = test_connection(&f.state, stale, None).await.unwrap();
    assert_eq!(info.version, "Fake SQL 1");

    // A connection that claims a saved password it does not have.
    let mut claims = new_conn();
    claims.password_saved = true;
    assert_eq!(
        test_connection(&f.state, claims, None).await.unwrap_err(),
        PASSWORD_REQUIRED
    );
}

#[tokio::test]
async fn unreadable_config_opens_read_only_and_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let original = "{ not json";
    std::fs::write(&path, original).unwrap();

    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let state = AppState::open(
        ConfigStore::at(&path),
        secrets.clone(),
        vec![Arc::new(FakeDriver)],
    );
    assert!(state.startup_error().unwrap().contains("not valid"));
    assert!(list_connections(&state).await.is_empty());

    let conn = new_conn();
    let err = save_connection(&state, conn.clone(), Some(PASSWORD.into()))
        .await
        .unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    let err = delete_connection(&state, conn.id.clone())
        .await
        .unwrap_err();
    assert!(err.contains("read-only"), "{err}");

    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    // Nothing reached the keychain either.
    assert!(secrets.get(&conn.id).unwrap().is_none());

    // A healthy file has no startup error.
    let ok = fixture(Arc::new(MemoryStore::default()));
    assert!(ok.state.startup_error().is_none());
}

// ------------------------------------------------------------------- folders

#[tokio::test]
async fn folders_nest_and_deleting_moves_children_to_the_parent() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let a = create_folder(&f.state, "Cliente A".into(), None)
        .await
        .unwrap();
    let x = create_folder(&f.state, "Proyecto X".into(), Some(a.id.clone()))
        .await
        .unwrap();
    assert_eq!(x.parent_id.as_deref(), Some(a.id.as_str()));
    let conn = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    move_connection(&f.state, conn.id.clone(), Some(a.id.clone()))
        .await
        .unwrap();

    delete_folder(&f.state, a.id.clone()).await.unwrap();
    let folders = list_folders(&f.state).await;
    assert_eq!(folders.len(), 1);
    assert_eq!(
        (folders[0].id.as_str(), folders[0].parent_id.clone()),
        (x.id.as_str(), None)
    );
    let conns = list_connections(&f.state).await;
    assert_eq!(conns.len(), 1);
    assert_eq!(conns[0].folder_id, None);
    assert!(conns[0].password_saved);

    // Persisted: a fresh state over the same file sees it.
    let reopened = AppState::new(
        ConfigStore::at(f._dir.path().join("config.json")),
        f.secrets.clone(),
        vec![],
    )
    .unwrap();
    assert_eq!(list_folders(&reopened).await, folders);
}

#[tokio::test]
async fn folders_rename_and_validate_names() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let a = create_folder(&f.state, "  A ".into(), None).await.unwrap();
    assert_eq!(a.name, "A");
    rename_folder(&f.state, a.id.clone(), "B".into())
        .await
        .unwrap();
    assert_eq!(list_folders(&f.state).await[0].name, "B");
    assert_eq!(
        rename_folder(&f.state, a.id.clone(), "  ".into())
            .await
            .unwrap_err(),
        "empty_name"
    );
    assert_eq!(
        create_folder(&f.state, "".into(), None).await.unwrap_err(),
        "empty_name"
    );
    assert_eq!(
        create_folder(&f.state, "C".into(), Some("nope".into()))
            .await
            .unwrap_err(),
        "not_found"
    );
    assert_eq!(
        rename_folder(&f.state, "nope".into(), "Z".into())
            .await
            .unwrap_err(),
        "not_found"
    );
    assert_eq!(
        delete_folder(&f.state, "nope".into()).await.unwrap_err(),
        "not_found"
    );
}

#[tokio::test]
async fn folders_move_rejects_cycles() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let a = create_folder(&f.state, "A".into(), None).await.unwrap();
    let b = create_folder(&f.state, "B".into(), Some(a.id.clone()))
        .await
        .unwrap();
    let c = create_folder(&f.state, "C".into(), Some(b.id.clone()))
        .await
        .unwrap();
    for target in [&a, &b, &c] {
        assert_eq!(
            move_folder(&f.state, a.id.clone(), Some(target.id.clone()))
                .await
                .unwrap_err(),
            "cycle"
        );
    }
    assert_eq!(
        move_folder(&f.state, "nope".into(), None)
            .await
            .unwrap_err(),
        "not_found"
    );
    assert_eq!(
        move_folder(&f.state, a.id.clone(), Some("nope".into()))
            .await
            .unwrap_err(),
        "not_found"
    );
    move_folder(&f.state, c.id.clone(), None).await.unwrap();
    move_folder(&f.state, a.id.clone(), Some(c.id.clone()))
        .await
        .unwrap();
    let folders = list_folders(&f.state).await;
    let parent = |id: &str| {
        folders
            .iter()
            .find(|x| x.id == id)
            .unwrap()
            .parent_id
            .clone()
    };
    assert_eq!(parent(&c.id), None);
    assert_eq!(parent(&a.id), Some(c.id.clone()));
}

#[tokio::test]
async fn folders_move_connection_and_expanded_persist() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let a = create_folder(&f.state, "A".into(), None).await.unwrap();
    let conn = save_connection(&f.state, new_conn(), None)
        .await
        .unwrap()
        .connection;
    move_connection(&f.state, conn.id.clone(), Some(a.id.clone()))
        .await
        .unwrap();
    assert_eq!(
        move_connection(&f.state, "nope".into(), None)
            .await
            .unwrap_err(),
        "not_found"
    );
    assert_eq!(
        move_connection(&f.state, conn.id.clone(), Some("nope".into()))
            .await
            .unwrap_err(),
        "not_found"
    );
    set_expanded(&f.state, vec![a.id.clone(), conn.id.clone(), a.id.clone()])
        .await
        .unwrap();

    let reopened = AppState::new(
        ConfigStore::at(f._dir.path().join("config.json")),
        f.secrets.clone(),
        vec![],
    )
    .unwrap();
    assert_eq!(
        list_connections(&reopened).await[0].folder_id.as_deref(),
        Some(a.id.as_str())
    );
    assert_eq!(
        ConfigStore::at(f._dir.path().join("config.json"))
            .load()
            .unwrap()
            .expanded,
        vec![a.id.clone(), conn.id.clone()]
    );
    assert_eq!(
        list_expanded(&reopened).await,
        vec![a.id.clone(), conn.id.clone()]
    );
}

#[tokio::test]
async fn set_zoom_clamps_and_persists() {
    let f = fixture(Arc::new(MemoryStore::default()));
    assert_eq!(get_preferences(&f.state).await.zoom, 100);
    assert_eq!(set_zoom(&f.state, 500).await.unwrap(), 200);
    assert_eq!(set_zoom(&f.state, 20).await.unwrap(), 70);
    assert_eq!(set_zoom(&f.state, 130).await.unwrap(), 130);
    assert_eq!(get_preferences(&f.state).await.zoom, 130);
    assert_eq!(
        ConfigStore::at(f._dir.path().join("config.json"))
            .load()
            .unwrap()
            .preferences
            .zoom,
        130
    );
}

#[tokio::test]
async fn folders_read_only_mode_rejects_everything() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let original = "{ not json";
    std::fs::write(&path, original).unwrap();
    let state = AppState::open(
        ConfigStore::at(&path),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(FakeDriver)],
    );
    let ro = |r: Result<(), String>| assert!(r.unwrap_err().contains("read-only"));
    assert!(create_folder(&state, "A".into(), None)
        .await
        .unwrap_err()
        .contains("read-only"));
    ro(rename_folder(&state, "x".into(), "y".into()).await);
    ro(delete_folder(&state, "x".into()).await);
    ro(move_folder(&state, "x".into(), None).await);
    ro(move_connection(&state, "x".into(), None).await);
    ro(set_expanded(&state, vec!["x".into()]).await);
    assert!(set_zoom(&state, 150)
        .await
        .unwrap_err()
        .contains("read-only"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[tokio::test]
async fn folders_list_server_databases_connects_and_lists() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let saved = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    // Explicit password.
    let names = list_server_databases(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap();
    assert_eq!(names, ["a", "b", "c", "master"]);
    // Saved password.
    assert_eq!(
        list_server_databases(&f.state, saved, None)
            .await
            .unwrap()
            .len(),
        4
    );
    // Nothing saved and nothing given.
    assert_eq!(
        list_server_databases(&f.state, new_conn(), None)
            .await
            .unwrap_err(),
        PASSWORD_REQUIRED
    );
    assert!(
        list_server_databases(&f.state, new_conn(), Some("bad".into()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn folders_move_survives_a_corrupt_parent_loop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"version":1,"connections":[],"folders":[
            {"id":"a","name":"A","parentId":null},
            {"id":"x","name":"X","parentId":"y"},
            {"id":"y","name":"Y","parentId":"x"}]}"#,
    )
    .unwrap();
    let state = AppState::new(
        ConfigStore::at(&path),
        Arc::new(MemoryStore::default()),
        vec![],
    )
    .unwrap();
    let r = tokio::time::timeout(TIMEOUT, move_folder(&state, "a".into(), Some("x".into())))
        .await
        .expect("move_folder hung");
    assert_eq!(r.unwrap_err(), "cycle");
}

#[tokio::test]
async fn folders_editing_a_connection_keeps_its_folder() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let a = create_folder(&f.state, "A".into(), None).await.unwrap();
    let mut conn = new_conn();
    conn.folder_id = Some(a.id.clone());
    let saved = save_connection(&f.state, conn, None)
        .await
        .unwrap()
        .connection;
    assert_eq!(saved.folder_id.as_deref(), Some(a.id.as_str()));

    let mut edit = saved.clone();
    edit.name = "Renamed".into();
    edit.folder_id = None; // a UI that does not send folderId
    let out = save_connection(&f.state, edit, None)
        .await
        .unwrap()
        .connection;
    assert_eq!(out.folder_id.as_deref(), Some(a.id.as_str()));
    let listed = list_connections(&f.state).await;
    assert_eq!(listed[0].name, "Renamed");
    assert_eq!(listed[0].folder_id.as_deref(), Some(a.id.as_str()));
}

#[tokio::test]
async fn folders_new_connection_with_unknown_folder_goes_to_root() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let mut conn = new_conn();
    conn.folder_id = Some("ghost".into());
    let saved = save_connection(&f.state, conn, None)
        .await
        .unwrap()
        .connection;
    assert_eq!(saved.folder_id, None);
    assert_eq!(list_connections(&f.state).await[0].folder_id, None);
}

// ------------------------------------------------- duplicate and copy (D7)

#[tokio::test]
async fn duplicate_copies_the_connection_and_its_password() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let folder = create_folder(&f.state, "Cliente A".into(), None)
        .await
        .unwrap();
    let mut conn = new_conn();
    conn.name = "Prod".into();
    conn.folder_id = Some(folder.id.clone());
    let original = save_connection(&f.state, conn, Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;

    let copy = duplicate_connection(&f.state, original.id.clone())
        .await
        .unwrap();
    assert_ne!(copy.id, original.id);
    assert_eq!(copy.name, "Prod (copy)");
    assert_eq!(copy.folder_id.as_deref(), Some(folder.id.as_str()));
    assert!(copy.password_saved);
    assert_eq!(
        (copy.server.as_str(), copy.user.as_str()),
        (original.server.as_str(), original.user.as_str())
    );
    assert_eq!(f.secrets.get(&copy.id).unwrap().as_deref(), Some(PASSWORD));
    assert_eq!(list_connections(&f.state).await.len(), 2);

    // The copy has its own keychain entry: deleting it keeps the original's.
    delete_connection(&f.state, copy.id.clone()).await.unwrap();
    assert_eq!(f.secrets.get(&copy.id).unwrap(), None);
    assert_eq!(
        f.secrets.get(&original.id).unwrap().as_deref(),
        Some(PASSWORD)
    );
    // And the original still connects with it.
    test_connection(&f.state, original, None).await.unwrap();
}

#[tokio::test]
async fn duplicate_without_saved_password_and_unknown_id() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let original = save_connection(&f.state, new_conn(), None)
        .await
        .unwrap()
        .connection;
    let copy = duplicate_connection(&f.state, original.id.clone())
        .await
        .unwrap();
    assert!(!copy.password_saved);
    assert_eq!(f.secrets.get(&copy.id).unwrap(), None);
    assert_eq!(
        duplicate_connection(&f.state, "nope".into())
            .await
            .unwrap_err(),
        NOT_FOUND
    );
}

#[tokio::test]
async fn duplicate_is_rejected_in_read_only_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let original = "{ not json";
    std::fs::write(&path, original).unwrap();
    let state = AppState::open(
        ConfigStore::at(&path),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(FakeDriver)],
    );
    let err = duplicate_connection(&state, "x".into()).await.unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[tokio::test]
async fn copy_connection_string_with_and_without_the_password() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let mut conn = new_conn();
    conn.databases = DatabaseScope::Selected {
        names: vec!["Ventas".into()],
    };
    let saved = save_connection(&f.state, conn, Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    let clipboard = Arc::new(FakeClipboard::default());

    let included = copy_connection_string(&f.state, clipboard.clone(), saved.id.clone(), false)
        .await
        .unwrap();
    assert!(!included);
    let text = clipboard.texts.lock().unwrap().last().unwrap().clone();
    assert!(
        text.starts_with("Server=fake-host,1433;Database=Ventas;User ID=sa;"),
        "{text}"
    );
    assert!(
        !text.contains("Password") && !text.contains(PASSWORD),
        "{text}"
    );

    let included = copy_connection_string(&f.state, clipboard.clone(), saved.id.clone(), true)
        .await
        .unwrap();
    assert!(included);
    let text = clipboard.texts.lock().unwrap().last().unwrap().clone();
    assert!(text.contains(&format!("Password={PASSWORD}")), "{text}");
    let parsed = tulka_core::parse_connection_string(&text).unwrap();
    assert_eq!(parsed.password.as_deref(), Some(PASSWORD));

    assert_eq!(
        copy_connection_string(&f.state, clipboard, "nope".into(), false)
            .await
            .unwrap_err(),
        NOT_FOUND
    );
}

#[tokio::test]
async fn copy_with_password_but_none_saved_copies_without_it() {
    let f = fixture(Arc::new(MemoryStore::default()));
    let saved = save_connection(&f.state, new_conn(), None)
        .await
        .unwrap()
        .connection;
    let clipboard = Arc::new(FakeClipboard::default());
    let included = copy_connection_string(&f.state, clipboard.clone(), saved.id.clone(), true)
        .await
        .unwrap();
    assert!(!included);
    let text = clipboard.texts.lock().unwrap().last().unwrap().clone();
    assert!(!text.contains("Password"), "{text}");
}

/// A store that records reads, to check the keychain is touched only when
/// the password is wanted.
#[derive(Default)]
struct CountingStore {
    inner: MemoryStore,
    reads: std::sync::atomic::AtomicUsize,
}

impl SecretStore for CountingStore {
    fn set(&self, id: &str, secret: &str) -> Result<(), tulka_core::SecretError> {
        self.inner.set(id, secret)
    }
    fn get(&self, id: &str) -> Result<Option<String>, tulka_core::SecretError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(id)
    }
    fn delete(&self, id: &str) -> Result<(), tulka_core::SecretError> {
        self.inner.delete(id)
    }
}

#[tokio::test]
async fn copy_without_password_never_reads_the_keychain() {
    let store = Arc::new(CountingStore::default());
    let f = fixture(store.clone());
    let saved = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;
    let clipboard = Arc::new(FakeClipboard::default());
    copy_connection_string(&f.state, clipboard.clone(), saved.id.clone(), false)
        .await
        .unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    copy_connection_string(&f.state, clipboard, saved.id, true)
        .await
        .unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn parse_connection_string_reports_codes() {
    let p = parse_connection_string("Server=h,1500;UID=sa;PWD=x".into()).unwrap();
    assert_eq!((p.server.as_str(), p.port), ("h", Some(1500)));
    assert_eq!(
        parse_connection_string("hola mundo".into()).unwrap_err(),
        "unrecognized"
    );
    assert_eq!(
        parse_connection_string("Server=h;Encrypt=x".into()).unwrap_err(),
        "bad_value:Encrypt"
    );
}

/// A memory store whose reads or writes can be made to fail.
#[derive(Default)]
struct FlakyStore {
    inner: MemoryStore,
    fail_get: Mutex<Option<tulka_core::SecretError>>,
    fail_set: Mutex<Option<tulka_core::SecretError>>,
}

impl SecretStore for FlakyStore {
    fn set(&self, id: &str, secret: &str) -> Result<(), tulka_core::SecretError> {
        match self.fail_set.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => self.inner.set(id, secret),
        }
    }
    fn get(&self, id: &str) -> Result<Option<String>, tulka_core::SecretError> {
        match self.fail_get.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => self.inner.get(id),
        }
    }
    fn delete(&self, id: &str) -> Result<(), tulka_core::SecretError> {
        self.inner.delete(id)
    }
}

#[tokio::test]
async fn duplicate_fails_on_keychain_errors_but_not_on_an_unavailable_keychain() {
    use tulka_core::SecretError;
    let store = Arc::new(FlakyStore::default());
    let f = fixture(store.clone());
    let original = save_connection(&f.state, new_conn(), Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection;

    *store.fail_get.lock().unwrap() = Some(SecretError::Other("boom".into()));
    let err = duplicate_connection(&f.state, original.id.clone())
        .await
        .unwrap_err();
    assert!(err.contains("could not read the saved password"), "{err}");
    assert_eq!(list_connections(&f.state).await.len(), 1);

    *store.fail_get.lock().unwrap() = None;
    *store.fail_set.lock().unwrap() = Some(SecretError::Other("boom".into()));
    let err = duplicate_connection(&f.state, original.id.clone())
        .await
        .unwrap_err();
    assert!(err.contains("could not store the password"), "{err}");
    assert_eq!(list_connections(&f.state).await.len(), 1);

    // An unavailable keychain is soft: the copy is saved without a password.
    *store.fail_set.lock().unwrap() = Some(SecretError::Unavailable("locked".into()));
    let copy = duplicate_connection(&f.state, original.id.clone())
        .await
        .unwrap();
    assert!(!copy.password_saved);
    *store.fail_set.lock().unwrap() = None;
    *store.fail_get.lock().unwrap() = Some(SecretError::Unavailable("locked".into()));
    let copy = duplicate_connection(&f.state, original.id.clone())
        .await
        .unwrap();
    assert!(!copy.password_saved);
    assert_eq!(list_connections(&f.state).await.len(), 3);
}

#[tokio::test]
async fn closed_tab_generations_stay_bounded() {
    let f = connected("t0").await.0;
    for i in 0..300 {
        close_session(&f.state, format!("gone-{i}")).await.unwrap();
    }
    assert_eq!(f.state.closed_tab_count(), 256);
}

#[tokio::test]
async fn session_saves_even_when_the_config_is_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "{ not json").unwrap();
    let store = Arc::new(tulka_store::SessionStore::in_memory());
    let state = AppState::open(
        ConfigStore::at(&path),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(FakeDriver)],
    )
    .with_session(store);
    assert!(state.startup_error().is_some());

    assert_eq!(load_session(&state).await, None);
    save_session(&state, r#"{"tabs":[]}"#.into()).await.unwrap();
    assert_eq!(
        load_session(&state).await.as_deref(),
        Some(r#"{"tabs":[]}"#)
    );
}
