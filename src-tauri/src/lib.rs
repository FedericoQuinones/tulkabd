mod commands;
mod events;

use commands::{connections as conn, explorer as ex, files, queries, query, session};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tauri::Manager;
use tulka_app::AppState;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Opt-in startup probe: with `TULKA_STARTUP_PROBE=1` the UI reports the first
/// paint of the restored workspace and the elapsed time since launch is printed
/// to stdout. Local only.
#[tauri::command]
fn startup_probe() {
    if std::env::var_os("TULKA_STARTUP_PROBE").is_some_and(|v| v == "1")
        && let Some(t) = STARTED.get()
    {
        println!("TULKA_READY_MS={}", t.elapsed().as_millis());
    }
}

/// Opt-in UI probe: prints `msg` and returns true under `TULKA_STARTUP_PROBE=1`;
/// otherwise does nothing and returns false so the UI stops reporting.
#[tauri::command]
fn probe_log(msg: String) -> bool {
    let on = std::env::var_os("TULKA_STARTUP_PROBE").is_some_and(|v| v == "1");
    if on {
        println!("{msg}");
    }
    on
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = STARTED.set(Instant::now());
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        // For the open dialog of `pick_sql_files` (from Rust). No capability
        // grants the webview a dialog permission.
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // The state needs the AppHandle (to emit events), so it is built
            // here. Tauri runs setup before the webview can invoke a command.
            // The metadata cache opens in milliseconds; the search index loads
            // on its own thread.
            let events = Arc::new(events::TauriEvents(app.handle().clone()));
            app.manage(AppState::open_default(events));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            startup_probe,
            probe_log,
            conn::startup_error,
            conn::list_connections,
            conn::save_connection,
            conn::delete_connection,
            conn::test_connection,
            conn::list_folders,
            conn::create_folder,
            conn::rename_folder,
            conn::delete_folder,
            conn::move_connection,
            conn::move_folder,
            conn::list_expanded,
            conn::set_expanded,
            conn::get_preferences,
            conn::set_zoom,
            conn::list_server_databases,
            conn::parse_connection_string,
            conn::duplicate_connection,
            conn::copy_connection_string,
            ex::connect_explorer,
            ex::disconnect_explorer,
            ex::refresh_metadata,
            ex::explorer_databases,
            ex::explorer_objects,
            ex::explorer_columns,
            ex::table_structure,
            ex::object_definition,
            ex::search_objects,
            ex::completion_metadata,
            query::open_session,
            query::use_database,
            query::run_query,
            query::cancel_query,
            query::fetch_rows,
            query::copy_results,
            query::close_session,
            session::load_session,
            session::save_session,
            queries::list_saved_queries,
            queries::read_saved_query,
            queries::save_query,
            queries::rename_saved_query,
            queries::delete_saved_query,
            queries::reveal_saved_query,
            files::pick_sql_files,
            files::allow_dropped_paths,
            files::read_sql_file,
            files::write_sql_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
