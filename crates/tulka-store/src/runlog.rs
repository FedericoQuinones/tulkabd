use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, ErrorCode};
use serde::{Deserialize, Serialize};

/// Bump when the table layout changes; a mismatching file is discarded.
const RUNLOG_VERSION: i64 = 1;

/// How long a call waits on a file locked by another process.
const BUSY_TIMEOUT: Duration = Duration::from_millis(200);

/// Longest statement text kept per entry, in bytes.
pub const MAX_LOGGED_SQL_BYTES: usize = 100 * 1024;

const SCHEMA: &str = "
CREATE TABLE run (
    id INTEGER PRIMARY KEY,
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
";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunLogOutcome {
    Ok,
    Error,
    Cancelled,
    Blocked,
    Disconnected,
}

impl RunLogOutcome {
    fn as_str(self) -> &'static str {
        match self {
            RunLogOutcome::Ok => "ok",
            RunLogOutcome::Error => "error",
            RunLogOutcome::Cancelled => "cancelled",
            RunLogOutcome::Blocked => "blocked",
            RunLogOutcome::Disconnected => "disconnected",
        }
    }

    fn parse(s: &str) -> Option<RunLogOutcome> {
        Some(match s {
            "ok" => RunLogOutcome::Ok,
            "error" => RunLogOutcome::Error,
            "cancelled" => RunLogOutcome::Cancelled,
            "blocked" => RunLogOutcome::Blocked,
            "disconnected" => RunLogOutcome::Disconnected,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunLogEntry {
    pub id: i64,
    /// Unix milliseconds, UTC.
    pub at_ms: i64,
    pub connection_id: String,
    /// Name of the connection when the statement ran.
    pub connection_name: String,
    /// Color of the connection when the statement ran.
    pub color: String,
    pub database: String,
    /// At most `MAX_LOGGED_SQL_BYTES` bytes.
    pub sql: String,
    pub sql_truncated: bool,
    pub outcome: RunLogOutcome,
    pub rows: Option<u64>,
    /// `None` for a blocked statement and for a cancelled confirmation.
    pub elapsed_ms: Option<u64>,
}

/// A run to record; the store assigns the id and applies the size cap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRun {
    pub at_ms: i64,
    pub connection_id: String,
    pub connection_name: String,
    pub color: String,
    pub database: String,
    pub sql: String,
    pub outcome: RunLogOutcome,
    pub rows: Option<u64>,
    pub elapsed_ms: Option<u64>,
}

/// Local history of executed statements in its own SQLite file. It keeps
/// statement text only, never passwords or result rows. Every call is blocking.
pub struct RunLogStore {
    conn: Mutex<Connection>,
}

/// Why the run log file could not be opened.
enum OpenError {
    /// Not a database, corrupt, or another layout: the file can be replaced.
    Unusable,
    /// Locked by another process, unreadable, or an I/O error: the file may
    /// hold the user's history and must be kept.
    Unavailable,
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => OpenError::Unusable,
            _ => OpenError::Unavailable,
        }
    }
}

/// Creates the file readable by its owner only, before SQLite opens it, so the
/// statement text is never exposed to other users even briefly.
fn create_private(path: &Path) -> Result<(), OpenError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map(|_| ())
        .map_err(|_| OpenError::Unavailable)
}

fn try_open(path: &Path) -> Result<Connection, OpenError> {
    create_private(path)?;
    let conn = Connection::open(path)?;
    // A file locked by another instance costs startup at most this long.
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // The first read fails with "not a database" on garbage files.
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version == RUNLOG_VERSION {
        let run: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'run'",
            [],
            |r| r.get(0),
        )?;
        if run != 1 {
            return Err(OpenError::Unusable);
        }
    } else if version != 0 || tables != 0 {
        return Err(OpenError::Unusable);
    } else {
        init(&conn)?;
    }
    // `journal_mode` returns a row, so it cannot go through `pragma_update`.
    let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
    Ok(conn)
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", RUNLOG_VERSION)
}

fn remove_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
}

/// Cuts `sql` to at most `MAX_LOGGED_SQL_BYTES`, on a char boundary.
fn cap_sql(sql: &str) -> (&str, bool) {
    if sql.len() <= MAX_LOGGED_SQL_BYTES {
        return (sql, false);
    }
    let mut end = MAX_LOGGED_SQL_BYTES;
    while !sql.is_char_boundary(end) {
        end -= 1;
    }
    (&sql[..end], true)
}

impl RunLogStore {
    /// Never fails. A file that is not a valid database or has another
    /// `user_version` is deleted (with its `-wal`/`-shm` sidecars) and starts
    /// empty. A file that cannot be opened right now (locked by another
    /// instance, unreadable, an I/O error) is kept untouched and the log
    /// lives in memory. The file is private to its owner on Unix.
    pub fn open(path: &Path) -> RunLogStore {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = match try_open(path) {
            Err(OpenError::Unusable) => {
                remove_files(path);
                try_open(path)
            }
            other => other,
        };
        match conn {
            Ok(conn) => RunLogStore {
                conn: Mutex::new(conn),
            },
            Err(_) => Self::in_memory(),
        }
    }

    /// `<data dir>/runlog.db`; in memory when there is no data dir.
    pub fn open_default() -> RunLogStore {
        match directories::ProjectDirs::from("dev", "tulkabd", "tulkabd") {
            Some(dirs) => Self::open(&dirs.data_dir().join("runlog.db")),
            None => Self::in_memory(),
        }
    }

    pub fn in_memory() -> RunLogStore {
        let conn = Connection::open_in_memory().expect("in-memory SQLite is always available");
        init(&conn).expect("schema is valid");
        RunLogStore {
            conn: Mutex::new(conn),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records a run, cutting its text to `MAX_LOGGED_SQL_BYTES`.
    pub fn insert(&self, run: &NewRun) -> Result<RunLogEntry, String> {
        let (sql, sql_truncated) = cap_sql(&run.sql);
        let conn = self.lock();
        conn.execute(
            "INSERT INTO run (at_ms, connection_id, connection_name, color, database, sql,
                              sql_truncated, outcome, rows, elapsed_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run.at_ms,
                run.connection_id,
                run.connection_name,
                run.color,
                run.database,
                sql,
                sql_truncated,
                run.outcome.as_str(),
                run.rows.map(|n| n as i64),
                run.elapsed_ms.map(|n| n as i64),
            ],
        )
        .map_err(|e| format!("could not record the run: {e}"))?;
        Ok(RunLogEntry {
            id: conn.last_insert_rowid(),
            at_ms: run.at_ms,
            connection_id: run.connection_id.clone(),
            connection_name: run.connection_name.clone(),
            color: run.color.clone(),
            database: run.database.clone(),
            sql: sql.to_string(),
            sql_truncated,
            outcome: run.outcome,
            rows: run.rows,
            elapsed_ms: run.elapsed_ms,
        })
    }

    /// Newest first; `before` is an id to page backwards.
    pub fn list(&self, before: Option<i64>, limit: u32) -> Vec<RunLogEntry> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, at_ms, connection_id, connection_name, color, database, sql,
                    sql_truncated, outcome, rows, elapsed_ms
             FROM run WHERE id < ?1 ORDER BY id DESC LIMIT ?2",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![before.unwrap_or(i64::MAX), limit], |r| {
            let outcome: String = r.get(8)?;
            Ok(RunLogEntry {
                id: r.get(0)?,
                at_ms: r.get(1)?,
                connection_id: r.get(2)?,
                connection_name: r.get(3)?,
                color: r.get(4)?,
                database: r.get(5)?,
                sql: r.get(6)?,
                sql_truncated: r.get(7)?,
                outcome: RunLogOutcome::parse(&outcome).unwrap_or(RunLogOutcome::Error),
                rows: r.get::<_, Option<i64>>(9)?.map(|n| n as u64),
                elapsed_ms: r.get::<_, Option<i64>>(10)?.map(|n| n as u64),
            })
        });
        match rows {
            Ok(rows) => rows.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Deletes entries recorded before `older_than_ms`; returns how many.
    pub fn prune(&self, older_than_ms: i64) -> usize {
        self.lock()
            .execute("DELETE FROM run WHERE at_ms < ?1", params![older_than_ms])
            .unwrap_or(0)
    }

    pub fn clear(&self) -> Result<(), String> {
        self.lock()
            .execute("DELETE FROM run", [])
            .map(|_| ())
            .map_err(|e| format!("could not clear the run log: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sql: &str, at_ms: i64) -> NewRun {
        NewRun {
            at_ms,
            connection_id: "c1".into(),
            connection_name: "prod".into(),
            color: "#ff0000".into(),
            database: "master".into(),
            sql: sql.into(),
            outcome: RunLogOutcome::Ok,
            rows: Some(3),
            elapsed_ms: Some(12),
        }
    }

    #[test]
    fn insert_then_list_newest_first() {
        let store = RunLogStore::in_memory();
        let a = store.insert(&run("select 1", 100)).unwrap();
        let b = store.insert(&run("select 2", 200)).unwrap();
        assert!(b.id > a.id);
        let list = store.list(None, 10);
        assert_eq!(list, vec![b, a.clone()]);
        assert_eq!(a.sql, "select 1");
        assert!(!a.sql_truncated);
        assert_eq!(a.connection_name, "prod");
        assert_eq!(a.rows, Some(3));
    }

    #[test]
    fn list_pages_with_before() {
        let store = RunLogStore::in_memory();
        for i in 0..5 {
            store.insert(&run(&format!("q{i}"), i)).unwrap();
        }
        let p1 = store.list(None, 2);
        let p2 = store.list(Some(p1[1].id), 2);
        let p3 = store.list(Some(p2[1].id), 2);
        let sqls: Vec<_> = p1
            .iter()
            .chain(&p2)
            .chain(&p3)
            .map(|e| e.sql.as_str())
            .collect();
        assert_eq!((p1.len(), p2.len(), p3.len()), (2, 2, 1));
        assert_eq!(sqls, ["q4", "q3", "q2", "q1", "q0"]);
        assert!(store.list(Some(p3[0].id), 2).is_empty());
    }

    #[test]
    fn long_sql_is_cut_at_a_char_boundary_and_marked() {
        let store = RunLogStore::in_memory();
        // A two-byte char straddles the limit.
        let mut sql = "a".repeat(MAX_LOGGED_SQL_BYTES - 1);
        sql.push_str(&"ñ".repeat(100 * 1024));
        assert!(sql.len() >= 200 * 1024 - 1);
        let entry = store.insert(&run(&sql, 1)).unwrap();
        assert!(entry.sql.len() <= MAX_LOGGED_SQL_BYTES);
        assert!(entry.sql_truncated);
        assert_eq!(entry.sql.len(), MAX_LOGGED_SQL_BYTES - 1);
        assert!(sql.starts_with(&entry.sql));
        let stored = &store.list(None, 1)[0];
        assert!(stored.sql_truncated);
        assert_eq!(stored.sql, entry.sql);
    }

    #[test]
    fn prune_drops_only_older_entries() {
        let store = RunLogStore::in_memory();
        store.insert(&run("old", 100)).unwrap();
        store.insert(&run("edge", 200)).unwrap();
        store.insert(&run("new", 300)).unwrap();
        assert_eq!(store.prune(200), 1);
        let sqls: Vec<_> = store.list(None, 10).into_iter().map(|e| e.sql).collect();
        assert_eq!(sqls, ["new", "edge"]);
    }

    #[test]
    fn clear_empties_the_log() {
        let store = RunLogStore::in_memory();
        store.insert(&run("x", 1)).unwrap();
        store.clear().unwrap();
        assert!(store.list(None, 10).is_empty());
    }

    #[test]
    fn unusable_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        std::fs::write(&path, vec![0x42u8; 4096]).unwrap();
        let store = RunLogStore::open(&path);
        assert!(store.list(None, 10).is_empty());
        store.insert(&run("x", 1)).unwrap();
        drop(store);
        assert_eq!(RunLogStore::open(&path).list(None, 10).len(), 1);
    }

    #[test]
    fn entries_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        RunLogStore::open(&path).insert(&run("x", 1)).unwrap();
        assert_eq!(RunLogStore::open(&path).list(None, 10)[0].sql, "x");
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        let store = RunLogStore::open(&path);
        store.insert(&run("x", 1)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn round_trips_every_outcome() {
        let store = RunLogStore::in_memory();
        let all = [
            RunLogOutcome::Ok,
            RunLogOutcome::Error,
            RunLogOutcome::Cancelled,
            RunLogOutcome::Blocked,
            RunLogOutcome::Disconnected,
        ];
        for outcome in all {
            let mut r = run("x", 1);
            r.outcome = outcome;
            r.rows = None;
            r.elapsed_ms = None;
            let e = store.insert(&r).unwrap();
            assert_eq!(e.outcome, outcome);
            assert_eq!((e.rows, e.elapsed_ms), (None, None));
        }
        let got: Vec<_> = store
            .list(None, 10)
            .into_iter()
            .rev()
            .map(|e| e.outcome)
            .collect();
        assert_eq!(got, all);
        assert_eq!(
            serde_json::to_string(&RunLogOutcome::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }
}
