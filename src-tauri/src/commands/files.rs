//! `.sql` files from disk (plan D7). The open dialog runs from Rust: the
//! webview gets no dialog permission, so only paths the user picked here,
//! dropped on the window or restored with the session are allowed.

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
        .add_filter(filter_name, &["sql", "SQL"])
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
#[tauri::command]
pub fn allow_dropped_paths(state: State<'_, AppState>, paths: Vec<String>) {
    tulka_app::allow_paths(&state, paths);
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
