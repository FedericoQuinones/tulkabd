//! Tauri commands: thin wrappers over `tulka-app`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// The last text copied with a secret, until it is cleared or the app exits.
static SECRET_COPY: Mutex<Option<String>> = Mutex::new(None);

/// Set once the app is exiting, so the delayed clear does not touch a
/// clipboard plugin that is being torn down.
static EXITING: AtomicBool = AtomicBool::new(false);

/// Clears the clipboard if it still holds the last secret copy. Anything the
/// user copied since is left alone.
fn clear_secret_copy(app: &AppHandle) {
    let Some(text) = SECRET_COPY.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    // The plugin's state may already be gone during shutdown; that panics
    // inside `clipboard()`, and there is nothing left to clear then.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let clipboard = app.clipboard();
        if clipboard.read_text().is_ok_and(|current| current == text) {
            let _ = clipboard.clear();
        }
    }));
}

/// How long quitting waits for the clipboard before it carries on.
const QUIT_CLEAR_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

/// Called when the app is asked to quit, and again when it exits. The
/// clipboard is read on a helper thread: on Linux the read can block while
/// the main thread (which owns the WebView's clipboard) is waiting here, so
/// quitting waits a moment and then goes on regardless.
pub(crate) fn clear_secret_copy_on_quit(app: &AppHandle, exiting: bool) {
    if exiting {
        EXITING.store(true, Ordering::SeqCst);
    }
    let (done, wait) = std::sync::mpsc::channel();
    let app = app.clone();
    std::thread::spawn(move || {
        clear_secret_copy(&app);
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(QUIT_CLEAR_WAIT);
}

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
        *SECRET_COPY.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
        let app = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SECRET_CLIPBOARD_TTL);
            // A newer secret copy has its own timer.
            let current = SECRET_COPY
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if !EXITING.load(Ordering::SeqCst) && current.as_deref() == Some(text.as_str()) {
                clear_secret_copy(&app);
            }
        });
        Ok(())
    }
}
