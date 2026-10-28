use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};

use crate::private_file;

/// Bump when the table layout changes; a mismatching file is discarded.
const SESSION_VERSION: i64 = 1;

/// How long a call waits on a file locked by another process.
const BUSY_TIMEOUT: Duration = Duration::from_millis(200);

/// How many allowed `.sql` paths are remembered; the oldest go first.
const MAX_ALLOWED_FILES: i64 = 500;

const SESSION_TABLE: &str = "
CREATE TABLE session (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    json TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
";

/// Created on every open, without a new `user_version`: a session file from
/// before it existed just gains the table.
const ALLOWED_TABLE: &str = "
CREATE TABLE IF NOT EXISTS allowed_file (
    path TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
";

/// The workspace session (open tabs, layout, editor text) in its own SQLite
/// file, separate from the metadata cache. The JSON is opaque to the backend:
/// its format belongs to the UI. It holds editor text, never passwords, and is
/// never logged. Every call is blocking.
///
/// The file also holds the `.sql` paths the user picked or dropped
/// (`allowed_file`). They are written only by the backend from the native
/// dialog and drop events; the session JSON, which the UI writes, never adds
/// to them.
pub struct SessionStore {
    conn: Mutex<Connection>,
}

/// Why a session file could not be opened.
enum OpenError {
    /// Not a database, corrupt, or another layout: the file can be replaced.
    Unusable,
    /// Locked by another process, unreadable, or an I/O error: the file may
    /// hold the user's text and must be kept.
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

fn try_open(path: &Path) -> Result<Connection, OpenError> {
    // A file that cannot be written (read-only) still opens for reading.
    let _ = private_file::create_private(path);
    private_file::restrict_permissions(path);
    let conn = Connection::open(path)?;
    // A file locked by another instance costs startup at most this long.
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // The first read fails with "not a database" on garbage files.
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version == SESSION_VERSION {
        let session: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'session'",
            [],
            |r| r.get(0),
        )?;
        if session != 1 {
            return Err(OpenError::Unusable);
        }
    } else if version != 0 || tables != 0 {
        return Err(OpenError::Unusable);
    } else {
        init(&conn)?;
    }
    // Not essential: on a file that cannot be written the session still
    // loads, and the allowed paths are just not remembered.
    let _ = conn.execute_batch(ALLOWED_TABLE);
    // `journal_mode` returns a row, so it cannot go through `pragma_update`.
    let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
    private_file::restrict_permissions(path);
    Ok(conn)
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SESSION_TABLE)?;
    conn.execute_batch(ALLOWED_TABLE)?;
    conn.pragma_update(None, "user_version", SESSION_VERSION)
}

fn remove_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
}

impl SessionStore {
    /// Never fails. A file that is not a valid database or has another
    /// `user_version` is deleted (with its `-wal`/`-shm` sidecars) and starts
    /// empty. A file that cannot be opened right now (locked by another
    /// instance, unreadable, an I/O error) is kept untouched and the session
    /// lives in memory.
    pub fn open(path: &Path) -> SessionStore {
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
            Ok(conn) => SessionStore {
                conn: Mutex::new(conn),
            },
            Err(_) => Self::in_memory(),
        }
    }

    /// `<data dir>/session.db`; in memory when there is no data dir.
    pub fn open_default() -> SessionStore {
        match directories::ProjectDirs::from("dev", "tulkabd", "tulkabd") {
            Some(dirs) => Self::open(&dirs.data_dir().join("session.db")),
            None => Self::in_memory(),
        }
    }

    pub fn in_memory() -> SessionStore {
        let conn = Connection::open_in_memory().expect("in-memory SQLite is always available");
        init(&conn).expect("schema is valid");
        SessionStore {
            conn: Mutex::new(conn),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The saved session JSON, if any.
    pub fn load(&self) -> Option<String> {
        self.lock()
            .query_row("SELECT json FROM session WHERE id = 1", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    /// Replaces the saved session in one transaction: a failure leaves the
    /// previous session intact.
    pub fn save(&self, json: &str) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let mut conn = self.lock();
        let run = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO session (id, json, saved_at) VALUES (1, ?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET json = excluded.json, saved_at = excluded.saved_at",
                params![json, now],
            )?;
            tx.commit()
        };
        run(&mut conn).map_err(|e| format!("could not save the session: {e}"))
    }

    /// The `.sql` paths the user picked or dropped in earlier runs (at most
    /// 500, the newest). Empty when they cannot be read.
    pub fn allowed_files(&self) -> Vec<PathBuf> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare("SELECT path FROM allowed_file ORDER BY at") else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok).map(PathBuf::from).collect()
    }

    /// Remembers `paths` as the newest allowed ones, in order (a known path
    /// is moved to the newest), and drops the oldest beyond 500. Only the
    /// last 500 of a batch are written. A failure is ignored: the paths are
    /// still allowed in this run, they are only not remembered.
    pub fn remember_allowed(&self, paths: &[PathBuf]) {
        let keep = &paths[paths.len().saturating_sub(MAX_ALLOWED_FILES as usize)..];
        if keep.is_empty() {
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let mut conn = self.lock();
        let run = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            // Strictly after every path already there, so the order holds
            // when several land in the same millisecond.
            let newest: Option<i64> =
                tx.query_row("SELECT MAX(at) FROM allowed_file", [], |r| r.get(0))?;
            let base = newest.map_or(now, |n| now.max(n + 1));
            {
                let mut insert = tx.prepare_cached(
                    "INSERT INTO allowed_file (path, at) VALUES (?1, ?2)
                     ON CONFLICT (path) DO UPDATE SET at = excluded.at",
                )?;
                for (i, path) in keep.iter().enumerate() {
                    insert.execute(params![path.to_string_lossy(), base + i as i64])?;
                }
            }
            tx.execute(
                "DELETE FROM allowed_file WHERE path NOT IN
                 (SELECT path FROM allowed_file ORDER BY at DESC LIMIT ?1)",
                params![MAX_ALLOWED_FILES],
            )?;
            tx.commit()
        };
        let _ = run(&mut conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_file_has_no_session() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(SessionStore::open(&dir.path().join("s.db")).load(), None);
    }

    #[test]
    fn saved_session_survives_reopen_and_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let store = SessionStore::open(&path);
        store.save(r#"{"a":1}"#).unwrap();
        store.save(r#"{"a":2}"#).unwrap();
        drop(store);
        assert_eq!(
            SessionStore::open(&path).load().as_deref(),
            Some(r#"{"a":2}"#)
        );
    }

    #[test]
    fn garbage_file_opens_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        std::fs::write(&path, vec![0x42u8; 4096]).unwrap();
        let store = SessionStore::open(&path);
        assert_eq!(store.load(), None);
        store.save("{}").unwrap();
        assert_eq!(store.load().as_deref(), Some("{}"));
    }

    #[test]
    fn other_user_version_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let store = SessionStore::open(&path);
            store.save("old").unwrap();
            store
                .lock()
                .pragma_update(None, "user_version", SESSION_VERSION + 1)
                .unwrap();
        }
        let store = SessionStore::open(&path);
        assert_eq!(store.load(), None);
        store.save("new").unwrap();
        assert_eq!(store.load().as_deref(), Some("new"));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        SessionStore::open(&path).save("text").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            // Running as root: permissions do not stop reads, nothing to test.
            return;
        }
        let store = SessionStore::open(&path);
        assert_eq!(store.load(), None);
        store.save("memory").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(SessionStore::open(&path).load().as_deref(), Some("text"));
    }

    #[test]
    fn locked_file_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        SessionStore::open(&path).save("text").unwrap();
        {
            // Another instance holding an exclusive lock on the file.
            let other = Connection::open(&path).unwrap();
            other
                .pragma_update(None, "locking_mode", "EXCLUSIVE")
                .unwrap();
            other.execute_batch("BEGIN EXCLUSIVE").unwrap();
            let start = std::time::Instant::now();
            let store = SessionStore::open(&path);
            // A locked file must not hold up startup for long.
            assert!(start.elapsed() < std::time::Duration::from_secs(1));
            assert_eq!(store.load(), None);
            other.execute_batch("ROLLBACK").unwrap();
        }
        assert_eq!(SessionStore::open(&path).load().as_deref(), Some("text"));
    }

    #[test]
    fn failed_save_keeps_the_previous_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let store = SessionStore::open(&path);
        store.save("first").unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER fail BEFORE UPDATE ON session WHEN NEW.json = 'second'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        assert!(store.save("second").is_err());
        assert_eq!(store.load().as_deref(), Some("first"));
        drop(store);
        assert_eq!(SessionStore::open(&path).load().as_deref(), Some("first"));
    }

    #[test]
    fn five_megabytes_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let big = "x".repeat(5 * 1024 * 1024);
        SessionStore::open(&path).save(&big).unwrap();
        assert_eq!(SessionStore::open(&path).load(), Some(big));
    }

    #[test]
    fn in_memory_works() {
        let store = SessionStore::in_memory();
        assert_eq!(store.load(), None);
        store.save("{}").unwrap();
        assert_eq!(store.load().as_deref(), Some("{}"));
    }

    #[test]
    fn allowed_files_round_trip_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let store = SessionStore::open(&path);
        assert!(store.allowed_files().is_empty());
        let a = PathBuf::from("/q/a.sql");
        let b = PathBuf::from("/q/b.sql");
        store.remember_allowed(&[a.clone(), b.clone()]);
        // Remembering a path again does not duplicate it.
        store.remember_allowed(std::slice::from_ref(&a));
        let mut got = store.allowed_files();
        got.sort();
        assert_eq!(got, [a.clone(), b.clone()]);
        drop(store);
        let mut got = SessionStore::open(&path).allowed_files();
        got.sort();
        assert_eq!(got, [a, b]);
    }

    #[test]
    fn allowed_files_keep_the_newest_500() {
        let store = SessionStore::in_memory();
        // One call per path, so each has its own, increasing, timestamp order.
        for i in 0..520 {
            store.remember_allowed(&[PathBuf::from(format!("/q/{i}.sql"))]);
        }
        let got = store.allowed_files();
        assert_eq!(got.len(), 500);
        assert!(!got.contains(&PathBuf::from("/q/0.sql")));
        assert!(!got.contains(&PathBuf::from("/q/19.sql")));
        assert!(got.contains(&PathBuf::from("/q/20.sql")));
        assert!(got.contains(&PathBuf::from("/q/519.sql")));
        // A path seen again is the newest, so it survives the next trim.
        store.remember_allowed(&[PathBuf::from("/q/20.sql")]);
        store.remember_allowed(&[PathBuf::from("/q/new.sql")]);
        let got = store.allowed_files();
        assert!(got.contains(&PathBuf::from("/q/20.sql")));
        assert!(!got.contains(&PathBuf::from("/q/21.sql")));
    }

    #[test]
    fn a_session_file_without_the_allowed_table_gets_it_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SESSION_TABLE).unwrap();
            conn.pragma_update(None, "user_version", SESSION_VERSION)
                .unwrap();
            conn.execute(
                "INSERT INTO session (id, json, saved_at) VALUES (1, 'kept', 0)",
                [],
            )
            .unwrap();
        }
        let store = SessionStore::open(&path);
        assert_eq!(store.load().as_deref(), Some("kept"));
        store.remember_allowed(&[PathBuf::from("/q/a.sql")]);
        assert_eq!(store.allowed_files(), [PathBuf::from("/q/a.sql")]);
    }

    #[test]
    fn a_big_batch_is_fast_and_keeps_the_last_500_of_it() {
        let store = SessionStore::in_memory();
        let paths: Vec<PathBuf> = (0..2000)
            .map(|i| PathBuf::from(format!("/q/{i}.sql")))
            .collect();
        let start = std::time::Instant::now();
        store.remember_allowed(&paths);
        assert!(start.elapsed() < Duration::from_millis(200));
        let got = store.allowed_files();
        assert_eq!(got.len(), 500);
        assert_eq!(got.first(), Some(&PathBuf::from("/q/1500.sql")));
        assert_eq!(got.last(), Some(&PathBuf::from("/q/1999.sql")));
    }

    #[test]
    fn remembering_nothing_opens_no_transaction() {
        let store = SessionStore::in_memory();
        // Another connection would see a write lock if a transaction were open;
        // here it is enough that nothing changes and nothing fails.
        store.remember_allowed(&[]);
        assert!(store.allowed_files().is_empty());
    }

    #[test]
    fn a_read_only_session_file_without_the_allowed_table_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SESSION_TABLE).unwrap();
            conn.pragma_update(None, "user_version", SESSION_VERSION)
                .unwrap();
            conn.execute(
                "INSERT INTO session (id, json, saved_at) VALUES (1, 'kept', 0)",
                [],
            )
            .unwrap();
        }
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
        // The table cannot be created on a read-only file: opening goes on.
        let store = SessionStore::open(&path);
        assert_eq!(store.load().as_deref(), Some("kept"));
        assert!(store.allowed_files().is_empty());
        store.remember_allowed(&[PathBuf::from("/q/a.sql")]);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let store = SessionStore::open(&path);
        store.save("{}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn existing_file_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        {
            let store = SessionStore::open(&path);
            store.save(r#"{"a":1}"#).unwrap();
        }
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let store = SessionStore::open(&path);
        assert_eq!(mode(&path), 0o600);
        assert_eq!(store.load().as_deref(), Some(r#"{"a":1}"#));
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
        let path = dir.path().join("s.db");
        let store = SessionStore::open(&path);
        store.save("{}").unwrap();
        // The store is still open, so the sidecars exist.
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        assert!(wal.exists());
        std::fs::set_permissions(&wal, std::fs::Permissions::from_mode(0o644)).unwrap();
        let second = SessionStore::open(&path);
        let mode = std::fs::metadata(&wal).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        drop(second);
        drop(store);
    }
}
