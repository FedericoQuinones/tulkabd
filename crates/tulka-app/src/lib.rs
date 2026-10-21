//! Application services of tulkabd, independent of Tauri: saved connections,
//! one database session per query tab, script execution with streamed
//! events, Rust-side result buffers and copying results, and the Explorer
//! (metadata sessions, background indexing, the tree and the palette).
//!
//! Every command of the UI is a plain async function taking [`AppState`];
//! the Tauri layer only wraps them. Errors are plain strings, with the
//! sentinels [`PASSWORD_REQUIRED`], [`DISCONNECTED`], [`BUSY`], [`CLOSED`]
//! and [`NOT_CONNECTED`]. Background events go to [`AppEvents`].

pub mod connections;
pub mod explorer;
pub mod query;
pub mod results;
pub mod state;
pub mod types;

pub use connections::{
    create_folder, delete_connection, delete_folder, list_connections, list_expanded, list_folders,
    list_server_databases, move_connection, move_folder, rename_folder, save_connection,
    set_expanded, test_connection,
};
pub use explorer::{
    completion_metadata, connect_explorer, disconnect_explorer, explorer_columns,
    explorer_databases, explorer_objects, object_definition, refresh_metadata, search_objects,
    table_structure,
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
    AppEvent, AppEvents, Clipboard, CompletionMetadata, ConnectionStatus, CopyMode, CopyOutcome,
    ExplorerInfo, NoopEvents, PaletteHit, RunOutcome, RunSummary, SaveConnectionResult,
    SearchResponse, SessionInfo, UiEvent, BUSY, CLOSED, CYCLE, DISCONNECTED, EMPTY_NAME,
    NOT_CONNECTED, NOT_FOUND, PASSWORD_REQUIRED,
};

/// The core types used in the command signatures (`ConnectionConfig`,
/// `ServerInfo`, `Value`, ...).
pub use tulka_core;
/// The metadata cache (`MetaCache`, `CachedDatabase`) used in the command
/// signatures and by [`AppState::with_metadata`].
pub use tulka_store;
