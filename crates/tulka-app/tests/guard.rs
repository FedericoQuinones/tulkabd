//! Guard enforcement in `run_query`, against a recording fake driver.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;
use tulka_app::*;
use tulka_core::guard::{GuardFinding, GuardKind};
use tulka_core::{
    CancelHandle, ConfigStore, ConnectParams, ConnectionConfig, DatabaseSnapshot, Driver,
    DriverError, Environment, ExecEvent, GuardLevel, MemoryStore, ServerInfo, Session,
    TableStructure,
};

type Executed = Arc<Mutex<Vec<String>>>;

/// Records every script that reaches the server and analyzes with the real
/// T-SQL analyzer.
struct RecordingDriver(Executed);

#[async_trait]
impl Driver for RecordingDriver {
    fn id(&self) -> &'static str {
        "fake"
    }

    async fn connect(&self, _p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        Ok(Box::new(RecordingSession(self.0.clone())))
    }

    fn guard_findings(&self, sql: &str) -> Vec<GuardFinding> {
        tulka_mssql::guard::findings(sql)
    }
}

struct RecordingSession(Executed);

struct NoCancel;

#[async_trait]
impl CancelHandle for NoCancel {
    async fn cancel(&self) -> Result<(), DriverError> {
        Ok(())
    }
}

#[async_trait]
impl Session for RecordingSession {
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
    async fn use_database(&mut self, _name: &str) -> Result<(), DriverError> {
        Ok(())
    }
    async fn current_database(&mut self) -> Result<String, DriverError> {
        Ok("master".into())
    }
    async fn snapshot(&mut self, _database: &str) -> Result<DatabaseSnapshot, DriverError> {
        unimplemented!()
    }
    async fn structure(
        &mut self,
        _database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<TableStructure, DriverError> {
        unimplemented!()
    }
    async fn definition(
        &mut self,
        _database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<String, DriverError> {
        unimplemented!()
    }
    async fn execute(&mut self, sql: &str, tx: Sender<ExecEvent>) -> Result<(), DriverError> {
        self.0.lock().unwrap().push(sql.to_string());
        let _ = tx.send(ExecEvent::BatchDone { batch: 0 }).await;
        Ok(())
    }
    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(NoCancel)
    }
}

struct Fixture {
    state: AppState,
    executed: Executed,
    conn_id: String,
    _dir: tempfile::TempDir,
}

/// A connection at `level` with an open session on tab `t1`.
async fn fixture(level: GuardLevel) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let executed = Executed::default();
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(RecordingDriver(executed.clone()))],
    )
    .unwrap();
    let mut conn = ConnectionConfig::new("Fake", "host", 1433, "sa", Environment::Local);
    conn.driver = "fake".into();
    conn.guard = level;
    let conn = save_connection(&state, conn, Some("pw".into()))
        .await
        .unwrap()
        .connection;
    open_session(&state, "t1".into(), conn.id.clone(), None, None)
        .await
        .unwrap();
    Fixture {
        state,
        executed,
        conn_id: conn.id,
        _dir: dir,
    }
}

async fn run(f: &Fixture, sql: &str, confirmed: bool) -> RunSummary {
    run_query(&f.state, "t1".into(), sql.into(), confirmed, |_| {})
        .await
        .unwrap()
}

#[tokio::test]
async fn off_runs_dangerous_sql() {
    let f = fixture(GuardLevel::Off).await;
    let s = run(&f, "DELETE FROM t", false).await;
    assert_eq!(s.outcome, RunOutcome::Ok);
    assert!(s.findings.is_empty());
    assert_eq!(*f.executed.lock().unwrap(), ["DELETE FROM t"]);
}

#[tokio::test]
async fn block_never_reaches_the_driver() {
    let f = fixture(GuardLevel::Block).await;
    // Even a confirmed run is refused.
    for confirmed in [false, true] {
        let s = run(&f, "DELETE FROM t", confirmed).await;
        assert_eq!(s.outcome, RunOutcome::Blocked);
        assert_eq!(s.elapsed_ms, 0);
        assert_eq!(s.findings[0].kind, GuardKind::DeleteWithoutWhere);
    }
    assert!(f.executed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn confirm_asks_then_runs_when_confirmed() {
    let f = fixture(GuardLevel::Confirm).await;
    let s = run(&f, "DELETE FROM t", false).await;
    assert_eq!(s.outcome, RunOutcome::NeedsConfirmation);
    assert_eq!(s.findings.len(), 1);
    assert!(f.executed.lock().unwrap().is_empty());

    let s = run(&f, "DELETE FROM t", true).await;
    assert_eq!(s.outcome, RunOutcome::Ok);
    assert!(s.findings.is_empty());
    assert_eq!(f.executed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn safe_sql_runs_under_block() {
    let f = fixture(GuardLevel::Block).await;
    assert_eq!(run(&f, "SELECT 1", false).await.outcome, RunOutcome::Ok);
    assert_eq!(f.executed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn session_off_overrides_block_until_cleared() {
    let f = fixture(GuardLevel::Block).await;
    set_guard_off_for_session(&f.state, &f.conn_id, true);
    assert_eq!(
        run(&f, "DELETE FROM t", false).await.outcome,
        RunOutcome::Ok
    );
    set_guard_off_for_session(&f.state, &f.conn_id, false);
    assert_eq!(
        run(&f, "DELETE FROM t", false).await.outcome,
        RunOutcome::Blocked
    );
    assert_eq!(f.executed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn blocked_tab_stays_usable_and_a_busy_check_comes_after_analysis() {
    let f = fixture(GuardLevel::Block).await;
    assert_eq!(
        run(&f, "DROP TABLE t", false).await.outcome,
        RunOutcome::Blocked
    );
    // The session was never taken: a safe script still runs.
    assert_eq!(run(&f, "SELECT 1", false).await.outcome, RunOutcome::Ok);
}

#[tokio::test]
async fn permanent_change_returns_previous_and_clears_session_off() {
    let f = fixture(GuardLevel::Confirm).await;
    set_guard_off_for_session(&f.state, &f.conn_id, true);
    assert_eq!(
        guard_off_for_session(&f.state),
        std::slice::from_ref(&f.conn_id)
    );

    let previous = set_connection_guard(&f.state, f.conn_id.clone(), GuardLevel::Off)
        .await
        .unwrap();
    assert_eq!(previous, GuardLevel::Confirm);
    let saved = list_connections(&f.state).await;
    assert_eq!(saved[0].guard, GuardLevel::Off);
    assert!(!guard_off_for_session(&f.state).contains(&f.conn_id));
}

#[tokio::test]
async fn unknown_connection_is_not_found() {
    let f = fixture(GuardLevel::Off).await;
    let err = set_connection_guard(&f.state, "nope".into(), GuardLevel::Block)
        .await
        .unwrap_err();
    assert_eq!(err, NOT_FOUND);
}
