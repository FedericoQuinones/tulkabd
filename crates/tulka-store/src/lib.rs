//! SQLite cache of the Explorer metadata, so search works at startup even
//! offline. Tauri-free and blocking: callers use `spawn_blocking`.

mod cache;
mod search;
mod session;

pub use cache::{CachedDatabase, MetaCache, SCHEMA_VERSION};
pub use search::{SearchEntry, SearchHit, SearchIndex, SearchResults, SearchScope};
pub use session::SessionStore;
