//! Guard: keeps dangerous statements from reaching the server, according to
//! the level of the connection a script runs on.

use chrono::{SecondsFormat, Utc};
use tulka_core::{ConnectionConfig, GuardLevel};

use crate::connections::mutate;
use crate::query::no_session;
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
///
/// The saved level and the session-off set are updated one after the other,
/// not together: for a few microseconds a run may see the new level while
/// the connection is still in the set (so it runs unguarded once more). That
/// is the same as the run having started just before the change.
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

/// What [`check`] decided.
pub(crate) enum Verdict {
    /// The script must not run now; answer with this summary.
    Hold(RunSummary),
    /// The script may run. `generation` is that of the tab the connection
    /// was read from: the run must take the session of that same tab, not
    /// one that replaced it meanwhile.
    Run { generation: u64 },
}

/// Analyzes `sql` for the connection of `tab_id`. A tab that does not exist
/// is an error (the same one as running on it without a session), and so is
/// a tab whose connection is no longer saved: the level that applies is
/// unknown, so the script is not run.
pub(crate) async fn check(
    state: &AppState,
    tab_id: &str,
    sql: &str,
    confirmed: bool,
) -> Result<Verdict, String> {
    // The tabs lock is held only to read these two values.
    let Some((connection_id, generation)) = state
        .tabs()
        .open
        .get(tab_id)
        .map(|t| (t.connection_id.clone(), t.generation))
    else {
        return Err(no_session());
    };
    let run = Verdict::Run { generation };
    let conn = state
        .connections
        .lock()
        .await
        .connections
        .iter()
        .find(|c| c.id == connection_id)
        .cloned()
        .ok_or_else(|| "the connection does not exist".to_string())?;
    let level = effective_guard(state, &conn);
    if level == GuardLevel::Off || (level == GuardLevel::Confirm && confirmed) {
        return Ok(run);
    }
    let findings = state.driver(&conn.driver)?.guard_findings(sql);
    if findings.is_empty() {
        return Ok(run);
    }
    let outcome = if level == GuardLevel::Block {
        RunOutcome::Blocked
    } else {
        RunOutcome::NeedsConfirmation
    };
    Ok(Verdict::Hold(RunSummary {
        elapsed_ms: 0,
        finished_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        outcome,
        findings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TabState;
    use std::sync::Arc;
    use tulka_core::{ConfigStore, MemoryStore};

    fn state_with_tab(connection_id: &str) -> AppState {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(
            ConfigStore::at(dir.path().join("config.json")),
            Arc::new(MemoryStore::default()),
            vec![],
        )
        .unwrap();
        state.tabs().open.insert(
            "t1".into(),
            TabState {
                generation: 7,
                connection_id: connection_id.into(),
                database: "master".into(),
                session: None,
                cancel: None,
                results: Vec::new(),
                disconnected: false,
            },
        );
        state
    }

    #[tokio::test]
    async fn a_tab_whose_connection_is_gone_is_an_error() {
        let state = state_with_tab("gone");
        let err = check(&state, "t1", "DROP TABLE x", false).await.err();
        assert!(err.is_some());
    }

    #[tokio::test]
    async fn an_unknown_tab_is_an_error() {
        let state = state_with_tab("gone");
        let err = check(&state, "nope", "DROP TABLE x", false).await.err();
        assert_eq!(err.as_deref(), Some("no session is open on this tab"));
    }
}
