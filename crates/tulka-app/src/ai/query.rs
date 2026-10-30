//! `run_query`: a read-only query for the assistant, with limits on rows,
//! cell size, output size and time, and an entry in the run log.

use std::future::Future;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tulka_core::{Column, DriverError, ExecEvent, MessageKind, Value};
use tulka_store::{NewRun, RunLogOutcome, RunSource};

use super::{format, AiState, ToolText, Visible, AI_CELL_CHARS, AI_MAX_OUTPUT_CHARS};
use crate::connections::blocking;
use crate::results::shorten_text;

/// How long a cancelled query may take to stop before it is abandoned.
const CANCEL_GRACE: Duration = Duration::from_secs(10);
/// Events buffered between the driver and the collector.
const EVENT_BUFFER: usize = 64;
/// Characters counted for a cell that is not text.
const OTHER_CELL_CHARS: usize = 20;

/// What `run_query` runs.
#[derive(Clone, Debug, Default)]
pub struct QueryRequest {
    pub connection: String,
    pub database: Option<String>,
    pub sql: String,
    pub max_rows: Option<u32>,
}

/// Why the collector asked the driver to stop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Timeout,
    Client,
    OutputCut,
}

struct ResultSet {
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    /// More rows arrived than `max_rows`.
    limit_reached: bool,
}

/// Gathers what the driver streams, within the limits.
struct Collector {
    max_rows: usize,
    sets: Vec<ResultSet>,
    messages: Vec<String>,
    /// A message of kind error, as the query's error.
    error: Option<String>,
    disconnected: bool,
    /// Rows received from the driver.
    received: u64,
    /// Characters kept so far, roughly.
    chars: usize,
    cut: bool,
}

impl Collector {
    fn new(max_rows: u32) -> Self {
        Self {
            max_rows: max_rows as usize,
            sets: Vec::new(),
            messages: Vec::new(),
            error: None,
            disconnected: false,
            received: 0,
            chars: 0,
            cut: false,
        }
    }

    /// Takes one event. Returns true when the output just went over its
    /// budget, so the query should be cancelled.
    fn push(&mut self, event: ExecEvent) -> bool {
        if self.cut {
            return false;
        }
        match event {
            ExecEvent::ResultStart { columns, .. } => {
                self.chars += columns
                    .iter()
                    .map(|c| c.name.chars().count() + 1)
                    .sum::<usize>();
                self.sets.push(ResultSet {
                    columns,
                    rows: Vec::new(),
                    limit_reached: false,
                });
            }
            ExecEvent::Rows { rows, .. } => {
                let Some(set) = self.sets.last_mut() else {
                    return false;
                };
                for row in rows {
                    self.received += 1;
                    if set.rows.len() >= self.max_rows {
                        set.limit_reached = true;
                        continue;
                    }
                    let row: Vec<Value> = row.into_iter().map(short_cell).collect();
                    self.chars += row.iter().map(cell_chars).sum::<usize>() + 1;
                    set.rows.push(row);
                    if self.chars > AI_MAX_OUTPUT_CHARS {
                        self.cut = true;
                        return true;
                    }
                }
            }
            ExecEvent::Message { kind, text, line } => match kind {
                MessageKind::Error => {
                    self.error.get_or_insert_with(|| match line {
                        Some(line) => format!("Error (line {line}): {text}"),
                        None => format!("Error: {text}"),
                    });
                }
                _ => {
                    let text = shorten_text(&text, AI_CELL_CHARS).unwrap_or(text);
                    self.messages.push(format::message_line(kind, &text, line));
                }
            },
            ExecEvent::Disconnected { .. } => self.disconnected = true,
            ExecEvent::RowsAffected { .. } | ExecEvent::BatchDone { .. } => {}
        }
        false
    }

    /// The text of what was collected, within the output budget.
    fn text(&self, max_rows: u32, cut: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (i, set) in self.sets.iter().enumerate() {
            let mut part = String::new();
            if self.sets.len() > 1 {
                part.push_str(&format!("Result set {}:\n", i + 1));
            }
            part.push_str(&tulka_core::to_ai(&set.columns, &set.rows));
            if set.limit_reached {
                part.push_str(&format::limit_reached(max_rows));
                part.push('\n');
            }
            parts.push(part);
        }
        if !self.messages.is_empty() {
            parts.push(format!("{}\n", self.messages.join("\n")));
        }
        let mut text = parts.join("\n");
        if text.is_empty() {
            text = format!("{}\n", format::NO_RESULTS);
        }
        // The running count is an estimate: cut for real if it was low.
        let mut cut = cut;
        if text.chars().count() > AI_MAX_OUTPUT_CHARS {
            let end = text
                .char_indices()
                .nth(AI_MAX_OUTPUT_CHARS)
                .map_or(text.len(), |(end, _)| end);
            // Back to the end of the last whole line.
            let end = text[..end].rfind('\n').map_or(0, |i| i + 1);
            text.truncate(end);
            cut = true;
        }
        if cut {
            text.push_str(format::OUTPUT_CUT);
            text.push('\n');
        }
        text
    }

    /// Rows the collector received, and whether it kept anything to show.
    fn has_output(&self) -> bool {
        !self.sets.is_empty() || !self.messages.is_empty()
    }
}

fn short_cell(value: Value) -> Value {
    match value {
        Value::Text(text) => match shorten_text(&text, AI_CELL_CHARS) {
            Some(shortened) => Value::Text(shortened),
            None => Value::Text(text),
        },
        other => other,
    }
}

fn cell_chars(value: &Value) -> usize {
    match value {
        Value::Text(text) => text.chars().count() + 1,
        _ => OTHER_CELL_CHARS,
    }
}

impl AiState {
    /// Records a run. A log that cannot be written never stops the query.
    async fn log_run(&self, v: &Visible, database: &str, sql: &str, result: Logged) {
        let run = NewRun {
            at_ms: Utc::now().timestamp_millis(),
            connection_id: v.conn.id.clone(),
            connection_name: v.conn.name.clone(),
            color: v.conn.color.clone(),
            database: database.to_string(),
            sql: sql.to_string(),
            outcome: result.outcome,
            rows: result.rows,
            elapsed_ms: result.elapsed_ms,
            source: RunSource::Ai,
            client: self.client(),
        };
        let log = self.runlog.clone();
        let stored = blocking(move || log.insert(&run)).await;
        if !matches!(stored, Ok(Ok(_))) {
            eprintln!("tulkabd: could not record an AI query in the run log");
        }
    }
}

/// What the run log gets for a run.
struct Logged {
    outcome: RunLogOutcome,
    rows: Option<u64>,
    elapsed_ms: Option<u64>,
}

impl Logged {
    /// Nothing ran: the query was refused or the session could not be made.
    fn not_run(outcome: RunLogOutcome) -> Self {
        Self {
            outcome,
            rows: None,
            elapsed_ms: None,
        }
    }
}

/// Runs `req.sql` read-only on the connection. `cancelled` resolves when the
/// client gives up on the call; the query is then cancelled.
pub async fn run_query(
    s: &AiState,
    req: QueryRequest,
    cancelled: impl Future<Output = ()> + Send,
) -> ToolText {
    let (visible, prefs) = match s.load().await {
        Ok(loaded) => loaded,
        Err(e) => return e,
    };
    let v = match AiState::find(&visible, &req.connection) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let sql = req.sql.as_str();
    let requested_db = req.database.clone().unwrap_or_default();
    let database = match v.database(req.database.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            s.log_run(v, &requested_db, sql, Logged::not_run(RunLogOutcome::Error))
                .await;
            return e;
        }
    };
    let mut log_db = database.clone().unwrap_or_default();

    let driver = match s.driver_for(v) {
        Ok(d) => d,
        Err(e) => return e,
    };
    if let Some(violation) = driver.read_only_violation(sql) {
        s.log_run(v, &log_db, sql, Logged::not_run(RunLogOutcome::Blocked))
            .await;
        return ToolText::error(format::not_run(&violation));
    }

    let (driver, params) = match s.prepare(v, database.clone()).await {
        Ok(p) => p,
        Err(e) => {
            s.log_run(v, &log_db, sql, Logged::not_run(RunLogOutcome::Error))
                .await;
            return e;
        }
    };
    let _slot = s.slot().await;
    let mut session = match driver.connect(&params).await {
        Ok(session) => session,
        Err(e) => {
            s.log_run(v, &log_db, sql, Logged::not_run(RunLogOutcome::Error))
                .await;
            return ToolText::error(format::connect_failed(&v.label, &e));
        }
    };
    if log_db.is_empty() {
        if let Ok(current) = session.current_database().await {
            log_db = current;
        }
    }

    let max_rows = req
        .max_rows
        .unwrap_or(prefs.ai_max_rows)
        .min(prefs.ai_max_rows)
        .max(1);
    let timeout_s = prefs.ai_query_timeout_s;
    let cancel = session.cancel_handle();
    let (tx, mut rx) = mpsc::channel(EVENT_BUFFER);
    let mut collector = Collector::new(max_rows);
    let started = Instant::now();

    let exec = session.execute_read_only(sql, max_rows, tx);
    let mut exec = exec;
    let deadline = sleep(Duration::from_secs(u64::from(timeout_s)));
    tokio::pin!(deadline);
    let grace = sleep(CANCEL_GRACE);
    tokio::pin!(grace);
    tokio::pin!(cancelled);
    let mut stop: Option<Stop> = None;
    // `None` when the query did not stop in time after being cancelled.
    let result: Option<Result<(), DriverError>> = loop {
        tokio::select! {
            r = &mut exec => break Some(r),
            Some(event) = rx.recv() => {
                if collector.push(event) && stop.is_none() {
                    stop = Some(Stop::OutputCut);
                    let _ = cancel.cancel().await;
                    grace.as_mut().reset(tokio::time::Instant::now() + CANCEL_GRACE);
                }
            }
            () = &mut deadline, if stop.is_none() => {
                stop = Some(Stop::Timeout);
                let _ = cancel.cancel().await;
                grace.as_mut().reset(tokio::time::Instant::now() + CANCEL_GRACE);
            }
            () = &mut cancelled, if stop.is_none() => {
                stop = Some(Stop::Client);
                let _ = cancel.cancel().await;
                grace.as_mut().reset(tokio::time::Instant::now() + CANCEL_GRACE);
            }
            () = &mut grace, if stop.is_some() => break None,
        }
    };
    drop(exec);
    while let Ok(event) = rx.try_recv() {
        collector.push(event);
    }
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    drop(session);

    let logged = |outcome| Logged {
        outcome,
        rows: Some(collector.received),
        elapsed_ms: Some(elapsed_ms),
    };
    let (outcome, answer) = match (stop, &result) {
        (Some(Stop::Timeout), _) => (
            RunLogOutcome::Cancelled,
            ToolText::error(format::timed_out(timeout_s)),
        ),
        (Some(Stop::Client), _) => (RunLogOutcome::Cancelled, ToolText::error(format::CANCELLED)),
        (Some(Stop::OutputCut), _) => (
            RunLogOutcome::Ok,
            ToolText::ok(collector.text(max_rows, true)),
        ),
        (None, Some(Err(e))) => {
            let outcome = match e {
                DriverError::Disconnected(_) => RunLogOutcome::Disconnected,
                DriverError::Cancelled => RunLogOutcome::Cancelled,
                _ => RunLogOutcome::Error,
            };
            let failure = if collector.has_output() {
                format!(
                    "{}{}\n",
                    collector.text(max_rows, false),
                    format::on_connection(&v.label, e)
                )
            } else {
                format::on_connection(&v.label, e)
            };
            (outcome, ToolText::error(failure))
        }
        _ if collector.disconnected => (
            RunLogOutcome::Disconnected,
            ToolText::error(format::on_connection(
                &v.label,
                &DriverError::Disconnected(String::new()),
            )),
        ),
        _ => match collector.error.clone() {
            Some(error) => (
                RunLogOutcome::Error,
                ToolText::error(format!("{}{error}\n", collector.text(max_rows, false))),
            ),
            None => (
                RunLogOutcome::Ok,
                ToolText::ok(collector.text(max_rows, false)),
            ),
        },
    };
    s.log_run(v, &log_db, sql, logged(outcome)).await;
    answer
}
