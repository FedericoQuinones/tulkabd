//! Saved queries: one `.sql` file per query under
//! `<data dir>/queries/<database>/<name>.sql`, and their palette index.
//!
//! # Names on disk
//!
//! Database and query names become file names through
//! [`encode_component`]: the characters Windows forbids (`<>:"/\|?*` and
//! ASCII control characters), `%` itself, a trailing dot or space, and the
//! first letter of a reserved device name (`CON`, `NUL`, `COM1`, ...) are
//! written as `%XX`. Everything else (accents, `ñ`, inner spaces) is kept. So
//! the encoding is reversible, two different names never share a file, and
//! an encoded name is never empty, `.`, `..` or a path with separators:
//! every path stays one directory below the root. [`decode_component`]
//! accepts only the exact output of [`encode_component`]; files with any
//! other name are ignored. Scans of the root (listing, the index load, the
//! temp file sweep) never follow symlinks, so they never leave it. Names are compared exactly (case-sensitive), as
//! far as the file system allows: on a case-insensitive one (Windows,
//! macOS) names that differ only in case share one folder or file, as SQL
//! Server's default collation would, and saving without `overwrite` fails
//! with `"exists"` instead of overwriting the other one.
//!
//! # Writes and the index
//!
//! Writes are atomic (a temporary file in the same directory, then a
//! rename) and serialized by the store. The palette index of the names is
//! built from the disk on a background thread at startup and rebuilt after
//! every save, rename and delete; rebuilds and writes share one lock, so a
//! slower startup load never replaces a newer index. The startup load first
//! removes the temporary files a crash may have left. Errors are plain
//! strings and never contain the query text.

use std::fmt::Write as _;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::UNIX_EPOCH;

use tulka_store::{SavedQueryEntry, SavedQueryIndex};

use crate::connections::blocking;
use crate::state::AppState;
use crate::types::{SavedQuery, SavedQueryHit, EMPTY_NAME, EXISTS, NOT_FOUND};

/// Saved queries returned by one palette search.
pub(crate) const SAVED_QUERY_HITS: usize = 20;

const EXTENSION: &str = ".sql";

/// Temporary files of [`write_atomic`]: `.tulka-<uuid>.tmp`.
const TEMP_PREFIX: &str = ".tulka-";
const TEMP_SUFFIX: &str = ".tmp";

/// Characters Windows forbids in file names, besides control characters.
const FORBIDDEN: &str = "<>:\"/\\|?*";

/// Windows device names, reserved with or without an extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

fn escaped(c: char) -> bool {
    c == '%' || c.is_ascii_control() || FORBIDDEN.contains(c)
}

/// Whether Windows reads `s` as a device: the part before the first dot,
/// without trailing spaces, is a reserved name in any case.
fn is_reserved(s: &str) -> bool {
    let stem = s.split('.').next().unwrap_or("").trim_end_matches(' ');
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// Encodes a database or query name as one file name (see the module docs).
pub fn encode_component(s: &str) -> String {
    let reserved = is_reserved(s);
    let count = s.chars().count();
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        let trailing = i + 1 == count && (c == '.' || c == ' ');
        if escaped(c) || trailing || (i == 0 && reserved) {
            // Only ASCII is ever escaped, so one byte.
            let _ = write!(out, "%{:02X}", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}

/// The name [`encode_component`] turned into `s`; `None` when `s` is not
/// exactly such an output (a bad escape, lowercase hex, an escape that was
/// not needed, a character that should have been escaped, or empty).
pub fn decode_component(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() != 2 || !hex.chars().all(|h| h.is_ascii_hexdigit()) {
                return None;
            }
            let byte = u8::from_str_radix(&hex, 16).ok()?;
            if !byte.is_ascii() {
                return None;
            }
            out.push(byte as char);
        } else {
            out.push(c);
        }
    }
    (!out.is_empty() && encode_component(&out) == s).then_some(out)
}

fn io_error(what: &str, e: std::io::Error) -> String {
    match e.kind() {
        ErrorKind::NotFound => NOT_FOUND.to_string(),
        _ => format!("could not {what} the saved query: {e}"),
    }
}

/// The saved queries on disk. Every call is blocking.
pub struct QueriesStore {
    /// `None` when there is no data directory: nothing is listed and every
    /// write fails.
    root: Option<PathBuf>,
    /// Serializes writes, so "exists" checks hold until the rename.
    write: Mutex<()>,
}

impl QueriesStore {
    /// A store under `root` (created on the first save).
    pub fn at(root: PathBuf) -> Self {
        QueriesStore {
            root: Some(root),
            write: Mutex::new(()),
        }
    }

    /// `<data dir>/queries`; a store that saves nothing when there is no
    /// data directory.
    pub fn open_default() -> Self {
        match directories::ProjectDirs::from("dev", "tulkabd", "tulkabd") {
            Some(dirs) => Self::at(dirs.data_dir().join("queries")),
            None => Self::unavailable(),
        }
    }

    /// A store without a directory: lists nothing, saves nothing.
    pub(crate) fn unavailable() -> Self {
        QueriesStore {
            root: None,
            write: Mutex::new(()),
        }
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.write.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn root_or_err(&self) -> Result<&Path, String> {
        self.root
            .as_deref()
            .ok_or_else(|| "saved queries are unavailable: there is no data directory".into())
    }

    /// The file of query `name` of `database`, existing or not. Fails with
    /// `"empty_name"` on an empty database or a blank query name.
    pub fn path_of(&self, database: &str, name: &str) -> Result<PathBuf, String> {
        if database.is_empty() || name.trim().is_empty() {
            return Err(EMPTY_NAME.into());
        }
        let root = self.root_or_err()?;
        Ok(root
            .join(encode_component(database))
            .join(encode_component(name) + EXTENSION))
    }

    /// The queries of `database`, or of every database, sorted by database
    /// and then name (case-insensitive). Badly named files are skipped.
    pub fn list(&self, database: Option<&str>) -> Vec<SavedQuery> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match database {
            Some("") => {}
            Some(db) => {
                let dir = root.join(encode_component(db));
                // Not through a symlink: it could point out of the root.
                if fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                    list_dir(db, &dir, &mut out);
                }
            }
            None => {
                for entry in database_dirs(root) {
                    let db = entry.file_name().to_str().and_then(decode_component);
                    if let Some(db) = db {
                        list_dir(&db, &entry.path(), &mut out);
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            a.database
                .cmp(&b.database)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// The text of a saved query (`"not_found"` if there is none).
    pub fn read(&self, database: &str, name: &str) -> Result<String, String> {
        let path = self.path_of(database, name)?;
        let bytes = fs::read(&path).map_err(|e| io_error("read", e))?;
        let text = String::from_utf8(bytes)
            .map_err(|_| "could not read the saved query: it is not UTF-8 text".to_string())?;
        Ok(match text.strip_prefix('\u{feff}') {
            Some(rest) => rest.to_string(),
            None => text,
        })
    }

    /// Writes `text` as query `name` of `database`. Fails with `"exists"`
    /// when it exists and `overwrite` is false.
    pub fn save(
        &self,
        database: &str,
        name: &str,
        text: &str,
        overwrite: bool,
    ) -> Result<(), String> {
        let path = self.path_of(database, name)?;
        let _guard = self.lock();
        if !overwrite && path.exists() {
            return Err(EXISTS.into());
        }
        let dir = path.parent().expect("a saved query is inside its database");
        fs::create_dir_all(dir).map_err(|e| io_error("save", e))?;
        write_atomic(dir, &path, text.as_bytes()).map_err(|e| io_error("save", e))
    }

    /// Renames query `name` of `database` to `new_name`. Fails with
    /// `"not_found"` when it does not exist and with `"exists"` when
    /// `new_name` does.
    pub fn rename(&self, database: &str, name: &str, new_name: &str) -> Result<(), String> {
        let from = self.path_of(database, name)?;
        let to = self.path_of(database, new_name)?;
        let _guard = self.lock();
        if !from.is_file() {
            return Err(NOT_FOUND.into());
        }
        if from == to {
            return Ok(());
        }
        if to.exists() && !is_case_alias(&from, &to) {
            return Err(EXISTS.into());
        }
        fs::rename(&from, &to).map_err(|e| io_error("rename", e))
    }

    /// Removes the temporary files a crash left in the database folders
    /// (never listed, but they would pile up). Call it before any write.
    fn sweep_temp_files(&self) {
        let Some(root) = &self.root else {
            return;
        };
        let _guard = self.lock();
        for db in database_dirs(root) {
            for entry in fs::read_dir(db.path()).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let stale = name
                    .to_str()
                    .is_some_and(|n| n.starts_with(TEMP_PREFIX) && n.ends_with(TEMP_SUFFIX));
                if stale && is_regular_file(&entry) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    /// Deletes query `name` of `database` (`"not_found"` if there is none),
    /// and its database folder once empty.
    pub fn delete(&self, database: &str, name: &str) -> Result<(), String> {
        let path = self.path_of(database, name)?;
        let _guard = self.lock();
        if !path.is_file() {
            return Err(NOT_FOUND.into());
        }
        fs::remove_file(&path).map_err(|e| io_error("delete", e))?;
        if let Some(dir) = path.parent() {
            // Fails, harmlessly, while other queries remain.
            let _ = fs::remove_dir(dir);
        }
        Ok(())
    }
}

/// The real folders directly under `root`. Symlinks (and junctions) are
/// skipped, so no scan ever leaves the root.
fn database_dirs(root: &Path) -> impl Iterator<Item = fs::DirEntry> {
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
}

/// A regular file, not a symlink to one.
fn is_regular_file(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|t| t.is_file())
}

/// Adds the `.sql` files of `dir` as queries of `database`; symlinks are
/// skipped.
fn list_dir(database: &str, dir: &Path, out: &mut Vec<SavedQuery>) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name
            .to_str()
            .and_then(|f| f.strip_suffix(EXTENSION))
            .and_then(decode_component)
        else {
            continue;
        };
        if !is_regular_file(&entry) {
            continue;
        }
        // Does not follow links, and `entry` is a regular file.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as i64);
        out.push(SavedQuery {
            database: database.to_string(),
            name,
            modified_ms,
        });
    }
}

/// Writes `bytes` to `path` through a temporary file in `dir`, so `path`
/// always holds either the old or the new text. The temporary name does
/// not end in `.sql`, so a leftover one is never listed.
fn write_atomic(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = dir.join(format!(
        "{TEMP_PREFIX}{}{TEMP_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = fs::File::create_new(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// On a case-insensitive file system (Windows, macOS), `to` "exists" when
/// it differs from `from` only in case. Then it is `from` itself, unless the
/// directory really holds an entry spelled exactly like `to`.
fn is_case_alias(from: &Path, to: &Path) -> bool {
    let (Some(f), Some(t)) = (from.file_name(), to.file_name()) else {
        return false;
    };
    if f.to_string_lossy().to_lowercase() != t.to_string_lossy().to_lowercase() {
        return false;
    }
    let Some(dir) = to.parent() else {
        return false;
    };
    !fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name() == t)
}

/// The store and its palette index, shared by the commands.
pub(crate) struct SavedQueries {
    store: QueriesStore,
    index: RwLock<SavedQueryIndex>,
    /// Held across a write and the index rebuild that follows it, and by
    /// the startup load, so the index always reflects the latest write.
    reload: Mutex<()>,
}

impl SavedQueries {
    /// Without a directory: nothing is saved or found.
    pub(crate) fn unavailable() -> Arc<Self> {
        Self::new(QueriesStore::unavailable())
    }

    fn new(store: QueriesStore) -> Arc<Self> {
        Arc::new(SavedQueries {
            store,
            index: RwLock::new(SavedQueryIndex::default()),
            reload: Mutex::new(()),
        })
    }

    /// Over `store`, loading the index on a background thread: returns at
    /// once, and the palette finds no saved query until it is loaded.
    pub(crate) fn start(store: QueriesStore) -> Arc<Self> {
        let queries = Self::new(store);
        let loader = queries.clone();
        // Without a thread the index stays empty until the first write.
        let _ = std::thread::Builder::new()
            .name("tulka-queries-load".into())
            .spawn(move || {
                // Before any write: every write holds the reload lock.
                let _guard = loader.reload_lock();
                loader.store.sweep_temp_files();
                loader.rebuild();
            });
        queries
    }

    fn reload_lock(&self) -> MutexGuard<'_, ()> {
        self.reload.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn rebuild(&self) {
        let entries = self
            .store
            .list(None)
            .into_iter()
            .map(|q| SavedQueryEntry {
                database: q.database,
                name: q.name,
            })
            .collect();
        let built = SavedQueryIndex::build(entries);
        *self.index.write().unwrap_or_else(PoisonError::into_inner) = built;
    }

    /// Runs a write on the store, then rebuilds the index.
    fn write<T>(&self, f: impl FnOnce(&QueriesStore) -> Result<T, String>) -> Result<T, String> {
        let _guard = self.reload_lock();
        let result = f(&self.store);
        self.rebuild();
        result
    }

    /// The palette's matches among the saved query names.
    pub(crate) fn search(&self, query: &str) -> Vec<SavedQueryHit> {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .search(query, SAVED_QUERY_HITS)
            .into_iter()
            .map(|m| SavedQueryHit {
                database: m.database,
                name: m.name,
                indices: m.indices,
            })
            .collect()
    }
}

/// The saved queries of `database`, or of every database.
pub async fn list_saved_queries(state: &AppState, database: Option<String>) -> Vec<SavedQuery> {
    let q = state.queries.clone();
    blocking(move || q.store.list(database.as_deref()))
        .await
        .unwrap_or_default()
}

/// The text of a saved query; `"not_found"` when there is none.
pub async fn read_saved_query(
    state: &AppState,
    database: String,
    name: String,
) -> Result<String, String> {
    let q = state.queries.clone();
    blocking(move || q.store.read(&database, &name)).await?
}

/// Saves `text` as query `name` of `database`: `"exists"` when it exists
/// and `overwrite` is false, `"empty_name"` on a blank name.
pub async fn save_query(
    state: &AppState,
    database: String,
    name: String,
    text: String,
    overwrite: bool,
) -> Result<(), String> {
    let q = state.queries.clone();
    blocking(move || q.write(|s| s.save(&database, &name, &text, overwrite))).await?
}

/// Renames a saved query: `"not_found"`, `"exists"` when `new_name` is
/// taken, `"empty_name"` on a blank name.
pub async fn rename_saved_query(
    state: &AppState,
    database: String,
    name: String,
    new_name: String,
) -> Result<(), String> {
    let q = state.queries.clone();
    blocking(move || q.write(|s| s.rename(&database, &name, &new_name))).await?
}

/// Deletes a saved query; `"not_found"` when there is none.
pub async fn delete_saved_query(
    state: &AppState,
    database: String,
    name: String,
) -> Result<(), String> {
    let q = state.queries.clone();
    blocking(move || q.write(|s| s.delete(&database, &name))).await?
}

/// The file of an existing saved query (to reveal it in the file manager);
/// `"not_found"` when there is none.
pub async fn saved_query_path(
    state: &AppState,
    database: String,
    name: String,
) -> Result<PathBuf, String> {
    let q = state.queries.clone();
    blocking(move || {
        let path = q.store.path_of(&database, &name)?;
        if path.is_file() {
            Ok(path)
        } else {
            Err(NOT_FOUND.to_string())
        }
    })
    .await?
}
