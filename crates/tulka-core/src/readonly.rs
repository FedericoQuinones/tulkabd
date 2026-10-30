//! The verdict of the read-only check that gates what an AI assistant may run.

use serde::{Deserialize, Serialize};

/// Why a text was refused.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReadOnlyReason {
    /// There is nothing to run.
    Empty,
    /// A statement starts with something other than a query.
    NotASelect,
    /// A word that writes data or changes state appears inside a query.
    Forbidden,
    /// A four-part name reaches through a linked server.
    LinkedServer,
    /// The text could not be tokenized or its parentheses do not balance.
    Unparsed,
    /// The driver has no read-only check.
    Unsupported,
}

/// The first reason a text is not a plain read-only query.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReadOnlyViolation {
    pub reason: ReadOnlyReason,
    /// The offending word as written (`INTO`, `sp_who`), when there is one.
    pub word: Option<String>,
    /// 0-based line relative to the analyzed text.
    pub line: u32,
}
