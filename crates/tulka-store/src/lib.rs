//! SQLite cache of the Explorer metadata, so search works at startup even
//! offline. Tauri-free and blocking: callers use `spawn_blocking`.

mod cache;

pub use cache::{CachedDatabase, MetaCache, SCHEMA_VERSION};
