//! Tauri commands: thin wrappers over `tulka-app`.

use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tulka_app::Clipboard;

pub mod connections;
pub mod explorer;
pub mod files;
pub mod guard;
pub mod queries;
pub mod query;
pub mod runlog;
pub mod session;
pub mod settings;

/// How long copied text with a secret stays on the clipboard.
const SECRET_CLIPBOARD_TTL: std::time::Duration = std::time::Duration::from_secs(45);

/// Writes copied text to the system clipboard through the plugin.
pub(crate) struct PluginClipboard(pub(crate) AppHandle);

impl Clipboard for PluginClipboard {
    fn write_text(&self, text: String) -> Result<(), String> {
        self.0
            .clipboard()
            .write_text(text)
            .map_err(|e| e.to_string())
    }

    fn write_secret_text(&self, text: String) -> Result<(), String> {
        self.0
            .clipboard()
            .write_secret(text.clone())
            .map_err(|e| e.to_string())?;
        // Copied secrets should not sit on the clipboard indefinitely. The
        // clipboard is cleared only if it still holds this exact text, so
        // anything the user copied since is left alone.
        let app = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SECRET_CLIPBOARD_TTL);
            let clipboard = app.clipboard();
            if clipboard.read_text().is_ok_and(|current| current == text) {
                let _ = clipboard.clear();
            }
        });
        Ok(())
    }
}
