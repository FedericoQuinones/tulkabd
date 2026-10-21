mod commands;

use commands::{connections as conn, query};
use tulka_app::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::open_default())
        .invoke_handler(tauri::generate_handler![
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
