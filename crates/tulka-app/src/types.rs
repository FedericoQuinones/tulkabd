//! Values returned or streamed to the UI. Their JSON shapes mirror
//! `src/lib/ipc.ts`.

use serde::{Deserialize, Serialize};
use tulka_core::{Column, ConnectionConfig, ExecEvent, MessageKind, ServerInfo};

/// Command error meaning there is no saved password and none was given.
pub const PASSWORD_REQUIRED: &str = "password_required";
/// Command error meaning the tab's connection was lost; only `open_session`
/// gets the tab working again.
pub const DISCONNECTED: &str = "disconnected";

/// Events streamed by `run_query`: [`ExecEvent`] with `Rows` replaced by
/// `RowCount`, because rows stay in the Rust-side buffers.
///
/// Serialized internally tagged, like [`ExecEvent`]:
/// `{"kind": "rowCount", "index": 0, "total": 500, "truncated": false}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UiEvent {
    /// A new result set starts; its buffer exists from now on.
    ResultStart {
        index: u32,
        columns: Vec<Column>,
    },
    /// Result set `index` now holds `total` rows (at most the cap);
    /// `truncated` once rows beyond the cap were dropped.
    RowCount {
        index: u32,
        total: usize,
        truncated: bool,
    },
    /// A server message. Serialized as `messageKind` because the tag is `kind`.
    Message {
        #[serde(rename = "messageKind")]
        kind: MessageKind,
        text: String,
        line: Option<u32>,
    },
    RowsAffected {
        count: u64,
    },
    BatchDone {
        batch: u32,
    },
    Disconnected {
        reason: String,
    },
}

impl UiEvent {
    /// Converts an execution event that is forwarded as is. `Rows` has no
    /// counterpart and yields `None`.
    pub(crate) fn from_exec(ev: ExecEvent) -> Option<Self> {
        Some(match ev {
            ExecEvent::ResultStart { index, columns } => UiEvent::ResultStart { index, columns },
            ExecEvent::Rows { .. } => return None,
            ExecEvent::Message { kind, text, line } => UiEvent::Message { kind, text, line },
            ExecEvent::RowsAffected { count } => UiEvent::RowsAffected { count },
            ExecEvent::BatchDone { batch } => UiEvent::BatchDone { batch },
            ExecEvent::Disconnected { reason } => UiEvent::Disconnected { reason },
        })
    }
}

/// How a `run_query` ended. Serialized as `"ok"`, `"error"`, `"cancelled"`
/// or `"disconnected"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunOutcome {
    Ok,
    Error,
    Cancelled,
    Disconnected,
}

/// Returned by `run_query` once the script ended.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub elapsed_ms: u64,
    /// RFC 3339 UTC timestamp, e.g. `2026-10-21T14:03:00.123Z`.
    pub finished_at: String,
    pub outcome: RunOutcome,
}

/// Returned by `open_session`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub server: ServerInfo,
    /// Databases the connection exposes (all, or only the selected ones).
    pub databases: Vec<String>,
    pub current_database: String,
}

/// Returned by `save_connection`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveConnectionResult {
    /// The connection as stored (with the final `password_saved`).
    pub connection: ConnectionConfig,
    /// The password could not be stored because there is no usable keychain.
    pub keyring_unavailable: bool,
}

/// Text format of `copy_results`: `"excel"` or `"ai"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CopyMode {
    Excel,
    Ai,
}

/// Returned by `copy_results`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CopyOutcome {
    /// Data rows copied (the header line is not counted).
    pub rows: usize,
    /// Estimated tokens of the copied text; `0` in Excel mode.
    pub tokens: usize,
}

/// The system clipboard, injected so tests can use a fake. The Tauri layer
/// passes the clipboard-manager plugin.
pub trait Clipboard: Send + Sync {
    fn write_text(&self, text: String) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tulka_core::Environment;

    fn to_json<T: Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    #[test]
    fn ui_event_json_shape() {
        let col = Column {
            name: "a".into(),
            type_name: "int".into(),
        };
        let cases = [
            (
                UiEvent::ResultStart {
                    index: 0,
                    columns: vec![col],
                },
                json!({"kind": "resultStart", "index": 0, "columns": [{"name": "a", "typeName": "int"}]}),
            ),
            (
                UiEvent::RowCount {
                    index: 1,
                    total: 500,
                    truncated: false,
                },
                json!({"kind": "rowCount", "index": 1, "total": 500, "truncated": false}),
            ),
            (
                UiEvent::Message {
                    kind: MessageKind::Warning,
                    text: "hola".into(),
                    line: None,
                },
                json!({"kind": "message", "messageKind": "warning", "text": "hola", "line": null}),
            ),
            (
                UiEvent::RowsAffected { count: 3 },
                json!({"kind": "rowsAffected", "count": 3}),
            ),
            (
                UiEvent::BatchDone { batch: 2 },
                json!({"kind": "batchDone", "batch": 2}),
            ),
            (
                UiEvent::Disconnected {
                    reason: "gone".into(),
                },
                json!({"kind": "disconnected", "reason": "gone"}),
            ),
        ];
        for (ev, expected) in cases {
            assert_eq!(to_json(&ev), expected);
        }
    }

    #[test]
    fn ui_event_matches_exec_event_shape() {
        // Every forwarded event serializes exactly like the ExecEvent it
        // came from.
        let events = [
            ExecEvent::Message {
                kind: MessageKind::Error,
                text: "x".into(),
                line: Some(4),
            },
            ExecEvent::RowsAffected { count: 1 },
            ExecEvent::BatchDone { batch: 0 },
            ExecEvent::Disconnected { reason: "r".into() },
        ];
        for ev in events {
            let expected = to_json(&ev);
            assert_eq!(to_json(&UiEvent::from_exec(ev).unwrap()), expected);
        }
        let rows = ExecEvent::Rows {
            index: 0,
            rows: vec![],
        };
        assert_eq!(UiEvent::from_exec(rows), None);
    }

    #[test]
    fn run_summary_json_shape() {
        let s = RunSummary {
            elapsed_ms: 42,
            finished_at: "2026-10-21T14:03:00.123Z".into(),
            outcome: RunOutcome::Cancelled,
        };
        assert_eq!(
            to_json(&s),
            json!({"elapsedMs": 42, "finishedAt": "2026-10-21T14:03:00.123Z", "outcome": "cancelled"})
        );
        for (o, text) in [
            (RunOutcome::Ok, "ok"),
            (RunOutcome::Error, "error"),
            (RunOutcome::Disconnected, "disconnected"),
        ] {
            assert_eq!(to_json(&o), json!(text));
        }
    }

    #[test]
    fn session_info_json_shape() {
        let s = SessionInfo {
            server: ServerInfo {
                version: "Microsoft SQL Server 2022".into(),
                user: "sa".into(),
            },
            databases: vec!["master".into()],
            current_database: "master".into(),
        };
        assert_eq!(
            to_json(&s),
            json!({
                "server": {"version": "Microsoft SQL Server 2022", "user": "sa"},
                "databases": ["master"],
                "currentDatabase": "master"
            })
        );
    }

    #[test]
    fn save_connection_result_json_shape() {
        let mut c = ConnectionConfig::new("A", "h", 1433, "u", Environment::Local);
        c.id = "id-1".into();
        let r = SaveConnectionResult {
            connection: c.clone(),
            keyring_unavailable: true,
        };
        assert_eq!(
            to_json(&r),
            json!({"connection": to_json(&c), "keyringUnavailable": true})
        );
        assert_eq!(to_json(&r)["connection"]["passwordSaved"], json!(false));
    }

    #[test]
    fn copy_types_json_shape() {
        assert_eq!(
            to_json(&CopyOutcome { rows: 2, tokens: 9 }),
            json!({"rows": 2, "tokens": 9})
        );
        assert_eq!(
            serde_json::from_value::<CopyMode>(json!("excel")).unwrap(),
            CopyMode::Excel
        );
        assert_eq!(
            serde_json::from_value::<CopyMode>(json!("ai")).unwrap(),
            CopyMode::Ai
        );
    }
}
