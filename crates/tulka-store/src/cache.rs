use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::config::DbConfig;
use rusqlite::{params, params_from_iter, Connection, ErrorCode, OpenFlags};
use serde::Serialize;
use tulka_core::{ColumnInfo, DatabaseSnapshot, DbObject, ObjectKind};

use crate::private_file;

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

/// How long opening the cache waits on a file locked by another process.
const BUSY_TIMEOUT: Duration = Duration::from_millis(200);

/// How long a read-only open waits on a busy file.
const READ_BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// `find_objects` looks at this many matches at most.
const MAX_FOUND: usize = 5000;

/// A database of a connection as cached. Serialized camelCase:
/// `{"name", "noAccess", "indexedAt", "tables", "views", "procedures"}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedDatabase {
    pub name: String,
    pub no_access: bool,
    /// When it was last indexed, as given to `replace_database` (tulka-app
    /// writes Unix milliseconds); `None` until then or when `no_access`.
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
    // Created before SQLite opens it, so it never exists with the default umask.
    let _ = private_file::create_private(path);
    private_file::restrict_permissions(path);
    let conn = Connection::open(path)?;
    // A file locked by another process costs startup at most this long.
    conn.busy_timeout(BUSY_TIMEOUT)?;
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
    // The WAL sidecars exist only once the journal mode is set.
    private_file::restrict_permissions(path);
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

/// True when the file itself is bad: not a database, corrupt, or a layout
/// that is not the current one (`try_open` reports that as `InvalidQuery`, a
/// missing table as a generic SQL error).
fn is_unusable(e: &rusqlite::Error) -> bool {
    match e {
        rusqlite::Error::InvalidQuery => true,
        _ => matches!(
            e.sqlite_error_code(),
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt | ErrorCode::Unknown)
        ),
    }
}

/// Escapes `%`, `_` and `\` so a term matches only itself under `LIKE`.
fn like_pattern(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 2);
    out.push('%');
    for c in term.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// What [`MetaCache::find_objects`] looks for.
pub struct ObjectQuery<'a> {
    /// Connections to search; none means no results.
    pub connection_ids: &'a [String],
    pub database: Option<&'a str>,
    /// Every term must match (substring, ignoring ASCII case). Already split
    /// on whitespace and non-empty.
    pub terms: &'a [String],
    pub kind: Option<ObjectKind>,
    /// Match column names instead of object names.
    pub in_columns: bool,
    /// At most this many hits are returned.
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectMatch {
    pub connection_id: String,
    pub database: String,
    pub object: DbObject,
    /// The matching column when searching in columns.
    pub column: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectMatches {
    pub hits: Vec<ObjectMatch>,
    /// Matches found, at most 5000.
    pub total: usize,
    /// More than 5000 matched: `total` is a floor and the best hits may be
    /// missing.
    pub capped: bool,
}

impl MetaCache {
    /// Never fails. A file that is not a valid database, is corrupt or has
    /// another `user_version` is deleted (with its `-wal`/`-shm` sidecars)
    /// and recreated. A file that cannot be opened right now (busy, locked,
    /// unreadable, an I/O or sharing error) is kept untouched. If no file can
    /// be used, falls back to memory (see [`MetaCache::is_persistent`]).
    pub fn open(path: &Path) -> MetaCache {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = try_open(path).or_else(|e| {
            // Only a file known to be damaged or of another layout is
            // replaced; anything else may be a healthy file in use.
            if !is_unusable(&e) {
                return Err(e);
            }
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

    /// `<data dir>/tulkabd.db`, if the platform has a data dir.
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "tulkabd", "tulkabd")
            .map(|dirs| dirs.data_dir().join("tulkabd.db"))
    }

    /// The default file; falls back to [`MetaCache::in_memory`] when there is
    /// no data dir or the file cannot be used.
    pub fn open_default() -> MetaCache {
        match Self::default_path() {
            Some(path) => {
                if let Some(dir) = path.parent() {
                    private_file::ensure_private_dir(dir);
                }
                Self::open(&path)
            }
            None => Self::in_memory(),
        }
    }

    /// Opens an existing cache for reading only (`PRAGMA query_only`), waiting
    /// up to 1 s on a busy file. `None` if it is missing, unreadable or of
    /// another schema version. Never creates, deletes or rewrites the file.
    pub fn open_existing(path: &Path) -> Option<MetaCache> {
        // Read-write without `CREATE`: a WAL database needs write access to
        // its `-shm` even to be read, and a missing file stays missing.
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;
        conn.busy_timeout(READ_BUSY_TIMEOUT).ok()?;
        // The app owns the file: closing this reader must not checkpoint the
        // WAL into it.
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .ok()?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .ok()?;
        if version != SCHEMA_VERSION {
            return None;
        }
        conn.query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))
            .ok()?;
        conn.pragma_update(None, "query_only", true).ok()?;
        private_file::restrict_permissions(path);
        Some(MetaCache {
            conn: Mutex::new(conn),
            persistent: true,
        })
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

    /// Column names of every table and view of one database, as
    /// `(schema, name, columns)` ordered by schema and name, columns in
    /// ordinal order. One query, for autocompletion.
    pub fn column_names(
        &self,
        connection_id: &str,
        database: &str,
    ) -> Vec<(String, String, Vec<String>)> {
        let conn = self.lock();
        let q = || -> rusqlite::Result<Vec<(String, String, Vec<String>)>> {
            let mut st = conn.prepare(
                "SELECT o.schema, o.name, c.name
                 FROM columns c JOIN objects o ON o.id = c.object_id
                 WHERE o.connection_id = ?1 AND o.database = ?2
                 ORDER BY o.schema, o.name, o.id, c.ordinal",
            )?;
            let mut rows = st.query(params![connection_id, database])?;
            let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
            while let Some(r) = rows.next()? {
                let (schema, name, column): (String, String, String) =
                    (r.get(0)?, r.get(1)?, r.get(2)?);
                match out.last_mut() {
                    Some(last) if last.0 == schema && last.1 == name => last.2.push(column),
                    _ => out.push((schema, name, vec![column])),
                }
            }
            Ok(out)
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

    /// Objects (or, with `in_columns`, columns) whose name contains every
    /// term. Looks at up to 5000 matches and ranks them: a name equal to the
    /// whole query first, then one starting with the first term, then the
    /// rest; ties go to the shorter name, then alphabetical order. Read
    /// errors yield no matches.
    pub fn find_objects(&self, q: &ObjectQuery<'_>) -> ObjectMatches {
        if q.connection_ids.is_empty() || q.terms.is_empty() {
            return ObjectMatches::default();
        }
        let mut args: Vec<String> = q.connection_ids.to_vec();
        let (select, name_expr) = if q.in_columns {
            (
                "SELECT o.connection_id, o.database, o.schema, o.name, o.kind, c.name
                 FROM columns c JOIN objects o ON o.id = c.object_id",
                "c.name",
            )
        } else {
            (
                "SELECT connection_id, database, schema, name, kind, NULL FROM objects o",
                "(o.schema || '.' || o.name)",
            )
        };
        let marks = vec!["?"; args.len()].join(",");
        let mut sql = format!("{select} WHERE o.connection_id IN ({marks})");
        if let Some(db) = q.database {
            sql.push_str(" AND o.database = ?");
            args.push(db.to_string());
        }
        if let Some(kind) = q.kind {
            sql.push_str(" AND o.kind = ?");
            args.push(kind_str(kind).to_string());
        }
        for term in q.terms {
            sql.push_str(&format!(" AND {name_expr} LIKE ? ESCAPE '\\'"));
            args.push(like_pattern(term));
        }
        sql.push_str(&format!(" LIMIT {}", MAX_FOUND + 1));

        let conn = self.lock();
        let found = || -> rusqlite::Result<Vec<ObjectMatch>> {
            let mut st = conn.prepare(&sql)?;
            let rows = st.query_map(params_from_iter(&args), |r| {
                let kind: String = r.get(4)?;
                Ok(ObjectMatch {
                    connection_id: r.get(0)?,
                    database: r.get(1)?,
                    object: DbObject {
                        schema: r.get(2)?,
                        name: r.get(3)?,
                        kind: kind_from(&kind),
                    },
                    column: r.get(5)?,
                })
            })?;
            rows.collect()
        };
        let Ok(mut hits) = found() else {
            return ObjectMatches::default();
        };
        drop(conn);
        let capped = hits.len() > MAX_FOUND;
        hits.truncate(MAX_FOUND);
        let total = hits.len();

        let whole = q.terms.join(" ").to_lowercase();
        let first = q.terms[0].to_lowercase();
        let ranked_name = |h: &ObjectMatch| -> String {
            h.column.as_deref().unwrap_or(&h.object.name).to_lowercase()
        };
        let mut keyed: Vec<(u8, usize, String, ObjectMatch)> = hits
            .into_iter()
            .map(|h| {
                let name = ranked_name(&h);
                let rank = if name == whole {
                    0
                } else if name.starts_with(&first) {
                    1
                } else {
                    2
                };
                (rank, name.chars().count(), name, h)
            })
            .collect();
        keyed.sort_by(|a, b| {
            (a.0, a.1, &a.2, &a.3.object.schema, &a.3.object.name)
                .cmp(&(b.0, b.1, &b.2, &b.3.object.schema, &b.3.object.name))
                .then_with(|| {
                    (&a.3.connection_id, &a.3.database).cmp(&(&b.3.connection_id, &b.3.database))
                })
        });
        ObjectMatches {
            hits: keyed.into_iter().take(q.limit).map(|k| k.3).collect(),
            total,
            capped,
        }
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

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let c = MetaCache::open(&path);
        c.replace_database("c1", "db", &snap(), 1).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn existing_file_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let c = MetaCache::open(&path);
            c.replace_database("c1", "db", &snap(), 1).unwrap();
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let c = MetaCache::open(&path);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!c.all_objects().is_empty());
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
    fn column_names_of_a_database() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "db", &snap(), 1).unwrap();
        c.replace_database("c1", "other", &snap(), 1).unwrap();
        assert_eq!(
            c.column_names("c1", "db"),
            vec![
                (
                    "dbo".to_string(),
                    "ActiveUsers".to_string(),
                    vec!["Id".to_string()]
                ),
                (
                    "dbo".to_string(),
                    "Users".to_string(),
                    vec!["Id".to_string(), "Age".to_string()]
                ),
            ]
        );
        assert!(c.column_names("c2", "db").is_empty());
    }

    #[test]
    fn cached_database_json_shape() {
        let d = CachedDatabase {
            name: "db".into(),
            no_access: true,
            indexed_at: None,
            tables: 1,
            views: 2,
            procedures: 3,
        };
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            serde_json::json!({"name": "db", "noAccess": true, "indexedAt": null, "tables": 1, "views": 2, "procedures": 3})
        );
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

    fn persisted(path: &Path) {
        let c = MetaCache::open(path);
        c.replace_database("c1", "db", &snap(), 1).unwrap();
    }

    #[test]
    fn open_existing_missing_is_none_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        assert!(MetaCache::open_existing(&p).is_none());
        assert!(!p.exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn open_existing_other_version_is_none_and_keeps_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        persisted(&p);
        {
            let raw = rusqlite::Connection::open(&p).unwrap();
            raw.pragma_update(None, "user_version", 99).unwrap();
        }
        let before = std::fs::read(&p).unwrap();
        assert!(MetaCache::open_existing(&p).is_none());
        assert_eq!(std::fs::read(&p).unwrap(), before);
        // An empty file is not a cache either, and stays empty.
        let empty = dir.path().join("empty.db");
        std::fs::write(&empty, b"").unwrap();
        assert!(MetaCache::open_existing(&empty).is_none());
        assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
    }

    #[test]
    fn open_existing_is_query_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        persisted(&p);
        let ro = MetaCache::open_existing(&p).unwrap();
        assert_eq!(ro.all_objects().len(), 3);
        assert!(ro.replace_database("c1", "db", &snap(), 2).is_err());
        ro.forget_connection("c1");
        ro.clear();
        drop(ro);
        assert_eq!(MetaCache::open(&p).all_objects().len(), 3);
    }

    #[test]
    fn open_existing_while_a_writer_holds_the_file_does_not_delete_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        persisted(&p);
        let writer = rusqlite::Connection::open(&p).unwrap();
        writer
            .execute_batch("BEGIN IMMEDIATE; DELETE FROM columns")
            .unwrap();
        let ro = MetaCache::open_existing(&p).expect("WAL readers are not blocked");
        // The reader sees the committed state, not the writer's pending one.
        assert_eq!(ro.columns("c1", "db", "dbo", "Users").len(), 2);
        assert_eq!(ro.all_objects().len(), 3);
        writer.execute_batch("ROLLBACK").unwrap();
        drop(ro);
        drop(writer);
        assert!(p.exists());
        assert_eq!(MetaCache::open(&p).all_objects().len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn open_keeps_an_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        persisted(&p);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&p).is_ok() {
            // Running as root: permissions do not stop reads, nothing to test.
            return;
        }
        let c = MetaCache::open(&p);
        assert!(!c.is_persistent());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(MetaCache::open(&p).all_objects().len(), 3);
    }

    #[test]
    fn open_keeps_a_file_another_process_holds_busy() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.db");
        persisted(&p);
        let other = rusqlite::Connection::open(&p).unwrap();
        other
            .pragma_update(None, "locking_mode", "EXCLUSIVE")
            .unwrap();
        other.execute_batch("BEGIN EXCLUSIVE").unwrap();
        let start = std::time::Instant::now();
        let c = MetaCache::open(&p);
        // Gives up quickly, falls back to memory and does not delete.
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(!c.is_persistent());
        other.execute_batch("ROLLBACK").unwrap();
        drop(other);
        assert_eq!(MetaCache::open(&p).all_objects().len(), 3);
    }

    fn terms(q: &str) -> Vec<String> {
        q.split_whitespace().map(String::from).collect()
    }

    fn find(c: &MetaCache, conns: &[&str], q: &str) -> ObjectMatches {
        let ids: Vec<String> = conns.iter().map(|s| s.to_string()).collect();
        c.find_objects(&ObjectQuery {
            connection_ids: &ids,
            database: None,
            terms: &terms(q),
            kind: None,
            in_columns: false,
            limit: 50,
        })
    }

    fn names(m: &ObjectMatches) -> Vec<String> {
        m.hits.iter().map(|h| h.object.name.clone()).collect()
    }

    fn named(c: &MetaCache, conn: &str, db: &str, objs: &[(&str, &str, ObjectKind)]) {
        let snap = DatabaseSnapshot {
            objects: objs.iter().map(|(s, n, k)| obj(s, n, *k)).collect(),
            columns: vec![],
        };
        c.replace_database(conn, db, &snap, 1).unwrap();
    }

    #[test]
    fn find_objects_matches_every_term_case_insensitively() {
        let c = MetaCache::in_memory();
        named(
            &c,
            "c1",
            "db",
            &[
                ("dbo", "PedidosCliente", ObjectKind::Table),
                ("dbo", "Pedidos", ObjectKind::Table),
                ("ventas", "Clientes", ObjectKind::Table),
            ],
        );
        let m = find(&c, &["c1"], "PEDIDOS cliente");
        assert_eq!(names(&m), ["PedidosCliente"]);
        assert_eq!((m.total, m.capped), (1, false));
        // Terms may also hit the schema part.
        assert_eq!(names(&find(&c, &["c1"], "ventas clientes")), ["Clientes"]);
        assert!(find(&c, &["c1"], "pedidos zzz").hits.is_empty());
        assert!(find(&c, &[], "pedidos").hits.is_empty());
    }

    #[test]
    fn find_objects_ranks_exact_then_prefix_then_shorter() {
        let c = MetaCache::in_memory();
        named(
            &c,
            "c1",
            "db",
            &[
                ("dbo", "OldPedido", ObjectKind::Table),
                ("dbo", "PedidoDetalle", ObjectKind::Table),
                ("dbo", "PedidoX", ObjectKind::Table),
                ("dbo", "Pedido", ObjectKind::View),
                ("dbo", "PedidoB", ObjectKind::Table),
                ("dbo", "PedidoA", ObjectKind::Table),
            ],
        );
        assert_eq!(
            names(&find(&c, &["c1"], "pedido")),
            [
                "Pedido",
                "PedidoA",
                "PedidoB",
                "PedidoX",
                "PedidoDetalle",
                "OldPedido"
            ]
        );
        let limited = c.find_objects(&ObjectQuery {
            connection_ids: &["c1".to_string()],
            database: None,
            terms: &terms("pedido"),
            kind: None,
            in_columns: false,
            limit: 2,
        });
        assert_eq!(names(&limited), ["Pedido", "PedidoA"]);
        assert_eq!(limited.total, 6);
    }

    #[test]
    fn find_objects_respects_connections_database_and_kind() {
        let c = MetaCache::in_memory();
        named(&c, "c1", "a", &[("dbo", "Pedidos", ObjectKind::Table)]);
        named(&c, "c1", "b", &[("dbo", "PedidosV", ObjectKind::View)]);
        named(&c, "c2", "a", &[("dbo", "PedidosOtro", ObjectKind::Table)]);
        assert_eq!(find(&c, &["c1"], "pedidos").total, 2);
        assert_eq!(find(&c, &["c1", "c2"], "pedidos").total, 3);
        let ids = ["c1".to_string(), "c2".to_string()];
        let q = |database, kind| {
            c.find_objects(&ObjectQuery {
                connection_ids: &ids,
                database,
                terms: &terms("pedidos"),
                kind,
                in_columns: false,
                limit: 50,
            })
        };
        let m = q(Some("a"), None);
        assert_eq!(m.hits.len(), 2);
        assert!(m.hits.iter().all(|h| h.database == "a"));
        let m = q(None, Some(ObjectKind::View));
        assert_eq!(names(&m), ["PedidosV"]);
        assert_eq!(m.hits[0].connection_id, "c1");
        assert_eq!(m.hits[0].database, "b");
        assert!(q(Some("b"), Some(ObjectKind::Table)).hits.is_empty());
    }

    #[test]
    fn find_objects_in_columns_returns_table_and_column() {
        let c = MetaCache::in_memory();
        c.replace_database("c1", "db", &snap(), 1).unwrap();
        let ids = ["c1".to_string()];
        let m = c.find_objects(&ObjectQuery {
            connection_ids: &ids,
            database: None,
            terms: &terms("age"),
            kind: None,
            in_columns: true,
            limit: 50,
        });
        assert_eq!(m.hits.len(), 1);
        assert_eq!(m.hits[0].object.name, "Users");
        assert_eq!(m.hits[0].column.as_deref(), Some("Age"));
        // The object name is not searched in column mode.
        let m = c.find_objects(&ObjectQuery {
            connection_ids: &ids,
            database: None,
            terms: &terms("users"),
            kind: None,
            in_columns: true,
            limit: 50,
        });
        assert!(m.hits.is_empty());
        // Object search leaves `column` empty.
        assert_eq!(find(&c, &["c1"], "users").hits[0].column, None);
    }

    #[test]
    fn find_objects_escapes_like_wildcards() {
        let c = MetaCache::in_memory();
        named(
            &c,
            "c1",
            "db",
            &[
                ("dbo", "a_b", ObjectKind::Table),
                ("dbo", "axb", ObjectKind::Table),
                ("dbo", "100%", ObjectKind::Table),
                ("dbo", "1000", ObjectKind::Table),
                ("dbo", "a\\b", ObjectKind::Table),
            ],
        );
        assert_eq!(names(&find(&c, &["c1"], "a_b")), ["a_b"]);
        assert_eq!(names(&find(&c, &["c1"], "100%")), ["100%"]);
        assert_eq!(names(&find(&c, &["c1"], "a\\b")), ["a\\b"]);
    }

    #[test]
    fn find_objects_caps_at_5000_matches() {
        let c = MetaCache::in_memory();
        let snap = DatabaseSnapshot {
            objects: (0..5200)
                .map(|i| obj("dbo", &format!("t{i}"), ObjectKind::Table))
                .collect(),
            columns: vec![],
        };
        c.replace_database("c1", "db", &snap, 1).unwrap();
        let m = find(&c, &["c1"], "t");
        assert_eq!((m.total, m.capped, m.hits.len()), (5000, true, 50));
    }
}
