use tauri::State;
use tulka_app::{AppState, DataLocations};

#[tauri::command]
pub fn data_locations() -> DataLocations {
    tulka_app::data_locations()
}

/// Shows `config`, `queries` or `data` in the system file manager. Uses the
/// opener's free function from Rust: the plugin is not registered, so the
/// webview gets no opener commands.
#[tauri::command]
pub async fn reveal_data_location(which: String) -> Result<(), String> {
    // Blocking: creating folders, and a D-Bus call on Linux.
    tauri::async_runtime::spawn_blocking(move || {
        let path = tulka_app::reveal_target(&which)?;
        tauri_plugin_opener::reveal_item_in_dir(path)
            .map_err(|e| format!("could not show the location: {e}"))
    })
    .await
    .map_err(|e| format!("internal error: {e}"))?
}

#[tauri::command]
pub async fn clear_metadata_cache(state: State<'_, AppState>) -> Result<(), String> {
    tulka_app::clear_metadata_cache(&state).await;
    Ok(())
}

#[tauri::command]
pub fn mcp_setup() -> tulka_app::ai::setup::McpSetup {
    tulka_app::ai::setup::mcp_setup()
}

/// Copies one of the MCP setups (`claude`, `json`, `vscode`) to the clipboard.
#[tauri::command]
pub fn copy_mcp_setup(app: tauri::AppHandle, kind: String) -> Result<(), String> {
    let clipboard: std::sync::Arc<dyn tulka_app::Clipboard> =
        std::sync::Arc::new(super::PluginClipboard(app));
    tulka_app::ai::setup::copy_mcp_setup(&kind, clipboard)
}
