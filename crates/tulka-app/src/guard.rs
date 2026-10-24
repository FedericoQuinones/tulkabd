//! Guard: keeps dangerous statements from reaching the server, according to
//! the level of the connection a script runs on.

use chrono::{SecondsFormat, Utc};
use tulka_core::{ConnectionConfig, GuardLevel};

use crate::connections::mutate;
use crate::state::AppState;
use crate::types::{RunOutcome, RunSummary, NOT_FOUND};

/// The level in force for `conn`: off while the connection is in the
/// "off until the app closes" set, otherwise the saved level.
pub fn effective_guard(state: &AppState, conn: &ConnectionConfig) -> GuardLevel {
    if off_set(state).contains(&conn.id) {
        GuardLevel::Off
    } else {
        conn.guard
    }
}

/// Switches the guard of a connection off (or back on) until the app closes.
/// Nothing is saved.
pub fn set_guard_off_for_session(state: &AppState, connection_id: &str, off: bool) {
    let mut set = off_set(state);
    if off {
        set.insert(connection_id.to_string());
    } else {
        set.remove(connection_id);
    }
}

/// The connections whose guard is off until the app closes.
pub fn guard_off_for_session(state: &AppState) -> Vec<String> {
    let mut ids: Vec<String> = off_set(state).iter().cloned().collect();
    ids.sort();
    ids
}

/// Saves a new guard level for a connection and returns the previous one
/// (so the caller can offer an undo). Also ends any "off until the app
/// closes" for it. Fails with `"not_found"` for an unknown connection.
pub async fn set_connection_guard(
    state: &AppState,
    connection_id: String,
    level: GuardLevel,
) -> Result<GuardLevel, String> {
    let previous = mutate(state, |config| {
        let conn = config
            .connections
            .iter_mut()
            .find(|c| c.id == connection_id)
            .ok_or_else(|| NOT_FOUND.to_string())?;
        Ok(std::mem::replace(&mut conn.guard, level))
    })
    .await?;
    set_guard_off_for_session(state, &connection_id, false);
    Ok(previous)
}

fn off_set(state: &AppState) -> std::sync::MutexGuard<'_, std::collections::HashSet<String>> {
    state
        .guard_off_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Analyzes `sql` for the connection of `tab_id`. Returns the summary to
/// answer with when the script must not run now, `None` when it may run.
/// A tab without a session or with an unknown connection passes through, so
/// the usual errors come from the run itself.
pub(crate) async fn check(
    state: &AppState,
    tab_id: &str,
    sql: &str,
    confirmed: bool,
) -> Result<Option<RunSummary>, String> {
    // The tabs lock is held only to read the connection id.
    let Some(connection_id) = state
        .tabs()
        .open
        .get(tab_id)
        .map(|t| t.connection_id.clone())
    else {
        return Ok(None);
    };
    let Some(conn) = state
        .connections
        .lock()
        .await
        .connections
        .iter()
        .find(|c| c.id == connection_id)
        .cloned()
    else {
        return Ok(None);
    };
    let level = effective_guard(state, &conn);
    if level == GuardLevel::Off || (level == GuardLevel::Confirm && confirmed) {
        return Ok(None);
    }
    let findings = state.driver(&conn.driver)?.guard_findings(sql);
    if findings.is_empty() {
        return Ok(None);
    }
    let outcome = if level == GuardLevel::Block {
        RunOutcome::Blocked
    } else {
        RunOutcome::NeedsConfirmation
    };
    Ok(Some(RunSummary {
        elapsed_ms: 0,
        finished_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        outcome,
        findings,
    }))
}
