//! The workspace session: an opaque JSON owned by the UI, kept in its own
//! SQLite file. It holds editor text and is never logged.

use crate::connections::blocking;
use crate::state::AppState;

/// The saved session JSON, or `None` when there is none.
pub async fn load_session(state: &AppState) -> Option<String> {
    let store = state.session.clone();
    blocking(move || store.load()).await.ok().flatten()
}

/// Replaces the saved session. Works even when the connections config is
/// read-only.
pub async fn save_session(state: &AppState, json: String) -> Result<(), String> {
    let store = state.session.clone();
    blocking(move || store.save(&json)).await?
}
