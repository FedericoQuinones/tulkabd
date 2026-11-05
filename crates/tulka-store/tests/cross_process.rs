//! The app and the MCP server write the same run log file from different
//! processes; two connections to one file stand in for them here.

use std::sync::Arc;
use std::thread;

use tulka_store::{NewRun, RunLogOutcome, RunLogStore, RunSource, SharedRunLog};

fn run(sql: String, source: RunSource) -> NewRun {
    NewRun {
        at_ms: 1,
        connection_id: "c".into(),
        connection_name: "C".into(),
        color: "#000000".into(),
        database: "d".into(),
        sql,
        outcome: RunLogOutcome::Ok,
        rows: None,
        elapsed_ms: None,
        source,
        client: None,
    }
}

const OLD_SCHEMA: &str = "
    CREATE TABLE run (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        at_ms INTEGER NOT NULL,
        connection_id TEXT NOT NULL,
        connection_name TEXT NOT NULL,
        color TEXT NOT NULL,
        database TEXT NOT NULL,
        sql TEXT NOT NULL,
        sql_truncated INTEGER NOT NULL,
        outcome TEXT NOT NULL,
        rows INTEGER,
        elapsed_ms INTEGER
    );
    CREATE INDEX run_at_ms ON run (at_ms);
    PRAGMA user_version = 1;";

#[test]
fn two_writers_lose_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runlog.db");
    let app = Arc::new(RunLogStore::open(&path));
    let mcp = Arc::new(SharedRunLog::open(&path));
    let a = {
        let app = app.clone();
        thread::spawn(move || {
            for i in 0..200 {
                app.insert(&run(format!("app {i}"), RunSource::App))
                    .unwrap();
            }
        })
    };
    let b = {
        let mcp = mcp.clone();
        thread::spawn(move || {
            for i in 0..200 {
                mcp.insert(&run(format!("ai {i}"), RunSource::Ai)).unwrap();
            }
        })
    };
    a.join().unwrap();
    b.join().unwrap();
    assert_eq!(app.list(None, 1000).len(), 400);
    assert_eq!(app.list_after(0, Some(RunSource::Ai), 1000).len(), 200);
    assert_eq!(mcp.list(None, 1000).len(), 400);
}

#[test]
fn concurrent_migration_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runlog.db");
    {
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute_batch(OLD_SCHEMA).unwrap();
    }
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let spawn = |shared: bool| {
        let (path, barrier) = (path.clone(), barrier.clone());
        thread::spawn(move || {
            barrier.wait();
            if shared {
                let log = SharedRunLog::open(&path);
                log.insert(&run("shared".into(), RunSource::Ai)).unwrap();
            } else {
                // The app waits only 200 ms on a busy file and then keeps its
                // log in memory; on a slow disk the other opener may hold the
                // file that long, so retry until the row reaches the file.
                let check = SharedRunLog::open(&path);
                let stored =
                    |check: &SharedRunLog| check.list(None, 10).iter().any(|e| e.sql == "store");
                for _ in 0..50 {
                    let log = RunLogStore::open(&path);
                    log.insert(&run("store".into(), RunSource::App)).unwrap();
                    if stored(&check) {
                        return;
                    }
                    thread::sleep(std::time::Duration::from_millis(100));
                }
                panic!("the app never reached the file");
            }
        })
    };
    let (a, b) = (spawn(true), spawn(false));
    a.join().unwrap();
    b.join().unwrap();
    // Neither open replaced the file: both rows are there.
    let sqls: Vec<String> = SharedRunLog::open(&path)
        .list(None, 10)
        .into_iter()
        .map(|e| e.sql)
        .collect();
    assert!(sqls.contains(&"shared".to_string()));
    assert!(sqls.contains(&"store".to_string()));
}
