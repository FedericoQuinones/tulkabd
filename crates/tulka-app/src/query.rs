//! Sessions per query tab: opening, running scripts, cancelling, reading and
//! copying results. See [`crate::state`] for the locking scheme.

use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::RwLock;
use tokio::time::Instant;
use tulka_core::{
    estimate_tokens, system_decimal_separator, to_ai, to_excel_tsv, Column, DatabaseScope,
    DriverError, ExecEvent, MessageKind, Session, Value,
};

use crate::connections::{blocking, connect_params, resolve_password};
use crate::results::{ResultBuffer, Selection, MAX_ROWS};
use crate::state::{AppState, SharedBuffer, TabState, Tabs};
use crate::types::{
    Clipboard, CopyMode, CopyOutcome, RunOutcome, RunSummary, SessionInfo, UiEvent, BUSY, CLOSED,
    DISCONNECTED,
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

/// Opens a session for `tab_id` on a saved connection, replacing the tab's
/// previous session (a query running on it is cancelled). `database` is the
/// login database; without it, a connection limited to selected databases
/// starts in the first of them. Without `password`, the saved one is used;
/// fails with `"password_required"` if there is none, and with `"closed"` if
/// the tab was closed (or opened again) while connecting.
pub async fn open_session(
    state: &AppState,
    tab_id: String,
    connection_id: String,
    database: Option<String>,
    password: Option<String>,
) -> Result<SessionInfo, String> {
    // Taken before connecting, so a close_session meanwhile is newer.
    let generation = state.new_generation();
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
        generation,
        connection_id,
        session: Some(session),
        cancel: None,
        results: Vec::new(),
        disconnected: false,
    };
    let previous = {
        let mut tabs = state.tabs();
        let closed = tabs.closed_at.get(&tab_id).is_some_and(|&g| g > generation);
        let superseded = tabs
            .open
            .get(&tab_id)
            .is_some_and(|t| t.generation > generation);
        if closed || superseded {
            Err(tab)
        } else {
            Ok(tabs.open.insert(tab_id, tab))
        }
    };
    match previous {
        Ok(previous) => {
            cancel_running(previous).await;
            Ok(SessionInfo {
                server,
                databases,
                current_database,
            })
        }
        Err(stale) => {
            // Dropping the new tab closes its session.
            drop(stale);
            Err(CLOSED.into())
        }
    }
}

/// Switches the tab's session to `database`.
pub async fn use_database(
    state: &AppState,
    tab_id: String,
    database: String,
) -> Result<(), String> {
    let (checkout, mut session) = Checkout::take(state, &tab_id, false)?;
    let result = session.use_database(&database).await;
    let lost = matches!(result, Err(DriverError::Disconnected(_)));
    checkout.checkin(session, lost);
    match result {
        Ok(()) => Ok(()),
        Err(DriverError::Disconnected(_)) => Err(DISCONNECTED.into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs a script on the tab's session. Events go to `on_event` as they
/// happen, rows go to the tab's result buffers (cleared first) and the
/// summary is returned once the script ended. Fails with `"disconnected"`
/// after a lost connection, until `open_session` is called again, and with
/// `"busy"` while another command uses the session.
pub async fn run_query<F>(
    state: &AppState,
    tab_id: String,
    sql: String,
    on_event: F,
) -> Result<RunSummary, String>
where
    F: Fn(UiEvent) + Send + Sync,
{
    let (checkout, mut session) = Checkout::take(state, &tab_id, true)?;
    let started = Instant::now();
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    let forwarder = Forwarder {
        state,
        tab_id: &tab_id,
        generation: checkout.generation,
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

    checkout.checkin(session, outcome == RunOutcome::Disconnected);
    Ok(RunSummary {
        elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        finished_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        outcome,
    })
}

/// Cancels the query running on the tab; does nothing if none is running.
pub async fn cancel_query(state: &AppState, tab_id: String) -> Result<(), String> {
    let handle = state
        .tabs()
        .open
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
    let buffer = result_of(state, &tab_id, result_index)?;
    let buffer = buffer.read().await;
    Ok(buffer.window(offset, limit.min(MAX_FETCH_ROWS)).to_vec())
}

/// Copies (part of) a result to the clipboard as Excel TSV or in the
/// "Copy for AI" format. `tokens` is estimated only in AI mode. Formatting
/// and the clipboard write run on a blocking thread.
pub async fn copy_results(
    state: &AppState,
    tab_id: String,
    result_index: u32,
    mode: CopyMode,
    selection: Selection,
    clipboard: Arc<dyn Clipboard>,
) -> Result<CopyOutcome, String> {
    let buffer = result_of(state, &tab_id, result_index)?;
    blocking(move || {
        let (text, outcome) = buffer
            .blocking_read()
            .with_selection(&selection, |columns, rows| match mode {
                CopyMode::Excel => {
                    let text = to_excel_tsv(columns, rows, system_decimal_separator());
                    let outcome = CopyOutcome {
                        rows: rows.len(),
                        tokens: 0,
                    };
                    (text, outcome)
                }
                CopyMode::Ai => {
                    let text = to_ai(columns, rows);
                    let outcome = CopyOutcome {
                        rows: rows.len(),
                        tokens: estimate_tokens(&text),
                    };
                    (text, outcome)
                }
            });
        clipboard.write_text(text).map(|()| outcome)
    })
    .await?
}

/// Closes the tab's session, cancelling a running query. Closing a tab
/// without a session is not an error.
pub async fn close_session(state: &AppState, tab_id: String) -> Result<(), String> {
    let removed = {
        let mut tabs = state.tabs();
        let generation = state.new_generation();
        let removed = tabs.open.remove(&tab_id);
        tabs.closed_at.insert(tab_id, generation);
        removed
    };
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

/// The buffer of a result, shared so it is read without the tabs lock.
fn result_of(state: &AppState, tab_id: &str, index: u32) -> Result<SharedBuffer, String> {
    state
        .tabs()
        .open
        .get(tab_id)
        .ok_or_else(no_session)?
        .results
        .get(index as usize)
        .cloned()
        .ok_or_else(|| format!("result {index} does not exist"))
}

/// A session taken out of its tab by a running command. Check it in with
/// [`Checkout::checkin`]; if that never happens (the command panicked or its
/// future was dropped), dropping the guard marks the tab disconnected, since
/// the session is gone.
struct Checkout<'a> {
    state: &'a AppState,
    tab_id: &'a str,
    generation: u64,
    done: bool,
}

impl<'a> Checkout<'a> {
    /// Takes the session out of the tab. For a run, also publishes its cancel
    /// handle and clears the previous results, in the same critical section.
    fn take(
        state: &'a AppState,
        tab_id: &'a str,
        run: bool,
    ) -> Result<(Self, Box<dyn Session>), String> {
        let mut tabs = state.tabs();
        let tab = tabs.open.get_mut(tab_id).ok_or_else(no_session)?;
        if tab.disconnected {
            return Err(DISCONNECTED.into());
        }
        let session = tab.session.take().ok_or_else(|| BUSY.to_string())?;
        if run {
            tab.cancel = Some(session.cancel_handle().into());
            tab.results.clear();
        }
        let checkout = Checkout {
            state,
            tab_id,
            generation: tab.generation,
            done: false,
        };
        Ok((checkout, session))
    }

    /// Puts the session back, or marks the tab disconnected if it was `lost`.
    /// If the tab was closed or replaced meanwhile, the session is dropped,
    /// which closes its connection.
    fn checkin(mut self, session: Box<dyn Session>, lost: bool) {
        self.done = true;
        let leftover = {
            let mut tabs = self.state.tabs();
            match self.tab(&mut tabs) {
                Some(tab) => {
                    tab.cancel = None;
                    if lost {
                        tab.disconnected = true;
                        Some(session)
                    } else {
                        tab.session = Some(session);
                        None
                    }
                }
                None => Some(session),
            }
        };
        drop(leftover);
    }

    /// The tab this session belongs to, unless it was closed or replaced.
    fn tab<'t>(&self, tabs: &'t mut Tabs) -> Option<&'t mut TabState> {
        tabs.open
            .get_mut(self.tab_id)
            .filter(|t| t.generation == self.generation)
    }
}

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let mut tabs = self.state.tabs();
        if let Some(tab) = self.tab(&mut tabs) {
            tab.cancel = None;
            tab.disconnected = true;
        }
    }
}

fn new_buffer(columns: Vec<Column>) -> SharedBuffer {
    Arc::new(RwLock::new(ResultBuffer::new(columns, MAX_ROWS)))
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
                    self.start_result(index, columns.clone());
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

    /// The buffer of result `index` in this run's tab, created (empty) if
    /// needed; `None` if the tab was closed or replaced.
    fn buffer(&self, index: u32, columns: Option<Vec<Column>>) -> Option<SharedBuffer> {
        let mut tabs = self.state.tabs();
        let tab = tabs
            .open
            .get_mut(self.tab_id)
            .filter(|t| t.generation == self.generation)?;
        let index = index as usize;
        if tab.results.len() <= index {
            tab.results
                .resize_with(index + 1, || new_buffer(Vec::new()));
        }
        if let Some(columns) = columns {
            tab.results[index] = new_buffer(columns);
        }
        Some(tab.results[index].clone())
    }

    fn start_result(&self, index: u32, columns: Vec<Column>) {
        self.buffer(index, Some(columns));
    }

    /// Appends rows to result `index`; returns its row count and truncated
    /// flag, or `None` if the tab was closed or replaced.
    async fn push_rows(&self, index: u32, rows: Vec<Vec<Value>>) -> Option<(usize, bool)> {
        let buffer = self.buffer(index, None)?;
        let mut buffer = buffer.write().await;
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
