//! Sessions per query tab: opening, running scripts, cancelling, reading and
//! copying results. See [`crate::state`] for the locking scheme.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use tokio::sync::mpsc::{self, Receiver};
use tokio::time::Instant;
use tulka_core::{
    estimate_tokens, system_decimal_separator, to_ai, to_excel_tsv, DatabaseScope, DriverError,
    ExecEvent, MessageKind, Session, Value,
};

use crate::connections::{blocking, connect_params, resolve_password};
use crate::results::{ResultBuffer, Selection, MAX_ROWS};
use crate::state::{AppState, TabState};
use crate::types::{
    Clipboard, CopyMode, CopyOutcome, RunOutcome, RunSummary, SessionInfo, UiEvent, DISCONNECTED,
};

/// Most rows returned by one `fetch_rows`.
pub const MAX_FETCH_ROWS: usize = 1000;
/// A `RowCount` is sent once this many rows arrived since the last one...
const ROW_COUNT_ROWS: usize = 500;
/// ...or once the last one is this old, whichever comes first.
const ROW_COUNT_INTERVAL: Duration = Duration::from_millis(50);
/// Events buffered between the driver and the forwarder.
const EVENT_QUEUE: usize = 32;

fn no_session() -> String {
    "no session is open on this tab".into()
}

fn busy() -> String {
    "a query is already running on this tab".into()
}

/// Opens a session for `tab_id` on a saved connection, replacing the tab's
/// previous session (a query running on it is cancelled). `database` is the
/// login database; without it, a connection limited to selected databases
/// starts in the first of them. Without `password`, the saved one is used;
/// fails with `"password_required"` if there is none.
pub async fn open_session(
    state: &AppState,
    tab_id: String,
    connection_id: String,
    database: Option<String>,
    password: Option<String>,
) -> Result<SessionInfo, String> {
    let conn = state
        .connections
        .lock()
        .await
        .connections
        .iter()
        .find(|c| c.id == connection_id)
        .cloned()
        .ok_or_else(|| "the connection does not exist".to_string())?;
    let password = resolve_password(state, &conn, password).await?;
    let driver = state.driver(&conn.driver)?;
    let database = database.filter(|d| !d.is_empty());
    let chosen_database = database.is_some();

    let mut session = driver
        .connect(&connect_params(&conn, password, database))
        .await
        .map_err(|e| e.to_string())?;
    let server = session.server_info().await.map_err(|e| e.to_string())?;
    let mut databases = session.list_databases().await.map_err(|e| e.to_string())?;
    if let DatabaseScope::Selected { names } = &conn.databases {
        databases.retain(|d| names.contains(d));
    }
    let mut current_database = session
        .current_database()
        .await
        .map_err(|e| e.to_string())?;
    if !chosen_database && !databases.is_empty() && !databases.contains(&current_database) {
        // Best effort: if the switch fails, stay in the login database.
        if session.use_database(&databases[0]).await.is_ok() {
            current_database = databases[0].clone();
        }
    }

    let tab = TabState {
        generation: state.new_generation(),
        connection_id,
        session: Some(session),
        cancel: None,
        results: Vec::new(),
        disconnected: false,
    };
    let previous = state.tabs.lock().await.insert(tab_id, tab);
    cancel_running(previous).await;
    Ok(SessionInfo {
        server,
        databases,
        current_database,
    })
}

/// Switches the tab's session to `database`.
pub async fn use_database(
    state: &AppState,
    tab_id: String,
    database: String,
) -> Result<(), String> {
    let (generation, mut session) = checkout(state, &tab_id, false).await?;
    let result = session.use_database(&database).await;
    let lost = matches!(result, Err(DriverError::Disconnected(_)));
    checkin(state, &tab_id, generation, session, lost).await;
    match result {
        Ok(()) => Ok(()),
        Err(DriverError::Disconnected(_)) => Err(DISCONNECTED.into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs a script on the tab's session. Events go to `on_event` as they
/// happen, rows go to the tab's result buffers (cleared first) and the
/// summary is returned once the script ended. Fails with `"disconnected"`
/// after a lost connection, until `open_session` is called again.
pub async fn run_query<F>(
    state: &AppState,
    tab_id: String,
    sql: String,
    on_event: F,
) -> Result<RunSummary, String>
where
    F: Fn(UiEvent) + Send + Sync,
{
    let (generation, mut session) = checkout(state, &tab_id, true).await?;
    let started = Instant::now();
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    let forwarder = Forwarder {
        state,
        tab_id: &tab_id,
        generation,
        emit: &on_event,
    };
    // The driver must be polled until `execute` returns, also after a
    // cancel, so both run to completion here.
    let (result, saw_disconnect) = tokio::join!(session.execute(&sql, tx), forwarder.run(rx));
    let elapsed = started.elapsed();

    let outcome = match &result {
        Err(DriverError::Disconnected(_)) => RunOutcome::Disconnected,
        _ if saw_disconnect => RunOutcome::Disconnected,
        Ok(()) => RunOutcome::Ok,
        Err(DriverError::Cancelled) => RunOutcome::Cancelled,
        Err(_) => RunOutcome::Error,
    };
    // Errors the driver does not report as events itself.
    match &result {
        Err(DriverError::Disconnected(reason)) if !saw_disconnect => {
            on_event(UiEvent::Disconnected {
                reason: reason.clone(),
            });
        }
        Err(
            e @ (DriverError::Unreachable(_) | DriverError::AuthFailed(_) | DriverError::Tls(_)),
        ) => {
            on_event(UiEvent::Message {
                kind: MessageKind::Error,
                text: e.to_string(),
                line: None,
            });
        }
        _ => {}
    }

    checkin(
        state,
        &tab_id,
        generation,
        session,
        outcome == RunOutcome::Disconnected,
    )
    .await;
    Ok(RunSummary {
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        finished_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        outcome,
    })
}

/// Cancels the query running on the tab; does nothing if none is running.
pub async fn cancel_query(state: &AppState, tab_id: String) -> Result<(), String> {
    let handle = state
        .tabs
        .lock()
        .await
        .get(&tab_id)
        .and_then(|t| t.cancel.clone());
    match handle {
        Some(handle) => handle.cancel().await.map_err(|e| e.to_string()),
        None => Ok(()),
    }
}

/// Up to `limit` (at most 1000) rows of a result, from `offset`. Works while
/// the result is still streaming: rows below the last `RowCount` total are
/// always there.
pub async fn fetch_rows(
    state: &AppState,
    tab_id: String,
    result_index: u32,
    offset: usize,
    limit: usize,
) -> Result<Vec<Vec<Value>>, String> {
    let tabs = state.tabs.lock().await;
    let buffer = result_of(&tabs, &tab_id, result_index)?;
    Ok(buffer.window(offset, limit.min(MAX_FETCH_ROWS)).to_vec())
}

/// Copies (part of) a result to the clipboard as Excel TSV or in the
/// "Copy for AI" format. `tokens` is estimated only in AI mode.
pub async fn copy_results(
    state: &AppState,
    tab_id: String,
    result_index: u32,
    mode: CopyMode,
    selection: Selection,
    clipboard: &dyn Clipboard,
) -> Result<CopyOutcome, String> {
    let (columns, rows) = {
        let tabs = state.tabs.lock().await;
        result_of(&tabs, &tab_id, result_index)?.select(&selection)
    };
    let row_count = rows.len();
    let (text, tokens) = blocking(move || match mode {
        CopyMode::Excel => (to_excel_tsv(&columns, &rows, system_decimal_separator()), 0),
        CopyMode::Ai => {
            let text = to_ai(&columns, &rows);
            let tokens = estimate_tokens(&text);
            (text, tokens)
        }
    })
    .await?;
    clipboard.write_text(text)?;
    Ok(CopyOutcome {
        rows: row_count,
        tokens,
    })
}

/// Closes the tab's session, cancelling a running query. Closing a tab
/// without a session is not an error.
pub async fn close_session(state: &AppState, tab_id: String) -> Result<(), String> {
    let removed = state.tabs.lock().await.remove(&tab_id);
    cancel_running(removed).await;
    Ok(())
}

/// Cancels the query running on a tab that was just removed or replaced.
/// Its run notices the generation change, drains and drops the session.
async fn cancel_running(tab: Option<TabState>) {
    if let Some(handle) = tab.and_then(|t| t.cancel) {
        let _ = handle.cancel().await;
    }
}

fn result_of<'a>(
    tabs: &'a HashMap<String, TabState>,
    tab_id: &str,
    index: u32,
) -> Result<&'a ResultBuffer, String> {
    tabs.get(tab_id)
        .ok_or_else(no_session)?
        .results
        .get(index as usize)
        .ok_or_else(|| format!("result {index} does not exist"))
}

/// Takes the session out of the tab. For a run, also publishes its cancel
/// handle and clears the previous results, in the same critical section.
async fn checkout(
    state: &AppState,
    tab_id: &str,
    run: bool,
) -> Result<(u64, Box<dyn Session>), String> {
    let mut tabs = state.tabs.lock().await;
    let tab = tabs.get_mut(tab_id).ok_or_else(no_session)?;
    if tab.disconnected {
        return Err(DISCONNECTED.into());
    }
    let session = tab.session.take().ok_or_else(busy)?;
    if run {
        tab.cancel = Some(session.cancel_handle().into());
        tab.results.clear();
    }
    Ok((tab.generation, session))
}

/// Puts the session back, or marks the tab disconnected if it was `lost`.
/// If the tab was closed or replaced meanwhile, the session is dropped.
async fn checkin(
    state: &AppState,
    tab_id: &str,
    generation: u64,
    session: Box<dyn Session>,
    lost: bool,
) {
    let mut tabs = state.tabs.lock().await;
    let leftover = match tabs.get_mut(tab_id) {
        Some(tab) if tab.generation == generation => {
            tab.cancel = None;
            if lost {
                tab.disconnected = true;
                Some(session)
            } else {
                tab.session = Some(session);
                None
            }
        }
        _ => Some(session),
    };
    drop(tabs);
    // Dropping a session closes its connection.
    drop(leftover);
}

/// The buffer of result `index`, created (empty) if needed.
fn buffer_at(results: &mut Vec<ResultBuffer>, index: u32) -> &mut ResultBuffer {
    let index = index as usize;
    if results.len() <= index {
        results.resize_with(index + 1, || ResultBuffer::new(Vec::new(), MAX_ROWS));
    }
    &mut results[index]
}

/// Moves the driver's events to the UI sink and the rows to the buffers.
struct Forwarder<'a, F> {
    state: &'a AppState,
    tab_id: &'a str,
    generation: u64,
    emit: &'a F,
}

impl<F: Fn(UiEvent)> Forwarder<'_, F> {
    /// Forwards until the driver closes the channel. Returns whether a
    /// `Disconnected` event was seen.
    async fn run(&self, mut rx: Receiver<ExecEvent>) -> bool {
        let mut counter = RowCounter::default();
        let mut disconnected = false;
        loop {
            let next = match counter.deadline() {
                Some(deadline) => tokio::select! {
                    ev = rx.recv() => ev,
                    () = tokio::time::sleep_until(deadline) => {
                        counter.flush(self.emit);
                        continue;
                    }
                },
                None => rx.recv().await,
            };
            let Some(ev) = next else { break };
            match ev {
                ExecEvent::Rows { index, rows } => {
                    let received = rows.len();
                    if let Some((total, truncated)) = self.push_rows(index, rows).await {
                        counter.rows(index, received, total, truncated, self.emit);
                    }
                }
                ExecEvent::ResultStart { index, columns } => {
                    counter.finish(self.emit);
                    // The buffer exists before the UI hears of the result.
                    self.start_result(index, columns.clone()).await;
                    (self.emit)(UiEvent::ResultStart { index, columns });
                    counter.start(index);
                }
                other => {
                    counter.flush(self.emit);
                    disconnected |= matches!(other, ExecEvent::Disconnected { .. });
                    if let Some(ev) = UiEvent::from_exec(other) {
                        (self.emit)(ev);
                    }
                }
            }
        }
        counter.finish(self.emit);
        disconnected
    }

    async fn start_result(&self, index: u32, columns: Vec<tulka_core::Column>) {
        let mut tabs = self.state.tabs.lock().await;
        if let Some(tab) = tabs
            .get_mut(self.tab_id)
            .filter(|t| t.generation == self.generation)
        {
            *buffer_at(&mut tab.results, index) = ResultBuffer::new(columns, MAX_ROWS);
        }
    }

    /// Appends rows to result `index`; returns its row count and truncated
    /// flag, or `None` if the tab was closed or replaced.
    async fn push_rows(&self, index: u32, rows: Vec<Vec<Value>>) -> Option<(usize, bool)> {
        let mut tabs = self.state.tabs.lock().await;
        let tab = tabs
            .get_mut(self.tab_id)
            .filter(|t| t.generation == self.generation)?;
        let buffer = buffer_at(&mut tab.results, index);
        buffer.push_rows(rows);
        Some((buffer.rows.len(), buffer.truncated))
    }
}

/// Throttles `RowCount` events of the result being streamed: one at most
/// every [`ROW_COUNT_ROWS`] rows or [`ROW_COUNT_INTERVAL`], whichever comes
/// first, plus a final one per result.
#[derive(Default)]
struct RowCounter {
    index: Option<u32>,
    /// Last (total, truncated) sent for `index`.
    sent: Option<(usize, bool)>,
    /// Latest (total, truncated) not sent yet.
    pending: Option<(usize, bool)>,
    rows_since: usize,
    last: Option<Instant>,
}

impl RowCounter {
    fn start(&mut self, index: u32) {
        *self = RowCounter {
            index: Some(index),
            last: Some(Instant::now()),
            ..RowCounter::default()
        };
    }

    fn rows(
        &mut self,
        index: u32,
        received: usize,
        total: usize,
        truncated: bool,
        emit: &impl Fn(UiEvent),
    ) {
        if self.index != Some(index) {
            // Rows without a preceding ResultStart for this index.
            self.finish(emit);
            self.start(index);
        }
        self.pending = Some((total, truncated));
        self.rows_since += received;
        let due = self.last.is_none_or(|t| t.elapsed() >= ROW_COUNT_INTERVAL);
        if self.rows_since >= ROW_COUNT_ROWS || due {
            self.flush(emit);
        }
    }

    /// When the pending count must be sent at the latest.
    fn deadline(&self) -> Option<Instant> {
        self.pending?;
        Some(
            self.last
                .map_or_else(Instant::now, |t| t + ROW_COUNT_INTERVAL),
        )
    }

    fn flush(&mut self, emit: &impl Fn(UiEvent)) {
        let (Some(index), Some(state)) = (self.index, self.pending.take()) else {
            return;
        };
        self.rows_since = 0;
        self.last = Some(Instant::now());
        if self.sent != Some(state) {
            emit(UiEvent::RowCount {
                index,
                total: state.0,
                truncated: state.1,
            });
            self.sent = Some(state);
        }
    }

    /// Sends the final count of the current result (also when it had no
    /// rows) and forgets it.
    fn finish(&mut self, emit: &impl Fn(UiEvent)) {
        let Some(index) = self.index else { return };
        self.flush(emit);
        if self.sent.is_none() {
            emit(UiEvent::RowCount {
                index,
                total: 0,
                truncated: false,
            });
        }
        *self = RowCounter::default();
    }
}
