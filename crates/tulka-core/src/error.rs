use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors reported by a driver.
///
/// Serialized adjacently tagged: `{"kind": "authFailed", "value": "..."}`,
/// `{"kind": "query", "value": {"message": "...", "line": 4}}`,
/// `{"kind": "cancelled"}`.
#[derive(Error, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum DriverError {
    /// The server could not be reached (DNS, refused, timeout, network down).
    #[error("server unreachable: {0}")]
    Unreachable(String),
    /// The server rejected the credentials.
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    /// The TLS handshake failed (for example an untrusted certificate).
    #[error("TLS error: {0}")]
    Tls(String),
    /// The server reported an error for a statement. `line` is relative to
    /// the editor text passed to `execute`, when known.
    #[error("{message}")]
    Query { message: String, line: Option<u32> },
    /// The request was cancelled by the user.
    #[error("cancelled")]
    Cancelled,
    /// The connection was lost; the session cannot be used any more.
    #[error("disconnected: {0}")]
    Disconnected(String),
    #[error("{0}")]
    Other(String),
}
