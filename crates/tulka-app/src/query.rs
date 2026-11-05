//! Sessions per query tab: opening, running scripts, cancelling, reading and
//! copying results. See [`crate::state`] for the locking scheme.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::{Notify, RwLock};
use tokio::time::Instant;
use tulka_core::{
    estimate_tokens, system_decimal_separator, to_ai, to_excel_tsv, Column, DatabaseScope,
    DriverError, ExecEvent, MessageKind, NumberFormat, Session, Value,
};
use tulka_store::RunLogOutcome;

use crate::connections::{blocking, connect_params, resolve_password};
use crate::results::{
    display_cell, ByteBudget, PushOutcome, ResultBuffer, Selection, ViewSpec, CELLS_CAPPED_WARNING,
    MAX_RUN_BYTES,
};
use crate::runlog;
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
/// Messages forwarded per run; later ones are counted and summed up in a
/// final notice. Errors have their own, smaller allowance so they get through
/// a flood of other messages but cannot flood the list themselves.
const MAX_MESSAGES: usize = 10_000;
const MAX_ERRORS: usize = 1_000;
/// Consecutive "rows affected" reports are merged for up to this long.
const AFFECTED_MERGE: Duration = Duration::from_millis(100);
/// Result sets kept per run; later ones are drained and counted.
const MAX_RESULT_SETS: usize = 100;
const EVENT_QUEUE: usize = 32;

pub(crate) fn no_session() -> String {
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
        database: current_database.clone(),
        session: Some(session),
        cancel: None,
        starting: false,
        stop_pending: false,
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
    // No generation to match: switching database runs no user script, so
    // there is no guard analysis it could outdate.
    let (checkout, mut session) = Checkout::take(state, &tab_id, false, None)?;
    let generation = checkout.generation;
    let result = session.use_database(&database).await;
    let lost = matches!(result, Err(DriverError::Disconnected(_)));
    checkout.checkin(session, lost);
    match result {
        Ok(()) => {
            if let Some(tab) = state
                .tabs()
                .open
                .get_mut(&tab_id)
                .filter(|t| t.generation == generation)
            {
                tab.database = database;
            }
            Ok(())
        }
        Err(DriverError::Disconnected(_)) => Err(DISCONNECTED.into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs a script on the tab's session. Events go to `on_event` as they
/// happen, rows go to the tab's result buffers (cleared first) and the
/// summary is returned once the script ended. Fails with `"disconnected"`
/// after a lost connection, until `open_session` is called again, and with
/// `"busy"` while another command uses the session.
///
/// Before the session is touched, the script is analyzed with the driver's
/// guard, according to the connection's effective level (see
/// [`crate::guard::effective_guard`]). If dangerous statements are found,
/// nothing is sent to the server: with `block` the summary is `Blocked`,
/// with `confirm` and `confirmed == false` it is `NeedsConfirmation`; both
/// carry the `findings`. This happens here, in the backend, so no caller can
/// skip it; `confirmed` never overrides `block`.
pub async fn run_query<F>(
    state: &AppState,
    tab_id: String,
    sql: String,
    confirmed: bool,
    on_event: F,
) -> Result<RunSummary, String>
where
    F: Fn(UiEvent) + Send + Sync,
{
    let _starting = Starting::begin(state, &tab_id);
    let generation = match crate::guard::check(state, &tab_id, &sql, confirmed).await? {
        crate::guard::Verdict::Hold(held) => {
            // A refused script is logged; one waiting for confirmation is not
            // (the confirmation dialog logs its own cancel).
            if held.outcome == RunOutcome::Blocked {
                let snap = runlog::snapshot(state, &tab_id).await;
                runlog::record(state, snap, &sql, RunLogOutcome::Blocked, None, None).await;
            }
            return Ok(held);
        }
        crate::guard::Verdict::Run { generation } => generation,
    };
    let snap = runlog::snapshot(state, &tab_id).await;
    let prefs = crate::connections::get_preferences(state).await;
    let (checkout, mut session) = Checkout::take(state, &tab_id, true, Some(generation))?;
    let started = Instant::now();
    let (tx, rx) = mpsc::channel(EVENT_QUEUE);
    let forwarder = Forwarder {
        state,
        tab_id: &tab_id,
        generation: checkout.generation,
        emit: &on_event,
        max_rows: prefs.max_rows as usize,
        budget: std::sync::Mutex::new(ByteBudget::default()),
    };
    // The driver must be polled until `execute` returns, also after a
    // cancel, so all of these run to completion here. The timer ends as soon
    // as the script does; if it fires first it cancels the script the same
    // way Stop does.
    let cancel = session.cancel_handle();
    if checkout.stop_pending {
        let _ = cancel.cancel().await;
    }
    let finished = Notify::new();
    let done = AtomicBool::new(false);
    let timer = async {
        let limit = Duration::from_secs(u64::from(prefs.query_timeout_s));
        if limit.is_zero() {
            return;
        }
        // `biased`: if the script ended at the same moment the limit was
        // reached, it counts as finished and nothing is cancelled or reported.
        tokio::select! {
            biased;
            () = finished.notified() => {}
            () = tokio::time::sleep(limit) => {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                on_event(UiEvent::Message {
                    kind: MessageKind::Warning,
                    text: format!(
                        "Query timeout ({} s) reached; the query was cancelled.",
                        prefs.query_timeout_s
                    ),
                    line: None,
                });
                let _ = cancel.cancel().await;
            }
        }
    };
    let execute = async {
        let result = session.execute(&sql, tx).await;
        done.store(true, Ordering::SeqCst);
        finished.notify_one();
        result
    };
    let (result, tally, ()) = tokio::join!(execute, forwarder.run(rx), timer);
    let saw_disconnect = tally.disconnected;
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

    // A `USE` in the script changes the session's database: read it back.
    let database = if outcome == RunOutcome::Disconnected {
        None
    } else {
        session.current_database().await.ok()
    };
    if let Some(name) = &database {
        let mut tabs = state.tabs();
        if let Some(tab) = checkout.tab(&mut tabs) {
            tab.database.clone_from(name);
        }
    }
    checkout.checkin(session, outcome == RunOutcome::Disconnected);
    let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let logged = match outcome {
        RunOutcome::Ok => RunLogOutcome::Ok,
        RunOutcome::Cancelled => RunLogOutcome::Cancelled,
        RunOutcome::Disconnected => RunLogOutcome::Disconnected,
        _ => RunLogOutcome::Error,
    };
    runlog::record(state, snap, &sql, logged, tally.rows(), Some(elapsed_ms)).await;
    Ok(RunSummary {
        elapsed_ms,
        finished_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        outcome,
        findings: Vec::new(),
        database,
    })
}

/// How many transactions are open on the tab's session. `0` when the tab has
/// no session; fails with `"busy"` while a command uses the session and with
/// `"disconnected"` after a lost connection (the server rolled back by then).
pub async fn transaction_count(state: &AppState, tab_id: String) -> Result<u32, String> {
    let (checkout, mut session) = match Checkout::take(state, &tab_id, false, None) {
        Ok(taken) => taken,
        Err(e) if e == no_session() => return Ok(0),
        Err(e) => return Err(e),
    };
    let result = session.transaction_count().await;
    let lost = matches!(result, Err(DriverError::Disconnected(_)));
    checkout.checkin(session, lost);
    match result {
        Ok(count) => Ok(count),
        Err(DriverError::Disconnected(_)) => Err(DISCONNECTED.into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Commits or rolls back every transaction open on the tab's session, and
/// records it in the run log. Does nothing for a tab without a session. Fails
/// with `"busy"` while a command uses the session and with `"disconnected"`
/// after a lost connection.
pub async fn end_transactions(
    state: &AppState,
    tab_id: String,
    commit: bool,
) -> Result<(), String> {
    let (checkout, mut session) = match Checkout::take(state, &tab_id, false, None) {
        Ok(taken) => taken,
        Err(e) if e == no_session() => return Ok(()),
        Err(e) => return Err(e),
    };
    let snap = runlog::snapshot(state, &tab_id).await;
    let started = Instant::now();
    let result = session.end_transactions(commit).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let lost = matches!(result, Err(DriverError::Disconnected(_)));
    checkout.checkin(session, lost);
    let sql = if commit {
        "COMMIT TRANSACTION"
    } else {
        "ROLLBACK TRANSACTION"
    };
    let outcome = if result.is_ok() {
        RunLogOutcome::Ok
    } else {
        RunLogOutcome::Error
    };
    runlog::record(state, snap, sql, outcome, None, Some(elapsed_ms)).await;
    match result {
        Ok(()) => Ok(()),
        Err(DriverError::Disconnected(_)) => Err(DISCONNECTED.into()),
        Err(e) => Err(e.to_string()),
    }
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
        None => {
            // The run may not have published its handle yet (the pane shows
            // "running" first): remember the Stop, but only for a run that
            // is starting, so an old Stop never cancels a later run.
            if let Some(tab) = state.tabs().open.get_mut(&tab_id) {
                tab.stop_pending = tab.starting;
            }
            Ok(())
        }
    }
}

/// Up to `limit` (at most 1000) visible rows of a result (see
/// [`set_result_view`]), from `offset`, in display form: long text and
/// binary cells are shortened (see [`display_cell`]). Works while the result
/// is still streaming: rows below the last `RowCount` total are always there.
pub async fn fetch_rows(
    state: &AppState,
    tab_id: String,
    result_index: u32,
    offset: usize,
    limit: usize,
) -> Result<Vec<Vec<Value>>, String> {
    let buffer = result_of(state, &tab_id, result_index)?;
    let buffer = buffer.read().await;
    Ok(buffer
        .window(offset, limit.min(MAX_FETCH_ROWS))
        .iter()
        .map(|row| row.iter().map(display_cell).collect())
        .collect())
}

/// Sorts and filters the rows of a result already received, without running
/// the query again; returns how many rows are visible. `fetch_rows` and
/// `copy_results` then read the visible rows, in their order (row indexes
/// are of visible rows). An empty `view` goes back to the rows as received;
/// a new run starts without a view. Fails with `"busy"` while the tab runs a
/// query, since its results may still be receiving rows. Runs on a blocking
/// thread.
pub async fn set_result_view(
    state: &AppState,
    tab_id: String,
    result_index: u32,
    view: ViewSpec,
) -> Result<usize, String> {
    let buffer = {
        let tabs = state.tabs();
        let tab = tabs.open.get(&tab_id).ok_or_else(no_session)?;
        if tab.cancel.is_some() {
            return Err(BUSY.into());
        }
        result_in(tab, result_index)?
    };
    blocking(move || {
        let mut buffer = buffer.blocking_write();
        if let Some(column) = view.columns_used().find(|&c| c >= buffer.columns.len()) {
            return Err(format!("column {column} does not exist"));
        }
        buffer.set_view(&view);
        Ok(buffer.visible_len())
    })
    .await?
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
    let separator = match crate::connections::get_preferences(state)
        .await
        .copy_numbers
    {
        NumberFormat::System => system_decimal_separator(),
        NumberFormat::Invariant => '.',
    };
    blocking(move || {
        let (text, outcome) = buffer
            .blocking_read()
            .with_selection(&selection, |columns, rows| match mode {
                CopyMode::Excel => {
                    let text = to_excel_tsv(columns, rows, separator);
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

/// Closed tab ids remembered to discard late `open_session` results.
const MAX_CLOSED_TABS: usize = 256;

/// Closes the tab's session, cancelling a running query. Closing a tab
/// without a session is not an error.
pub async fn close_session(state: &AppState, tab_id: String) -> Result<(), String> {
    let removed = {
        let mut tabs = state.tabs();
        let generation = state.new_generation();
        let removed = tabs.open.remove(&tab_id);
        tabs.closed_at.insert(tab_id, generation);
        if tabs.closed_at.len() > MAX_CLOSED_TABS {
            let oldest = tabs
                .closed_at
                .iter()
                .min_by_key(|(_, &g)| g)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                tabs.closed_at.remove(&id);
            }
        }
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
    result_in(state.tabs().open.get(tab_id).ok_or_else(no_session)?, index)
}

fn result_in(tab: &TabState, index: u32) -> Result<SharedBuffer, String> {
    tab.results
        .get(index as usize)
        .cloned()
        .ok_or_else(|| format!("result {index} does not exist"))
}

/// A session taken out of its tab by a running command. Check it in with
/// [`Checkout::checkin`]; if that never happens (the command panicked or its
/// future was dropped), dropping the guard marks the tab disconnected, since
/// the session is gone.
/// Marks the tab as starting a run until dropped (or until the run takes the
/// session), so a Stop pressed in that window is remembered.
struct Starting<'a> {
    state: &'a AppState,
    tab_id: &'a str,
    generation: Option<u64>,
}

impl<'a> Starting<'a> {
    fn begin(state: &'a AppState, tab_id: &'a str) -> Self {
        let mut tabs = state.tabs();
        let generation = tabs.open.get_mut(tab_id).map(|tab| {
            tab.starting = true;
            tab.stop_pending = false;
            tab.generation
        });
        Starting {
            state,
            tab_id,
            generation,
        }
    }
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        let mut tabs = self.state.tabs();
        if let Some(tab) = tabs
            .open
            .get_mut(self.tab_id)
            .filter(|t| Some(t.generation) == self.generation)
        {
            tab.starting = false;
            tab.stop_pending = false;
        }
    }
}

struct Checkout<'a> {
    state: &'a AppState,
    tab_id: &'a str,
    generation: u64,
    /// A Stop arrived before the cancel handle was published; the run
    /// applies it as soon as it starts.
    stop_pending: bool,
    done: bool,
}

impl<'a> Checkout<'a> {
    /// Takes the session out of the tab. For a run, also publishes its cancel
    /// handle and clears the previous results, in the same critical section.
    /// With `expected_generation`, fails with `"busy"` if the tab is not that
    /// generation any more (it was replaced after the guard looked at it), so
    /// a script is never run on a session the guard did not analyze it for.
    fn take(
        state: &'a AppState,
        tab_id: &'a str,
        run: bool,
        expected_generation: Option<u64>,
    ) -> Result<(Self, Box<dyn Session>), String> {
        let mut tabs = state.tabs();
        let tab = tabs.open.get_mut(tab_id).ok_or_else(no_session)?;
        if expected_generation.is_some_and(|g| g != tab.generation) {
            return Err(BUSY.into());
        }
        if tab.disconnected {
            return Err(DISCONNECTED.into());
        }
        let mut session = tab.session.take().ok_or_else(|| BUSY.to_string())?;
        let mut stop_pending = false;
        if run {
            // A cancel from an earlier run must not stop this one; a Stop
            // pressed before the handle existed (`stop_pending`) must.
            session.reset_cancel();
            tab.cancel = Some(session.cancel_handle().into());
            tab.results.clear();
            stop_pending = std::mem::take(&mut tab.stop_pending);
            tab.starting = false;
        }
        let checkout = Checkout {
            state,
            tab_id,
            generation: tab.generation,
            stop_pending,
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

/// The warning shown once per run when [`MAX_RUN_BYTES`] cut a result at
/// `rows` rows.
fn budget_warning(index: u32, rows: usize) -> String {
    let mb = MAX_RUN_BYTES / (1024 * 1024);
    let n = index + 1;
    if rows == 0 {
        format!("Result {n} was not kept because the run exceeded the memory limit ({mb} MB); later results were not kept either. Narrow the query.")
    } else {
        format!("Result {n} was cut at {rows} rows because the run exceeded the memory limit ({mb} MB); later results were not kept. Narrow the query.")
    }
}

fn new_buffer(columns: Vec<Column>, max_rows: usize) -> SharedBuffer {
    Arc::new(RwLock::new(ResultBuffer::new(columns, max_rows)))
}

/// What a run produced, for the run log.
#[derive(Default)]
struct RunTally {
    /// A `Disconnected` event was seen.
    disconnected: bool,
    /// Every row that arrived, also those the buffer dropped for its limit.
    rows_received: u64,
    /// The sum of the `RowsAffected` counts.
    rows_affected: u64,
    saw_result: bool,
    saw_affected: bool,
}

impl RunTally {
    /// Rows received if any result set came, else rows affected if any
    /// statement reported them, else nothing.
    fn rows(&self) -> Option<u64> {
        if self.saw_result {
            Some(self.rows_received)
        } else if self.saw_affected {
            Some(self.rows_affected)
        } else {
            None
        }
    }
}

/// Moves the driver's events to the UI sink and the rows to the buffers.
struct Forwarder<'a, F> {
    state: &'a AppState,
    tab_id: &'a str,
    generation: u64,
    emit: &'a F,
    /// Rows kept per result; later ones are counted but dropped.
    max_rows: usize,
    /// Memory left for the stored rows of this run, all results together.
    budget: std::sync::Mutex<ByteBudget>,
}

impl<F: Fn(UiEvent)> Forwarder<'_, F> {
    /// Forwards until the driver closes the channel and tallies what came.
    async fn run(&self, mut rx: Receiver<ExecEvent>) -> RunTally {
        let mut counter = RowCounter::default();
        let mut tally = RunTally::default();
        // The capped-cell warning is shown once per run.
        let mut warned_capped = false;
        let mut warned_budget = false;
        let mut gate = MessageGate::default();
        loop {
            let deadline = [counter.deadline(), gate.deadline()]
                .into_iter()
                .flatten()
                .min();
            let next = match deadline {
                Some(deadline) => tokio::select! {
                    ev = rx.recv() => ev,
                    () = tokio::time::sleep_until(deadline) => {
                        let now = Instant::now();
                        if counter.deadline().is_some_and(|d| d <= now) {
                            counter.flush(self.emit);
                        }
                        if gate.deadline().is_some_and(|d| d <= now) {
                            gate.flush_affected(self.emit);
                        }
                        continue;
                    }
                },
                None => rx.recv().await,
            };
            let Some(ev) = next else { break };
            match ev {
                ExecEvent::Rows { index, rows } => {
                    let received = rows.len();
                    tally.saw_result = true;
                    tally.rows_received += received as u64;
                    if index as usize >= MAX_RESULT_SETS {
                        continue;
                    }
                    if let Some((total, truncated, memory_cut, outcome)) =
                        self.push_rows(index, rows).await
                    {
                        counter.rows(index, received, (total, truncated, memory_cut), self.emit);
                        let capped = outcome.capped;
                        if outcome.over_budget && !warned_budget {
                            warned_budget = true;
                            (self.emit)(UiEvent::Message {
                                kind: MessageKind::Warning,
                                text: budget_warning(index, total),
                                line: None,
                            });
                        }
                        if capped && !warned_capped {
                            warned_capped = true;
                            (self.emit)(UiEvent::Message {
                                kind: MessageKind::Warning,
                                text: CELLS_CAPPED_WARNING.into(),
                                line: None,
                            });
                        }
                    }
                }
                ExecEvent::ResultStart { index, columns } => {
                    tally.saw_result = true;
                    if index as usize >= MAX_RESULT_SETS {
                        gate.sets_dropped += 1;
                        continue;
                    }
                    counter.finish(self.emit);
                    gate.flush_affected(self.emit);
                    // The buffer exists before the UI hears of the result.
                    self.start_result(index, columns.clone());
                    (self.emit)(UiEvent::ResultStart { index, columns });
                    counter.start(index);
                }
                ExecEvent::RowsAffected { count } => {
                    counter.flush(self.emit);
                    tally.saw_affected = true;
                    tally.rows_affected += count;
                    gate.affected(count);
                }
                ExecEvent::Message { kind, text, line } => {
                    counter.flush(self.emit);
                    gate.message(
                        UiEvent::Message { kind, text, line },
                        kind == MessageKind::Error,
                        self.emit,
                    );
                }
                other => {
                    counter.flush(self.emit);
                    tally.disconnected |= matches!(other, ExecEvent::Disconnected { .. });
                    // Keeps the order: what was merged so far came before this.
                    gate.flush_affected(self.emit);
                    if let Some(ev) = UiEvent::from_exec(other) {
                        (self.emit)(ev);
                    }
                }
            }
        }
        counter.finish(self.emit);
        gate.finish(self.emit);
        tally
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
                .resize_with(index + 1, || new_buffer(Vec::new(), self.max_rows));
        }
        if let Some(columns) = columns {
            tab.results[index] = new_buffer(columns, self.max_rows);
        }
        Some(tab.results[index].clone())
    }

    fn start_result(&self, index: u32, columns: Vec<Column>) {
        self.buffer(index, Some(columns));
    }

    /// Appends rows to result `index`; returns its row count, its truncated
    /// flag and what the push did, or `None` if the tab was closed or
    /// replaced.
    async fn push_rows(
        &self,
        index: u32,
        rows: Vec<Vec<Value>>,
    ) -> Option<(usize, bool, bool, PushOutcome)> {
        let buffer = self.buffer(index, None)?;
        let mut buffer = buffer.write().await;
        let outcome = {
            let mut budget = self.budget.lock().unwrap_or_else(|e| e.into_inner());
            buffer.push_rows(rows, &mut budget)
        };
        Some((
            buffer.rows.len(),
            buffer.truncated,
            buffer.memory_cut,
            outcome,
        ))
    }
}

/// Keeps the message list of a run readable and bounded: consecutive "rows
/// affected" reports become one, and past [`MAX_MESSAGES`] only errors get
/// through; a final notice says how much was left out.
#[derive(Default)]
struct MessageGate {
    /// Sum, count and first time of the merged reports not sent yet.
    affected: Option<(u64, u32, Instant)>,
    sent: usize,
    errors_sent: usize,
    omitted: u64,
    errors_omitted: u64,
    sets_dropped: u64,
}

impl MessageGate {
    fn affected(&mut self, count: u64) {
        match &mut self.affected {
            Some((total, times, _)) => {
                *total += count;
                *times += 1;
            }
            None => self.affected = Some((count, 1, Instant::now())),
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.affected.map(|(_, _, first)| first + AFFECTED_MERGE)
    }

    fn flush_affected(&mut self, emit: &impl Fn(UiEvent)) {
        let Some((count, times, _)) = self.affected.take() else {
            return;
        };
        let ev = if times == 1 {
            UiEvent::RowsAffected { count }
        } else {
            UiEvent::RowsAffectedMany { count, times }
        };
        self.send(ev, false, emit);
    }

    /// A message in order: whatever was merged before it goes first.
    fn message(&mut self, ev: UiEvent, always: bool, emit: &impl Fn(UiEvent)) {
        self.flush_affected(emit);
        self.send(ev, always, emit);
    }

    fn send(&mut self, ev: UiEvent, always: bool, emit: &impl Fn(UiEvent)) {
        let (sent, limit, omitted) = if always {
            (&mut self.errors_sent, MAX_ERRORS, &mut self.errors_omitted)
        } else {
            (&mut self.sent, MAX_MESSAGES, &mut self.omitted)
        };
        if *sent < limit {
            *sent += 1;
            emit(ev);
        } else {
            *omitted += 1;
        }
    }

    /// Sends what is still merged and the notices about what was left out.
    fn finish(&mut self, emit: &impl Fn(UiEvent)) {
        self.flush_affected(emit);
        let warn = |text: String| UiEvent::Message {
            kind: MessageKind::Warning,
            text,
            line: None,
        };
        if self.omitted + self.errors_omitted > 0 {
            let mut text = format!(
                "{} more messages omitted (limit {MAX_MESSAGES} per run",
                self.omitted + self.errors_omitted
            );
            if self.errors_omitted > 0 {
                text.push_str(&format!(
                    "; {} of them errors, limit {MAX_ERRORS}",
                    self.errors_omitted
                ));
            }
            text.push_str(").");
            emit(warn(text));
        }
        if self.sets_dropped > 0 {
            emit(warn(format!(
                "{} more result sets were read but not kept (limit {MAX_RESULT_SETS} per run).",
                self.sets_dropped
            )));
        }
    }
}

/// Throttles `RowCount` events of the result being streamed: one at most
/// every [`ROW_COUNT_ROWS`] rows or [`ROW_COUNT_INTERVAL`], whichever comes
/// first, plus a final one per result.
#[derive(Default)]
struct RowCounter {
    index: Option<u32>,
    /// Last (total, truncated, memory cut) sent for `index`.
    sent: Option<(usize, bool, bool)>,
    /// Latest (total, truncated, memory cut) not sent yet.
    pending: Option<(usize, bool, bool)>,
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
        state: (usize, bool, bool),
        emit: &impl Fn(UiEvent),
    ) {
        if self.index != Some(index) {
            // Rows without a preceding ResultStart for this index.
            self.finish(emit);
            self.start(index);
        }
        self.pending = Some(state);
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
                memory_cut: state.2,
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
                memory_cut: false,
            });
        }
        *self = RowCounter::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::sync::mpsc::Sender;
    use tulka_core::{ConfigStore, DatabaseSnapshot, MemoryStore, ServerInfo, TableStructure};

    struct NoCancel;

    #[async_trait]
    impl tulka_core::CancelHandle for NoCancel {
        async fn cancel(&self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    /// A session that is only ever checked out and in.
    struct Inert;

    #[async_trait]
    impl Session for Inert {
        async fn server_info(&mut self) -> Result<ServerInfo, DriverError> {
            unimplemented!()
        }
        async fn list_databases(&mut self) -> Result<Vec<String>, DriverError> {
            unimplemented!()
        }
        async fn use_database(&mut self, _: &str) -> Result<(), DriverError> {
            unimplemented!()
        }
        async fn current_database(&mut self) -> Result<String, DriverError> {
            unimplemented!()
        }
        async fn snapshot(&mut self, _: &str) -> Result<DatabaseSnapshot, DriverError> {
            unimplemented!()
        }
        async fn structure(
            &mut self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<TableStructure, DriverError> {
            unimplemented!()
        }
        async fn definition(&mut self, _: &str, _: &str, _: &str) -> Result<String, DriverError> {
            unimplemented!()
        }
        async fn execute(&mut self, _: &str, _: Sender<ExecEvent>) -> Result<(), DriverError> {
            unimplemented!()
        }
        fn cancel_handle(&self) -> Box<dyn tulka_core::CancelHandle> {
            Box::new(NoCancel)
        }
    }

    fn state_with_tab(generation: u64) -> (AppState, tempfile::TempDir) {
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
                generation,
                connection_id: "c".into(),
                database: "master".into(),
                session: Some(Box::new(Inert)),
                cancel: None,
                starting: false,
                stop_pending: false,
                results: Vec::new(),
                disconnected: false,
            },
        );
        (state, dir)
    }

    #[test]
    fn take_refuses_a_tab_replaced_since_the_guard_check() {
        let (state, _dir) = state_with_tab(2);
        // The guard looked at generation 1; the tab is now generation 2.
        let err = Checkout::take(&state, "t1", true, Some(1)).err();
        assert_eq!(err.as_deref(), Some(BUSY));
        // Nothing was taken or cleared: the new tab keeps its session.
        assert!(state.tabs().open["t1"].session.is_some());
    }

    #[test]
    fn take_proceeds_when_the_generation_is_the_one_checked() {
        let (state, _dir) = state_with_tab(2);
        assert!(Checkout::take(&state, "t1", true, Some(2)).is_ok());
        assert!(state.tabs().open["t1"].session.is_none());
    }

    #[tokio::test]
    async fn a_stop_before_the_handle_exists_is_applied_by_the_next_take_once() {
        let (state, _dir) = state_with_tab(1);
        let _starting = Starting::begin(&state, "t1");
        cancel_query(&state, "t1".into()).await.unwrap();
        let (checkout, session) = Checkout::take(&state, "t1", true, None).unwrap();
        assert!(checkout.stop_pending);
        checkout.checkin(session, false);
        // Consumed: the run after it is not stopped.
        let (checkout, _session) = Checkout::take(&state, "t1", true, None).unwrap();
        assert!(!checkout.stop_pending);
    }

    #[tokio::test]
    async fn a_stop_with_no_run_starting_is_not_remembered() {
        let (state, _dir) = state_with_tab(1);
        // A Stop that arrives after a run ended, before the next one began.
        cancel_query(&state, "t1".into()).await.unwrap();
        assert!(!state.tabs().open["t1"].stop_pending);
        let (checkout, _session) = Checkout::take(&state, "t1", true, None).unwrap();
        assert!(!checkout.stop_pending);
    }

    #[tokio::test]
    async fn the_start_flag_ends_with_the_take_or_when_the_run_gives_up() {
        let (state, _dir) = state_with_tab(1);
        {
            let _starting = Starting::begin(&state, "t1");
            assert!(state.tabs().open["t1"].starting);
        }
        assert!(!state.tabs().open["t1"].starting);
        let starting = Starting::begin(&state, "t1");
        cancel_query(&state, "t1".into()).await.unwrap();
        let (checkout, _session) = Checkout::take(&state, "t1", true, None).unwrap();
        assert!(checkout.stop_pending && !state.tabs().open["t1"].starting);
        drop(starting);
        assert!(!state.tabs().open["t1"].stop_pending);
    }

    /// Runs `events` through a Forwarder and returns what the UI heard.
    async fn forward(events: Vec<ExecEvent>) -> (Vec<UiEvent>, RunTally) {
        let (state, _dir) = state_with_tab(1);
        let heard = std::sync::Mutex::new(Vec::new());
        let emit = |ev: UiEvent| heard.lock().unwrap().push(ev);
        let f = Forwarder {
            state: &state,
            tab_id: "t1",
            generation: 1,
            emit: &emit,
            max_rows: 1000,
            budget: std::sync::Mutex::new(ByteBudget::default()),
        };
        let (tx, rx) = mpsc::channel(events.len().max(1));
        for ev in events {
            tx.send(ev).await.unwrap();
        }
        drop(tx);
        let tally = f.run(rx).await;
        let heard = heard.lock().unwrap().clone();
        (heard, tally)
    }

    fn info(text: &str) -> ExecEvent {
        ExecEvent::Message {
            kind: MessageKind::Info,
            text: text.into(),
            line: None,
        }
    }

    #[tokio::test]
    async fn consecutive_rows_affected_are_merged_into_one_event() {
        let mut events: Vec<ExecEvent> = (0..1000)
            .map(|_| ExecEvent::RowsAffected { count: 2 })
            .collect();
        events.push(info("done"));
        events.push(ExecEvent::RowsAffected { count: 5 });
        let (heard, tally) = forward(events).await;
        assert_eq!(
            heard,
            vec![
                UiEvent::RowsAffectedMany {
                    count: 2000,
                    times: 1000
                },
                UiEvent::Message {
                    kind: MessageKind::Info,
                    text: "done".into(),
                    line: None
                },
                UiEvent::RowsAffected { count: 5 },
            ]
        );
        assert_eq!(tally.rows(), Some(2005));
    }

    #[tokio::test]
    async fn errors_have_their_own_cap_and_the_notice_counts_them() {
        let events: Vec<ExecEvent> = (0..20_000)
            .map(|i| ExecEvent::Message {
                kind: MessageKind::Error,
                text: format!("err {i}"),
                line: None,
            })
            .collect();
        let (heard, _) = forward(events).await;
        let errors = heard
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    UiEvent::Message {
                        kind: MessageKind::Error,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(errors, MAX_ERRORS);
        let Some(UiEvent::Message { text, .. }) = heard.last() else {
            panic!("no notice");
        };
        assert!(
            text.starts_with("19000 more messages omitted")
                && text.contains("19000 of them errors"),
            "{text}"
        );
        assert_eq!(heard.len(), MAX_ERRORS + 1);
    }

    #[tokio::test]
    async fn messages_stop_at_the_cap_but_errors_and_the_notice_get_through() {
        let mut events: Vec<ExecEvent> = (0..MAX_MESSAGES + 25)
            .map(|i| info(&i.to_string()))
            .collect();
        events.push(ExecEvent::Message {
            kind: MessageKind::Error,
            text: "boom".into(),
            line: Some(1),
        });
        let (heard, _) = forward(events).await;
        let texts: Vec<_> = heard
            .iter()
            .filter_map(|e| match e {
                UiEvent::Message { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        // The cap, the error past it, and the notice.
        assert_eq!(texts.len(), MAX_MESSAGES + 2);
        assert_eq!(texts[MAX_MESSAGES], "boom");
        assert!(texts[MAX_MESSAGES + 1].starts_with("25 more messages omitted"));
    }

    #[tokio::test]
    async fn result_sets_past_the_cap_are_drained_and_counted() {
        let mut events = Vec::new();
        for index in 0..MAX_RESULT_SETS as u32 + 3 {
            events.push(ExecEvent::ResultStart {
                index,
                columns: vec![Column {
                    name: "c".into(),
                    type_name: "int".into(),
                }],
            });
            events.push(ExecEvent::Rows {
                index,
                rows: vec![vec![Value::Int(1)]],
            });
        }
        let (heard, tally) = forward(events).await;
        let starts = heard
            .iter()
            .filter(|e| matches!(e, UiEvent::ResultStart { .. }))
            .count();
        assert_eq!(starts, MAX_RESULT_SETS);
        assert_eq!(tally.rows_received, MAX_RESULT_SETS as u64 + 3);
        assert!(matches!(
            heard.last(),
            Some(UiEvent::Message { text, .. }) if text.starts_with("3 more result sets")
        ));
    }

    #[tokio::test]
    async fn run_budget_cuts_rows_and_warns_once() {
        let (state, _dir) = state_with_tab(1);
        let messages = std::sync::Mutex::new(Vec::new());
        let emit = |ev: UiEvent| {
            if let UiEvent::Message { text, .. } = ev {
                messages.lock().unwrap().push(text);
            }
        };
        let f = Forwarder {
            state: &state,
            tab_id: "t1",
            generation: 1,
            emit: &emit,
            max_rows: 1000,
            budget: std::sync::Mutex::new(ByteBudget::new(10 * 1024)),
        };
        let (tx, rx) = mpsc::channel(16);
        let col = Column {
            name: "c".into(),
            type_name: "varchar".into(),
        };
        tx.send(ExecEvent::ResultStart {
            index: 0,
            columns: vec![col],
        })
        .await
        .unwrap();
        for _ in 0..3 {
            let rows = vec![vec![Value::Text("x".repeat(1024))]; 20];
            tx.send(ExecEvent::Rows { index: 0, rows }).await.unwrap();
        }
        drop(tx);
        let tally = f.run(rx).await;
        // Every row was drained from the server, few were stored.
        assert_eq!(tally.rows_received, 60);
        let buffer = state.tabs().open["t1"].results[0].clone();
        let b = buffer.read().await;
        assert!(b.truncated && b.memory_cut && b.rows.len() < 20);
        let messages = messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains(&format!("Result 1 was cut at {} rows", b.rows.len())));
    }
}
