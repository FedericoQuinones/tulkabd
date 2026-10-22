//! Command logic against a live SQL Server.
//!
//! Skipped (they print "skipped" and pass) unless `TULKA_MSSQL_TEST` is set,
//! e.g. `eval "$(bash scripts/mssql-test.sh)"`, which exports
//! `TULKA_MSSQL_TEST="server=127.0.0.1;port=14333;user=sa;password=..."`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tulka_app::*;
use tulka_core::{ConfigStore, ConnectionConfig, Environment, MemoryStore, Value};
use tulka_mssql::MssqlDriver;

struct Live {
    state: Arc<AppState>,
    conn: ConnectionConfig,
    password: String,
    _dir: TempDir,
}

/// A state with one saved connection to the test server (password saved in
/// a MemoryStore), or `None` when `TULKA_MSSQL_TEST` is not set.
async fn live() -> Option<Live> {
    let Ok(raw) = std::env::var("TULKA_MSSQL_TEST") else {
        println!("skipped (TULKA_MSSQL_TEST not set)");
        return None;
    };
    let mut conn = ConnectionConfig::new("Live", "127.0.0.1", 1433, "", Environment::Test);
    conn.trust_server_certificate = true;
    let mut password = String::new();
    for part in raw.split(';').filter(|s| !s.trim().is_empty()) {
        let (key, value) = part
            .split_once('=')
            .expect("TULKA_MSSQL_TEST entries must be key=value");
        match key.trim().to_lowercase().as_str() {
            "server" => conn.server = value.trim().to_string(),
            "port" => conn.port = value.trim().parse().expect("port must be a number"),
            "user" => conn.user = value.to_string(),
            "password" => password = value.to_string(),
            _ => {}
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(MssqlDriver)],
    )
    .unwrap();
    let conn = save_connection(&state, conn, Some(password.clone()))
        .await
        .unwrap()
        .connection;
    Some(Live {
        state: Arc::new(state),
        conn,
        password,
        _dir: dir,
    })
}

async fn run(state: &AppState, tab: &str, sql: &str) -> (RunSummary, Vec<UiEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let summary = run_query(state, tab.into(), sql.into(), move |e| {
        sink.lock().unwrap().push(e)
    })
    .await
    .unwrap();
    let events = events.lock().unwrap().clone();
    (summary, events)
}

#[tokio::test]
async fn open_session_returns_session_info() {
    let Some(l) = live().await else { return };
    let info = open_session(&l.state, "t1".into(), l.conn.id.clone(), None, None)
        .await
        .unwrap();
    assert!(info.server.version.contains("SQL Server"), "{info:?}");
    assert_eq!(info.server.user, l.conn.user);
    assert!(info.databases.iter().any(|d| d == "master"));
    assert_eq!(info.current_database, "master");

    use_database(&l.state, "t1".into(), "tempdb".into())
        .await
        .unwrap();
    let (summary, _) = run(&l.state, "t1", "SELECT DB_NAME()").await;
    assert_eq!(summary.outcome, RunOutcome::Ok);
    let rows = fetch_rows(&l.state, "t1".into(), 0, 0, 10).await.unwrap();
    assert_eq!(rows, vec![vec![Value::Text("tempdb".into())]]);
    close_session(&l.state, "t1".into()).await.unwrap();
}

#[tokio::test]
async fn run_query_select_fills_the_buffer() {
    let Some(l) = live().await else { return };
    open_session(&l.state, "t1".into(), l.conn.id.clone(), None, None)
        .await
        .unwrap();
    let sql = "SELECT TOP (2500) CAST(ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS int) AS n \
               FROM sys.all_objects a CROSS JOIN sys.all_objects b\n\
               PRINT 'done'";
    let (summary, events) = run(&l.state, "t1", sql).await;
    assert_eq!(summary.outcome, RunOutcome::Ok);
    assert!(
        matches!(&events[0], UiEvent::ResultStart { index: 0, columns } if columns[0].name == "n")
    );
    let last = events
        .iter()
        .rev()
        .find_map(|e| match e {
            UiEvent::RowCount {
                index: 0,
                total,
                truncated,
            } => Some((*total, *truncated)),
            _ => None,
        })
        .unwrap();
    assert_eq!(last, (2500, false));
    assert!(events.contains(&UiEvent::RowsAffected { count: 2500 }));
    assert!(events
        .iter()
        .any(|e| matches!(e, UiEvent::Message { text, .. } if text == "done")));

    let rows = fetch_rows(&l.state, "t1".into(), 0, 2000, 1000)
        .await
        .unwrap();
    assert_eq!(rows.len(), 500);
    assert_eq!(rows[0], vec![Value::Int(2001)]);
    close_session(&l.state, "t1".into()).await.unwrap();
}

#[tokio::test]
async fn cancel_of_waitfor_is_fast_and_the_session_survives() {
    let Some(l) = live().await else { return };
    open_session(&l.state, "t1".into(), l.conn.id.clone(), None, None)
        .await
        .unwrap();
    let state = l.state.clone();
    let started = Instant::now();
    let running = tokio::spawn(async move {
        run_query(
            &state,
            "t1".into(),
            "WAITFOR DELAY '00:00:20'".into(),
            |_| {},
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    cancel_query(&l.state, "t1".into()).await.unwrap();
    let summary = tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .expect("cancel took longer than 3 s")
        .unwrap()
        .unwrap();
    assert_eq!(summary.outcome, RunOutcome::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(4));

    let (summary, _) = run(&l.state, "t1", "SELECT 1 AS one").await;
    assert_eq!(summary.outcome, RunOutcome::Ok);
    let rows = fetch_rows(&l.state, "t1".into(), 0, 0, 10).await.unwrap();
    assert_eq!(rows, vec![vec![Value::Int(1)]]);
    close_session(&l.state, "t1".into()).await.unwrap();
}

#[tokio::test]
async fn wrong_password_is_a_clear_error() {
    let Some(l) = live().await else { return };
    let wrong = format!("{}-wrong", l.password);
    let err = open_session(
        &l.state,
        "t1".into(),
        l.conn.id.clone(),
        None,
        Some(wrong.clone()),
    )
    .await
    .unwrap_err();
    assert!(err.starts_with("authentication failed:"), "{err}");
    assert!(err.contains("Login failed"), "{err}");
    assert!(!err.contains(&wrong));
    let err = test_connection(&l.state, l.conn.clone(), Some(wrong.clone()))
        .await
        .unwrap_err();
    assert!(err.starts_with("authentication failed:"), "{err}");
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AppEvent>>);

impl AppEvents for Recorder {
    fn emit(&self, ev: AppEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

const EXPLORER_SETUP: &str = "
IF DB_ID('tulka_explorer') IS NOT NULL
BEGIN
  ALTER DATABASE tulka_explorer SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
  DROP DATABASE tulka_explorer;
END
GO
CREATE DATABASE tulka_explorer
GO
USE tulka_explorer
GO
CREATE TABLE dbo.Pedidos(Id int NOT NULL PRIMARY KEY, Cliente nvarchar(50) NULL)
GO
CREATE VIEW dbo.vPedidos AS SELECT Id, Cliente FROM dbo.Pedidos
GO
USE master
";

#[tokio::test]
async fn connect_explorer_indexes_the_server_and_search_finds_it() {
    let Some(l) = live().await else { return };
    let (summary, events) = {
        open_session(&l.state, "setup".into(), l.conn.id.clone(), None, None)
            .await
            .unwrap();
        let r = run(&l.state, "setup", EXPLORER_SETUP).await;
        close_session(&l.state, "setup".into()).await.unwrap();
        r
    };
    assert_eq!(summary.outcome, RunOutcome::Ok, "{events:?}");

    // A state over a fresh cache that records the Explorer events.
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(MssqlDriver)],
    )
    .unwrap()
    .with_metadata(
        Arc::new(tulka_store::MetaCache::in_memory()),
        recorder.clone(),
    );
    let conn = save_connection(&state, l.conn.clone(), Some(l.password.clone()))
        .await
        .unwrap()
        .connection;

    let info = connect_explorer(&state, conn.id.clone(), None)
        .await
        .unwrap();
    assert!(info.databases.iter().any(|d| d == "tulka_explorer"));
    let idle = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let done = recorder
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, AppEvent::IndexIdle { .. }));
            if done {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        idle.is_ok(),
        "indexing never finished: {:?}",
        recorder.0.lock().unwrap()
    );

    let dbs = explorer_databases(&state, conn.id.clone()).await;
    let db = dbs.iter().find(|d| d.name == "tulka_explorer").unwrap();
    assert!(db.indexed_at.is_some() && !db.no_access);
    assert_eq!((db.tables, db.views), (1, 1));

    let r = search_objects(&state, "pedidos".into(), 20).await;
    assert_eq!(r.indexing, 0);
    let hit = r
        .hits
        .iter()
        .find(|h| h.database == "tulka_explorer" && h.object.name == "Pedidos")
        .unwrap_or_else(|| panic!("{r:?}"));
    assert!(hit.connected);

    let structure = table_structure(
        &state,
        conn.id.clone(),
        "tulka_explorer".into(),
        "dbo".into(),
        "Pedidos".into(),
    )
    .await
    .unwrap();
    assert_eq!(structure.columns.len(), 2);
    let def = object_definition(
        &state,
        conn.id.clone(),
        "tulka_explorer".into(),
        "dbo".into(),
        "vPedidos".into(),
    )
    .await
    .unwrap();
    assert!(def.contains("CREATE VIEW"), "{def}");
    let m = completion_metadata(&state, conn.id.clone(), "tulka_explorer".into()).await;
    assert_eq!(m.columns["dbo.Pedidos"], ["Id", "Cliente"]);

    disconnect_explorer(&state, conn.id.clone()).await.unwrap();
    assert_eq!(
        table_structure(
            &state,
            conn.id,
            "tulka_explorer".into(),
            "dbo".into(),
            "Pedidos".into()
        )
        .await
        .unwrap_err(),
        NOT_CONNECTED
    );
}

/// Measurement, not a check: full indexing time and search latency over the
/// databases made by `scripts/mssql-seed-many.sql`. Run with
/// `cargo test -p tulka-app --test live_mssql -- --ignored --nocapture seeded`.
#[tokio::test]
#[ignore]
async fn seeded_server_indexing_and_search_timings() {
    let Some(l) = live().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let recorder = Arc::new(Recorder::default());
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(MssqlDriver)],
    )
    .unwrap()
    .with_metadata(
        Arc::new(tulka_store::MetaCache::in_memory()),
        recorder.clone(),
    );
    let conn = save_connection(&state, l.conn.clone(), Some(l.password.clone()))
        .await
        .unwrap()
        .connection;

    let start = Instant::now();
    connect_explorer(&state, conn.id.clone(), None)
        .await
        .unwrap();
    let connected = start.elapsed();
    tokio::time::timeout(Duration::from_secs(600), async {
        while !recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, AppEvent::IndexIdle { .. }))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("indexing never finished");
    println!(
        "TIMING connect={connected:?} full_index={:?}",
        start.elapsed()
    );

    for q in ["table_1", "proc_4", "vw 12", "zzz"] {
        let mut times = Vec::new();
        let mut last = None;
        for _ in 0..20 {
            let t = Instant::now();
            last = Some(search_objects(&state, q.into(), 50).await);
            times.push(t.elapsed());
        }
        times.sort();
        let r = last.unwrap();
        println!(
            "TIMING search {q:?}: median={:?} max={:?} total_matches={}",
            times[10], times[19], r.total_matches
        );
    }
    disconnect_explorer(&state, conn.id).await.unwrap();
}
