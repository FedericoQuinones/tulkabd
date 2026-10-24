//! `.sql` files opened from disk. The open dialog runs from Rust (the
//! webview gets no dialog permission) and allows the paths picked in it.
//! The UI allows the paths dropped on the window or restored with the
//! session through `allow_dropped_paths`, which takes any `.sql` path: the
//! allowed list keeps the UI to the files the user chose, it is not a
//! security boundary against the webview.

use tauri::{State, Window};
use tauri_plugin_dialog::DialogExt;
use tulka_app::{AppState, FileEncoding, LineEnding, OpenedFile};

/// Shows the native open dialog for `.sql` files (several at once) and
/// allows the picked ones. `filter_name` labels the `*.sql` filter (it
/// comes from the UI strings). Empty when the dialog is cancelled.
#[tauri::command]
pub async fn pick_sql_files(
    window: Window,
    state: State<'_, AppState>,
    filter_name: String,
) -> Result<Vec<String>, String> {
    let dialog = window
        .dialog()
        .file()
        .add_filter(filter_name, &["sql", "SQL", "Sql"])
        .set_parent(&window);
    // Blocks until the dialog closes: not on an async worker.
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_files())
        .await
        .map_err(|e| format!("internal error: {e}"))?
        .unwrap_or_default();
    // A path that is not valid Unicode could not be sent back as is.
    let paths = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok()?.into_os_string().into_string().ok())
        .collect();
    Ok(tulka_app::allow_paths(&state, paths))
}

/// Allows `.sql` paths dropped on the window or restored with the session.
/// The backend cannot tell where they came from: any absolute `.sql` path
/// is allowed.
#[tauri::command]
pub fn allow_dropped_paths(state: State<'_, AppState>, paths: Vec<String>) {
    tulka_app::allow_paths(&state, paths);
}

/// For each path, whether it is allowed and its file still exists; `null`
/// when that could not be checked.
#[tauri::command]
pub async fn sql_files_exist(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<Option<bool>>, String> {
    tulka_app::sql_files_exist(&state, paths).await
}

#[tauri::command]
pub async fn read_sql_file(state: State<'_, AppState>, path: String) -> Result<OpenedFile, String> {
    tulka_app::read_sql_file(&state, path).await
}

#[tauri::command]
pub async fn write_sql_file(
    state: State<'_, AppState>,
    path: String,
    text: String,
    encoding: FileEncoding,
    line_ending: LineEnding,
) -> Result<(), String> {
    tulka_app::write_sql_file(&state, path, text, encoding, line_ending).await
}
