use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

/// Bump when the table layout changes; a mismatching file is discarded.
const SESSION_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE session (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    json TEXT NOT NULL,
    saved_at INTEGER NOT NULL
);
";

/// The workspace session (open tabs, layout, editor text) in its own SQLite
/// file, separate from the metadata cache. The JSON is opaque to the backend:
/// its format belongs to the UI. It holds editor text, never passwords, and is
/// never logged. Every call is blocking.
pub struct SessionStore {
    conn: Mutex<Connection>,
}

fn try_open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // The first read fails with "not a database" on garbage files.
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version == SESSION_VERSION {
        conn.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))?;
    } else if version != 0 || tables != 0 {
        return Err(rusqlite::Error::InvalidQuery);
    } else {
        init(&conn)?;
    }
    // `journal_mode` returns a row, so it cannot go through `pragma_update`.
    let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
    Ok(conn)
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
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
    /// Never fails. A file that cannot be opened, is not a valid database or
    /// has another `user_version` is deleted (with its `-wal`/`-shm`
    /// sidecars) and starts empty; if even that fails, falls back to memory.
    pub fn open(path: &Path) -> SessionStore {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = try_open(path).or_else(|_| {
            remove_files(path);
            try_open(path)
        });
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
}
