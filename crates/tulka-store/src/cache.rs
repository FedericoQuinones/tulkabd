use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection};
use tulka_core::{ColumnInfo, DatabaseSnapshot, DbObject, ObjectKind};

/// Bump when the table layout changes; a mismatching file is discarded and
/// rebuilt (it is only a cache).
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE databases (
    connection_id TEXT NOT NULL,
    name TEXT NOT NULL,
    no_access INTEGER NOT NULL DEFAULT 0,
    indexed_at INTEGER,
    PRIMARY KEY (connection_id, name)
);
CREATE TABLE objects (
    id INTEGER PRIMARY KEY,
    connection_id TEXT NOT NULL,
    database TEXT NOT NULL,
    schema TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL
);
CREATE INDEX objects_by_database ON objects (connection_id, database);
CREATE TABLE columns (
    object_id INTEGER NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    name TEXT NOT NULL,
    type_name TEXT NOT NULL,
    nullable INTEGER NOT NULL,
    primary_key INTEGER NOT NULL,
    default_value TEXT,
    PRIMARY KEY (object_id, ordinal)
);
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedDatabase {
    pub name: String,
    pub no_access: bool,
    pub indexed_at: Option<i64>,
    pub tables: u32,
    pub views: u32,
    pub procedures: u32,
}

/// Metadata cache backed by SQLite. Every call is blocking: async callers
/// should go through `spawn_blocking`. Read methods return empty results and
/// `mark_no_access`/`set_databases`/`forget_connection`/`clear` ignore errors,
/// because losing a cache write is never worth failing the app.
pub struct MetaCache {
    conn: Mutex<Connection>,
    persistent: bool,
}

fn kind_str(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Table => "table",
        ObjectKind::View => "view",
        ObjectKind::Procedure => "procedure",
    }
}

fn kind_from(s: &str) -> ObjectKind {
    match s {
        "view" => ObjectKind::View,
        "procedure" => ObjectKind::Procedure,
        _ => ObjectKind::Table,
    }
}

/// Opens `path` and makes sure it holds a current-version schema. Err means
/// the file is unusable (corrupt, foreign or another version).
fn try_open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    // The first read fails with "not a database" on garbage files.
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
    if version == SCHEMA_VERSION {
        conn.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))?;
    } else if version != 0 || tables != 0 {
        return Err(rusqlite::Error::InvalidQuery);
    } else {
        init(&conn)?;
    }
    configure(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    // `journal_mode` returns a row, so it cannot go through `pragma_update`.
    let _ = conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()));
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

impl MetaCache {
    /// Never fails. A file that cannot be opened, is not a valid database or
    /// has another `user_version` is deleted (with its `-wal`/`-shm`
    /// sidecars) and recreated; if even that fails, falls back to memory
    /// (see [`MetaCache::is_persistent`]).
    pub fn open(path: &Path) -> MetaCache {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = try_open(path).or_else(|_| {
            remove_files(path);
            try_open(path)
        });
        match conn {
            Ok(conn) => MetaCache {
                conn: Mutex::new(conn),
                persistent: true,
            },
            Err(_) => Self::in_memory(),
        }
    }

    /// `<data dir>/tulkabd.db`; falls back to [`MetaCache::in_memory`] when
    /// there is no data dir or the file cannot be used.
    pub fn open_default() -> MetaCache {
        match directories::ProjectDirs::from("dev", "tulkabd", "tulkabd") {
            Some(dirs) => Self::open(&dirs.data_dir().join("tulkabd.db")),
            None => Self::in_memory(),
        }
    }

    pub fn in_memory() -> MetaCache {
        let conn = Connection::open_in_memory().expect("in-memory SQLite is always available");
        init(&conn).expect("schema is valid");
        let _ = conn.pragma_update(None, "foreign_keys", true);
        MetaCache {
            conn: Mutex::new(conn),
            persistent: false,
        }
    }

    /// False for [`MetaCache::in_memory`] and when `open` had to fall back
    /// to memory: nothing will survive a restart.
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces everything cached for one database in a single transaction
    /// and clears its `no_access` flag.
    pub fn replace_database(
        &self,
        connection_id: &str,
        database: &str,
        snap: &DatabaseSnapshot,
        indexed_at: i64,
    ) -> Result<(), String> {
        let mut conn = self.lock();
        let run = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM objects WHERE connection_id = ?1 AND database = ?2",
                params![connection_id, database],
            )?;
            tx.execute(
                "INSERT INTO databases (connection_id, name, no_access, indexed_at)
                 VALUES (?1, ?2, 0, ?3)
                 ON CONFLICT (connection_id, name)
                 DO UPDATE SET no_access = 0, indexed_at = excluded.indexed_at",
                params![connection_id, database, indexed_at],
            )?;
            let mut ids = Vec::with_capacity(snap.objects.len());
            {
                let mut ins = tx.prepare(
                    "INSERT INTO objects (connection_id, database, schema, name, kind)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for o in &snap.objects {
                    ins.execute(params![
                        connection_id,
                        database,
                        o.schema,
                        o.name,
                        kind_str(o.kind)
                    ])?;
                    ids.push(tx.last_insert_rowid());
                }
            }
            {
                let mut ins = tx.prepare(
                    "INSERT INTO columns (object_id, ordinal, name, type_name, nullable, primary_key, default_value)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?;
                for (idx, cols) in &snap.columns {
                    let Some(id) = ids.get(*idx) else { continue };
                    for (i, c) in cols.iter().enumerate() {
                        ins.execute(params![
                            id,
                            i as i64,
                            c.name,
                            c.type_name,
                            c.nullable,
                            c.primary_key,
                            c.default
                        ])?;
                    }
                }
            }
            tx.commit()
        };
        run(&mut conn).map_err(|e| e.to_string())
    }

    /// Flags a database as unreadable for this login and drops whatever was
    /// cached for it.
    pub fn mark_no_access(&self, connection_id: &str, database: &str) {
        let mut conn = self.lock();
        let _ = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM objects WHERE connection_id = ?1 AND database = ?2",
                params![connection_id, database],
            )?;
            tx.execute(
                "INSERT INTO databases (connection_id, name, no_access, indexed_at)
                 VALUES (?1, ?2, 1, NULL)
                 ON CONFLICT (connection_id, name)
                 DO UPDATE SET no_access = 1, indexed_at = NULL",
                params![connection_id, database],
            )?;
            tx.commit()
        })();
    }

    /// Makes the connection's database list match `names`: unknown names are
    /// added, vanished ones are deleted with their objects.
    pub fn set_databases(&self, connection_id: &str, names: &[String]) {
        let mut conn = self.lock();
        let _ = (|| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            let existing: Vec<String> = {
                let mut st = tx.prepare("SELECT name FROM databases WHERE connection_id = ?1")?;
                let rows = st.query_map([connection_id], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for old in existing.iter().filter(|e| !names.contains(e)) {
                tx.execute(
                    "DELETE FROM objects WHERE connection_id = ?1 AND database = ?2",
                    params![connection_id, old],
                )?;
                tx.execute(
                    "DELETE FROM databases WHERE connection_id = ?1 AND name = ?2",
                    params![connection_id, old],
                )?;
            }
            for name in names {
                tx.execute(
                    "INSERT OR IGNORE INTO databases (connection_id, name) VALUES (?1, ?2)",
                    params![connection_id, name],
                )?;
            }
            tx.commit()
        })();
    }

    /// Cached databases of a connection, by name, with object counts read
    /// from the cache (no live connection involved).
    pub fn databases(&self, connection_id: &str) -> Vec<CachedDatabase> {
        let conn = self.lock();
        let q = || -> rusqlite::Result<Vec<CachedDatabase>> {
            let mut st = conn.prepare(
                "SELECT d.name, d.no_access, d.indexed_at,
                    (SELECT count(*) FROM objects o WHERE o.connection_id = d.connection_id AND o.database = d.name AND o.kind = 'table'),
                    (SELECT count(*) FROM objects o WHERE o.connection_id = d.connection_id AND o.database = d.name AND o.kind = 'view'),
                    (SELECT count(*) FROM objects o WHERE o.connection_id = d.connection_id AND o.database = d.name AND o.kind = 'procedure')
                 FROM databases d WHERE d.connection_id = ?1 ORDER BY d.name",
            )?;
            let rows = st.query_map([connection_id], |r| {
                Ok(CachedDatabase {
                    name: r.get(0)?,
                    no_access: r.get(1)?,
                    indexed_at: r.get(2)?,
                    tables: r.get(3)?,
                    views: r.get(4)?,
                    procedures: r.get(5)?,
                })
            })?;
            rows.collect()
        };
        q().unwrap_or_default()
    }

    /// Objects of one kind, ordered by schema then name.
    pub fn objects(&self, connection_id: &str, database: &str, kind: ObjectKind) -> Vec<DbObject> {
        let conn = self.lock();
        let q = || -> rusqlite::Result<Vec<DbObject>> {
            let mut st = conn.prepare(
                "SELECT schema, name FROM objects
                 WHERE connection_id = ?1 AND database = ?2 AND kind = ?3
                 ORDER BY schema, name",
            )?;
            let rows = st.query_map(params![connection_id, database, kind_str(kind)], |r| {
                Ok(DbObject {
                    schema: r.get(0)?,
                    name: r.get(1)?,
                    kind,
                })
            })?;
            rows.collect()
        };
        q().unwrap_or_default()
    }

    /// Columns of one object in their original (ordinal) order.
    pub fn columns(
        &self,
        connection_id: &str,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Vec<ColumnInfo> {
        let conn = self.lock();
        let q = || -> rusqlite::Result<Vec<ColumnInfo>> {
            let mut st = conn.prepare(
                "SELECT c.name, c.type_name, c.nullable, c.primary_key, c.default_value
                 FROM columns c JOIN objects o ON o.id = c.object_id
                 WHERE o.connection_id = ?1 AND o.database = ?2 AND o.schema = ?3 AND o.name = ?4
                 ORDER BY c.ordinal",
            )?;
            let rows = st.query_map(params![connection_id, database, schema, name], |r| {
                Ok(ColumnInfo {
                    name: r.get(0)?,
                    type_name: r.get(1)?,
                    nullable: r.get(2)?,
                    primary_key: r.get(3)?,
                    default: r.get(4)?,
                })
            })?;
            rows.collect()
        };
        q().unwrap_or_default()
    }

    /// Every cached object as `(connection_id, database, object)`, to build
    /// the search index. Insertion order (rowid scan, no sort).
    pub fn all_objects(&self) -> Vec<(String, String, DbObject)> {
        let conn = self.lock();
        let q = || -> rusqlite::Result<Vec<(String, String, DbObject)>> {
            let mut st = conn.prepare(
                "SELECT connection_id, database, schema, name, kind FROM objects ORDER BY id",
            )?;
            let rows = st.query_map([], |r| {
                let kind: String = r.get(4)?;
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    DbObject {
                        schema: r.get(2)?,
                        name: r.get(3)?,
                        kind: kind_from(&kind),
                    },
                ))
            })?;
            rows.collect()
        };
        q().unwrap_or_default()
    }

    pub fn forget_connection(&self, connection_id: &str) {
        let conn = self.lock();
        let _ = conn.execute(
            "DELETE FROM objects WHERE connection_id = ?1",
            [connection_id],
        );
        let _ = conn.execute(
            "DELETE FROM databases WHERE connection_id = ?1",
            [connection_id],
        );
    }

    pub fn clear(&self) {
        let conn = self.lock();
        let _ = conn.execute_batch("DELETE FROM objects; DELETE FROM databases;");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(schema: &str, name: &str, kind: ObjectKind) -> DbObject {
        DbObject {
            schema: schema.into(),
            name: name.into(),
            kind,
        }
    }
    fn col(name: &str, pk: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_name: "int".into(),
            nullable: !pk,
            primary_key: pk,
            default: if pk { None } else { Some("((0))".into()) },
        }
    }
    fn snap() -> DatabaseSnapshot {
        DatabaseSnapshot {
            objects: vec![
                obj("dbo", "Users", ObjectKind::Table),
                obj("dbo", "ActiveUsers", ObjectKind::View),
                obj("dbo", "sp_Purge", ObjectKind::Procedure),
            ],
            columns: vec![
                (0, vec![col("Id", true), col("Age", false)]),
                (1, vec![col("Id", true)]),
            ],
        }
    }

    #[test]
    fn roundtrip() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "db", &snap(), 100).unwrap();
        assert_eq!(
            c.objects("c1", "db", ObjectKind::Table),
            vec![obj("dbo", "Users", ObjectKind::Table)]
        );
        assert_eq!(c.objects("c1", "db", ObjectKind::Procedure).len(), 1);
        assert_eq!(
            c.columns("c1", "db", "dbo", "Users"),
            vec![col("Id", true), col("Age", false)]
        );
        assert!(c.columns("c1", "db", "dbo", "sp_Purge").is_empty());
        assert_eq!(c.all_objects().len(), 3);
    }

    #[test]
    fn replace_does_not_duplicate() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "db", &snap(), 100).unwrap();
        c.replace_database("c1", "db", &snap(), 200).unwrap();
        assert_eq!(c.all_objects().len(), 3);
        assert_eq!(c.databases("c1")[0].indexed_at, Some(200));
        assert_eq!(c.columns("c1", "db", "dbo", "Users").len(), 2);
    }

    #[test]
    fn set_databases_drops_vanished() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "old", &snap(), 1).unwrap();
        c.replace_database("c1", "keep", &snap(), 1).unwrap();
        c.set_databases("c1", &["keep".to_string(), "new".to_string()]);
        let names: Vec<_> = c.databases("c1").into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["keep", "new"]);
        assert_eq!(c.all_objects().len(), 3);
    }

    #[test]
    fn databases_count_by_kind() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "db", &snap(), 5).unwrap();
        let d = &c.databases("c1")[0];
        assert_eq!(
            (d.tables, d.views, d.procedures, d.no_access),
            (1, 1, 1, false)
        );
        assert_eq!(d.indexed_at, Some(5));
    }

    #[test]
    fn no_access_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        {
            let c = MetaCache::open(&p);
            assert!(c.is_persistent());
            c.set_databases("c1", &["secret".to_string()]);
            c.mark_no_access("c1", "secret");
        }
        let c = MetaCache::open(&p);
        let d = &c.databases("c1")[0];
        assert!(d.no_access);
        assert_eq!(d.indexed_at, None);
    }

    #[test]
    fn corrupt_file_is_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        std::fs::write(
            &p,
            b"this is definitely not a sqlite database, just garbage bytes",
        )
        .unwrap();
        let c = MetaCache::open(&p);
        assert!(c.is_persistent());
        assert!(c.all_objects().is_empty());
        c.replace_database("c1", "db", &snap(), 1).unwrap();
        drop(c);
        assert_eq!(MetaCache::open(&p).all_objects().len(), 3);
    }

    #[test]
    fn wrong_user_version_is_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        {
            let c = MetaCache::open(&p);
            c.replace_database("c1", "db", &snap(), 1).unwrap();
        }
        {
            let raw = rusqlite::Connection::open(&p).unwrap();
            raw.pragma_update(None, "user_version", 99).unwrap();
        }
        let c = MetaCache::open(&p);
        assert!(c.all_objects().is_empty());
        let v: i64 = c
            .conn
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn forget_connection_only_that_one() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "db", &snap(), 1).unwrap();
        c.replace_database("c2", "db", &snap(), 1).unwrap();
        c.forget_connection("c1");
        assert!(c.databases("c1").is_empty());
        assert_eq!(c.databases("c2").len(), 1);
        assert_eq!(c.all_objects().len(), 3);
        assert_eq!(c.columns("c2", "db", "dbo", "Users").len(), 2);
        c.clear();
        assert!(c.all_objects().is_empty() && c.databases("c2").is_empty());
    }

    #[test]
    fn no_password_columns() {
        let c = MetaCache::in_memory();
        let conn = c.conn.lock().unwrap();
        let mut st = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .unwrap();
        let tables: Vec<String> = st
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(!tables.is_empty());
        for t in tables {
            let mut st = conn
                .prepare(&format!("PRAGMA table_info(\"{t}\")"))
                .unwrap();
            let cols: Vec<String> = st
                .query_map([], |r| r.get(1))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for c in cols {
                let l = c.to_lowercase();
                assert!(
                    !l.contains("pass") && !l.contains("secret") && !l.contains("pwd"),
                    "{t}.{c}"
                );
            }
        }
    }
}
