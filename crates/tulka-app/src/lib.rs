//! Application services of tulkabd, independent of Tauri: saved connections,
//! one database session per query tab, script execution with streamed
//! events, Rust-side result buffers and copying results, and the Explorer
//! (metadata sessions, background indexing, the tree and the palette),
//! saved queries (`.sql` files per database) and `.sql` files opened from
//! disk.
//!
//! Every command of the UI is a plain async function taking [`AppState`];
//! the Tauri layer only wraps them. Errors are plain strings, with the
//! sentinels [`PASSWORD_REQUIRED`], [`DISCONNECTED`], [`BUSY`], [`CLOSED`],
//! [`NOT_CONNECTED`], [`NOT_ALLOWED`] and [`NOT_TEXT`]. Background events go
//! to [`AppEvents`].

pub mod connections;
pub mod explorer;
pub mod files;
pub mod queries;
pub mod query;
pub mod results;
pub mod session;
pub mod state;
pub mod types;

pub use connections::{
    copy_connection_string, create_folder, delete_connection, delete_folder, duplicate_connection,
    get_preferences, list_connections, list_expanded, list_folders, list_server_databases,
    move_connection, move_folder, parse_connection_string, rename_folder, save_connection,
    set_expanded, set_zoom, test_connection,
};
pub use explorer::{
    completion_metadata, connect_explorer, disconnect_explorer, explorer_columns,
    explorer_databases, explorer_objects, object_definition, refresh_metadata, search_objects,
    table_structure,
};
pub use files::{
    allow_paths, decode_text, encode_text, read_sql_file, sql_files_exist, write_sql_file,
    FileEncoding, LineEnding, OpenedFile, TextFile,
};
pub use queries::{
    decode_component, delete_saved_query, encode_component, list_saved_queries, read_saved_query,
    rename_saved_query, save_query, saved_query_path, QueriesStore,
};
pub use query::{
    cancel_query, close_session, copy_results, fetch_rows, open_session, run_query, use_database,
    MAX_FETCH_ROWS,
};
pub use results::{
    cap_cell, display_cell, ResultBuffer, Selection, CELLS_CAPPED_WARNING, DISPLAY_BYTES,
    DISPLAY_TEXT_CHARS, MAX_CELL_BYTES, MAX_ROWS,
};
pub use session::{load_session, save_session};
pub use state::{AppState, SharedBuffer, TabState};
pub use types::{
    AppEvent, AppEvents, Clipboard, CompletionMetadata, ConnectionStatus, CopyMode, CopyOutcome,
    ExplorerInfo, NoopEvents, PaletteHit, RunOutcome, RunSummary, SaveConnectionResult, SavedQuery,
    SavedQueryHit, SearchResponse, SessionInfo, UiEvent, BUSY, CLOSED, CYCLE, DISCONNECTED,
    EMPTY_NAME, EXISTS, NOT_ALLOWED, NOT_CONNECTED, NOT_FOUND, NOT_TEXT, PASSWORD_REQUIRED,
};

/// The core types used in the command signatures (`ConnectionConfig`,
/// `ServerInfo`, `Value`, ...).
pub use tulka_core;
/// The metadata cache (`MetaCache`, `CachedDatabase`) used in the command
/// signatures and by [`AppState::with_metadata`].
pub use tulka_store;
