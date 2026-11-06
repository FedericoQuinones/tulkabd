//! Streaming file formats for saving a result set: CSV, TSV and JSON.
//!
//! Unlike the copy formats in [`crate::export`], these write row by row to
//! any [`Write`], so a large result never needs a second copy in memory.
//! Numbers always use `.`: a file is for programs and for Excel's own import,
//! not for the clipboard's locale handling.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use crate::export::{header_name, hex};
use crate::{Column, Value};

/// Largest integer a JS double holds exactly (2^53 - 1). Integers beyond it
/// are written to JSON as strings so a reader never rounds them.
const JSON_SAFE_INT: i64 = 9_007_199_254_740_991;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// File format of a saved result.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SaveFormat {
    Csv,
    Tsv,
    Json,
}

impl SaveFormat {
    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            SaveFormat::Csv => "csv",
            SaveFormat::Tsv => "tsv",
            SaveFormat::Json => "json",
        }
    }
}

/// Writes one result set to `out`: [`TableWriter::new`] writes the start of
/// the file, [`TableWriter::write_row`] each row and [`TableWriter::finish`]
/// the end.
///
/// - CSV: RFC 4180 (comma, `\r\n`, fields with a comma, quote or line break
///   in double quotes), UTF-8 with a BOM so Excel reads accents correctly.
/// - TSV: tabs and line breaks inside a value become spaces (there is no
///   quoting in TSV, so they would break the table); no BOM.
/// - JSON: an array with one object per row, keyed by column name.
///
/// In all of them `NULL` is an empty field (CSV, TSV) or `null` (JSON),
/// binary is `0x` + hex. Dates and times are ISO 8601 in JSON and in the
/// server's notation (`2026-10-20 14:03:00`) in CSV and TSV, which Excel parses.
pub struct TableWriter<W: Write> {
    out: W,
    format: SaveFormat,
    /// Each JSON key as written, with its colon (`"name":`), encoded once.
    keys: Vec<Vec<u8>>,
    rows: usize,
}

impl<W: Write> TableWriter<W> {
    /// Starts the file: BOM and header for CSV, header for TSV, `[` for JSON.
    pub fn new(mut out: W, format: SaveFormat, columns: &[Column]) -> io::Result<Self> {
        let keys = unique_names(columns)
            .iter()
            .map(|k| {
                let mut b = serde_json::to_vec(k)?;
                b.push(b':');
                Ok(b)
            })
            .collect::<io::Result<Vec<_>>>()?;
        match format {
            SaveFormat::Csv => {
                out.write_all(UTF8_BOM)?;
                write_delimited_line(&mut out, columns.iter().map(header_name), format)?;
            }
            SaveFormat::Tsv => {
                write_delimited_line(&mut out, columns.iter().map(header_name), format)?;
            }
            SaveFormat::Json => out.write_all(b"[")?,
        }
        Ok(Self {
            out,
            format,
            keys,
            rows: 0,
        })
    }

    pub fn write_row(&mut self, row: &[Value]) -> io::Result<()> {
        match self.format {
            SaveFormat::Csv | SaveFormat::Tsv => {
                write_delimited_line(&mut self.out, row.iter().map(delimited_cell), self.format)?;
            }
            SaveFormat::Json => self.write_json_row(row)?,
        }
        self.rows += 1;
        Ok(())
    }

    /// The underlying writer, for a caller that formats into a buffer and
    /// moves the bytes on between rows.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.out
    }

    /// Ends the file, flushes, and returns the writer and the number of rows
    /// written.
    pub fn finish(mut self) -> io::Result<(W, usize)> {
        if self.format == SaveFormat::Json {
            self.out
                .write_all(if self.rows == 0 { b"]\n" } else { b"\n]\n" })?;
        }
        self.out.flush()?;
        Ok((self.out, self.rows))
    }

    fn write_json_row(&mut self, row: &[Value]) -> io::Result<()> {
        // One object per line keeps the file easy to read and to diff.
        self.out
            .write_all(if self.rows == 0 { b"\n" } else { b",\n" })?;
        self.out.write_all(b"{")?;
        for (i, key) in self.keys.iter().enumerate() {
            if i > 0 {
                self.out.write_all(b",")?;
            }
            self.out.write_all(key)?;
            write_json_value(&mut self.out, row.get(i).unwrap_or(&Value::Null))?;
        }
        self.out.write_all(b"}")
    }
}

/// JSON object keys: the column names, with a repeated or empty name made
/// unique by a numeric suffix (`col`, `col_2`, `col_3`). A suffixed name never
/// collides with a name the query itself produced.
fn unique_names(columns: &[Column]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    columns
        .iter()
        .map(|c| {
            let base = header_name(c);
            let mut name = base.to_string();
            let mut n = 1;
            while !seen.insert(name.clone()) {
                n += 1;
                name = format!("{base}_{n}");
            }
            name
        })
        .collect()
}

fn write_json_value(out: &mut impl Write, v: &Value) -> io::Result<()> {
    match v {
        Value::Null => out.write_all(b"null"),
        Value::Bool(b) => out.write_all(if *b { b"true" } else { b"false" }),
        Value::Int(i) if i.unsigned_abs() <= JSON_SAFE_INT as u64 => write!(out, "{i}"),
        Value::Int(i) => write!(out, "\"{i}\""),
        Value::Float(f) if f.is_finite() => {
            // `Display` for f64 is the shortest text that round-trips and
            // never uses an exponent, so it is always valid JSON.
            if *f == 0.0 {
                out.write_all(b"0")
            } else {
                write!(out, "{f}")
            }
        }
        // JSON has no NaN or infinity.
        Value::Float(f) => json_string(out, &f.to_string()),
        // Exact decimal text: a JSON number would be rounded by most readers.
        Value::Decimal(d) => json_string(out, d),
        Value::Text(t) => json_string(out, t),
        Value::Bytes(b) => json_string(out, &hex(b)),
        Value::Date(s) | Value::Time(s) | Value::Uuid(s) => json_string(out, s),
        Value::DateTime(s) | Value::DateTimeOffset(s) => json_string(out, &iso_datetime(s)),
    }
}

fn json_string(out: &mut impl Write, s: &str) -> io::Result<()> {
    serde_json::to_writer(out, s).map_err(io::Error::from)
}

/// The server's `2026-10-20 14:03:00.123` and `2026-10-20 14:03:00 -03:00`
/// as ISO 8601: `T` between date and time and no space before the offset.
fn iso_datetime(s: &str) -> String {
    let (dt, offset) = match s.rsplit_once(' ') {
        Some((dt, off)) if dt.contains(' ') && off.starts_with(['+', '-']) => (dt, off),
        _ => (s, ""),
    };
    format!("{}{offset}", dt.replacen(' ', "T", 1))
}

/// The text of one value in CSV and TSV.
fn delimited_cell(v: &Value) -> Cow<'_, str> {
    match v {
        Value::Null => Cow::Borrowed(""),
        Value::Bool(b) => Cow::Borrowed(if *b { "1" } else { "0" }),
        Value::Int(i) => Cow::Owned(i.to_string()),
        Value::Float(f) if *f == 0.0 => Cow::Borrowed("0"),
        Value::Float(f) => Cow::Owned(f.to_string()),
        Value::Decimal(d) => Cow::Borrowed(d),
        Value::Text(t) => text_cell(t),
        Value::Bytes(b) => Cow::Owned(hex(b)),
        // The server's own notation, as Copy writes it: Excel parses it.
        // ISO 8601 is only for JSON.
        Value::Date(s)
        | Value::Time(s)
        | Value::DateTime(s)
        | Value::DateTimeOffset(s)
        | Value::Uuid(s) => Cow::Borrowed(s),
    }
}

/// The one place where text from the database is turned into a cell.
///
/// A spreadsheet runs a cell that starts with `=`, `+`, `-` or `@` as a
/// formula. Whether to defuse those (for example by prefixing `'`) is a
/// pending decision, so for now the text is written as it is. Add the
/// prefix here and every text cell of CSV and TSV gets it.
fn text_cell(t: &str) -> Cow<'_, str> {
    Cow::Borrowed(t)
}

fn write_delimited_line<S: AsRef<str>>(
    out: &mut impl Write,
    cells: impl Iterator<Item = S>,
    format: SaveFormat,
) -> io::Result<()> {
    let csv = format == SaveFormat::Csv;
    for (i, cell) in cells.enumerate() {
        if i > 0 {
            out.write_all(if csv { b"," } else { b"\t" })?;
        }
        if csv {
            write_csv_field(out, cell.as_ref())?;
        } else {
            write_tsv_field(out, cell.as_ref())?;
        }
    }
    out.write_all(if csv { b"\r\n" } else { b"\n" })
}

/// RFC 4180: a field with a comma, a quote or a line break goes in double
/// quotes, and its quotes are doubled.
fn write_csv_field(out: &mut impl Write, s: &str) -> io::Result<()> {
    if !s.contains([',', '"', '\r', '\n']) {
        return out.write_all(s.as_bytes());
    }
    out.write_all(b"\"")?;
    for (i, part) in s.split('"').enumerate() {
        if i > 0 {
            out.write_all(b"\"\"")?;
        }
        out.write_all(part.as_bytes())?;
    }
    out.write_all(b"\"")
}

/// TSV has no quoting, so a tab or a line break inside a value would split
/// it into more cells or rows: each one (a `\r\n` counts as one) becomes a
/// space.
fn write_tsv_field(out: &mut impl Write, s: &str) -> io::Result<()> {
    if !s.contains(['\t', '\r', '\n']) {
        return out.write_all(s.as_bytes());
    }
    let mut chars = s.chars().peekable();
    let mut buf = String::with_capacity(s.len());
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\t' | '\r' | '\n' => buf.push(' '),
            c => buf.push(c),
        }
    }
    out.write_all(buf.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(n: &[&str]) -> Vec<Column> {
        n.iter()
            .map(|s| Column {
                name: s.to_string(),
                type_name: "x".into(),
            })
            .collect()
    }

    fn render(format: SaveFormat, columns: &[Column], rows: &[Vec<Value>]) -> Vec<u8> {
        let mut w = TableWriter::new(Vec::new(), format, columns).unwrap();
        for r in rows {
            w.write_row(r).unwrap();
        }
        let (out, n) = w.finish().unwrap();
        assert_eq!(n, rows.len());
        out
    }

    fn text(format: SaveFormat, columns: &[Column], rows: &[Vec<Value>]) -> String {
        String::from_utf8(render(format, columns, rows)).unwrap()
    }

    fn t(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn csv_starts_with_a_bom_and_ends_lines_with_crlf() {
        let out = render(SaveFormat::Csv, &cols(&["a", "b"]), &[vec![t("x"), t("y")]]);
        assert_eq!(out, b"\xEF\xBB\xBFa,b\r\nx,y\r\n");
    }

    #[test]
    fn csv_quotes_commas_quotes_and_line_breaks() {
        let out = text(
            SaveFormat::Csv,
            &cols(&["a,b"]),
            &[
                vec![t("x,y")],
                vec![t("say \"hi\"")],
                vec![t("l1\nl2")],
                vec![t("l1\r\nl2")],
                vec![t("tab\there")],
            ],
        );
        assert_eq!(
            out,
            "\u{feff}\"a,b\"\r\n\"x,y\"\r\n\"say \"\"hi\"\"\"\r\n\"l1\nl2\"\r\n\"l1\r\nl2\"\r\ntab\there\r\n"
        );
    }

    #[test]
    fn csv_null_is_empty_and_empty_text_is_empty_too() {
        let out = text(
            SaveFormat::Csv,
            &cols(&["a", "b", "c"]),
            &[vec![Value::Null, t(""), Value::Int(1)]],
        );
        assert_eq!(out, "\u{feff}a,b,c\r\n,,1\r\n");
    }

    #[test]
    fn csv_values_use_plain_notation() {
        let out = text(
            SaveFormat::Csv,
            &cols(&["a", "b", "c", "d", "e", "f", "g", "h"]),
            &[vec![
                Value::Bool(true),
                Value::Int(i64::MIN),
                Value::Float(-0.0),
                Value::Float(12.5),
                Value::Decimal("-0.50".into()),
                Value::Bytes(vec![0, 0xab]),
                Value::DateTime("2026-10-20 14:03:00.123".into()),
                Value::DateTimeOffset("2026-10-20 14:03:00 -03:00".into()),
            ]],
        );
        assert_eq!(
            out,
            "\u{feff}a,b,c,d,e,f,g,h\r\n1,-9223372036854775808,0,12.5,-0.50,0x00AB,2026-10-20 14:03:00.123,2026-10-20 14:03:00 -03:00\r\n"
        );
    }

    #[test]
    fn unicode_is_written_as_utf8() {
        let out = text(
            SaveFormat::Csv,
            &cols(&["Año"]),
            &[vec![t("Ferretería 😀 日本")]],
        );
        assert_eq!(out, "\u{feff}Año\r\nFerretería 😀 日本\r\n");
        let json = text(SaveFormat::Json, &cols(&["Año"]), &[vec![t("ñ😀")]]);
        assert_eq!(json, "[\n{\"Año\":\"ñ😀\"}\n]\n");
    }

    #[test]
    fn empty_column_name_gets_the_placeholder_header() {
        let out = text(SaveFormat::Tsv, &cols(&["", "b"]), &[]);
        assert_eq!(out, "(no name)\tb\n");
    }

    #[test]
    fn tsv_turns_tabs_and_line_breaks_into_spaces() {
        let out = text(
            SaveFormat::Tsv,
            &cols(&["a\tb", "c"]),
            &[
                vec![t("x\ty\r\nz\nw\rv"), t("\"q\",")],
                vec![Value::Null, Value::Int(2)],
            ],
        );
        assert_eq!(out, "a b\tc\nx y z w v\t\"q\",\n\t2\n");
    }

    #[test]
    fn json_is_an_array_of_objects_with_one_per_line() {
        let out = text(
            SaveFormat::Json,
            &cols(&["id", "name"]),
            &[
                vec![Value::Int(1), t("a")],
                vec![Value::Int(2), Value::Null],
            ],
        );
        assert_eq!(
            out,
            "[\n{\"id\":1,\"name\":\"a\"},\n{\"id\":2,\"name\":null}\n]\n"
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[1]["name"], serde_json::Value::Null);
    }

    #[test]
    fn json_without_rows_is_an_empty_array() {
        assert_eq!(text(SaveFormat::Json, &cols(&["a"]), &[]), "[]\n");
    }

    #[test]
    fn json_makes_column_names_unique() {
        let out = text(
            SaveFormat::Json,
            &cols(&["col", "col", "", "", "col_2", "col"]),
            &[(1..=6).map(Value::Int).collect()],
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let obj = parsed[0].as_object().unwrap();
        assert_eq!(obj.len(), 6);
        assert_eq!(obj["col"], 1);
        assert_eq!(obj["col_2"], 2);
        assert_eq!(obj["(no name)"], 3);
        assert_eq!(obj["(no name)_2"], 4);
        // The query's own `col_2` comes after the generated one and is renamed.
        assert_eq!(obj["col_2_2"], 5);
        assert_eq!(obj["col_3"], 6);
    }

    #[test]
    fn json_keeps_big_numbers_and_decimals_exact() {
        let out = text(
            SaveFormat::Json,
            &cols(&["a", "b", "c", "d", "e", "f", "g"]),
            &[vec![
                Value::Int(JSON_SAFE_INT),
                Value::Int(JSON_SAFE_INT + 1),
                Value::Int(i64::MIN),
                Value::Decimal("12345678901234567890.123456789".into()),
                Value::Decimal("1.50".into()),
                Value::Float(0.1),
                Value::Int(-JSON_SAFE_INT),
            ]],
        );
        assert_eq!(
            out,
            "[\n{\"a\":9007199254740991,\"b\":\"9007199254740992\",\"c\":\"-9223372036854775808\",\"d\":\"12345678901234567890.123456789\",\"e\":\"1.50\",\"f\":0.1,\"g\":-9007199254740991}\n]\n"
        );
    }

    #[test]
    fn json_floats_bools_binary_and_dates() {
        let out = text(
            SaveFormat::Json,
            &cols(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]),
            &[vec![
                Value::Float(1e21),
                Value::Float(-0.0),
                Value::Float(f64::NAN),
                Value::Bool(false),
                Value::Bytes(vec![0x0a, 0xff]),
                Value::Date("2026-10-20".into()),
                Value::DateTime("2026-10-20 14:03:00".into()),
                Value::DateTimeOffset("2026-10-20 14:03:00.5 +05:30".into()),
                Value::Uuid("6F9619FF-8B86-D011-B42D-00C04FC964FF".into()),
            ]],
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let o = &parsed[0];
        assert_eq!(o["a"].as_f64(), Some(1e21));
        assert_eq!(o["b"], 0);
        assert_eq!(o["c"], "NaN");
        assert_eq!(o["d"], false);
        assert_eq!(o["e"], "0x0AFF");
        assert_eq!(o["f"], "2026-10-20");
        assert_eq!(o["g"], "2026-10-20T14:03:00");
        assert_eq!(o["h"], "2026-10-20T14:03:00.5+05:30");
        assert_eq!(o["i"], "6F9619FF-8B86-D011-B42D-00C04FC964FF");
    }

    #[test]
    fn json_escapes_control_characters_and_quotes() {
        let out = text(
            SaveFormat::Json,
            &cols(&["k\"ey"]),
            &[vec![t("a\"b\\c\n\u{1}")]],
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed[0]["k\"ey"], "a\"b\\c\n\u{1}");
    }

    #[test]
    fn a_short_row_is_padded_with_null_in_json() {
        let out = text(SaveFormat::Json, &cols(&["a", "b"]), &[vec![Value::Int(1)]]);
        assert_eq!(out, "[\n{\"a\":1,\"b\":null}\n]\n");
    }

    #[test]
    fn text_is_written_as_it_is() {
        // Pending decision: values starting with = + - @ are not prefixed.
        let out = text(SaveFormat::Csv, &cols(&["a"]), &[vec![t("=1+1")]]);
        assert_eq!(out, "\u{feff}a\r\n=1+1\r\n");
    }

    #[test]
    fn iso_datetime_leaves_unexpected_text_alone() {
        assert_eq!(iso_datetime("2026-10-20 14:03:00"), "2026-10-20T14:03:00");
        assert_eq!(iso_datetime("odd"), "odd");
        assert_eq!(iso_datetime("2026-10-20T14:03:00"), "2026-10-20T14:03:00");
    }
}
