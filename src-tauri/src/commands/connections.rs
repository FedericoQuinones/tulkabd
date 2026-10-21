use tauri::State;
use tulka_app::tulka_core::{ConnectionConfig, ServerInfo};
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
