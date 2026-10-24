use tauri::State;
use tulka_app::AppState;
use tulka_app::tulka_core::GuardLevel;

#[tauri::command]
pub fn set_guard_off_for_session(state: State<'_, AppState>, connection_id: String, off: bool) {
    tulka_app::set_guard_off_for_session(&state, &connection_id, off);
}

#[tauri::command]
pub async fn set_connection_guard(
    state: State<'_, AppState>,
    connection_id: String,
    level: GuardLevel,
) -> Result<GuardLevel, String> {
    tulka_app::set_connection_guard(&state, connection_id, level).await
}

#[tauri::command]
pub fn guard_off_for_session(state: State<'_, AppState>) -> Vec<String> {
    tulka_app::guard_off_for_session(&state)
}
