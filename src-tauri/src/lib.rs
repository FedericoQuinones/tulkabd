mod commands;

use commands::{connections as conn, query};
use std::sync::OnceLock;
use std::time::Instant;
use tulka_app::AppState;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Opt-in startup probe: with `TULKA_STARTUP_PROBE=1` the UI reports its first
/// paint and the elapsed time since launch is printed to stdout. Local only.
#[tauri::command]
fn startup_probe() {
    if std::env::var_os("TULKA_STARTUP_PROBE").is_some_and(|v| v == "1")
        && let Some(t) = STARTED.get()
    {
        println!("TULKA_READY_MS={}", t.elapsed().as_millis());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = STARTED.set(Instant::now());
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::open_default())
        .invoke_handler(tauri::generate_handler![
            startup_probe,
            conn::startup_error,
            conn::list_connections,
            conn::save_connection,
            conn::delete_connection,
            conn::test_connection,
            query::open_session,
            query::use_database,
            query::run_query,
            query::cancel_query,
            query::fetch_rows,
            query::copy_results,
            query::close_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
