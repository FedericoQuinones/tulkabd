//! The result of analyzing a script for statements that deserve a second
//! look before they run.

use serde::{Deserialize, Serialize};

/// Why a statement was flagged.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GuardKind {
    UpdateWithoutWhere,
    DeleteWithoutWhere,
    Drop,
    Truncate,
    /// The text could not be analyzed but contains a dangerous keyword.
    Unparsed,
}

/// One flagged statement.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GuardFinding {
    pub kind: GuardKind,
    /// The target as written, e.g. `dbo.Pedidos` or `[Sales].[Order Lines]`; `None` when unknown.
    pub object: Option<String>,
    /// The statement text, trimmed, at most 400 chars plus "…".
    pub statement: String,
    /// 0-based line of the statement start, relative to the analyzed text.
    pub line: u32,
}
