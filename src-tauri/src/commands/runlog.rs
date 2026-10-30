use tauri::State;
use tulka_app::AppState;
use tulka_app::tulka_store::RunLogEntry;

#[tauri::command]
pub async fn list_run_log(
    state: State<'_, AppState>,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<RunLogEntry>, String> {
    Ok(tulka_app::list_run_log(&state, before, limit).await)
}

#[tauri::command]
pub async fn clear_run_log(state: State<'_, AppState>) -> Result<(), String> {
    tulka_app::clear_run_log(&state).await
}

#[tauri::command]
pub async fn log_cancelled_run(
    state: State<'_, AppState>,
    tab_id: String,
    sql: String,
) -> Result<(), String> {
    tulka_app::log_cancelled_run(&state, tab_id, sql).await
}
