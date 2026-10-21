//! Text formats for copying result sets: Excel-friendly TSV and the compact
//! "Copy for AI" format.

use std::fmt::Write;

use crate::{Column, Value};

/// Field separator of the "Copy for AI" format. Fields containing it are
/// quoted, so this is the only place that decides both.
const AI_SEPARATOR: char = '|';

/// Longest binary value, in bytes, printed in full by [`to_ai`].
const AI_MAX_BYTES: usize = 32;

/// Tab-separated text for pasting into Excel: `\r\n` line endings, numbers
/// with `decimal` as the decimal separator and no thousands separator.
pub fn to_excel_tsv(columns: &[Column], rows: &[Vec<Value>], decimal: char) -> String {
    let mut out = String::new();
    write_line(
        &mut out,
        columns.iter().map(|c| quote_excel(&c.name)),
        '\t',
        "\r\n",
    );
    for row in rows {
        write_line(
            &mut out,
            row.iter().map(|v| excel_cell(v, decimal)),
            '\t',
            "\r\n",
        );
    }
    out
}

/// Compact text for pasting into an LLM prompt: headers once, `\n` line
/// endings, numbers with `.` and no trailing zeros.
pub fn to_ai(columns: &[Column], rows: &[Vec<Value>]) -> String {
    let mut out = String::new();
    write_line(
        &mut out,
        columns.iter().map(|c| quote_ai(&c.name)),
        AI_SEPARATOR,
        "\n",
    );
    for row in rows {
        write_line(&mut out, row.iter().map(ai_cell), AI_SEPARATOR, "\n");
    }
    out
}

/// Rough token estimate: `ceil(chars / 3.6)`.
pub fn estimate_tokens(text: &str) -> usize {
    // ceil(n / 3.6) = ceil(10n / 36), kept in integers to avoid float error.
    (text.chars().count() * 10).div_ceil(36)
}

fn write_line(out: &mut String, cells: impl Iterator<Item = String>, sep: char, eol: &str) {
    for (i, cell) in cells.enumerate() {
        if i > 0 {
            out.push(sep);
        }
        out.push_str(&cell);
    }
    out.push_str(eol);
}

fn excel_cell(v: &Value, decimal: char) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => bool_digit(*b),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => with_decimal(&f.to_string(), decimal),
        Value::Decimal(d) => with_decimal(d, decimal),
        Value::Text(t) => quote_excel(t),
        Value::Bytes(b) => hex(b),
        Value::Date(s)
        | Value::Time(s)
        | Value::DateTime(s)
        | Value::DateTimeOffset(s)
        | Value::Uuid(s) => s.clone(),
    }
}

fn ai_cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => bool_digit(*b),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => {
            // `Display` for f64 never uses an exponent and is the shortest
            // text that round-trips, so 3.0 prints as `3`.
            if *f == 0.0 {
                "0".to_string()
            } else {
                f.to_string()
            }
        }
        Value::Decimal(d) => trim_decimal(d),
        Value::Text(t) if t.is_empty() => "\"\"".to_string(),
        Value::Text(t) => quote_ai(t),
        Value::Bytes(b) if b.len() > AI_MAX_BYTES => format!("{}…", hex(&b[..AI_MAX_BYTES])),
        Value::Bytes(b) => hex(b),
        Value::Date(s)
        | Value::Time(s)
        | Value::DateTime(s)
        | Value::DateTimeOffset(s)
        | Value::Uuid(s) => s.clone(),
    }
}

fn bool_digit(b: bool) -> String {
    if b { "1" } else { "0" }.to_string()
}

/// `0x` followed by upper-case hex.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        let _ = write!(s, "{b:02X}");
    }
    s
}

/// Replaces the `.` of a number's text with `decimal`.
fn with_decimal(num: &str, decimal: char) -> String {
    if decimal == '.' {
        num.to_string()
    } else {
        num.replace('.', &decimal.to_string())
    }
}

/// Drops trailing fractional zeros (and a dangling `.`) from exact decimal
/// text, keeping the remaining digits untouched. `-0.00` becomes `0`.
fn trim_decimal(d: &str) -> String {
    if !d.contains('.') {
        return normalize_zero(d);
    }
    let t = d.trim_end_matches('0').trim_end_matches('.');
    normalize_zero(t)
}

fn normalize_zero(s: &str) -> String {
    match s.strip_prefix('-') {
        Some(rest) if rest.chars().all(|c| c == '0') => "0".to_string(),
        _ if s.is_empty() => "0".to_string(),
        _ => s.to_string(),
    }
}

fn quote_excel(t: &str) -> String {
    quote_if(t, |c| matches!(c, '\t' | '\n' | '\r' | '"'))
}

fn quote_ai(t: &str) -> String {
    quote_if(t, |c| c == AI_SEPARATOR || matches!(c, '\n' | '\r' | '"'))
}

/// Wraps `t` in double quotes (doubling inner ones) if any char matches.
fn quote_if(t: &str, needs_quotes: impl Fn(char) -> bool) -> String {
    if t.chars().any(needs_quotes) {
        format!("\"{}\"", t.replace('"', "\"\""))
    } else {
        t.to_string()
    }
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

    #[test]
    fn excel_tsv_spanish_locale() {
        let out = to_excel_tsv(
            &cols(&["Nombre", "Total", "Email"]),
            &[vec![
                Value::Text("Ferretería López".into()),
                Value::Decimal("8912450.50".into()),
                Value::Null,
            ]],
            ',',
        );
        assert_eq!(
            out,
            "Nombre\tTotal\tEmail\r\nFerretería López\t8912450,50\t\r\n"
        );
    }

    #[test]
    fn excel_tsv_quotes_tabs_newlines_and_quotes() {
        let out = to_excel_tsv(
            &cols(&["a"]),
            &[vec![Value::Text("x\ty\n\"z\"".into())]],
            '.',
        );
        assert_eq!(out, "a\r\n\"x\ty\n\"\"z\"\"\"\r\n");
    }

    #[test]
    fn ai_format_compact() {
        let out = to_ai(
            &cols(&["Nombre", "Email", "Pedidos", "Total"]),
            &[
                vec![
                    Value::Text("Super Avenida".into()),
                    Value::Null,
                    Value::Int(97),
                    Value::Decimal("6104880.00".into()),
                ],
                vec![
                    Value::Text("A|B".into()),
                    Value::Text("".into()),
                    Value::Int(1),
                    Value::Decimal("12.5000".into()),
                ],
            ],
        );
        assert_eq!(
            out,
            "Nombre|Email|Pedidos|Total\nSuper Avenida||97|6104880\n\"A|B\"|\"\"|1|12.5\n"
        );
    }

    #[test]
    fn ai_bytes_truncated() {
        let out = to_ai(&cols(&["b"]), &[vec![Value::Bytes(vec![0xab; 40])]]);
        assert_eq!(out, format!("b\n0x{}…\n", "AB".repeat(32)));
        let out = to_ai(&cols(&["b"]), &[vec![Value::Bytes(vec![0x0a, 0xff])]]);
        assert_eq!(out, "b\n0x0AFF\n");
    }

    #[test]
    fn ai_decimals_keep_exact_digits() {
        let one = |d: &str| to_ai(&cols(&["d"]), &[vec![Value::Decimal(d.into())]]);
        assert_eq!(one("-0.50"), "d\n-0.5\n");
        assert_eq!(one("100.00"), "d\n100\n");
        assert_eq!(one("0.00"), "d\n0\n");
        assert_eq!(one("-0.00"), "d\n0\n");
        assert_eq!(one("100"), "d\n100\n");
        assert_eq!(
            one("12345678901234567890.123456789"),
            "d\n12345678901234567890.123456789\n"
        );
    }

    #[test]
    fn floats_ai_and_excel() {
        let ai = |f: f64| to_ai(&cols(&["f"]), &[vec![Value::Float(f)]]);
        assert_eq!(ai(12.5), "f\n12.5\n");
        assert_eq!(ai(3.0), "f\n3\n");
        assert_eq!(ai(-0.0), "f\n0\n");
        assert_eq!(ai(1e21), format!("f\n1{}\n", "0".repeat(21)));
        assert_eq!(ai(f64::NAN), "f\nNaN\n");
        assert_eq!(ai(f64::INFINITY), "f\ninf\n");
        let xl = to_excel_tsv(&cols(&["f"]), &[vec![Value::Float(12.5)]], ',');
        assert_eq!(xl, "f\r\n12,5\r\n");
    }

    #[test]
    fn excel_misc_values() {
        let out = to_excel_tsv(
            &cols(&["a", "b", "c", "d", "e", "f"]),
            &[vec![
                Value::Bool(true),
                Value::Bool(false),
                Value::Int(-1234567),
                Value::Bytes(vec![0, 0xab]),
                Value::Date("2026-10-20".into()),
                Value::Decimal("-0.50".into()),
            ]],
            ',',
        );
        assert_eq!(
            out,
            "a\tb\tc\td\te\tf\r\n1\t0\t-1234567\t0x00AB\t2026-10-20\t-0,50\r\n"
        );
    }

    #[test]
    fn ai_quotes_headers_and_text() {
        let out = to_ai(
            &cols(&["a|b"]),
            &[
                vec![Value::Text("x\ny".into())],
                vec![Value::Text("say \"hi\"".into())],
            ],
        );
        assert_eq!(out, "\"a|b\"\n\"x\ny\"\n\"say \"\"hi\"\"\"\n");
    }

    #[test]
    fn empty_result_is_header_only() {
        assert_eq!(to_ai(&cols(&["a", "b"]), &[]), "a|b\n");
        assert_eq!(to_excel_tsv(&cols(&["a", "b"]), &[], '.'), "a\tb\r\n");
    }

    #[test]
    fn token_estimate() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcdefgh"), 3);
        assert_eq!(estimate_tokens(&"a".repeat(18)), 5);
        assert_eq!(estimate_tokens(&"é".repeat(9)), 3);
    }
}
