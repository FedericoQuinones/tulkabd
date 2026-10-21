use tauri::State;
use tulka_app::tulka_core::{ConnectionConfig, Folder, ServerInfo};
use tulka_app::{AppState, SaveConnectionResult};

/// Why the connections file could not be read at startup, if it could not.
#[tauri::command]
pub fn startup_error(state: State<'_, AppState>) -> Option<String> {
    state.startup_error()
}

#[tauri::command]
pub async fn list_connections(state: State<'_, AppState>) -> Result<Vec<ConnectionConfig>, String> {
    Ok(tulka_app::list_connections(&state).await)
}

#[tauri::command]
pub async fn save_connection(
    state: State<'_, AppState>,
    conn: ConnectionConfig,
    password: Option<String>,
) -> Result<SaveConnectionResult, String> {
    tulka_app::save_connection(&state, conn, password).await
}

#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> Result<(), String> {
    tulka_app::delete_connection(&state, id).await
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    conn: ConnectionConfig,
    password: Option<String>,
) -> Result<ServerInfo, String> {
    tulka_app::test_connection(&state, conn, password).await
}

#[tauri::command]
pub async fn list_folders(state: State<'_, AppState>) -> Result<Vec<Folder>, String> {
    Ok(tulka_app::list_folders(&state).await)
}

#[tauri::command]
pub async fn create_folder(
    state: State<'_, AppState>,
    name: String,
    parent_id: Option<String>,
) -> Result<Folder, String> {
    tulka_app::create_folder(&state, name, parent_id).await
}

#[tauri::command]
pub async fn rename_folder(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> Result<(), String> {
    tulka_app::rename_folder(&state, id, name).await
}

#[tauri::command]
pub async fn delete_folder(state: State<'_, AppState>, id: String) -> Result<(), String> {
    tulka_app::delete_folder(&state, id).await
}

#[tauri::command]
pub async fn move_connection(
    state: State<'_, AppState>,
    connection_id: String,
    folder_id: Option<String>,
) -> Result<(), String> {
    tulka_app::move_connection(&state, connection_id, folder_id).await
}

#[tauri::command]
pub async fn move_folder(
    state: State<'_, AppState>,
    id: String,
    parent_id: Option<String>,
) -> Result<(), String> {
    tulka_app::move_folder(&state, id, parent_id).await
}

#[tauri::command]
pub async fn list_expanded(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(tulka_app::list_expanded(&state).await)
}

#[tauri::command]
pub async fn set_expanded(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    tulka_app::set_expanded(&state, ids).await
}

#[tauri::command]
pub async fn list_server_databases(
    state: State<'_, AppState>,
    conn: ConnectionConfig,
    password: Option<String>,
) -> Result<Vec<String>, String> {
    tulka_app::list_server_databases(&state, conn, password).await
}
