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

/// Writes copied text to the system clipboard through the plugin.
pub(crate) struct PluginClipboard(pub(crate) AppHandle);

impl Clipboard for PluginClipboard {
    fn write_text(&self, text: String) -> Result<(), String> {
        self.0
            .clipboard()
            .write_text(text)
            .map_err(|e| e.to_string())
    }
}
