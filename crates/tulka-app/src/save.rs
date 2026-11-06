//! Saving a result set to a file.
//!
//! The rows written are the visible ones, in display order: what "Copy"
//! copies for a whole result, so a sort or filter set in the grid applies
//! (see [`crate::query::set_result_view`]).
//!
//! The file is written to a temporary file in the same folder and renamed
//! over the chosen name at the end, so a failure (a full disk, a revoked
//! share) never leaves a half-written file under that name. Rows are
//! streamed from the stored result to a buffered writer; the result is only
//! borrowed, never copied. The result stays read-locked meanwhile, so the
//! file is one consistent view; sorting or filtering the same result waits
//! for the save to end.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tulka_core::{SaveFormat, TableWriter};

use crate::connections::blocking;
use crate::query::{no_session, result_in};
use crate::results::ResultBuffer;
use crate::state::AppState;
use crate::types::BUSY;

const TEMP_PREFIX: &str = ".tulka-";
const TEMP_SUFFIX: &str = ".part";

/// Returned by [`save_results`].
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    /// Data rows written (the header is not counted).
    pub rows: usize,
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
    blocking(move || {
        let buffer = buffer.blocking_read();
        write_result(&buffer, format, &path).map(|rows| SaveOutcome { rows })
    })
    .await?
}

/// Streams `buffer`'s visible rows to `path` through a temporary file next
/// to it; returns the rows written.
fn write_result(buffer: &ResultBuffer, format: SaveFormat, path: &Path) -> Result<usize, String> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    if path.file_name().is_none() {
        return Err("the file has no name".into());
    }
    let tmp = dir.join(format!(
        "{TEMP_PREFIX}{}{TEMP_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> std::io::Result<usize> {
        let file = fs::File::create_new(&tmp)?;
        let mut w = TableWriter::new(BufWriter::new(file), format, &buffer.columns)?;
        for row in buffer.visible_rows() {
            w.write_row(row)?;
        }
        let (out, rows) = w.finish()?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(rows)
    })();
    match result {
        Ok(rows) => Ok(rows),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(format!("Could not save {}: {e}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::{ByteBudget, SortSpec, ViewSpec};
    use tulka_core::{Column, Value};

    fn buffer(rows: &[(i64, &str)]) -> ResultBuffer {
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
        b
    }

    #[test]
    fn writes_visible_rows_in_display_order() {
        let mut b = buffer(&[(1, "a"), (3, "c"), (2, "b")]);
        b.set_view(&ViewSpec {
            sort: Some(SortSpec {
                column: 0,
                descending: true,
            }),
            ..Default::default()
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.tsv");
        assert_eq!(write_result(&b, SaveFormat::Tsv, &path), Ok(3));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "n\ts\n3\tc\n2\tb\n1\ta\n"
        );
    }

    #[test]
    fn leaves_no_temporary_file_and_keeps_the_old_file_on_failure() {
        let b = buffer(&[(1, "a")]);
        let dir = tempfile::tempdir().unwrap();
        // The target is a directory, so the final rename fails.
        let path = dir.path().join("taken");
        fs::create_dir(&path).unwrap();
        let err = write_result(&b, SaveFormat::Csv, &path).unwrap_err();
        assert!(err.starts_with("Could not save"), "{err}");
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
        assert_eq!(write_result(&b, SaveFormat::Json, &path), Ok(1));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[\n{\"n\":1,\"s\":\"a\"}\n]\n"
        );
    }
}
