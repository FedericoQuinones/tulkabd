use tauri::State;
use tauri::ipc::Channel;
use tulka_app::AppState;
use tulka_app::ai::AiVisible;
use tulka_app::ai::agent::{self, ClaudeStatus};
use tulka_app::ai::chat::{self, ChatEvent};

/// Where Claude Code's install instructions are.
const CLAUDE_INSTALL_DOCS: &str = "https://code.claude.com/docs/en/setup";

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

/// Sends a message to Claude Code in the background and streams what it does
/// to `on_event`. Fails with `busy` while that chat is already running.
#[tauri::command]
pub async fn ai_chat_send(
    state: State<'_, AppState>,
    chat_id: String,
    session_id: Option<String>,
    text: String,
    on_event: Channel<ChatEvent>,
) -> Result<(), String> {
    // A send fails only when the window is gone; there is nobody to tell.
    chat::send(&state.chats, chat_id, session_id, text, move |ev| {
        let _ = on_event.send(ev);
    })
    .await
}

#[tauri::command]
pub async fn ai_chat_stop(state: State<'_, AppState>, chat_id: String) -> Result<(), String> {
    chat::stop(&state.chats, &chat_id).await;
    Ok(())
}
