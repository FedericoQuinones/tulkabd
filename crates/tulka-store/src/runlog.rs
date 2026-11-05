use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, ErrorCode, Row};
use serde::{Deserialize, Serialize};

use crate::private_file;

/// Bump when the table layout changes incompatibly; a mismatching file is
/// discarded by the app. Columns added later are migrated in place and do not
/// change this number.
const RUNLOG_VERSION: i64 = 1;

/// How long the app waits on a file locked by another process. The MCP
/// server writes to the same file, and on Windows a commit can hold the lock
/// for a few hundred milliseconds.
const BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// How long another process (the MCP server) waits on a busy file before it
/// gives up on one write.
const SHARED_BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest statement text kept per entry, in bytes.
pub const MAX_LOGGED_SQL_BYTES: usize = 100 * 1024;

/// Longest client name kept per entry, in chars.
const MAX_CLIENT_CHARS: usize = 100;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS run (
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
    elapsed_ms INTEGER,
    source TEXT NOT NULL DEFAULT 'app',
    client TEXT
);
CREATE INDEX IF NOT EXISTS run_at_ms ON run (at_ms);
";

/// Columns of the run table, in the order `entry_from_row` reads them.
const COLUMNS: &str = "id, at_ms, connection_id, connection_name, color, database, sql,
                    sql_truncated, outcome, rows, elapsed_ms, source, client";

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

/// Who ran a statement: the user in the app, or an AI client through the
/// MCP server. Stored as `app` / `ai`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunSource {
    #[default]
    App,
    Ai,
}

impl RunSource {
    fn as_str(self) -> &'static str {
        match self {
            RunSource::App => "app",
            RunSource::Ai => "ai",
        }
    }

    /// Anything unknown reads as `App`, the only source older versions wrote.
    fn parse(s: &str) -> RunSource {
        match s {
            "ai" => RunSource::Ai,
            _ => RunSource::App,
        }
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
    #[serde(default)]
    pub source: RunSource,
    /// Name of the MCP client that ran it, at most 100 chars.
    pub client: Option<String>,
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
    pub source: RunSource,
    /// Name of the MCP client; cut to 100 chars when stored.
    pub client: Option<String>,
}

/// Local history of executed statements in its own SQLite file. It keeps
/// statement text only, never passwords or result rows. Every call is blocking.
pub struct RunLogStore {
    conn: Mutex<Connection>,
}

/// Why the run log file could not be opened.
enum OpenError {
    /// Not a database, corrupt, or another layout: the app may replace the
    /// file; any other process must leave it alone.
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

/// `<data dir>/runlog.db`, if the platform has a data dir.
fn default_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "tulkabd", "tulkabd")
        .map(|dirs| dirs.data_dir().join("runlog.db"))
}

/// Opens `path` (creating it if missing) and brings it to the current layout.
/// Shared by the app and the MCP server; neither the layout check nor the
/// connection setup deletes anything.
fn open_connection(path: &Path, busy_timeout: Duration) -> Result<Connection, OpenError> {
    private_file::create_private(path).map_err(|_| OpenError::Unavailable)?;
    private_file::restrict_permissions(path);
    let conn = Connection::open(path)?;
    // A file locked by another process costs a call at most this long.
    conn.busy_timeout(busy_timeout)?;
    ensure_schema(&conn)?;
    configure(&conn)?;
    private_file::restrict_permissions(path);
    Ok(conn)
}

fn try_open(path: &Path) -> Result<Connection, OpenError> {
    open_connection(path, BUSY_TIMEOUT)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    // `journal_mode` returns a row, so it cannot go through `pragma_update`.
    let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
    // Deleted statement text is overwritten, not left in free pages.
    conn.pragma_update(None, "secure_delete", "ON")
}

/// Creates the table in an empty file, or adds the columns an older layout
/// lacks. Runs in one `BEGIN IMMEDIATE` so two processes opening the same
/// file at once cannot both alter it. Anything else is `Unusable`.
fn ensure_schema(conn: &Connection) -> Result<(), OpenError> {
    // The first statement fails with "not a database" on garbage files.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match migrate(conn) {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn migrate(conn: &Connection) -> Result<(), OpenError> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version == 0 && tables == 0 {
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", RUNLOG_VERSION)?;
        return Ok(());
    }
    if version != RUNLOG_VERSION {
        return Err(OpenError::Unusable);
    }
    let mut columns = Vec::new();
    {
        let mut stmt = conn.prepare("PRAGMA table_info(run)")?;
        let names = stmt.query_map([], |r| r.get::<_, String>(1))?;
        for name in names {
            columns.push(name?);
        }
    }
    if columns.is_empty() {
        return Err(OpenError::Unusable);
    }
    if !columns.iter().any(|c| c == "source") {
        conn.execute_batch("ALTER TABLE run ADD COLUMN source TEXT NOT NULL DEFAULT 'app'")?;
    }
    if !columns.iter().any(|c| c == "client") {
        conn.execute_batch("ALTER TABLE run ADD COLUMN client TEXT")?;
    }
    Ok(())
}

fn remove_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
}

/// Folds the write-ahead log into the main file and empties it, so deleted
/// text does not linger there. A no-op on an in-memory database.
fn checkpoint(conn: &Connection) {
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
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

/// Writes one run, cutting its text and client name to their caps.
fn insert_run(conn: &Connection, run: &NewRun) -> Result<RunLogEntry, String> {
    let (sql, sql_truncated) = cap_sql(&run.sql);
    let client: Option<String> = run
        .client
        .as_ref()
        .map(|c| c.chars().take(MAX_CLIENT_CHARS).collect());
    conn.execute(
        "INSERT INTO run (at_ms, connection_id, connection_name, color, database, sql,
                          sql_truncated, outcome, rows, elapsed_ms, source, client)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            run.source.as_str(),
            client,
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
        source: run.source,
        client,
    })
}

/// Reads a row selected with `COLUMNS`.
fn entry_from_row(r: &Row) -> rusqlite::Result<RunLogEntry> {
    let outcome: String = r.get(8)?;
    let source: String = r.get(11)?;
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
        source: RunSource::parse(&source),
        client: r.get(12)?,
    })
}

/// Newest first; a read error yields an empty or shorter list by design.
fn list_entries(conn: &Connection, before: Option<i64>, limit: u32) -> Vec<RunLogEntry> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM run WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
    )) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params![before.unwrap_or(i64::MAX), limit], entry_from_row);
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

impl RunLogStore {
    /// Never fails. A file that is not a valid database or has another
    /// `user_version` is deleted (with its `-wal`/`-shm` sidecars) and starts
    /// empty. A file that cannot be opened right now (locked by another
    /// process, unreadable, an I/O error) is kept untouched and the log
    /// lives in memory. The file is private to its owner on Unix.
    pub fn open(path: &Path) -> RunLogStore {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = match try_open(path) {
            // Only a file known to be unusable is replaced; a busy or
            // locked one belongs to another process and stays.
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
        match default_path() {
            Some(path) => {
                if let Some(dir) = path.parent() {
                    private_file::ensure_private_dir(dir);
                }
                Self::open(&path)
            }
            None => Self::in_memory(),
        }
    }

    pub fn in_memory() -> RunLogStore {
        let conn = Connection::open_in_memory().expect("in-memory SQLite is always available");
        if ensure_schema(&conn).is_err() {
            panic!("schema is valid");
        }
        let _ = conn.pragma_update(None, "secure_delete", "ON");
        RunLogStore {
            conn: Mutex::new(conn),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records a run, cutting its text to `MAX_LOGGED_SQL_BYTES`.
    pub fn insert(&self, run: &NewRun) -> Result<RunLogEntry, String> {
        insert_run(&self.lock(), run)
    }

    /// Newest first; `before` is an id to page backwards. A read error yields
    /// an empty or shorter list by design: the log is a convenience and must
    /// never get in the way of running a query.
    pub fn list(&self, before: Option<i64>, limit: u32) -> Vec<RunLogEntry> {
        list_entries(&self.lock(), before, limit)
    }

    /// Entries with `id > after`, oldest first, optionally of one source.
    pub fn list_after(
        &self,
        after: i64,
        source: Option<RunSource>,
        limit: u32,
    ) -> Vec<RunLogEntry> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM run
             WHERE id > ?1 AND (?2 IS NULL OR source = ?2) ORDER BY id LIMIT ?3"
        )) else {
            return Vec::new();
        };
        let source = source.map(RunSource::as_str);
        let rows = stmt.query_map(params![after, source, limit], entry_from_row);
        match rows {
            Ok(rows) => rows.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Highest id ever stored and still present; 0 when the log is empty.
    pub fn max_id(&self) -> i64 {
        self.lock()
            .query_row("SELECT COALESCE(MAX(id), 0) FROM run", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Deletes entries recorded before `older_than_ms`; returns how many.
    /// Errors are swallowed by design (reported as 0 deleted): pruning is
    /// housekeeping and will simply be tried again later.
    pub fn prune(&self, older_than_ms: i64) -> usize {
        let conn = self.lock();
        let deleted = conn
            .execute("DELETE FROM run WHERE at_ms < ?1", params![older_than_ms])
            .unwrap_or(0);
        if deleted > 0 {
            checkpoint(&conn);
        }
        deleted
    }

    pub fn clear(&self) -> Result<(), String> {
        let conn = self.lock();
        conn.execute("DELETE FROM run", [])
            .map_err(|e| format!("could not clear the run log: {e}"))?;
        checkpoint(&conn);
        Ok(())
    }
}

/// The run log as written by a process other than the app (the MCP server).
/// It opens lazily, waits up to 2 s on a busy file, creates the file only if
/// it is missing, adds missing columns, and never deletes or replaces it: a
/// file with an unknown layout is reported and left as it is.
pub struct SharedRunLog {
    /// `None` for an in-memory log.
    path: Option<PathBuf>,
    conn: Mutex<Option<Connection>>,
}

impl SharedRunLog {
    pub fn open(path: &Path) -> SharedRunLog {
        SharedRunLog {
            path: Some(path.to_path_buf()),
            conn: Mutex::new(None),
        }
    }

    /// `<data dir>/runlog.db`, like [`RunLogStore::open_default`]; in memory
    /// when there is no data dir.
    pub fn open_default() -> SharedRunLog {
        match default_path() {
            Some(path) => {
                if let Some(dir) = path.parent() {
                    private_file::ensure_private_dir(dir);
                }
                Self::open(&path)
            }
            None => Self::in_memory(),
        }
    }

    pub fn in_memory() -> SharedRunLog {
        let conn = Connection::open_in_memory().expect("in-memory SQLite is always available");
        if ensure_schema(&conn).is_err() {
            panic!("schema is valid");
        }
        SharedRunLog {
            path: None,
            conn: Mutex::new(Some(conn)),
        }
    }

    /// Runs `f` on the connection, opening it first when needed. An open that
    /// fails is retried by the next call.
    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let mut guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            let path = self.path.as_deref().ok_or("the run log is not available")?;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match open_connection(path, SHARED_BUSY_TIMEOUT) {
                Ok(conn) => *guard = Some(conn),
                Err(OpenError::Unusable) => {
                    return Err("the run log file has an unknown layout".to_string())
                }
                Err(OpenError::Unavailable) => {
                    return Err("could not open the run log file".to_string())
                }
            }
        }
        match guard.as_ref() {
            Some(conn) => f(conn),
            None => Err("the run log is not available".to_string()),
        }
    }

    /// Records a run, cutting its text to `MAX_LOGGED_SQL_BYTES`.
    pub fn insert(&self, run: &NewRun) -> Result<RunLogEntry, String> {
        self.with_conn(|conn| insert_run(conn, run))
    }

    /// Newest first; empty when the file cannot be read. For tests.
    pub fn list(&self, before: Option<i64>, limit: u32) -> Vec<RunLogEntry> {
        self.with_conn(|conn| Ok(list_entries(conn, before, limit)))
            .unwrap_or_default()
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
            source: RunSource::App,
            client: None,
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
    fn clear_and_prune_erase_the_text_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        let store = RunLogStore::open(&path);
        store.insert(&run("select 'MARKER_CLEAR_7731'", 1)).unwrap();
        store.insert(&run("select 'MARKER_PRUNE_5519'", 2)).unwrap();
        store.insert(&run("select 'keep'", 1000)).unwrap();
        assert_eq!(store.prune(500), 2);
        store.clear().unwrap();
        drop(store);
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        for file in [path.clone(), PathBuf::from(wal)] {
            if let Ok(bytes) = std::fs::read(&file) {
                for marker in ["MARKER_CLEAR_7731", "MARKER_PRUNE_5519", "keep"] {
                    assert!(
                        !bytes.windows(marker.len()).any(|w| w == marker.as_bytes()),
                        "{marker} left in {file:?}"
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        {
            let store = RunLogStore::open(&path);
            store.insert(&run("x", 1)).unwrap();
        }
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let store = RunLogStore::open(&path);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(store.list(None, 10).len(), 1);
        for suffix in ["-wal", "-shm"] {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            let side = PathBuf::from(s);
            if side.exists() {
                assert_eq!(mode(&side), 0o600, "{suffix}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_sidecars_are_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        let store = RunLogStore::open(&path);
        store.insert(&run("x", 1)).unwrap();
        // The store is still open, so the sidecars exist.
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        assert!(wal.exists());
        std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
        let second = RunLogStore::open(&path);
        let mode = std::fs::metadata(&wal).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(second);
        drop(store);
    }

    #[cfg(unix)]
    #[test]
    fn replaced_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        std::fs::write(&path, vec![0x42u8; 4096]).unwrap();
        let store = RunLogStore::open(&path);
        store.insert(&run("x", 1)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn sql_of_exactly_the_limit_is_not_truncated() {
        let store = RunLogStore::in_memory();
        let entry = store
            .insert(&run(&"a".repeat(MAX_LOGGED_SQL_BYTES), 1))
            .unwrap();
        assert_eq!(entry.sql.len(), MAX_LOGGED_SQL_BYTES);
        assert!(!entry.sql_truncated);
    }

    #[test]
    fn ids_are_not_reused_after_clear_or_prune() {
        let store = RunLogStore::in_memory();
        let a = store.insert(&run("a", 1)).unwrap();
        store.clear().unwrap();
        let b = store.insert(&run("b", 2)).unwrap();
        assert!(b.id > a.id);
        store.prune(10);
        let c = store.insert(&run("c", 3)).unwrap();
        assert!(c.id > b.id);
    }

    #[test]
    fn other_user_version_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        {
            let store = RunLogStore::open(&path);
            store.insert(&run("old", 1)).unwrap();
            store
                .lock()
                .pragma_update(None, "user_version", RUNLOG_VERSION + 1)
                .unwrap();
        }
        let store = RunLogStore::open(&path);
        assert!(store.list(None, 10).is_empty());
        store.insert(&run("new", 2)).unwrap();
        assert_eq!(store.list(None, 10)[0].sql, "new");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        RunLogStore::open(&path).insert(&run("text", 1)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            // Running as root: permissions do not stop reads, nothing to test.
            return;
        }
        let store = RunLogStore::open(&path);
        assert!(store.list(None, 10).is_empty());
        store.insert(&run("memory", 2)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let kept = RunLogStore::open(&path).list(None, 10);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].sql, "text");
    }

    #[test]
    fn locked_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        RunLogStore::open(&path).insert(&run("text", 1)).unwrap();
        {
            // Another instance holding an exclusive lock on the file.
            let other = Connection::open(&path).unwrap();
            other
                .pragma_update(None, "locking_mode", "EXCLUSIVE")
                .unwrap();
            other.execute_batch("BEGIN EXCLUSIVE").unwrap();
            let start = std::time::Instant::now();
            let store = RunLogStore::open(&path);
            // A locked file must not hold up startup for long.
            assert!(start.elapsed() < std::time::Duration::from_secs(2));
            assert!(store.list(None, 10).is_empty());
            other.execute_batch("ROLLBACK").unwrap();
        }
        assert_eq!(RunLogStore::open(&path).list(None, 10)[0].sql, "text");
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

    /// The layout written before `source` and `client` existed.
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

    const OLD_INSERT: &str = "INSERT INTO run (at_ms, connection_id, connection_name, color,
        database, sql, sql_truncated, outcome, rows, elapsed_ms)
        VALUES (1, 'c', 'C', '#000', 'd', ?1, 0, 'ok', NULL, NULL)";

    fn old_file(path: &Path) {
        let raw = Connection::open(path).unwrap();
        raw.execute_batch(OLD_SCHEMA).unwrap();
        raw.execute(OLD_INSERT, ["old row"]).unwrap();
    }

    #[test]
    fn plan4_file_gets_the_new_columns_and_keeps_its_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        old_file(&path);
        let store = RunLogStore::open(&path);
        let list = store.list(None, 10);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].sql, "old row");
        assert_eq!(list[0].source, RunSource::App);
        assert_eq!(list[0].client, None);
    }

    #[test]
    fn old_layout_insert_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        old_file(&path);
        drop(RunLogStore::open(&path));
        // What an older tulkabd does after the migration.
        let raw = Connection::open(&path).unwrap();
        raw.execute(OLD_INSERT, ["from old"]).unwrap();
        drop(raw);
        let list = RunLogStore::open(&path).list(None, 10);
        assert_eq!(list[0].sql, "from old");
        assert_eq!(list[0].source, RunSource::App);
    }

    #[test]
    fn source_and_client_round_trip() {
        let store = RunLogStore::in_memory();
        let mut r = run("select 1", 1);
        r.source = RunSource::Ai;
        r.client = Some("claude-code".into());
        let e = store.insert(&r).unwrap();
        assert_eq!(
            (e.source, e.client.as_deref()),
            (RunSource::Ai, Some("claude-code"))
        );
        assert_eq!(store.list(None, 1)[0], e);
        let mut long = run("select 2", 2);
        long.client = Some("ñ".repeat(150));
        let e = store.insert(&long).unwrap();
        assert_eq!(e.client.unwrap().chars().count(), 100);
        assert_eq!(serde_json::to_string(&RunSource::Ai).unwrap(), "\"ai\"");
    }

    #[test]
    fn list_after_filters_by_source_oldest_first() {
        let store = RunLogStore::in_memory();
        let a = store.insert(&run("app1", 1)).unwrap();
        let mut ai = run("ai1", 2);
        ai.source = RunSource::Ai;
        let b = store.insert(&ai).unwrap();
        store.insert(&run("app2", 3)).unwrap();
        ai.sql = "ai2".into();
        let d = store.insert(&ai).unwrap();
        let sqls = |v: Vec<RunLogEntry>| v.into_iter().map(|e| e.sql).collect::<Vec<_>>();
        assert_eq!(
            sqls(store.list_after(0, Some(RunSource::Ai), 10)),
            ["ai1", "ai2"]
        );
        assert_eq!(
            sqls(store.list_after(b.id, Some(RunSource::Ai), 10)),
            ["ai2"]
        );
        assert_eq!(
            sqls(store.list_after(a.id, None, 10)),
            ["ai1", "app2", "ai2"]
        );
        assert_eq!(sqls(store.list_after(0, None, 2)), ["app1", "ai1"]);
        assert!(store.list_after(d.id, None, 10).is_empty());
    }

    #[test]
    fn max_id_of_empty_is_zero() {
        let store = RunLogStore::in_memory();
        assert_eq!(store.max_id(), 0);
        let e = store.insert(&run("x", 1)).unwrap();
        assert_eq!(store.max_id(), e.id);
    }

    #[test]
    fn in_memory_and_file_stores_share_the_layout() {
        let dir = tempfile::tempdir().unwrap();
        let columns = |store: &RunLogStore| -> Vec<String> {
            let conn = store.lock();
            let mut st = conn.prepare("PRAGMA table_info(run)").unwrap();
            let rows = st.query_map([], |r| r.get(1)).unwrap();
            rows.map(Result::unwrap).collect()
        };
        let file = RunLogStore::open(&dir.path().join("r.db"));
        assert_eq!(columns(&file), columns(&RunLogStore::in_memory()));
    }

    #[test]
    fn shared_never_deletes_an_unknown_layout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        {
            let raw = Connection::open(&path).unwrap();
            raw.execute_batch(
                "CREATE TABLE other (x TEXT); INSERT INTO other VALUES ('keep me');
                 PRAGMA user_version = 7;",
            )
            .unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let shared = SharedRunLog::open(&path);
        let err = shared.insert(&run("x", 1)).unwrap_err();
        assert_eq!(err, "the run log file has an unknown layout");
        assert!(shared.list(None, 10).is_empty());
        drop(shared);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn shared_never_deletes_a_garbage_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        std::fs::write(&path, vec![0x42u8; 4096]).unwrap();
        let shared = SharedRunLog::open(&path);
        assert!(shared.insert(&run("x", 1)).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x42u8; 4096]);
    }

    #[test]
    fn shared_creates_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("r.db");
        let shared = SharedRunLog::open(&path);
        let mut r = run("select 1", 1);
        r.source = RunSource::Ai;
        shared.insert(&r).unwrap();
        assert!(path.exists());
        // The app reads what the other process wrote.
        let seen = RunLogStore::open(&path).list(None, 10);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].source, RunSource::Ai);
    }

    #[test]
    fn shared_adds_the_columns_to_an_old_file_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        old_file(&path);
        let shared = SharedRunLog::open(&path);
        shared.insert(&run("new", 2)).unwrap();
        let sqls: Vec<_> = shared.list(None, 10).into_iter().map(|e| e.sql).collect();
        assert_eq!(sqls, ["new", "old row"]);
    }

    #[test]
    fn shared_retries_after_a_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        RunLogStore::open(&path).insert(&run("text", 1)).unwrap();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let holder = {
            let path = path.clone();
            std::thread::spawn(move || {
                let other = Connection::open(&path).unwrap();
                other
                    .pragma_update(None, "locking_mode", "EXCLUSIVE")
                    .unwrap();
                other.execute_batch("BEGIN EXCLUSIVE").unwrap();
                locked_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_secs(3));
                other.execute_batch("ROLLBACK").unwrap();
            })
        };
        locked_rx.recv().unwrap();
        let shared = SharedRunLog::open(&path);
        let start = std::time::Instant::now();
        assert!(shared.insert(&run("during", 2)).is_err());
        assert!(start.elapsed() >= SHARED_BUSY_TIMEOUT - Duration::from_millis(200));
        holder.join().unwrap();
        shared.insert(&run("after", 3)).unwrap();
        let sqls: Vec<_> = shared.list(None, 10).into_iter().map(|e| e.sql).collect();
        assert_eq!(sqls, ["after", "text"]);
    }

    #[cfg(unix)]
    #[test]
    fn shared_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.db");
        let shared = SharedRunLog::open(&path);
        shared.insert(&run("x", 1)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        for suffix in ["-wal", "-shm"] {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            let side = PathBuf::from(s);
            if side.exists() {
                assert_eq!(mode(&side), 0o600, "{suffix}");
            }
        }
    }

    #[test]
    fn shared_in_memory_works() {
        let shared = SharedRunLog::in_memory();
        shared.insert(&run("x", 1)).unwrap();
        assert_eq!(shared.list(None, 10).len(), 1);
    }
}
