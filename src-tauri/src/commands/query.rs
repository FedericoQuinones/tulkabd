use std::sync::Arc;

use super::PluginClipboard;
use tauri::ipc::Channel;
use tauri::{AppHandle, State, Window};
use tauri_plugin_dialog::DialogExt;
use tulka_app::tulka_core::{SaveFormat, Value};
use tulka_app::{
    AppState, Clipboard, CopyMode, CopyOutcome, RunSummary, SaveOutcome, Selection, SessionInfo,
    UiEvent, ViewSpec,
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
    confirmed: bool,
    on_event: Channel<UiEvent>,
) -> Result<RunSummary, String> {
    // A send fails only when the window is gone; there is nobody to tell.
    tulka_app::run_query(&state, tab_id, sql, confirmed, move |ev| {
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
pub async fn set_result_view(
    state: State<'_, AppState>,
    tab_id: String,
    result_index: u32,
    view: ViewSpec,
) -> Result<usize, String> {
    tulka_app::set_result_view(&state, tab_id, result_index, view).await
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

/// Asks for a file in the native save dialog (the webview gets no dialog
/// permission and never names a path itself) and saves the visible rows of
/// the result there. `default_name` is only the suggestion shown in the
/// dialog; `filter_name` labels the file type filter (it comes from the UI
/// strings). `null` when the dialog is cancelled.
#[tauri::command]
pub async fn save_results(
    window: Window,
    state: State<'_, AppState>,
    tab_id: String,
    result_index: u32,
    format: SaveFormat,
    default_name: String,
    filter_name: String,
) -> Result<Option<SaveOutcome>, String> {
    let ext = format.extension();
    let dialog = window
        .dialog()
        .file()
        .add_filter(filter_name, &[ext, &ext.to_uppercase()])
        .set_file_name(default_name)
        .set_parent(&window);
    // Blocks until the dialog closes: not on an async worker.
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_save_file())
        .await
        .map_err(|e| format!("internal error: {e}"))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    // Some systems do not add the extension of the filter, but never at
    // the price of replacing a file the user was not asked about.
    let path = tulka_app::resolve_save_path(path, ext);
    tulka_app::save_results(&state, tab_id, result_index, format, path)
        .await
        .map(Some)
}

#[tauri::command]
pub async fn close_session(state: State<'_, AppState>, tab_id: String) -> Result<(), String> {
    tulka_app::close_session(&state, tab_id).await
}

#[tauri::command]
pub async fn transaction_count(state: State<'_, AppState>, tab_id: String) -> Result<u32, String> {
    tulka_app::transaction_count(&state, tab_id).await
}

#[tauri::command]
pub async fn end_transactions(
    state: State<'_, AppState>,
    tab_id: String,
    commit: bool,
) -> Result<(), String> {
    tulka_app::end_transactions(&state, tab_id, commit).await
}
