use std::sync::Arc;

use super::PluginClipboard;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tulka_app::tulka_core::Value;
use tulka_app::{
    AppState, Clipboard, CopyMode, CopyOutcome, RunSummary, Selection, SessionInfo, UiEvent,
};

#[tauri::command]
pub async fn open_session(
    state: State<'_, AppState>,
    tab_id: String,
    connection_id: String,
    database: Option<String>,
    password: Option<String>,
) -> Result<SessionInfo, String> {
    tulka_app::open_session(&state, tab_id, connection_id, database, password).await
}

#[tauri::command]
pub async fn use_database(
    state: State<'_, AppState>,
    tab_id: String,
    database: String,
) -> Result<(), String> {
    tulka_app::use_database(&state, tab_id, database).await
}

#[tauri::command]
pub async fn run_query(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
    on_event: Channel<UiEvent>,
) -> Result<RunSummary, String> {
    // A send fails only when the window is gone; there is nobody to tell.
    tulka_app::run_query(&state, tab_id, sql, move |ev| {
        let _ = on_event.send(ev);
    })
    .await
}

#[tauri::command]
pub async fn cancel_query(state: State<'_, AppState>, tab_id: String) -> Result<(), String> {
    tulka_app::cancel_query(&state, tab_id).await
}

#[tauri::command]
pub async fn fetch_rows(
    state: State<'_, AppState>,
    tab_id: String,
    result_index: u32,
    offset: usize,
    limit: usize,
) -> Result<Vec<Vec<Value>>, String> {
    tulka_app::fetch_rows(&state, tab_id, result_index, offset, limit).await
}

#[tauri::command]
pub async fn copy_results(
    app: AppHandle,
    state: State<'_, AppState>,
    tab_id: String,
    result_index: u32,
    mode: CopyMode,
    selection: Selection,
) -> Result<CopyOutcome, String> {
    let clipboard: Arc<dyn Clipboard> = Arc::new(PluginClipboard(app));
    tulka_app::copy_results(&state, tab_id, result_index, mode, selection, clipboard).await
}

#[tauri::command]
pub async fn close_session(state: State<'_, AppState>, tab_id: String) -> Result<(), String> {
    tulka_app::close_session(&state, tab_id).await
}
