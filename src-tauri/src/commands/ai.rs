use tulka_app::ai::AiVisible;
use tulka_app::ai::agent::{self, ClaudeStatus};

/// Where Claude Code's install instructions are.
const CLAUDE_INSTALL_DOCS: &str = "https://docs.claude.com/en/docs/claude-code/setup";

#[tauri::command]
pub async fn ai_claude_status() -> ClaudeStatus {
    agent::claude_status().await
}

#[tauri::command]
pub async fn ai_connect_claude() -> Result<ClaudeStatus, String> {
    agent::connect_claude().await
}

/// Opens a terminal running Claude Code with the tulkabd tools.
#[tauri::command]
pub async fn ai_open_terminal() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(agent::open_claude_terminal)
        .await
        .map_err(|e| format!("internal error: {e}"))?
}

#[tauri::command]
pub async fn ai_visible_connections() -> Result<Vec<AiVisible>, String> {
    tulka_app::ai::visible_connections().await
}

/// Opens the fixed install page in the browser. Uses the opener's free
/// function from Rust: the plugin is not registered, so the webview gets no
/// opener commands.
#[tauri::command]
pub async fn ai_open_install_docs() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        tauri_plugin_opener::open_url(CLAUDE_INSTALL_DOCS, None::<&str>)
            .map_err(|e| format!("could not open the page: {e}"))
    })
    .await
    .map_err(|e| format!("internal error: {e}"))?
}
