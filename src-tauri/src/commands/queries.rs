use tauri::State;
use tulka_app::{AppState, SavedQuery};

#[tauri::command]
pub async fn list_saved_queries(
    state: State<'_, AppState>,
    database: Option<String>,
) -> Result<Vec<SavedQuery>, String> {
    Ok(tulka_app::list_saved_queries(&state, database).await)
}

#[tauri::command]
pub async fn read_saved_query(
    state: State<'_, AppState>,
    database: String,
    name: String,
) -> Result<String, String> {
    tulka_app::read_saved_query(&state, database, name).await
}

#[tauri::command]
pub async fn save_query(
    state: State<'_, AppState>,
    database: String,
    name: String,
    text: String,
    overwrite: bool,
) -> Result<(), String> {
    tulka_app::save_query(&state, database, name, text, overwrite).await
}

#[tauri::command]
pub async fn rename_saved_query(
    state: State<'_, AppState>,
    database: String,
    name: String,
    new_name: String,
) -> Result<(), String> {
    tulka_app::rename_saved_query(&state, database, name, new_name).await
}

#[tauri::command]
pub async fn delete_saved_query(
    state: State<'_, AppState>,
    database: String,
    name: String,
) -> Result<(), String> {
    tulka_app::delete_saved_query(&state, database, name).await
}

/// Shows the query's `.sql` file in the system file manager. Uses the
/// opener's free function from Rust: the plugin is not registered, so the
/// webview gets no opener commands.
#[tauri::command]
pub async fn reveal_saved_query(
    state: State<'_, AppState>,
    database: String,
    name: String,
) -> Result<(), String> {
    let path = tulka_app::saved_query_path(&state, database, name).await?;
    // Blocking (a D-Bus call on Linux).
    tauri::async_runtime::spawn_blocking(move || tauri_plugin_opener::reveal_item_in_dir(path))
        .await
        .map_err(|e| format!("internal error: {e}"))?
        .map_err(|e| format!("could not show the file: {e}"))
}
