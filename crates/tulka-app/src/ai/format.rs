//! The texts an AI assistant receives. They are plain English and never
//! mention sentinels: the model reads them and acts on them.

use tulka_core::{Column, DriverError, MessageKind, ReadOnlyReason, ReadOnlyViolation, Value};

/// Answer when no connection is shared with AI assistants.
pub(crate) const NO_CONNECTIONS: &str =
    "No connections are shared with the AI. Turn on AI access for a connection in tulkabd.";

/// A connection that does not exist and one without AI access look the same,
/// so the answer never reveals which connections exist.
pub(crate) fn not_available(name: &str) -> String {
    format!("No connection named \"{name}\" is available to the AI. Use list_connections.")
}

pub(crate) fn database_not_available(database: &str, connection: &str, list: &[String]) -> String {
    let available = if list.is_empty() {
        "none".to_string()
    } else {
        list.join(", ")
    };
    format!("Database \"{database}\" is not available on \"{connection}\". Available: {available}.")
}

/// The assistant cannot ask for a password, so it is told how the user fixes it.
pub(crate) fn password_not_saved(connection: &str) -> String {
    format!(
        "The password of \"{connection}\" is not saved in tulkabd, so the AI cannot connect. \
         Open tulkabd, edit the connection and save its password."
    )
}

pub(crate) fn config_unreadable(error: &str) -> String {
    format!("tulkabd could not read its configuration: {error}")
}

pub(crate) fn no_databases_selected(connection: &str) -> String {
    format!("\"{connection}\" has no databases selected in tulkabd, so there is nothing to use.")
}

pub(crate) fn connect_failed(connection: &str, error: &DriverError) -> String {
    format!(
        "Could not connect to \"{connection}\": {}",
        driver_failure(error)
    )
}

/// What a driver error says to the assistant. The messages drivers produce
/// can carry the server address or the login name, which the assistant must
/// not learn, so only a server's own query error keeps its text.
pub(crate) fn driver_failure(error: &DriverError) -> String {
    match error {
        DriverError::Query {
            message,
            line: Some(line),
        } => format!("Error (line {line}): {message}"),
        DriverError::Query {
            message,
            line: None,
        } => format!("Error: {message}"),
        DriverError::Unreachable(_) => "the server could not be reached.".into(),
        DriverError::AuthFailed(_) => {
            "the saved login was rejected; open tulkabd and check the connection.".into()
        }
        DriverError::Tls(_) => "the secure connection (TLS) could not be set up.".into(),
        DriverError::Cancelled => "the request was cancelled.".into(),
        DriverError::Disconnected(_) => "the connection was lost.".into(),
        DriverError::NoAccess(_) => "the login has no access to that database.".into(),
        DriverError::Other(_) => "the request failed.".into(),
    }
}

/// A failure after the connection was made: a query error keeps its line and
/// message, anything else says which connection it happened on.
pub(crate) fn on_connection(connection: &str, error: &DriverError) -> String {
    match error {
        DriverError::Query { .. } => driver_failure(error),
        _ => format!("{connection}: {}", driver_failure(error)),
    }
}

/// Why a query was not run, for the assistant. `line` is shown 1-based.
pub(crate) fn not_run(violation: &ReadOnlyViolation) -> String {
    let line = violation.line + 1;
    let word = violation.word.as_deref().unwrap_or("something else");
    match violation.reason {
        ReadOnlyReason::NotASelect => format!(
            "Not run: only SELECT queries (and WITH … SELECT) are allowed; line {line} starts with {word}."
        ),
        ReadOnlyReason::Forbidden => format!(
            "Not run: {word} is not allowed in a read-only query (line {line}). \
             If it is a column or table name, write it in [brackets]."
        ),
        ReadOnlyReason::LinkedServer => {
            format!("Not run: four-part names (linked servers) are not allowed (line {line}).")
        }
        ReadOnlyReason::Unparsed => "Not run: the query has an unclosed string, identifier or comment. \
             Parentheses must balance too, and names with accents or other non-ASCII \
             letters must be written in [brackets], for example [año]."
            .into(),
        ReadOnlyReason::Empty => "Not run: the query is empty.".into(),
        ReadOnlyReason::Unsupported => {
            "Not run: this connection's driver cannot run read-only queries.".into()
        }
    }
}

pub(crate) fn timed_out(seconds: u32) -> String {
    format!("Timeout ({seconds} s) reached; the query was cancelled.")
}

pub(crate) const CANCELLED: &str = "The query was cancelled.";

pub(crate) const OUTPUT_CUT: &str = "Output cut at 100,000 characters; narrow the query.";

pub(crate) const NO_RESULTS: &str = "The query ran and returned no result sets.";

pub(crate) fn limit_reached(max_rows: u32) -> String {
    format!("Limit reached: only the first {max_rows} rows are shown.")
}

/// A server message as one line.
pub(crate) fn message_line(kind: MessageKind, text: &str, line: Option<u32>) -> String {
    let kind = match kind {
        MessageKind::Info => "Info",
        MessageKind::Warning => "Warning",
        MessageKind::Error => "Error",
    };
    match line {
        Some(line) => format!("{kind} (line {line}): {text}"),
        None => format!("{kind}: {text}"),
    }
}

/// A table of text cells in the compact `|` format shared with "Copy for
/// AI": an empty cell is NULL.
pub(crate) fn table(headers: &[&str], rows: Vec<Vec<String>>) -> String {
    let columns: Vec<Column> = headers
        .iter()
        .map(|h| Column {
            name: (*h).to_string(),
            type_name: "text".into(),
        })
        .collect();
    let rows: Vec<Vec<Value>> = rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|cell| {
                    if cell.is_empty() {
                        Value::Null
                    } else {
                        Value::Text(cell)
                    }
                })
                .collect()
        })
        .collect();
    tulka_core::to_ai(&columns, &rows)
}
