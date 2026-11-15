//! Forwards the app's background events to the window.

use tauri::{AppHandle, Emitter, Runtime};
use tulka_app::{AppEvent, AppEvents};

/// Name of the Tauri event carrying every [`AppEvent`] (`onAppEvent` in `ipc.ts`).
pub const APP_EVENT: &str = "app-event";

/// Emits each [`AppEvent`] to all windows as `app-event`.
pub struct TauriEvents<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> AppEvents for TauriEvents<R> {
    fn emit(&self, ev: AppEvent) {
        // A closing window has nobody to tell: ignore the error.
        let _ = self.0.emit(APP_EVENT, ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tulka_app::ConnectionStatus;

    fn json(ev: &AppEvent) -> String {
        serde_json::to_string(ev).unwrap()
    }

    /// The strings below are the `AppEvent` type declared in `src/lib/ipc.ts`.
    #[test]
    fn app_events_match_the_ipc_contract() {
        assert_eq!(APP_EVENT, "app-event");
        assert_eq!(
            json(&AppEvent::ExplorerStatus {
                connection_id: "c1".into(),
                status: ConnectionStatus::Error,
                message: Some("boom".into()),
            }),
            r#"{"kind":"explorerStatus","connectionId":"c1","status":"error","message":"boom"}"#
        );
        assert_eq!(
            json(&AppEvent::ExplorerStatus {
                connection_id: "c1".into(),
                status: ConnectionStatus::Connecting,
                message: None,
            }),
            r#"{"kind":"explorerStatus","connectionId":"c1","status":"connecting","message":null}"#
        );
        assert_eq!(
            json(&AppEvent::IndexProgress {
                connection_id: "c1".into(),
                database: "db".into(),
                done: true,
                no_access: false,
            }),
            r#"{"kind":"indexProgress","connectionId":"c1","database":"db","done":true,"noAccess":false}"#
        );
        assert_eq!(json(&AppEvent::IndexLoaded), r#"{"kind":"indexLoaded"}"#);
        assert_eq!(
            json(&AppEvent::IndexIdle {
                connection_id: "c1".into()
            }),
            r#"{"kind":"indexIdle","connectionId":"c1"}"#
        );
    }

    /// The strings below are the `ChatEvent` type declared in `src/lib/ipc.ts`.
    #[test]
    fn chat_events_match_the_ipc_contract() {
        use tulka_app::ai::chat::ChatEvent;
        let j = |ev: ChatEvent| serde_json::to_string(&ev).unwrap();
        assert_eq!(
            j(ChatEvent::Started {
                session_id: "s".into()
            }),
            r#"{"kind":"started","sessionId":"s"}"#
        );
        assert_eq!(
            j(ChatEvent::TextDelta { text: "a".into() }),
            r#"{"kind":"textDelta","text":"a"}"#
        );
        assert_eq!(
            j(ChatEvent::Text { text: "a".into() }),
            r#"{"kind":"text","text":"a"}"#
        );
        assert_eq!(
            j(ChatEvent::ToolCall {
                id: "t".into(),
                tool: "run_query".into(),
                input: serde_json::json!({"sql":"SELECT 1"}),
            }),
            r#"{"kind":"toolCall","id":"t","tool":"run_query","input":{"sql":"SELECT 1"}}"#
        );
        assert_eq!(
            j(ChatEvent::ToolResult {
                id: "t".into(),
                ok: true,
                preview: "p".into()
            }),
            r#"{"kind":"toolResult","id":"t","ok":true,"preview":"p"}"#
        );
        assert_eq!(
            j(ChatEvent::Done {
                duration_ms: 5,
                is_error: false,
                result: "r".into()
            }),
            r#"{"kind":"done","durationMs":5,"isError":false,"result":"r"}"#
        );
        assert_eq!(
            j(ChatEvent::Failed {
                message: "m".into()
            }),
            r#"{"kind":"failed","message":"m"}"#
        );
    }
}
