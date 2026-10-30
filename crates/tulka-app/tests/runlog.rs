//! Run log: every run is recorded by `run_query`, against a fake driver.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;
use tulka_app::tulka_store::{
    MetaCache, NewRun, RunLogOutcome, RunLogStore, RunSource, SharedRunLog,
};
use tulka_app::*;
use tulka_core::guard::GuardFinding;
use tulka_core::{
    CancelHandle, Column, ConfigStore, ConnectParams, ConnectionConfig, DatabaseSnapshot, Driver,
    DriverError, Environment, ExecEvent, GuardLevel, MemoryStore, ServerInfo, Session,
    TableStructure, Value,
};

/// Behaves according to the script text: `rows3`, `affected`, `error`,
/// `cancel`, `disconnect`; anything else just finishes.
struct FakeDriver;

#[async_trait]
impl Driver for FakeDriver {
    fn id(&self) -> &'static str {
        "fake"
    }
    async fn connect(&self, _p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        Ok(Box::new(FakeSession))
    }
    fn guard_findings(&self, sql: &str) -> Vec<GuardFinding> {
        tulka_mssql::guard::findings(sql)
    }
}

struct FakeSession;
struct NoCancel;

#[async_trait]
impl CancelHandle for NoCancel {
    async fn cancel(&self) -> Result<(), DriverError> {
        Ok(())
    }
}

#[async_trait]
impl Session for FakeSession {
    async fn server_info(&mut self) -> Result<ServerInfo, DriverError> {
        Ok(ServerInfo {
            version: "Fake".into(),
            user: "sa".into(),
            single_database: false,
        })
    }
    async fn list_databases(&mut self) -> Result<Vec<String>, DriverError> {
        Ok(vec!["master".into()])
    }
    async fn use_database(&mut self, _: &str) -> Result<(), DriverError> {
        Ok(())
    }
    async fn current_database(&mut self) -> Result<String, DriverError> {
        Ok("master".into())
    }
    async fn snapshot(&mut self, _: &str) -> Result<DatabaseSnapshot, DriverError> {
        unimplemented!()
    }
    async fn structure(
        &mut self,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<TableStructure, DriverError> {
        unimplemented!()
    }
    async fn definition(&mut self, _: &str, _: &str, _: &str) -> Result<String, DriverError> {
        unimplemented!()
    }
    async fn execute(&mut self, sql: &str, tx: Sender<ExecEvent>) -> Result<(), DriverError> {
        match sql {
            "rows3" => {
                let _ = tx
                    .send(ExecEvent::ResultStart {
                        index: 0,
                        columns: vec![Column {
                            name: "a".into(),
                            type_name: "int".into(),
                        }],
                    })
                    .await;
                let rows = (0..3).map(|i| vec![Value::Int(i)]).collect();
                let _ = tx.send(ExecEvent::Rows { index: 0, rows }).await;
            }
            "affected" => {
                let _ = tx.send(ExecEvent::RowsAffected { count: 4 }).await;
                let _ = tx.send(ExecEvent::RowsAffected { count: 3 }).await;
            }
            "error" => {
                return Err(DriverError::Query {
                    message: "boom".into(),
                    line: None,
                })
            }
            "cancel" => return Err(DriverError::Cancelled),
            "disconnect" => return Err(DriverError::Disconnected("gone".into())),
            _ => {}
        }
        Ok(())
    }
    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(NoCancel)
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AppEvent>>);

impl AppEvents for Recorder {
    fn emit(&self, ev: AppEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

struct Fixture {
    state: AppState,
    events: Arc<Recorder>,
    conn: ConnectionConfig,
    _dir: tempfile::TempDir,
}

async fn fixture(level: GuardLevel) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Recorder::default());
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(FakeDriver)],
    )
    .unwrap()
    .with_metadata(Arc::new(MetaCache::in_memory()), events.clone());
    let mut conn = ConnectionConfig::new("Prod", "host", 1433, "sa", Environment::Local);
    conn.driver = "fake".into();
    conn.guard = level;
    conn.color = "#123456".into();
    let conn = save_connection(&state, conn, Some("S3cret!".into()))
        .await
        .unwrap()
        .connection;
    open_session(&state, "t1".into(), conn.id.clone(), None, None)
        .await
        .unwrap();
    Fixture {
        state,
        events,
        conn,
        _dir: dir,
    }
}

async fn run(f: &Fixture, sql: &str) -> RunSummary {
    run_query(&f.state, "t1".into(), sql.into(), false, |_| {})
        .await
        .unwrap()
}

#[tokio::test]
async fn ok_run_is_logged_with_rows_and_snapshot() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "rows3").await;
    let log = list_run_log(&f.state, None, 10).await;
    assert_eq!(log.len(), 1);
    let e = &log[0];
    assert_eq!(e.sql, "rows3");
    assert_eq!(e.outcome, RunLogOutcome::Ok);
    assert_eq!(e.rows, Some(3));
    assert!(e.elapsed_ms.is_some());
    assert_eq!(e.connection_id, f.conn.id);
    assert_eq!(e.connection_name, "Prod");
    assert_eq!(e.color, "#123456");
    assert_eq!(e.database, "master");
}

#[tokio::test]
async fn rows_affected_counts_when_no_result_set() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "affected").await;
    run(&f, "nothing").await;
    let log = list_run_log(&f.state, None, 10).await;
    assert_eq!(log[1].rows, Some(7));
    assert_eq!(log[0].rows, None);
}

#[tokio::test]
async fn blocked_run_is_logged_without_time() {
    let f = fixture(GuardLevel::Block).await;
    let s = run(&f, "DELETE FROM t").await;
    assert_eq!(s.outcome, RunOutcome::Blocked);
    let log = list_run_log(&f.state, None, 10).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].outcome, RunLogOutcome::Blocked);
    assert_eq!(log[0].elapsed_ms, None);
    assert_eq!(log[0].rows, None);
}

#[tokio::test]
async fn needs_confirmation_is_not_logged() {
    let f = fixture(GuardLevel::Confirm).await;
    let s = run(&f, "DELETE FROM t").await;
    assert_eq!(s.outcome, RunOutcome::NeedsConfirmation);
    assert!(list_run_log(&f.state, None, 10).await.is_empty());
}

#[tokio::test]
async fn log_cancelled_run_writes_cancelled() {
    let f = fixture(GuardLevel::Confirm).await;
    log_cancelled_run(&f.state, "t1".into(), "DELETE FROM t".into())
        .await
        .unwrap();
    let log = list_run_log(&f.state, None, 10).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].outcome, RunLogOutcome::Cancelled);
    assert_eq!(log[0].elapsed_ms, None);
    assert_eq!(log[0].sql, "DELETE FROM t");
    assert_eq!(log[0].connection_name, "Prod");
}

#[tokio::test]
async fn error_and_disconnected_runs_are_logged() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "error").await;
    run(&f, "cancel").await;
    run(&f, "disconnect").await;
    let log = list_run_log(&f.state, None, 10).await;
    let outcomes: Vec<_> = log.iter().rev().map(|e| e.outcome).collect();
    assert_eq!(
        outcomes,
        [
            RunLogOutcome::Error,
            RunLogOutcome::Cancelled,
            RunLogOutcome::Disconnected
        ]
    );
}

#[tokio::test]
async fn run_logged_event_is_emitted() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "rows3").await;
    let log = list_run_log(&f.state, None, 10).await;
    let events = f.events.0.lock().unwrap();
    let logged: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AppEvent::RunLogged { entry } => Some(entry.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(logged, log);
}

#[tokio::test]
async fn log_never_contains_the_password() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "rows3").await;
    run(&f, "error").await;
    let log = list_run_log(&f.state, None, 10).await;
    let mut text = serde_json::to_string(&log).unwrap();
    text += &serde_json::to_string(&*f.events.0.lock().unwrap()).unwrap();
    assert!(!text.contains("S3cret!"));
}

#[tokio::test]
async fn clear_run_log_empties_it() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "rows3").await;
    clear_run_log(&f.state).await.unwrap();
    assert!(list_run_log(&f.state, None, 10).await.is_empty());
}

#[tokio::test]
async fn list_run_log_caps_the_limit() {
    let f = fixture(GuardLevel::Off).await;
    run(&f, "rows3").await;
    assert_eq!(list_run_log(&f.state, None, u32::MAX).await.len(), 1);
}

fn new_run(sql: &str, source: RunSource) -> NewRun {
    NewRun {
        at_ms: 1,
        connection_id: "c1".into(),
        connection_name: "Prod".into(),
        color: "#123456".into(),
        database: "master".into(),
        sql: sql.into(),
        outcome: RunLogOutcome::Ok,
        rows: Some(1),
        elapsed_ms: Some(1),
        source,
        client: (source == RunSource::Ai).then(|| "claude".to_string()),
    }
}

/// A state over a run log file, plus a second handle on the same file like
/// the MCP process has.
fn shared_fixture() -> (AppState, SharedRunLog, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runlog.db");
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(FakeDriver)],
    )
    .unwrap()
    .with_runlog(Arc::new(RunLogStore::open(&path)));
    (state, SharedRunLog::open(&path), dir)
}

#[tokio::test]
async fn poll_ai_runs_skips_history_then_returns_new_ai_entries() {
    let (state, shared, _dir) = shared_fixture();
    shared.insert(&new_run("old", RunSource::Ai)).unwrap();
    assert!(poll_ai_runs(&state).await.is_empty());
    assert!(poll_ai_runs(&state).await.is_empty());
    shared.insert(&new_run("a", RunSource::Ai)).unwrap();
    shared.insert(&new_run("b", RunSource::Ai)).unwrap();
    let got = poll_ai_runs(&state).await;
    assert_eq!(
        got.iter().map(|e| e.sql.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(got[0].client.as_deref(), Some("claude"));
    assert!(poll_ai_runs(&state).await.is_empty());
}

#[tokio::test]
async fn poll_ai_runs_ignores_app_entries() {
    let (state, shared, _dir) = shared_fixture();
    assert!(poll_ai_runs(&state).await.is_empty());
    shared.insert(&new_run("mine", RunSource::App)).unwrap();
    shared.insert(&new_run("theirs", RunSource::Ai)).unwrap();
    shared.insert(&new_run("mine2", RunSource::App)).unwrap();
    let got = poll_ai_runs(&state).await;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].sql, "theirs");
    assert!(poll_ai_runs(&state).await.is_empty());
}
