//! Per-result row buffers kept on the Rust side while the UI fetches windows,
//! and their sorted and filtered views.

use std::borrow::Cow;
use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use tulka_core::{Column, Value};

/// Most rows kept per result set; the rest of the stream is drained and
/// dropped, and the buffer is flagged as truncated.
pub const MAX_ROWS: usize = tulka_core::MAX_ROWS_DEFAULT as usize;

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
    /// Indexes into `rows` of the visible rows, in display order, set by
    /// [`ResultBuffer::set_view`]; `None` shows every row in arrival order.
    view: Option<Vec<u32>>,
}

/// How a received result is shown: filtered, then sorted. Every filter must
/// hold (they combine with AND). An empty spec shows the rows as received.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ViewSpec {
    #[serde(default)]
    pub sort: Option<SortSpec>,
    /// Keeps rows with a cell whose text, as the grid shows it, contains this
    /// (ignoring case). Empty means no text filter.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub columns: Vec<ColumnFilter>,
}

/// A stable sort on one column; see [`compare_values`].
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub column: usize,
    pub descending: bool,
}

/// A filter on one column.
///
/// Serialized internally tagged: `{"kind": "isNull", "column": 0}`,
/// `{"kind": "notNull", "column": 0}`,
/// `{"kind": "equals", "column": 0, "text": "x"}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ColumnFilter {
    IsNull {
        column: usize,
    },
    NotNull {
        column: usize,
    },
    /// The cell's text is exactly `text` (case-sensitive). Numbers match
    /// with or without thousands separators, long text and binary also as
    /// the grid shortens them (ending in `…`); `NULL` never matches.
    Equals {
        column: usize,
        text: String,
    },
}

impl ColumnFilter {
    pub fn column(&self) -> usize {
        match self {
            Self::IsNull { column } | Self::NotNull { column } | Self::Equals { column, .. } => {
                *column
            }
        }
    }

    fn matches(&self, row: &[Value]) -> bool {
        let cell = cell_at(row, self.column());
        match self {
            Self::IsNull { .. } => matches!(cell, Value::Null),
            Self::NotNull { .. } => !matches!(cell, Value::Null),
            Self::Equals { text, .. } => cell_equals(cell, text),
        }
    }
}

impl ViewSpec {
    /// The columns this spec refers to.
    pub fn columns_used(&self) -> impl Iterator<Item = usize> + '_ {
        self.sort
            .map(|s| s.column)
            .into_iter()
            .chain(self.columns.iter().map(ColumnFilter::column))
    }
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
            view: None,
        }
    }

    /// Filters and sorts the rows received so far for display; an empty
    /// `spec` goes back to every row in arrival order. Cells of columns the
    /// result does not have count as `NULL`.
    pub fn set_view(&mut self, spec: &ViewSpec) {
        let needle = spec
            .text
            .as_deref()
            .map(str::to_lowercase)
            .filter(|n| !n.is_empty());
        if spec.sort.is_none() && needle.is_none() && spec.columns.is_empty() {
            self.view = None;
            return;
        }
        let rows = &self.rows;
        // `rows.len()` is at most `cap` (100,000), well within u32.
        let mut view: Vec<u32> = (0..rows.len() as u32)
            .filter(|&i| {
                let row = &rows[i as usize];
                spec.columns.iter().all(|f| f.matches(row))
                    && needle
                        .as_deref()
                        .is_none_or(|n| row.iter().any(|v| cell_matches(v, n)))
            })
            .collect();
        if let Some(SortSpec { column, descending }) = spec.sort {
            // Sorting cells side by side, with decimals and offsets parsed
            // and text lower-cased once, is much faster than looking up and
            // parsing on every comparison. Two parsed keys of one variant
            // compare as `compare_values` does.
            let mut keyed: Vec<(&Value, Option<Parsed>, u32)> = view
                .iter()
                .map(|&i| {
                    let cell = cell_at(&rows[i as usize], column);
                    (cell, Parsed::of(cell), i)
                })
                .collect();
            keyed.sort_by(|a, b| {
                let order = match (&a.1, &b.1) {
                    (Some(Parsed::Dec(x)), Some(Parsed::Dec(y))) => x.cmp(y),
                    (Some(Parsed::Instant(x)), Some(Parsed::Instant(y))) => x.cmp(y),
                    (Some(Parsed::Folded(x)), Some(Parsed::Folded(y))) => x.cmp(y),
                    _ => compare_values(a.0, b.0),
                };
                if descending {
                    order.reverse()
                } else {
                    order
                }
            });
            view = keyed.into_iter().map(|(_, _, i)| i).collect();
        }
        self.view = Some(view);
    }

    /// Whether a view (sort or filter) is set.
    pub fn has_view(&self) -> bool {
        self.view.is_some()
    }

    /// How many rows the view shows.
    pub fn visible_len(&self) -> usize {
        self.view.as_ref().map_or(self.rows.len(), Vec::len)
    }

    /// The visible rows `start..end` (already clamped), in display order.
    fn visible(&self, start: usize, end: usize) -> Vec<&[Value]> {
        match &self.view {
            None => self.rows[start..end].iter().map(Vec::as_slice).collect(),
            Some(view) => view[start..end]
                .iter()
                .map(|&i| self.rows[i as usize].as_slice())
                .collect(),
        }
    }

    /// The visible rows in the inclusive range `from..=to`, clamped.
    fn visible_between(&self, from: usize, to: usize) -> Vec<&[Value]> {
        let end = to.saturating_add(1).min(self.visible_len());
        self.visible(from.min(end), end)
    }

    /// Appends rows up to `cap`; any excess is dropped and flags `truncated`.
    /// Text and binary cells of the kept rows are capped at
    /// [`MAX_CELL_BYTES`]; returns whether any was.
    pub fn push_rows(&mut self, mut rows: Vec<Vec<Value>>) -> bool {
        // Views are only set once a run ended, and a run gets new buffers.
        debug_assert!(self.view.is_none(), "rows pushed under a view");
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

    /// Up to `limit` visible rows starting at visible row `offset`; empty
    /// past the end.
    pub fn window(&self, offset: usize, limit: usize) -> Vec<&[Value]> {
        let len = self.visible_len();
        let start = offset.min(len);
        let end = start.saturating_add(limit).min(len);
        self.visible(start, end)
    }

    /// The columns and rows covered by `sel`, cloned.
    pub fn select(&self, sel: &Selection) -> (Vec<Column>, Vec<Vec<Value>>) {
        self.with_selection(sel, |columns, rows| {
            (columns.to_vec(), rows.iter().map(|r| r.to_vec()).collect())
        })
    }

    /// Calls `f` with the columns and rows covered by `sel`, whose row
    /// indexes are of visible rows. Rows are borrowed, in display order.
    pub fn with_selection<R>(
        &self,
        sel: &Selection,
        f: impl FnOnce(&[Column], &[&[Value]]) -> R,
    ) -> R {
        match sel {
            Selection::All => f(&self.columns, &self.visible(0, self.visible_len())),
            Selection::Rows { from, to } => f(&self.columns, &self.visible_between(*from, *to)),
            Selection::Cells { rows, cols } => {
                let cells: Vec<&[Value]> = self
                    .visible_between(rows.0, rows.1)
                    .into_iter()
                    .map(|row| clamp(row, cols.0, cols.1))
                    .collect();
                f(clamp(&self.columns, cols.0, cols.1), &cells)
            }
        }
    }
}

/// The cell of `row` at `column`; `NULL` when the row has no such column.
fn cell_at(row: &[Value], column: usize) -> &Value {
    row.get(column).unwrap_or(&Value::Null)
}

/// Sort order of cells: `NULL` before everything; booleans, integers,
/// floats and decimals by numeric value (decimals exactly, from their text);
/// binary by bytes; text and the rest (dates, times, ids) by their text,
/// ignoring case; `datetimeoffset` by its instant in UTC (then by its text;
/// text that does not parse goes last). Values of different kinds order by
/// kind, in that order. A total order, so sorting with it is well defined.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    let (ka, kb) = (sort_kind(a), sort_kind(b));
    if ka != kb {
        return ka.cmp(&kb);
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) if x.is_finite() && y.is_finite() => {
            x.partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
        (Value::DateTimeOffset(x), Value::DateTimeOffset(y)) => {
            match (Instant::parse(x), Instant::parse(y)) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => x.cmp(y),
            }
        }
        _ if ka == SortKind::Number => compare_numbers(a, b),
        _ => compare_ignoring_case(text_of(a), text_of(b)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SortKind {
    Null,
    Number,
    Binary,
    Text,
    /// Its own kind: ordered by instant, which plain text could not mix with.
    Offset,
}

/// A sort key parsed once per row by [`ResultBuffer::set_view`].
enum Parsed<'a> {
    Dec(Dec<'a>),
    Instant(Instant<'a>),
    /// A text-like value (see [`SortKind::Text`]) lower-cased; UTF-8 byte order is the order of its chars, so
    /// comparing two of these is [`compare_ignoring_case`].
    Folded(Cow<'a, str>),
}

impl<'a> Parsed<'a> {
    fn of(v: &'a Value) -> Option<Self> {
        match v {
            Value::Decimal(d) => Dec::parse(d).map(Self::Dec),
            Value::DateTimeOffset(t) => Instant::parse(t).map(Self::Instant),
            v if sort_kind(v) == SortKind::Text => {
                let t = text_of(v);
                Some(Self::Folded(
                    if t.is_ascii() && !t.bytes().any(|c| c.is_ascii_uppercase()) {
                        Cow::Borrowed(t)
                    } else {
                        Cow::Owned(t.chars().flat_map(char::to_lowercase).collect())
                    },
                ))
            }
            _ => None,
        }
    }
}

/// A `datetimeoffset` as UTC seconds and nanoseconds, then its text (so two
/// spellings of one instant still have a fixed order).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Instant<'a> {
    seconds: i64,
    nanos: u32,
    text: &'a str,
}

impl<'a> Instant<'a> {
    /// Parses `YYYY-MM-DD hh:mm:ss[.fffffff] ±hh:mm`, as the driver writes
    /// it (see `tulka-mssql`'s `convert.rs`).
    fn parse(text: &'a str) -> Option<Self> {
        let (local, offset) = text.rsplit_once(' ')?;
        let (date, time) = local.split_once(' ')?;
        let mut date = date.splitn(3, '-');
        let year: i64 = number(date.next()?)?;
        let month: i64 = number(date.next()?)?;
        let day: i64 = number(date.next()?)?;
        let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
        let mut clock = clock.splitn(3, ':');
        let hour: i64 = number(clock.next()?)?;
        let minute: i64 = number(clock.next()?)?;
        let second: i64 = number(clock.next()?)?;
        if fraction.len() > 9 || (!fraction.is_empty() && number::<u32>(fraction).is_none()) {
            return None;
        }
        let nanos = fraction
            .bytes()
            .chain(std::iter::repeat(b'0'))
            .take(9)
            .fold(0u32, |n, d| n * 10 + u32::from(d - b'0'));
        let (sign, offset) = match offset.as_bytes().first()? {
            b'+' => (1, &offset[1..]),
            b'-' => (-1, &offset[1..]),
            _ => return None,
        };
        let (oh, om) = offset.split_once(':')?;
        let offset_minutes = sign * (number::<i64>(oh)? * 60 + number::<i64>(om)?);
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let local_seconds =
            days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
        Some(Instant {
            seconds: local_seconds - offset_minutes * 60,
            nanos,
            text,
        })
    }
}

/// Parses a run of ASCII digits (no sign).
fn number<T: std::str::FromStr>(s: &str) -> Option<T> {
    if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn sort_kind(v: &Value) -> SortKind {
    match v {
        Value::Null => SortKind::Null,
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Decimal(_) => SortKind::Number,
        Value::Bytes(_) => SortKind::Binary,
        Value::Text(_) | Value::Date(_) | Value::Time(_) | Value::DateTime(_) | Value::Uuid(_) => {
            SortKind::Text
        }
        Value::DateTimeOffset(_) => SortKind::Offset,
    }
}

/// The text of a text-like value (`Text`, dates, times, ids).
fn text_of(v: &Value) -> &str {
    match v {
        Value::Text(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::DateTime(s)
        | Value::DateTimeOffset(s)
        | Value::Uuid(s) => s,
        _ => "",
    }
}

fn compare_ignoring_case(a: &str, b: &str) -> Ordering {
    // Same result as the general case, without its per-char work.
    if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
    }
}

/// A numeric value as a sortable key: infinities around the finite values,
/// then NaN, then decimals whose text does not parse (by that text).
enum NumKey<'a> {
    NegInf,
    Finite(Dec<'a>),
    PosInf,
    NaN,
    Unparsed(&'a str),
}

impl NumKey<'_> {
    fn rank(&self) -> u8 {
        match self {
            Self::NegInf => 0,
            Self::Finite(_) => 1,
            Self::PosInf => 2,
            Self::NaN => 3,
            Self::Unparsed(_) => 4,
        }
    }
}

/// A finite decimal number split from its text: `int` without leading
/// zeros, `frac` without trailing zeros, and zero never negative.
struct Dec<'a> {
    negative: bool,
    int: &'a str,
    frac: &'a str,
}

impl<'a> Dec<'a> {
    /// Parses `[-+]digits[.digits]` (either part may be empty, not both).
    fn parse(text: &'a str) -> Option<Self> {
        let (negative, rest) = match text.as_bytes().first() {
            Some(b'-') => (true, &text[1..]),
            Some(b'+') => (false, &text[1..]),
            _ => (false, text),
        };
        let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
        let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
        if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
            return None;
        }
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        Some(Dec {
            negative: negative && !(int.is_empty() && frac.is_empty()),
            int,
            frac,
        })
    }

    fn cmp(&self, other: &Self) -> Ordering {
        // Fractions compare as text once trailing zeros are gone.
        let magnitude = self
            .int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.cmp(other.int))
            .then_with(|| self.frac.cmp(other.frac));
        match (self.negative, other.negative) {
            (false, false) => magnitude,
            (true, true) => magnitude.reverse(),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        }
    }
}

/// The exact decimal text of a number (`Float` by its shortest round-trip
/// form, which keeps the order of floats), or `None` for a non-finite float.
fn number_text(v: &Value) -> Option<Cow<'_, str>> {
    match v {
        Value::Bool(b) => Some(Cow::Borrowed(if *b { "1" } else { "0" })),
        Value::Int(i) => Some(Cow::Owned(i.to_string())),
        Value::Float(f) if f.is_finite() => Some(Cow::Owned(f.to_string())),
        Value::Decimal(d) => Some(Cow::Borrowed(d)),
        _ => None,
    }
}

fn num_key<'a>(v: &Value, text: Option<&'a str>) -> NumKey<'a> {
    match (v, text) {
        (_, Some(t)) => Dec::parse(t).map_or(NumKey::Unparsed(t), NumKey::Finite),
        (Value::Float(f), None) if f.is_nan() => NumKey::NaN,
        (Value::Float(f), None) if *f < 0.0 => NumKey::NegInf,
        _ => NumKey::PosInf,
    }
}

fn compare_numbers(a: &Value, b: &Value) -> Ordering {
    let (ta, tb) = (number_text(a), number_text(b));
    let (ka, kb) = (num_key(a, ta.as_deref()), num_key(b, tb.as_deref()));
    match (&ka, &kb) {
        (NumKey::Finite(x), NumKey::Finite(y)) => x.cmp(y),
        (NumKey::Unparsed(x), NumKey::Unparsed(y)) => x.cmp(y),
        _ => ka.rank().cmp(&kb.rank()),
    }
}

/// The text of a cell as the grid shows it, without thousands separators;
/// `None` for `NULL`. Binary is `0x` and upper-case hex of every byte.
fn plain_text(v: &Value) -> Option<Cow<'_, str>> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => Cow::Borrowed(if *b { "1" } else { "0" }),
        Value::Int(i) => Cow::Owned(i.to_string()),
        Value::Float(f) => Cow::Owned(f.to_string()),
        Value::Decimal(d) => Cow::Borrowed(d),
        Value::Bytes(b) => Cow::Owned(hex(b, true)),
        other => Cow::Borrowed(text_of(other)),
    })
}

/// `0x` and the hex digits of `bytes`.
fn hex(bytes: &[u8], upper: bool) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(2 + bytes.len() * 2);
    out.push_str("0x");
    for b in bytes {
        let _ = if upper {
            write!(out, "{b:02X}")
        } else {
            write!(out, "{b:02x}")
        };
    }
    out
}

/// `number` with `,` between groups of three digits of its integer part, as
/// the grid shows it (`groupDigits` in `cellText.ts`); other text unchanged.
fn group_digits(number: &str) -> Cow<'_, str> {
    let (sign, rest) = number.strip_prefix('-').map_or(("", number), |r| ("-", r));
    let (int, frac) = match rest.find('.') {
        Some(dot) => rest.split_at(dot),
        None => (rest, ""),
    };
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    if int.len() <= 3 || !digits(int) || !digits(frac.get(1..).unwrap_or("")) {
        return Cow::Borrowed(number);
    }
    let mut out = String::with_capacity(number.len() + int.len() / 3);
    out.push_str(sign);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.push_str(frac);
    Cow::Owned(out)
}

/// Whether the cell's text, as the grid shows it, contains `needle_lower`
/// (already lower-case), ignoring case. `NULL` shows as `NULL`, numbers with
/// or without thousands separators, binary as `0x` and its first
/// [`DISPLAY_BYTES`] bytes in hex; text is searched in full, also past what
/// the grid shows of a long value.
pub fn cell_matches(value: &Value, needle_lower: &str) -> bool {
    match value {
        Value::Null => "null".contains(needle_lower),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Decimal(_) => {
            let plain = plain_text(value).unwrap_or_default();
            // A needle without `,` is in the grouped text only if it is in
            // the plain one.
            contains_ignoring_case(&plain, needle_lower)
                || (needle_lower.contains(',')
                    && contains_ignoring_case(&group_digits(&plain), needle_lower))
        }
        Value::Bytes(b) => {
            let shown = &b[..b.len().min(DISPLAY_BYTES)];
            hex(shown, false).contains(needle_lower)
        }
        other => contains_ignoring_case(text_of(other), needle_lower),
    }
}

/// Whether `haystack` contains `needle_lower` (lower-case), ignoring case.
fn contains_ignoring_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() {
        let needle = needle_lower.as_bytes();
        haystack
            .as_bytes()
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle))
    } else {
        haystack.to_lowercase().contains(needle_lower)
    }
}

/// The `Equals` filter: the cell's text is exactly `text`.
/// Besides the full value, a long text or binary cell also matches the
/// shortened form the grid shows (see [`display_cell`]): its first
/// [`DISPLAY_TEXT_CHARS`] characters, or `0x` and its first [`DISPLAY_BYTES`]
/// bytes, plus `…`.
fn cell_equals(value: &Value, text: &str) -> bool {
    let Some(plain) = plain_text(value) else {
        return false;
    };
    if plain == text {
        return true;
    }
    match value {
        Value::Int(_) | Value::Float(_) | Value::Decimal(_) => group_digits(&plain) == text,
        Value::Text(full) => text.strip_suffix('…').is_some_and(|shown| {
            full.char_indices()
                .nth(DISPLAY_TEXT_CHARS)
                .is_some_and(|(end, _)| &full[..end] == shown)
        }),
        Value::Bytes(bytes) => text.strip_suffix('…').is_some_and(|shown| {
            bytes.len() > DISPLAY_BYTES && hex(&bytes[..DISPLAY_BYTES], true) == shown
        }),
        _ => false,
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
        assert_eq!(b.window(8, 5)[0], &[Value::Int(80)][..]);
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

    fn buffer_of(rows: Vec<Vec<Value>>) -> ResultBuffer {
        let cols = rows.first().map_or(0, Vec::len);
        let mut b = ResultBuffer::new((0..cols).map(|c| col(&format!("c{c}"))).collect(), 1000);
        b.push_rows(rows);
        b
    }

    fn sorted(column: usize, descending: bool) -> ViewSpec {
        ViewSpec {
            sort: Some(SortSpec { column, descending }),
            ..ViewSpec::default()
        }
    }

    /// The first cell of every visible row, in order.
    fn first_cells(b: &ResultBuffer) -> Vec<Value> {
        b.window(0, usize::MAX)
            .iter()
            .map(|row| row[0].clone())
            .collect()
    }

    fn dec(s: &str) -> Value {
        Value::Decimal(s.into())
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    #[test]
    fn view_sorts_nulls_first_and_numbers_by_value() {
        let mut b = buffer_of(vec![
            vec![Value::Int(10)],
            vec![Value::Int(9)],
            vec![Value::Null],
            vec![Value::Float(2.5)],
        ]);
        b.set_view(&sorted(0, false));
        assert_eq!(
            first_cells(&b),
            vec![
                Value::Null,
                Value::Float(2.5),
                Value::Int(9),
                Value::Int(10)
            ]
        );
        b.set_view(&sorted(0, true));
        assert_eq!(
            first_cells(&b),
            vec![
                Value::Int(10),
                Value::Int(9),
                Value::Float(2.5),
                Value::Null
            ]
        );
    }

    #[test]
    fn decimals_compare_by_value_not_text() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_values(&dec("10.00"), &dec("9.5")), Greater);
        assert_eq!(compare_values(&dec("9.50"), &dec("9.5")), Equal);
        assert_eq!(compare_values(&dec("-10"), &dec("-9.5")), Less);
        assert_eq!(compare_values(&dec("-0.00"), &dec("0")), Equal);
        assert_eq!(compare_values(&dec("0.45"), &dec("0.5")), Less);
        assert_eq!(compare_values(&dec("007.1"), &dec("7.10")), Equal);
        // Exact beyond f64 precision.
        assert_eq!(
            compare_values(
                &dec("12345678901234567890123456789.01"),
                &dec("12345678901234567890123456789.02")
            ),
            Less
        );
        assert_eq!(compare_values(&Value::Int(10), &dec("9.99")), Greater);
        assert_eq!(compare_values(&Value::Int(-3), &Value::Float(-2.5)), Less);
        assert_eq!(compare_values(&dec("2.5"), &Value::Float(2.5)), Equal);
        assert_eq!(
            compare_values(&Value::Float(f64::NEG_INFINITY), &dec("-1000000000")),
            Less
        );
        assert_eq!(compare_values(&Value::Null, &Value::Int(i64::MIN)), Less);
    }

    fn dto(s: &str) -> Value {
        Value::DateTimeOffset(s.into())
    }

    #[test]
    fn date_time_offsets_sort_by_their_instant() {
        use std::cmp::Ordering::*;
        // 14:03 at -03:00 is 17:03 UTC, after 16:00 UTC written at +00:00.
        assert_eq!(
            compare_values(
                &dto("2026-10-20 14:03:00 -03:00"),
                &dto("2026-10-20 16:00:00 +00:00")
            ),
            Greater
        );
        // The same instant (00:01 UTC), across midnight and at a half-hour
        // offset: their text breaks the tie.
        assert_eq!(
            compare_values(
                &dto("2026-10-19 23:01:00.0 -01:00"),
                &dto("2026-10-20 05:31:00.0 +05:30")
            ),
            Less
        );
        assert_eq!(
            compare_values(
                &dto("2026-10-20 05:31:00.0 +05:30"),
                &dto("2026-10-19 23:01:00.0 -01:00")
            ),
            Greater
        );
        assert_eq!(
            compare_values(
                &dto("2026-10-20 05:31:00.0 +05:30"),
                &dto("2026-10-20 05:31:00.0 +05:30")
            ),
            Equal
        );
        // An earlier instant (10-19 22:00 UTC vs 10-20 01:00 UTC) whose text
        // sorts later.
        assert_eq!(
            compare_values(
                &dto("2026-10-20 12:00:00 +14:00"),
                &dto("2026-10-20 00:00:00 -01:00")
            ),
            Less
        );
        assert_eq!(
            compare_values(
                &dto("2026-10-20 00:00:00.0000001 +00:00"),
                &dto("2026-10-20 00:00:00 +00:00")
            ),
            Greater
        );
        // Unparsable text sorts after every parsed value, by its text.
        assert_eq!(
            compare_values(&dto("garbage"), &dto("9999-12-31 23:59:59 -14:00")),
            Greater
        );
        assert_eq!(compare_values(&dto("a"), &dto("b")), Less);

        let mut b = buffer_of(vec![
            vec![dto("2026-10-20 14:03:00 -03:00"), Value::Int(0)], // 17:03 UTC
            vec![dto("2026-10-20 18:00:00 +05:30"), Value::Int(1)], // 12:30 UTC
            vec![Value::Null, Value::Int(2)],
            vec![dto("2026-10-20 16:00:00 +00:00"), Value::Int(3)], // 16:00 UTC
            vec![dto("2026-10-20 09:00:00 -09:00"), Value::Int(4)], // 18:00 UTC
        ]);
        b.set_view(&sorted(0, false));
        let order: Vec<Value> = b.window(0, 10).iter().map(|r| r[1].clone()).collect();
        assert_eq!(
            order,
            [2, 1, 3, 0, 4].map(Value::Int).to_vec(),
            "NULL, then by UTC instant"
        );
        b.set_view(&sorted(0, true));
        let order: Vec<Value> = b.window(0, 10).iter().map(|r| r[1].clone()).collect();
        assert_eq!(order, [4, 0, 3, 1, 2].map(Value::Int).to_vec());
    }

    #[test]
    fn text_compares_without_case_and_others_by_their_text() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_values(&text("apple"), &text("Banana")), Less);
        assert_eq!(compare_values(&text("ÉCLAIR"), &text("éclair")), Equal);
        assert_eq!(compare_values(&text("Zeta"), &text("alpha")), Greater);
        assert_eq!(
            compare_values(
                &Value::Date("2026-01-09".into()),
                &Value::Date("2026-10-01".into())
            ),
            Less
        );
        assert_eq!(
            compare_values(&Value::Bytes(vec![0x01, 0xFF]), &Value::Bytes(vec![0x02])),
            Less
        );
        assert_eq!(compare_values(&Value::Null, &text("")), Less);
    }

    #[test]
    fn view_sort_is_stable_with_equal_values() {
        let mut b = buffer_of(vec![
            vec![text("b"), Value::Int(0)],
            vec![text("A"), Value::Int(1)],
            vec![text("a"), Value::Int(2)],
            vec![text("B"), Value::Int(3)],
        ]);
        let order = |b: &ResultBuffer| -> Vec<Value> {
            b.window(0, 10).iter().map(|r| r[1].clone()).collect()
        };
        b.set_view(&sorted(0, false));
        assert_eq!(
            order(&b),
            vec![Value::Int(1), Value::Int(2), Value::Int(0), Value::Int(3)]
        );
        b.set_view(&sorted(0, true));
        assert_eq!(
            order(&b),
            vec![Value::Int(0), Value::Int(3), Value::Int(1), Value::Int(2)]
        );
    }

    #[test]
    fn view_text_filter_finds_any_column_without_case() {
        let mut b = buffer_of(vec![
            vec![Value::Int(1), text("pedido 7")],
            vec![Value::Int(2), text("factura")],
            vec![Value::Int(3), text("PEDIDOS")],
        ]);
        b.set_view(&ViewSpec {
            text: Some("PED".into()),
            ..ViewSpec::default()
        });
        assert_eq!(b.visible_len(), 2);
        assert_eq!(first_cells(&b), vec![Value::Int(1), Value::Int(3)]);
        // Numbers match their digits, with or without grouping.
        b.set_view(&ViewSpec {
            text: Some("2".into()),
            ..ViewSpec::default()
        });
        assert_eq!(first_cells(&b), vec![Value::Int(2)]);
    }

    #[test]
    fn cell_matches_the_text_the_grid_shows() {
        assert!(cell_matches(&Value::Int(1234567), "1,234,5"));
        assert!(cell_matches(&Value::Int(1234567), "34567"));
        assert!(cell_matches(&dec("8912450.50"), "2,450.5"));
        assert!(!cell_matches(&dec("8912450.50"), "8,912,450.6"));
        assert!(cell_matches(&Value::Null, "null"));
        assert!(cell_matches(&Value::Bool(true), "1"));
        assert!(cell_matches(&Value::Bytes(vec![0xAB, 0x01]), "0xab01"));
        assert!(cell_matches(&Value::Uuid("6F9619FF-8B86".into()), "6f96"));
        assert!(cell_matches(&text("Ferretería LÓPEZ"), "ería lópez"));
        assert!(!cell_matches(&text("abc"), "abd"));
    }

    #[test]
    fn view_column_filters_combine_with_and() {
        let mut b = buffer_of(vec![
            vec![Value::Int(1), Value::Null, text("x")],
            vec![Value::Int(2), Value::Null, text("y")],
            vec![Value::Int(3), text("z"), text("x")],
            vec![Value::Int(4), Value::Null, text("x")],
        ]);
        b.set_view(&ViewSpec {
            columns: vec![
                ColumnFilter::IsNull { column: 1 },
                ColumnFilter::Equals {
                    column: 2,
                    text: "x".into(),
                },
            ],
            ..ViewSpec::default()
        });
        assert_eq!(first_cells(&b), vec![Value::Int(1), Value::Int(4)]);
        b.set_view(&ViewSpec {
            columns: vec![ColumnFilter::NotNull { column: 1 }],
            ..ViewSpec::default()
        });
        assert_eq!(first_cells(&b), vec![Value::Int(3)]);
        // Equals is exact text; numbers also match their grouped form.
        let mut b = buffer_of(vec![
            vec![text("X")],
            vec![dec("1234.50")],
            vec![Value::Null],
        ]);
        let equals = |t: &str| ViewSpec {
            columns: vec![ColumnFilter::Equals {
                column: 0,
                text: t.into(),
            }],
            ..ViewSpec::default()
        };
        b.set_view(&equals("x"));
        assert_eq!(b.visible_len(), 0);
        b.set_view(&equals("1,234.50"));
        assert_eq!(first_cells(&b), vec![dec("1234.50")]);
        b.set_view(&equals("1234.50"));
        assert_eq!(b.visible_len(), 1);
        b.set_view(&equals("NULL"));
        assert_eq!(b.visible_len(), 0);
    }

    #[test]
    fn equals_matches_long_values_as_the_grid_shows_them() {
        let long = "é".repeat(DISPLAY_TEXT_CHARS) + "tail";
        let shown_text = "é".repeat(DISPLAY_TEXT_CHARS) + "…";
        let blob: Vec<u8> = (0..40).collect();
        let shown_blob = format!("{}…", hex(&blob[..DISPLAY_BYTES], true));
        let mut b = buffer_of(vec![
            vec![Value::Text(long.clone()), Value::Bytes(blob.clone())],
            vec![text("short"), Value::Bytes(vec![0xAB])],
        ]);
        let equals = |column: usize, t: &str| ViewSpec {
            columns: vec![ColumnFilter::Equals {
                column,
                text: t.into(),
            }],
            ..ViewSpec::default()
        };
        // What the grid shows ("Filter by this value" on a long cell)...
        b.set_view(&equals(0, &shown_text));
        assert_eq!(b.visible_len(), 1);
        b.set_view(&equals(1, &shown_blob));
        assert_eq!(b.visible_len(), 1);
        // ...and the full value both match.
        b.set_view(&equals(0, &long));
        assert_eq!(b.visible_len(), 1);
        b.set_view(&equals(1, &hex(&blob, true)));
        assert_eq!(b.visible_len(), 1);
        // A short value is shown whole, so "…" makes it a different text.
        b.set_view(&equals(0, "short…"));
        assert_eq!(b.visible_len(), 0);
        b.set_view(&equals(1, "0xAB"));
        assert_eq!(b.visible_len(), 1);
        b.set_view(&equals(1, "0xAB…"));
        assert_eq!(b.visible_len(), 0);
    }

    #[test]
    fn empty_view_spec_restores_the_original_order() {
        let mut b = buffer_of(vec![vec![Value::Int(2)], vec![Value::Int(1)]]);
        b.set_view(&sorted(0, false));
        assert_eq!(first_cells(&b), vec![Value::Int(1), Value::Int(2)]);
        b.set_view(&ViewSpec {
            text: Some(String::new()),
            ..ViewSpec::default()
        });
        assert!(!b.has_view());
        assert_eq!(first_cells(&b), vec![Value::Int(2), Value::Int(1)]);
        assert_eq!(b.visible_len(), 2);
    }

    #[test]
    fn selection_indexes_are_visible_rows() {
        let mut b = grid(5, 2);
        b.set_view(&sorted(0, true));
        // Visible: rows 4, 3, 2, 1, 0.
        assert_eq!(b.window(1, 2), vec![&b.rows[3][..], &b.rows[2][..]]);
        let (_, rows) = b.select(&Selection::Rows { from: 0, to: 1 });
        assert_eq!(rows, vec![b.rows[4].clone(), b.rows[3].clone()]);
        let (_, rows) = b.select(&Selection::Cells {
            rows: (3, 9),
            cols: (1, 1),
        });
        assert_eq!(rows, vec![vec![Value::Int(11)], vec![Value::Int(1)]]);
        let (_, all) = b.select(&Selection::All);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0], b.rows[4]);
    }

    #[test]
    fn view_spec_json_shape() {
        use serde_json::json;
        let spec: ViewSpec = serde_json::from_value(json!({
            "sort": {"column": 2, "descending": true},
            "text": "ped",
            "columns": [
                {"kind": "isNull", "column": 0},
                {"kind": "notNull", "column": 1},
                {"kind": "equals", "column": 3, "text": "x"}
            ]
        }))
        .unwrap();
        assert_eq!(
            spec,
            ViewSpec {
                sort: Some(SortSpec {
                    column: 2,
                    descending: true
                }),
                text: Some("ped".into()),
                columns: vec![
                    ColumnFilter::IsNull { column: 0 },
                    ColumnFilter::NotNull { column: 1 },
                    ColumnFilter::Equals {
                        column: 3,
                        text: "x".into()
                    },
                ],
            }
        );
        let empty: ViewSpec =
            serde_json::from_value(json!({"sort": null, "text": null, "columns": []})).unwrap();
        assert_eq!(empty, ViewSpec::default());
    }

    /// 100,000 × 20 rows of mixed types, like a wide query result.
    fn wide_buffer() -> ResultBuffer {
        let mut b = ResultBuffer::new((0..20).map(|c| col(&format!("c{c}"))).collect(), MAX_ROWS);
        let rows = (0..MAX_ROWS)
            .map(|r| {
                (0..20)
                    .map(|c| match c % 5 {
                        _ if c == 7 => Value::Text(format!("Ferretería {}", r % 1000)),
                        0 => Value::Int(((r * 7919) % 100_003) as i64),
                        1 => Value::Decimal(format!("{}.{:02}", (r * 31) % 9973, r % 100)),
                        2 => Value::Text(format!("Cliente {} pedido {}", r % 977, r)),
                        _ if c == 19 => Value::DateTimeOffset(format!(
                            "2026-{:02}-{:02} {:02}:00:00.1234567 {}{:02}:30",
                            r % 12 + 1,
                            r % 28 + 1,
                            r % 24,
                            if r % 2 == 0 { '-' } else { '+' },
                            r % 14
                        )),
                        3 => Value::DateTime(format!(
                            "2026-{:02}-{:02} 10:00:00",
                            r % 12 + 1,
                            r % 28 + 1
                        )),
                        _ if r % 10 == 0 => Value::Null,
                        _ => Value::Float((r % 1000) as f64 / 3.0),
                    })
                    .collect()
            })
            .collect();
        b.push_rows(rows);
        b
    }

    #[test]
    #[ignore = "performance budget; run with --release"]
    fn view_sorts_and_filters_100k_rows_under_budget() {
        use std::time::{Duration, Instant};
        let mut b = wide_buffer();
        let budget = Duration::from_millis(200);
        let specs = [
            ("sort int", sorted(0, false)),
            ("sort decimal", sorted(1, true)),
            ("sort text", sorted(2, false)),
            ("sort datetime", sorted(3, false)),
            ("sort non-ASCII text", sorted(7, false)),
            ("sort datetimeoffset", sorted(19, true)),
            ("sort float with nulls", sorted(4, true)),
            (
                "text filter",
                ViewSpec {
                    text: Some("pedido 9".into()),
                    ..ViewSpec::default()
                },
            ),
            (
                "non-ASCII text filter",
                ViewSpec {
                    text: Some("RÍA 9".into()),
                    ..ViewSpec::default()
                },
            ),
            (
                "text filter without matches",
                ViewSpec {
                    text: Some("zzz".into()),
                    ..ViewSpec::default()
                },
            ),
            (
                "column filters and sort",
                ViewSpec {
                    sort: Some(SortSpec {
                        column: 2,
                        descending: false,
                    }),
                    text: Some("cliente".into()),
                    columns: vec![ColumnFilter::NotNull { column: 4 }],
                },
            ),
        ];
        for (name, spec) in specs {
            let started = Instant::now();
            b.set_view(&spec);
            let elapsed = started.elapsed();
            println!("{name}: {elapsed:?} ({} visible)", b.visible_len());
            assert!(elapsed < budget, "{name} took {elapsed:?}");
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
