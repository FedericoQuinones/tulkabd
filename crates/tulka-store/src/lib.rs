//! SQLite cache of the Explorer metadata, so search works at startup even
//! offline. Tauri-free and blocking: callers use `spawn_blocking`.

mod cache;
mod private_file;
mod runlog;
mod search;
mod session;

pub use cache::{CachedDatabase, MetaCache, SCHEMA_VERSION};
pub use runlog::{NewRun, RunLogEntry, RunLogOutcome, RunLogStore, MAX_LOGGED_SQL_BYTES};
pub use search::{
    SavedQueryEntry, SavedQueryIndex, SavedQueryMatch, SearchEntry, SearchHit, SearchIndex,
    SearchResults, SearchScope,
};
pub use session::SessionStore;
