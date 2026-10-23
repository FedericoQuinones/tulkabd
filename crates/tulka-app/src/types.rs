//! Values returned or streamed to the UI. Their JSON shapes mirror
//! `src/lib/ipc.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tulka_core::{Column, ConnectionConfig, DbObject, ExecEvent, MessageKind, ServerInfo};

/// Command error meaning there is no saved password and none was given.
pub const PASSWORD_REQUIRED: &str = "password_required";
/// Folder command error: a folder cannot move into itself or a descendant.
pub const CYCLE: &str = "cycle";
/// Folder command error: an unknown folder or connection id.
pub const NOT_FOUND: &str = "not_found";
/// Folder or saved-query command error: the name is empty.
pub const EMPTY_NAME: &str = "empty_name";
/// Saved-query command error: a query with that name already exists.
pub const EXISTS: &str = "exists";
/// Command error meaning the tab's connection was lost; only `open_session`
/// gets the tab working again.
pub const DISCONNECTED: &str = "disconnected";
/// Command error meaning another command (a run or a database switch) is
/// using the tab's session right now.
pub const BUSY: &str = "busy";
/// Command error of an `open_session` whose tab was closed (or opened again)
/// while it was connecting; its new session was closed.
pub const CLOSED: &str = "closed";
/// Command error of a live Explorer command (`table_structure`,
/// `object_definition`, `refresh_metadata`) on a connection whose Explorer
/// is not connected; `connect_explorer` first.
pub const NOT_CONNECTED: &str = "not_connected";

/// State of a connection's Explorer session: `"connecting"`,
/// `"connected"`, `"disconnected"` or `"error"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionStatus {
    Connecting,
    Connected,
    Disconnected,
    Error,
}

/// Events pushed to the UI outside of any command (the Explorer and its
/// background indexing).
///
/// Serialized internally tagged:
/// `{"kind": "explorerStatus", "connectionId": "..", "status": "error", "message": ".."}`,
/// `{"kind": "indexProgress", "connectionId": "..", "database": "..", "done": true, "noAccess": false}`,
/// `{"kind": "indexIdle", "connectionId": ".."}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AppEvent {
    /// The Explorer session changed state; `message` is the error of
    /// `"error"` (never containing a password).
    ExplorerStatus {
        connection_id: String,
        status: ConnectionStatus,
        message: Option<String>,
    },
    /// Indexing of one database started (`done: false`) or ended
    /// (`done: true`). `no_access` is set on an end when the database could
    /// not be read: it is marked in the cache and indexing goes on.
    IndexProgress {
        connection_id: String,
        database: String,
        done: bool,
        no_access: bool,
    },
    /// The connection's indexing queue is empty. Not sent after an error.
    IndexIdle { connection_id: String },
}

/// Where [`AppEvent`]s go. The Tauri layer emits them to the window; tests
/// record them. Called from background tasks: must not block.
pub trait AppEvents: Send + Sync {
    fn emit(&self, ev: AppEvent);
}

/// Drops every event.
pub struct NoopEvents;

impl AppEvents for NoopEvents {
    fn emit(&self, _ev: AppEvent) {}
}

/// Returned by `connect_explorer`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerInfo {
    pub server: ServerInfo,
    /// Databases the connection exposes (all, or only the selected ones),
    /// in server order.
    pub databases: Vec<String>,
}

/// One result of `search_objects`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaletteHit {
    pub connection_id: String,
    pub connection_name: String,
    /// Folder names from the root down to the connection's folder; empty at
    /// the root.
    pub folder_path: Vec<String>,
    /// The connection's Explorer is connected right now.
    pub connected: bool,
    pub database: String,
    pub object: DbObject,
    /// Positions to highlight in `"{schema}.{name}"`, counted in Unicode
    /// characters (code points), not UTF-16 units: the UI must index
    /// `Array.from(text)`, not the string.
    pub indices: Vec<u32>,
}

/// Returned by `search_objects`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    /// The best hits, at most the requested limit.
    pub hits: Vec<PaletteHit>,
    /// Every match in scope, beyond the limit too.
    pub total_matches: usize,
    pub databases_matched: usize,
    /// Databases of connected Explorers not indexed yet (queued or in
    /// progress); at least 1 while the startup index is still loading.
    pub indexing: usize,
    /// The best saved queries whose name matches, at most 20 (their own
    /// limit). Empty when the search is scoped to a folder.
    pub queries: Vec<SavedQueryHit>,
}

/// One saved query matched by `search_objects`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedQueryHit {
    pub database: String,
    pub name: String,
    /// Positions to highlight in `name`, in Unicode code points (see
    /// [`PaletteHit::indices`]).
    pub indices: Vec<u32>,
}

/// A query saved as `<data dir>/queries/<database>/<name>.sql`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    /// The database it belongs to (exact, case-sensitive).
    pub database: String,
    pub name: String,
    /// Last modification of the file, Unix milliseconds (0 if unknown).
    pub modified_ms: i64,
}

/// Returned by `completion_metadata`, read from the cache.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompletionMetadata {
    /// Distinct schemas of the objects, sorted.
    pub schemas: Vec<String>,
    /// Tables, views and procedures.
    pub objects: Vec<DbObject>,
    /// Column names of each table and view in ordinal order, keyed by the
    /// unquoted `"schema.name"`.
    pub columns: HashMap<String, Vec<String>>,
}

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
                single_database: false,
            },
            databases: vec!["master".into()],
            current_database: "master".into(),
        };
        assert_eq!(
            to_json(&s),
            json!({
                "server": {"version": "Microsoft SQL Server 2022", "user": "sa", "singleDatabase": false},
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
    fn app_event_json_shape() {
        let cases = [
            (
                AppEvent::ExplorerStatus {
                    connection_id: "c1".into(),
                    status: ConnectionStatus::Error,
                    message: Some("gone".into()),
                },
                json!({"kind": "explorerStatus", "connectionId": "c1", "status": "error", "message": "gone"}),
            ),
            (
                AppEvent::ExplorerStatus {
                    connection_id: "c1".into(),
                    status: ConnectionStatus::Connecting,
                    message: None,
                },
                json!({"kind": "explorerStatus", "connectionId": "c1", "status": "connecting", "message": null}),
            ),
            (
                AppEvent::IndexProgress {
                    connection_id: "c1".into(),
                    database: "db".into(),
                    done: true,
                    no_access: false,
                },
                json!({"kind": "indexProgress", "connectionId": "c1", "database": "db", "done": true, "noAccess": false}),
            ),
            (
                AppEvent::IndexIdle {
                    connection_id: "c1".into(),
                },
                json!({"kind": "indexIdle", "connectionId": "c1"}),
            ),
        ];
        for (ev, expected) in cases {
            assert_eq!(to_json(&ev), expected);
        }
        for (s, text) in [
            (ConnectionStatus::Connected, "connected"),
            (ConnectionStatus::Disconnected, "disconnected"),
        ] {
            assert_eq!(to_json(&s), json!(text));
        }
    }

    #[test]
    fn explorer_types_json_shape() {
        let object = DbObject {
            schema: "dbo".into(),
            name: "Pedidos".into(),
            kind: tulka_core::ObjectKind::Table,
        };
        let info = ExplorerInfo {
            server: ServerInfo {
                version: "v".into(),
                user: "sa".into(),
                single_database: false,
            },
            databases: vec!["a".into()],
        };
        assert_eq!(
            to_json(&info),
            json!({"server": {"version": "v", "user": "sa", "singleDatabase": false}, "databases": ["a"]})
        );
        let r = SearchResponse {
            hits: vec![PaletteHit {
                connection_id: "c1".into(),
                connection_name: "Main".into(),
                folder_path: vec!["Cliente A".into()],
                connected: true,
                database: "db".into(),
                object: object.clone(),
                indices: vec![4, 5],
            }],
            total_matches: 3,
            databases_matched: 2,
            indexing: 1,
            queries: vec![SavedQueryHit {
                database: "db".into(),
                name: "Reporte".into(),
                indices: vec![0],
            }],
        };
        assert_eq!(
            to_json(&r),
            json!({
                "hits": [{
                    "connectionId": "c1",
                    "connectionName": "Main",
                    "folderPath": ["Cliente A"],
                    "connected": true,
                    "database": "db",
                    "object": {"schema": "dbo", "name": "Pedidos", "kind": "table"},
                    "indices": [4, 5]
                }],
                "totalMatches": 3,
                "databasesMatched": 2,
                "indexing": 1,
                "queries": [{"database": "db", "name": "Reporte", "indices": [0]}]
            })
        );
        let m = CompletionMetadata {
            schemas: vec!["dbo".into()],
            objects: vec![object],
            columns: HashMap::from([("dbo.Pedidos".to_string(), vec!["id".to_string()])]),
        };
        assert_eq!(
            to_json(&m),
            json!({
                "schemas": ["dbo"],
                "objects": [{"schema": "dbo", "name": "Pedidos", "kind": "table"}],
                "columns": {"dbo.Pedidos": ["id"]}
            })
        );
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
