use serde::{Deserialize, Serialize};

/// A single cell value, independent of the database engine.
///
/// Serialized adjacently tagged: `{"kind": "null"}`, `{"kind": "int", "value": 7}`,
/// `{"kind": "decimal", "value": "8912450.50"}`.
///
/// Temporal values are ISO-like text in the server's own notation, with as
/// many fractional-second digits as the column's scale:
/// `Date` `2026-10-20`, `Time` `14:03:00.123`, `DateTime` `2026-10-20 14:03:00.123`,
/// `DateTimeOffset` `2026-10-20 14:03:00 -03:00`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact decimal text, never rounded (`decimal`, `numeric`, `money`).
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Date(String),
    Time(String),
    DateTime(String),
    DateTimeOffset(String),
    /// Upper-case hyphenated form, as SQL Server prints it.
    Uuid(String),
}
