//! Saving a result set to a file.
//!
//! The rows written are the visible ones, in display order: what "Copy"
//! copies for a whole result, so a sort or filter set in the grid applies
//! (see [`crate::query::set_result_view`]).
//!
//! The file is written to a temporary file in the same folder and renamed
//! over the chosen name at the end, so a failure (a full disk, a revoked
//! share) never leaves a half-written file under that name.
//!
//! # Locking
//!
//! The result is read-locked only briefly. Up front, one lock copies the
//! columns and the indexes of the visible rows. Then, for each chunk of rows,
//! the lock is taken again just to format that chunk into a memory buffer
//! and released before the bytes go to the file. Holding the lock across
//! the whole write would let a queued sort or filter (a writer, which the
//! lock serves before later readers) stall every `fetch_rows` of the grid
//! until the save ended. Rows never change once a run ended, only the view
//! does, so the indexes taken up front stay one consistent snapshot even if
//! the view changes meanwhile.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tulka_core::{SaveFormat, TableWriter};

use crate::connections::blocking;
use crate::query::{no_session, result_in};
use crate::state::{AppState, SharedBuffer};
use crate::types::BUSY;

const TEMP_PREFIX: &str = ".tulka-";
const TEMP_SUFFIX: &str = ".part";

/// Rows formatted under one lock.
const CHUNK_ROWS: usize = 2048;

/// Returned by [`save_results`].
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    /// Data rows written (the header is not counted).
    pub rows: usize,
    /// The result itself was cut by the row cap or the memory budget, so
    /// the file holds only the rows that were kept.
    pub truncated: bool,
    /// Some text or binary values were shortened when the result was
    /// received (see `MAX_CELL_BYTES`).
    pub capped: bool,
}

/// Writes the visible rows of a result to `path` in `format`. Fails with
/// `"busy"` while the tab runs a query, since the result may still be
/// receiving rows. `path` must come from the native save dialog, never from
/// the UI. Runs on a blocking thread.
pub async fn save_results(
    state: &AppState,
    tab_id: String,
    result_index: u32,
    format: SaveFormat,
    path: PathBuf,
) -> Result<SaveOutcome, String> {
    let buffer = {
        let tabs = state.tabs();
        let tab = tabs.open.get(&tab_id).ok_or_else(no_session)?;
        if tab.cancel.is_some() {
            return Err(BUSY.into());
        }
        result_in(tab, result_index)?
    };
    blocking(move || write_result(&buffer, format, &path, CHUNK_ROWS, || {})).await?
}

/// The path to write for a name picked in the save dialog. Some systems
/// (GTK) return what was typed without the filter's extension; it is added
/// unless that would name a file that exists, since the dialog only asked
/// about overwriting the bare name.
pub fn resolve_save_path(picked: PathBuf, extension: &str) -> PathBuf {
    if picked.extension().is_some() {
        return picked;
    }
    let mut with = picked.clone().into_os_string();
    with.push(".");
    with.push(extension);
    let with = PathBuf::from(with);
    if with.exists() {
        picked
    } else {
        with
    }
}

/// Streams the visible rows of `buffer` to `path` through a temporary file
/// next to it, `chunk` rows at a time (see the module docs on locking).
/// `between` runs after each chunk with no lock held (tests use it).
fn write_result(
    buffer: &SharedBuffer,
    format: SaveFormat,
    path: &Path,
    chunk: usize,
    mut between: impl FnMut(),
) -> Result<SaveOutcome, String> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if path.file_name().is_none() {
        return Err("the file has no name".into());
    }
    let (columns, indexes, truncated, capped) = {
        let b = buffer.blocking_read();
        (
            b.columns.clone(),
            b.visible_indexes(),
            b.truncated,
            b.capped,
        )
    };
    let tmp = dir.join(format!(
        "{TEMP_PREFIX}{}{TEMP_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> io::Result<usize> {
        let mut file = BufWriter::new(fs::File::create_new(&tmp)?);
        // Formats into memory under the lock; the file is written without it.
        let mut w = TableWriter::new(Vec::new(), format, &columns)?;
        for part in indexes.chunks(chunk.max(1)) {
            {
                let b = buffer.blocking_read();
                for &i in part {
                    w.write_row(b.row(i))?;
                }
            }
            file.write_all(w.get_mut())?;
            w.get_mut().clear();
            between();
        }
        let (rest, rows) = w.finish()?;
        file.write_all(&rest)?;
        let file = file.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(rows)
    })();
    match result {
        Ok(rows) => Ok(SaveOutcome {
            rows,
            truncated,
            capped,
        }),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(format!("{}: {e}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::{ByteBudget, ResultBuffer, SortSpec, ViewSpec};
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tulka_core::{Column, Value};

    fn buffer(rows: &[(i64, &str)]) -> SharedBuffer {
        let columns = ["n", "s"]
            .map(|name| Column {
                name: name.into(),
                type_name: "x".into(),
            })
            .to_vec();
        let mut b = ResultBuffer::new(columns, 1000);
        let rows = rows
            .iter()
            .map(|(n, s)| vec![Value::Int(*n), Value::Text((*s).into())])
            .collect();
        b.push_rows(rows, &mut ByteBudget::default());
        Arc::new(RwLock::new(b))
    }

    fn save(b: &SharedBuffer, format: SaveFormat, path: &Path) -> Result<SaveOutcome, String> {
        write_result(b, format, path, CHUNK_ROWS, || {})
    }

    fn sorted_desc() -> ViewSpec {
        ViewSpec {
            sort: Some(SortSpec {
                column: 0,
                descending: true,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn writes_visible_rows_in_display_order() {
        let b = buffer(&[(1, "a"), (3, "c"), (2, "b")]);
        b.blocking_write().set_view(&sorted_desc());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.tsv");
        let out = save(&b, SaveFormat::Tsv, &path).unwrap();
        assert_eq!(out.rows, 3);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "n\ts\n3\tc\n2\tb\n1\ta\n"
        );
    }

    #[test]
    fn reports_a_cut_result() {
        let b = buffer(&[(1, "a")]);
        {
            let mut g = b.blocking_write();
            g.truncated = true;
            g.capped = true;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = save(&b, SaveFormat::Csv, &dir.path().join("o.csv")).unwrap();
        assert!(out.truncated && out.capped);
    }

    #[test]
    fn a_view_change_during_a_save_is_not_blocked_and_does_not_change_the_file() {
        let rows: Vec<_> = (0..5).map(|i| (i, "x")).collect();
        let b = buffer(&rows);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("o.tsv");
        let mut changed = false;
        // Two rows a chunk; after the first chunk another thread sorts the
        // result. If the save still held the lock, that would never finish.
        let out = write_result(&b, SaveFormat::Tsv, &path, 2, || {
            if changed {
                return;
            }
            changed = true;
            let (tx, rx) = std::sync::mpsc::channel();
            let other = b.clone();
            std::thread::spawn(move || {
                other.blocking_write().set_view(&sorted_desc());
                let _ = tx.send(());
            });
            rx.recv_timeout(std::time::Duration::from_secs(5))
                .expect("the view change waited for the save");
        })
        .unwrap();
        assert_eq!(out.rows, 5);
        // The rows of the snapshot taken when the save began, in its order.
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "n\ts\n0\tx\n1\tx\n2\tx\n3\tx\n4\tx\n"
        );
    }

    #[test]
    fn leaves_no_temporary_file_and_keeps_the_old_file_on_failure() {
        let b = buffer(&[(1, "a")]);
        let dir = tempfile::tempdir().unwrap();
        // The target is a directory, so the final rename fails.
        let path = dir.path().join("taken");
        fs::create_dir(&path).unwrap();
        assert!(save(&b, SaveFormat::Csv, &path).is_err());
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["taken"]);
    }

    #[test]
    fn replaces_an_existing_file() {
        let b = buffer(&[(1, "a")]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json");
        fs::write(&path, "old").unwrap();
        assert_eq!(save(&b, SaveFormat::Json, &path).unwrap().rows, 1);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[\n{\"n\":1,\"s\":\"a\"}\n]\n"
        );
    }

    #[test]
    fn the_extension_is_added_only_when_that_does_not_name_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("report");
        // Typed without an extension and nothing in the way: add it.
        assert_eq!(
            resolve_save_path(bare.clone(), "csv"),
            dir.path().join("report.csv")
        );
        // An extension the user typed is kept, even a different one.
        let txt = dir.path().join("report.txt");
        assert_eq!(resolve_save_path(txt.clone(), "csv"), txt);
        // report.csv exists but the dialog only confirmed `report`: keep it.
        fs::write(dir.path().join("report.csv"), "keep me").unwrap();
        assert_eq!(resolve_save_path(bare.clone(), "csv"), bare);
    }
}
