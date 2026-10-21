//! Application services of tulkabd, independent of Tauri: saved connections,
//! one database session per query tab, script execution with streamed
//! events, Rust-side result buffers and copying results.
//!
//! Every command of the UI is a plain async function taking [`AppState`];
//! the Tauri layer only wraps them. Errors are plain strings, with the
//! sentinels [`PASSWORD_REQUIRED`], [`DISCONNECTED`], [`BUSY`] and [`CLOSED`].

pub mod connections;
pub mod query;
pub mod results;
pub mod state;
pub mod types;

pub use connections::{
    create_folder, delete_connection, delete_folder, list_connections, list_folders,
    list_server_databases, move_connection, move_folder, rename_folder, save_connection,
    set_expanded, test_connection,
};
pub use query::{
    cancel_query, close_session, copy_results, fetch_rows, open_session, run_query, use_database,
    MAX_FETCH_ROWS,
};
pub use results::{
    cap_cell, display_cell, ResultBuffer, Selection, CELLS_CAPPED_WARNING, DISPLAY_BYTES,
    DISPLAY_TEXT_CHARS, MAX_CELL_BYTES, MAX_ROWS,
};
pub use state::{AppState, SharedBuffer, TabState};
pub use types::{
    Clipboard, CopyMode, CopyOutcome, RunOutcome, RunSummary, SaveConnectionResult, SessionInfo,
    UiEvent, BUSY, CLOSED, CYCLE, DISCONNECTED, EMPTY_NAME, NOT_FOUND, PASSWORD_REQUIRED,
};

/// The core types used in the command signatures (`ConnectionConfig`,
/// `ServerInfo`, `Value`, ...).
pub use tulka_core;
