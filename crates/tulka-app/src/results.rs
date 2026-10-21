//! Per-result row buffers kept on the Rust side while the UI fetches windows.

use serde::{Deserialize, Serialize};
use tulka_core::{Column, Value};

/// Most rows kept per result set; the rest of the stream is drained and
/// dropped, and the buffer is flagged as truncated.
pub const MAX_ROWS: usize = 100_000;

/// Most bytes kept per text or binary cell; longer values are shortened when
/// stored (text at a character boundary).
pub const MAX_CELL_BYTES: usize = 1024 * 1024;

/// Text cells longer than this many characters are shortened (plus `…`) in
/// the rows sent to the grid.
pub const DISPLAY_TEXT_CHARS: usize = 1000;

/// Binary cells longer than this many bytes are sent to the grid with one
/// byte more than this, so it shows the first `DISPLAY_BYTES` and an `…`.
pub const DISPLAY_BYTES: usize = 32;

/// The warning shown once per run when [`MAX_CELL_BYTES`] shortened a value.
pub const CELLS_CAPPED_WARNING: &str = "Some values over 1 MB were shortened.";

/// The rows of one result set, capped at `cap` rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResultBuffer {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub cap: usize,
    /// Set when rows beyond `cap` were dropped.
    pub truncated: bool,
}

/// Part of a result to copy. Ranges are inclusive and clamped to the result.
///
/// Serialized internally tagged: `{"kind": "all"}`,
/// `{"kind": "rows", "from": 0, "to": 9}`,
/// `{"kind": "cells", "rows": [0, 1], "cols": [1, 2]}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Selection {
    All,
    Rows {
        from: usize,
        to: usize,
    },
    Cells {
        rows: (usize, usize),
        cols: (usize, usize),
    },
}

impl ResultBuffer {
    pub fn new(columns: Vec<Column>, cap: usize) -> Self {
        Self {
            columns,
            rows: Vec::new(),
            cap,
            truncated: false,
        }
    }

    /// Appends rows up to `cap`; any excess is dropped and flags `truncated`.
    /// Text and binary cells of the kept rows are capped at
    /// [`MAX_CELL_BYTES`]; returns whether any was.
    pub fn push_rows(&mut self, mut rows: Vec<Vec<Value>>) -> bool {
        let room = self.cap.saturating_sub(self.rows.len());
        if rows.len() > room {
            rows.truncate(room);
            self.truncated = true;
        }
        let mut capped = false;
        for cell in rows.iter_mut().flatten() {
            capped |= cap_cell(cell, MAX_CELL_BYTES);
        }
        self.rows.append(&mut rows);
        capped
    }

    /// Up to `limit` rows starting at `offset`; empty past the end.
    pub fn window(&self, offset: usize, limit: usize) -> &[Vec<Value>] {
        let start = offset.min(self.rows.len());
        let end = start.saturating_add(limit).min(self.rows.len());
        &self.rows[start..end]
    }

    /// The columns and rows covered by `sel`, cloned.
    pub fn select(&self, sel: &Selection) -> (Vec<Column>, Vec<Vec<Value>>) {
        self.with_selection(sel, |columns, rows| (columns.to_vec(), rows.to_vec()))
    }

    /// Calls `f` with the columns and rows covered by `sel`. Whole rows are
    /// borrowed; only a cell selection copies its part of each row.
    pub fn with_selection<R>(
        &self,
        sel: &Selection,
        f: impl FnOnce(&[Column], &[Vec<Value>]) -> R,
    ) -> R {
        match sel {
            Selection::All => f(&self.columns, &self.rows),
            Selection::Rows { from, to } => f(&self.columns, clamp(&self.rows, *from, *to)),
            Selection::Cells { rows, cols } => {
                let cells: Vec<Vec<Value>> = clamp(&self.rows, rows.0, rows.1)
                    .iter()
                    .map(|row| clamp(row, cols.0, cols.1).to_vec())
                    .collect();
                f(clamp(&self.columns, cols.0, cols.1), &cells)
            }
        }
    }
}

/// Shortens a text cell to at most `max` bytes (at a character boundary) or
/// a binary cell to its first `max` bytes. Returns whether it was shortened.
pub fn cap_cell(value: &mut Value, max: usize) -> bool {
    match value {
        Value::Text(text) if text.len() > max => {
            let mut end = max;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.shrink_to_fit();
            true
        }
        Value::Bytes(bytes) if bytes.len() > max => {
            bytes.truncate(max);
            bytes.shrink_to_fit();
            true
        }
        _ => false,
    }
}

/// The form of a cell sent to the grid: text over [`DISPLAY_TEXT_CHARS`]
/// characters becomes its first `DISPLAY_TEXT_CHARS` plus `…`, and binary
/// over [`DISPLAY_BYTES`] bytes its first `DISPLAY_BYTES + 1` bytes (the grid
/// shows `DISPLAY_BYTES` and marks the rest with `…`). Copying uses the
/// stored value, not this one.
pub fn display_cell(value: &Value) -> Value {
    match value {
        Value::Text(text) => match text.char_indices().nth(DISPLAY_TEXT_CHARS) {
            Some((end, _)) => Value::Text(format!("{}…", &text[..end])),
            None => value.clone(),
        },
        Value::Bytes(bytes) if bytes.len() > DISPLAY_BYTES => {
            Value::Bytes(bytes[..=DISPLAY_BYTES].to_vec())
        }
        _ => value.clone(),
    }
}

/// The inclusive range `from..=to` of `items`, clamped; empty when inverted
/// or out of range.
fn clamp<T>(items: &[T], from: usize, to: usize) -> &[T] {
    let end = to.saturating_add(1).min(items.len());
    let start = from.min(end);
    &items[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str) -> Column {
        Column {
            name: name.into(),
            type_name: "int".into(),
        }
    }

    fn grid(rows: usize, cols: usize) -> ResultBuffer {
        let mut b = ResultBuffer::new((0..cols).map(|c| col(&format!("c{c}"))).collect(), 100);
        b.push_rows(
            (0..rows)
                .map(|r| (0..cols).map(|c| Value::Int((r * 10 + c) as i64)).collect())
                .collect(),
        );
        b
    }

    #[test]
    fn caps_rows_and_flags_truncated() {
        let mut b = ResultBuffer::new(vec![], 100_000);
        b.push_rows(vec![vec![Value::Int(1)]; 60_000]);
        b.push_rows(vec![vec![Value::Int(1)]; 60_000]);
        assert_eq!(b.rows.len(), 100_000);
        assert!(b.truncated);
    }

    #[test]
    fn window_clamps() {
        let b = grid(10, 1);
        assert_eq!(b.window(8, 5).len(), 2);
        assert_eq!(b.window(8, 5)[0], vec![Value::Int(80)]);
        assert_eq!(b.window(20, 5).len(), 0);
    }

    #[test]
    fn select_cells_subsets_columns() {
        let b = grid(3, 3);
        let (columns, rows) = b.select(&Selection::Cells {
            rows: (0, 1),
            cols: (1, 2),
        });
        assert_eq!(columns, vec![col("c1"), col("c2")]);
        assert_eq!(
            rows,
            vec![
                vec![Value::Int(1), Value::Int(2)],
                vec![Value::Int(11), Value::Int(12)],
            ]
        );
    }

    #[test]
    fn select_all_and_rows_clamp() {
        let b = grid(3, 2);
        assert_eq!(
            b.select(&Selection::All),
            (b.columns.clone(), b.rows.clone())
        );
        let (columns, rows) = b.select(&Selection::Rows { from: 1, to: 99 });
        assert_eq!(columns.len(), 2);
        assert_eq!(rows, b.rows[1..].to_vec());
        assert!(b.select(&Selection::Rows { from: 2, to: 1 }).1.is_empty());
        assert!(b.select(&Selection::Rows { from: 5, to: 9 }).1.is_empty());
    }

    #[test]
    fn exact_cap_is_not_truncated() {
        let mut b = ResultBuffer::new(vec![], 3);
        b.push_rows(vec![vec![Value::Null]; 3]);
        assert!(!b.truncated);
        b.push_rows(vec![]);
        assert!(!b.truncated);
        b.push_rows(vec![vec![Value::Null]]);
        assert!(b.truncated);
        assert_eq!(b.rows.len(), 3);
    }

    #[test]
    fn cap_cell_cuts_text_at_a_char_boundary() {
        // "é" is two bytes: a 5-byte cap falls inside the third one.
        let mut v = Value::Text("ééé".into());
        assert!(cap_cell(&mut v, 5));
        assert_eq!(v, Value::Text("éé".into()));
        let mut v = Value::Text("abcd".into());
        assert!(cap_cell(&mut v, 3));
        assert_eq!(v, Value::Text("abc".into()));
        // At the limit nothing changes.
        let mut v = Value::Text("abc".into());
        assert!(!cap_cell(&mut v, 3));
        assert_eq!(v, Value::Text("abc".into()));
    }

    #[test]
    fn cap_cell_keeps_the_first_bytes() {
        let mut v = Value::Bytes(vec![1, 2, 3, 4]);
        assert!(cap_cell(&mut v, 2));
        assert_eq!(v, Value::Bytes(vec![1, 2]));
        let mut v = Value::Bytes(vec![1, 2]);
        assert!(!cap_cell(&mut v, 2));
        let mut v = Value::Int(7);
        assert!(!cap_cell(&mut v, 0));
        assert_eq!(v, Value::Int(7));
    }

    #[test]
    fn push_rows_caps_large_cells_of_kept_rows_only() {
        let big = "x".repeat(MAX_CELL_BYTES + 10);
        let mut b = ResultBuffer::new(vec![], 1);
        assert!(!b.push_rows(vec![vec![Value::Text("small".into())]]));
        // The row beyond the cap is dropped, so nothing is reported capped.
        assert!(!b.push_rows(vec![vec![Value::Text(big.clone())]]));
        let mut b = ResultBuffer::new(vec![], 10);
        assert!(b.push_rows(vec![vec![
            Value::Text(big),
            Value::Bytes(vec![0; MAX_CELL_BYTES + 1]),
        ]]));
        assert_eq!(
            b.rows[0],
            vec![
                Value::Text("x".repeat(MAX_CELL_BYTES)),
                Value::Bytes(vec![0; MAX_CELL_BYTES]),
            ]
        );
    }

    #[test]
    fn display_cell_trims_long_text_by_characters() {
        let at_limit = "é".repeat(DISPLAY_TEXT_CHARS);
        assert_eq!(
            display_cell(&Value::Text(at_limit.clone())),
            Value::Text(at_limit.clone())
        );
        let over = format!("{at_limit}ab");
        assert_eq!(
            display_cell(&Value::Text(over)),
            Value::Text(format!("{at_limit}…"))
        );
    }

    #[test]
    fn display_cell_sends_one_byte_over_the_shown_ones() {
        let at_limit: Vec<u8> = (0..DISPLAY_BYTES as u8).collect();
        assert_eq!(
            display_cell(&Value::Bytes(at_limit.clone())),
            Value::Bytes(at_limit)
        );
        let long: Vec<u8> = (0..200).collect();
        assert_eq!(
            display_cell(&Value::Bytes(long.clone())),
            Value::Bytes(long[..=DISPLAY_BYTES].to_vec())
        );
    }

    #[test]
    fn display_cell_keeps_other_values() {
        for v in [
            Value::Null,
            Value::Int(1),
            Value::Decimal("1.50".into()),
            Value::Text(String::new()),
        ] {
            assert_eq!(display_cell(&v), v);
        }
    }

    #[test]
    fn selection_json_shape() {
        use serde_json::json;
        let cases = [
            (Selection::All, json!({"kind": "all"})),
            (
                Selection::Rows { from: 0, to: 9 },
                json!({"kind": "rows", "from": 0, "to": 9}),
            ),
            (
                Selection::Cells {
                    rows: (0, 1),
                    cols: (1, 2),
                },
                json!({"kind": "cells", "rows": [0, 1], "cols": [1, 2]}),
            ),
        ];
        for (sel, expected) in cases {
            assert_eq!(serde_json::to_value(&sel).unwrap(), expected);
            assert_eq!(serde_json::from_value::<Selection>(expected).unwrap(), sel);
        }
    }
}
