mod commands;
mod events;

use commands::{
    connections as conn, explorer as ex, files, guard, queries, query, runlog, session, settings,
};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tauri::{DragDropEvent, Emitter, Manager, RunEvent, WindowEvent};
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

/// Event telling the UI that the user asked to quit the app (Cmd+Q on macOS).
pub const QUIT_REQUESTED: &str = "quit-requested";

/// Whether an exit request must wait for the UI. Only macOS needs it: Cmd+Q
/// quits without a close request on the window, so a transaction left open
/// would be dropped silently. A request with an exit code (`app.exit`), or
/// one made after the last window is gone, is the end of an exit the UI
/// already approved (it destroys the window when the user has answered), and
/// is never held back; otherwise the app could not quit.
fn hold_exit(macos: bool, code: Option<i32>, windows: usize) -> bool {
    macos && code.is_none() && windows > 0
}

/// Every command the webview can invoke. The list is the one place to add a
/// command: it builds the handler and the `registered_commands` test compares
/// it with an explicit list, so a new command is a reviewed decision (none
/// may widen the allowed `.sql` files).
macro_rules! commands {
    ($($cmd:path),* $(,)?) => {
        #[cfg(test)]
        const COMMAND_NAMES: &[&str] = &[$(stringify!($cmd)),*];

        fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
            tauri::generate_handler![$($cmd),*]
        }
    };
}

commands![
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
    conn::set_preferences,
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
    query::set_result_view,
    query::copy_results,
    query::close_session,
    query::transaction_count,
    query::end_transactions,
    session::load_session,
    session::save_session,
    queries::list_saved_queries,
    queries::read_saved_query,
    queries::save_query,
    queries::rename_saved_query,
    queries::delete_saved_query,
    queries::reveal_saved_query,
    files::pick_sql_files,
    files::sql_files_exist,
    files::read_sql_file,
    files::write_sql_file,
    guard::set_guard_off_for_session,
    guard::set_connection_guard,
    guard::guard_off_for_session,
    runlog::list_run_log,
    runlog::clear_run_log,
    runlog::log_cancelled_run,
    settings::data_locations,
    settings::reveal_data_location,
    settings::clear_metadata_cache,
];

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
            // Old run log entries are pruned a little after startup, so the
            // first paint does not wait for it, and then once an hour. No
            // statement ever waits for it.
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                loop {
                    tauri::async_runtime::block_on(async {
                        tulka_app::prune_run_log(&handle.state::<AppState>()).await;
                    });
                    std::thread::sleep(std::time::Duration::from_secs(60 * 60));
                }
            });
            Ok(())
        })
        // Files dropped on the window are allowed here, from the native event,
        // before the UI hears of them: the webview cannot allow a path itself.
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                let paths = paths
                    .iter()
                    .filter_map(|p| p.to_str().map(str::to_owned))
                    .collect();
                // Allowed in memory before the UI hears of the drop; writing
                // them down for later runs can take a while for a big drop,
                // so it does not hold up this thread.
                let accepted = tulka_app::accept_dropped_files(&window.state::<AppState>(), paths);
                if accepted.is_empty() {
                    return;
                }
                let handle = window.app_handle().clone();
                let remembered = accepted.clone();
                std::thread::spawn(move || {
                    tulka_app::remember_allowed_files(&handle.state::<AppState>(), &remembered);
                });
                let _ = window.emit("sql-files-dropped", accepted);
            }
        })
        .invoke_handler(handler())
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::ExitRequested { code, api, .. } = event
                && hold_exit(cfg!(target_os = "macos"), code, app.webview_windows().len())
            {
                api.prevent_exit();
                let _ = app.emit(QUIT_REQUESTED, ());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::hold_exit;

    #[test]
    fn quit_waits_for_the_ui_only_on_macos_with_a_window_open() {
        assert!(hold_exit(true, None, 1));
        // Other systems close the window first, which the UI already handles.
        assert!(!hold_exit(false, None, 1));
        // An exit with a code is explicit, and no window means the UI is done.
        assert!(!hold_exit(true, Some(0), 1));
        assert!(!hold_exit(true, None, 0));
    }

    /// The commands the webview may call. Adding one means editing this list
    /// on purpose: none may add to the allowed `.sql` files (that is done only
    /// from the native dialog and drop events).
    const EXPECTED: &[&str] = &[
        "conn::copy_connection_string",
        "conn::create_folder",
        "conn::delete_connection",
        "conn::delete_folder",
        "conn::duplicate_connection",
        "conn::get_preferences",
        "conn::list_connections",
        "conn::list_expanded",
        "conn::list_folders",
        "conn::list_server_databases",
        "conn::move_connection",
        "conn::move_folder",
        "conn::parse_connection_string",
        "conn::rename_folder",
        "conn::save_connection",
        "conn::set_expanded",
        "conn::set_preferences",
        "conn::set_zoom",
        "conn::startup_error",
        "conn::test_connection",
        "ex::completion_metadata",
        "ex::connect_explorer",
        "ex::disconnect_explorer",
        "ex::explorer_columns",
        "ex::explorer_databases",
        "ex::explorer_objects",
        "ex::object_definition",
        "ex::refresh_metadata",
        "ex::search_objects",
        "ex::table_structure",
        "files::pick_sql_files",
        "files::read_sql_file",
        "files::sql_files_exist",
        "files::write_sql_file",
        "guard::guard_off_for_session",
        "guard::set_connection_guard",
        "guard::set_guard_off_for_session",
        "probe_log",
        "queries::delete_saved_query",
        "queries::list_saved_queries",
        "queries::read_saved_query",
        "queries::rename_saved_query",
        "queries::reveal_saved_query",
        "queries::save_query",
        "query::cancel_query",
        "query::close_session",
        "query::copy_results",
        "query::end_transactions",
        "query::fetch_rows",
        "query::open_session",
        "query::run_query",
        "query::set_result_view",
        "query::transaction_count",
        "query::use_database",
        "runlog::clear_run_log",
        "runlog::list_run_log",
        "runlog::log_cancelled_run",
        "session::load_session",
        "session::save_session",
        "settings::clear_metadata_cache",
        "settings::data_locations",
        "settings::reveal_data_location",
        "startup_probe",
    ];

    #[test]
    fn registered_commands_are_the_reviewed_ones() {
        let mut got: Vec<String> = super::COMMAND_NAMES
            .iter()
            .map(|n| n.replace(' ', ""))
            .collect();
        got.sort();
        let mut want: Vec<String> = EXPECTED.iter().map(|n| n.to_string()).collect();
        want.sort();
        assert_eq!(got, want);
    }
}
