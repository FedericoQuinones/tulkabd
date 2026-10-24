//! The run log: one entry per statement sent (or refused) by `run_query`,
//! with where it ran, how many rows it touched and how long it took. It keeps
//! statement text only, never passwords or result rows. A failure to write
//! the log never fails the run it describes.

use std::sync::atomic::Ordering;

use chrono::Utc;
use tulka_store::{NewRun, RunLogEntry, RunLogOutcome};

use crate::connections::blocking;
use crate::query::no_session;
use crate::state::AppState;
use crate::types::AppEvent;

/// Most entries one `list_run_log` returns.
const MAX_LIST: u32 = 500;
/// How long entries are kept, until the preferences hold a setting for it.
const DEFAULT_KEEP_DAYS: i64 = 30;
/// Pruning after an insert happens at most this often.
const PRUNE_INTERVAL_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Where a run happens, as the entry will remember it.
pub(crate) struct Snapshot {
    connection_id: String,
    connection_name: String,
    color: String,
    database: String,
}

/// The connection and database of `tab_id`, or `None` if the tab or its
/// connection is gone.
pub(crate) async fn snapshot(state: &AppState, tab_id: &str) -> Option<Snapshot> {
    let (connection_id, database) = state
        .tabs()
        .open
        .get(tab_id)
        .map(|t| (t.connection_id.clone(), t.database.clone()))?;
    let config = state.connections.lock().await;
    let conn = config.connections.iter().find(|c| c.id == connection_id)?;
    Some(Snapshot {
        connection_name: conn.name.clone(),
        color: conn.color.clone(),
        connection_id,
        database,
    })
}

/// Records a run and tells the UI. Errors are dropped on purpose.
pub(crate) async fn record(
    state: &AppState,
    snapshot: Option<Snapshot>,
    sql: &str,
    outcome: RunLogOutcome,
    rows: Option<u64>,
    elapsed_ms: Option<u64>,
) {
    let Some(s) = snapshot else { return };
    let run = NewRun {
        at_ms: Utc::now().timestamp_millis(),
        connection_id: s.connection_id,
        connection_name: s.connection_name,
        color: s.color,
        database: s.database,
        sql: sql.to_string(),
        outcome,
        rows,
        elapsed_ms,
    };
    let store = state.runlog.clone();
    if let Ok(Ok(entry)) = blocking(move || store.insert(&run)).await {
        state.explorer.emit(AppEvent::RunLogged { entry });
        prune_if_due(state).await;
    }
}

/// Records that the user cancelled the confirmation of `sql` on `tab_id`:
/// nothing was sent to the server.
pub async fn log_cancelled_run(
    state: &AppState,
    tab_id: String,
    sql: String,
) -> Result<(), String> {
    let snap = snapshot(state, &tab_id).await.ok_or_else(no_session)?;
    record(
        state,
        Some(snap),
        &sql,
        RunLogOutcome::Cancelled,
        None,
        None,
    )
    .await;
    Ok(())
}

/// Entries newest first; `before` is an entry id to page backwards. At most
/// 500 are returned whatever `limit` says.
pub async fn list_run_log(state: &AppState, before: Option<i64>, limit: u32) -> Vec<RunLogEntry> {
    let store = state.runlog.clone();
    let limit = limit.min(MAX_LIST);
    blocking(move || store.list(before, limit))
        .await
        .unwrap_or_default()
}

pub async fn clear_run_log(state: &AppState) -> Result<(), String> {
    let store = state.runlog.clone();
    blocking(move || store.clear()).await?
}

/// Deletes the entries older than the retention period.
pub async fn prune_run_log(state: &AppState) {
    let now = Utc::now().timestamp_millis();
    state.runlog_pruned_ms.store(now, Ordering::Relaxed);
    let store = state.runlog.clone();
    let _ = blocking(move || store.prune(now - DEFAULT_KEEP_DAYS * DAY_MS)).await;
}

async fn prune_if_due(state: &AppState) {
    let now = Utc::now().timestamp_millis();
    let last = state.runlog_pruned_ms.load(Ordering::Relaxed);
    if now - last >= PRUNE_INTERVAL_MS {
        prune_run_log(state).await;
    }
}
