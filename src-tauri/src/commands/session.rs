use tauri::State;
use tulka_app::AppState;

#[tauri::command]
pub async fn load_session(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(tulka_app::load_session(&state).await)
}

#[tauri::command]
pub async fn save_session(state: State<'_, AppState>, json: String) -> Result<(), String> {
    tulka_app::save_session(&state, json).await
}
