use serde::{Deserialize, Serialize};

/// A single cell value, independent of the database engine.
///
/// Serialized adjacently tagged: `{"kind": "null"}`, `{"kind": "int", "value": "7"}`,
/// `{"kind": "decimal", "value": "8912450.50"}`.
///
/// `Int` travels as a JSON string so 64-bit values keep their precision in
/// JavaScript (numbers lose precision above 2^53).
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
    Int(#[serde(with = "int_as_string")] i64),
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

/// `i64` <-> JSON string, so the UI never rounds it through a JS number.
mod int_as_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}
