//! The texts an AI assistant receives. They are plain English and never
//! mention sentinels: the model reads them and acts on them.

use tulka_core::{Column, Value};

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

pub(crate) fn connect_failed(connection: &str, error: &str) -> String {
    format!("Could not connect to \"{connection}\": {error}")
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
