//! `run_query`: a read-only query for the assistant, with limits on rows,
//! cell size, output size and time, and an entry in the run log.

use std::future::Future;
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tulka_core::{Column, DriverError, ExecEvent, MessageKind, Value};
use tulka_store::{NewRun, RunLogOutcome, RunSource};

use super::tools::environment_name;
use super::{format, AiState, ToolText, Visible, AI_CELL_CHARS, AI_MAX_OUTPUT_CHARS};
use crate::connections::blocking;
use crate::results::shorten_text;

/// How long a cancelled query may take to stop before it is abandoned.
const CANCEL_GRACE: Duration = Duration::from_secs(10);
/// Events buffered between the driver and the collector.
const EVENT_BUFFER: usize = 64;

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
                    if set.rows.len() >= self.max_rows {
                        set.limit_reached = true;
                        continue;
                    }
                    let row: Vec<Value> = row.into_iter().map(short_cell).collect();
                    self.chars += row.iter().map(cell_chars).sum::<usize>();
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

    /// Whether there is anything to show besides a notice.
    fn has_output(&self) -> bool {
        !self.sets.is_empty() || !self.messages.is_empty()
    }

    /// The answer: a header line, the result sets, the messages, the notice
    /// when the output was cut and then `trailer` (why the query stopped, or
    /// its error). The whole stays within the output budget: rows and
    /// messages that do not fit are left out, and the header counts only
    /// the rows that are in the text.
    fn render(&self, head: &Head<'_>, trailer: &str) -> Rendered {
        let chars = |s: &str| s.chars().count();
        let notice = format!("{}\n", format::OUTPUT_CUT);
        let trailer = if trailer.is_empty() {
            String::new()
        } else {
            format!("{}\n", capped(trailer))
        };
        // Room kept for the header, the lines that name each result set,
        // the notice and the trailer.
        let overhead = chars(&head.prefix()) + 120 + self.sets.len() * 160;
        let mut left =
            AI_MAX_OUTPUT_CHARS.saturating_sub(overhead + chars(&notice) + chars(&trailer));
        let mut cut = self.cut;

        let no_rows: &[Vec<Value>] = &[];
        let mut tables: Vec<(String, usize)> = Vec::new();
        for set in &self.sets {
            let mut table = tulka_core::to_ai(&set.columns, no_rows);
            let header_chars = chars(&table);
            left = left.saturating_sub(header_chars);
            let mut shown = 0;
            for row in &set.rows {
                // Just the row's line: what `to_ai` writes after the header.
                let line = tulka_core::to_ai(&set.columns, std::slice::from_ref(row));
                let line = &line[table_header_len(&set.columns)..];
                if chars(line) > left {
                    cut = true;
                    left = 0;
                    break;
                }
                left -= chars(line);
                table.push_str(line);
                shown += 1;
            }
            tables.push((table, shown));
        }
        let mut messages = String::new();
        for line in &self.messages {
            if chars(line) + 1 > left {
                cut = true;
                break;
            }
            left -= chars(line) + 1;
            messages.push_str(line);
            messages.push('\n');
        }

        let mark = format::LIMIT_REACHED;
        let rows = |n: usize| {
            if n == 1 {
                "1 row".to_string()
            } else {
                format!("{n} rows")
            }
        };
        let limit = |set: &ResultSet| {
            if set.limit_reached {
                format!(" ({mark})")
            } else {
                String::new()
            }
        };
        let prefix = head.prefix();
        let ms = head.elapsed_ms;
        let mut out = String::new();
        match (self.sets.as_slice(), tables.as_slice()) {
            ([], _) => out.push_str(&format!("{prefix} · no rows returned · {ms} ms\n")),
            ([set], [(table, shown)]) => {
                out.push_str(&format!(
                    "{prefix} · {}{} · {ms} ms\n{table}",
                    rows(*shown),
                    limit(set)
                ));
            }
            (sets, tables) => {
                out.push_str(&format!(
                    "{prefix} · {} result sets · {ms} ms\n",
                    sets.len()
                ));
                for (i, (set, (table, shown))) in sets.iter().zip(tables).enumerate() {
                    out.push_str(&format!(
                        "#{} · {}{}\n{table}",
                        i + 1,
                        rows(*shown),
                        limit(set)
                    ));
                }
            }
        }
        if !messages.is_empty() {
            out.push_str("Messages:\n");
            out.push_str(&messages);
        }
        if cut {
            out.push_str(&notice);
        }
        out.push_str(&trailer);
        Rendered {
            text: out,
            rows: tables.iter().map(|(_, shown)| *shown as u64).sum(),
        }
    }
}

/// An answer and how many result rows it contains.
struct Rendered {
    text: String,
    rows: u64,
}

/// Longest error or notice text appended to an answer, in characters.
const TRAILER_CHARS: usize = 2000;

fn capped(text: &str) -> String {
    shorten_text(text, TRAILER_CHARS).unwrap_or_else(|| text.to_string())
}

/// Length of the header line `to_ai` writes for `columns`.
fn table_header_len(columns: &[Column]) -> usize {
    let no_rows: &[Vec<Value>] = &[];
    tulka_core::to_ai(columns, no_rows).len()
}

/// What the header line of an answer names.
struct Head<'a> {
    label: &'a str,
    environment: String,
    database: &'a str,
    elapsed_ms: u64,
}

impl Head<'_> {
    /// `Sales · TEST › Clientes`
    fn prefix(&self) -> String {
        let env = self.environment.to_uppercase();
        if self.database.is_empty() {
            format!("{} · {env}", self.label)
        } else {
            format!("{} · {env} › {}", self.label, self.database)
        }
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

/// About how many characters a cell takes in the answer, with its separator.
fn cell_chars(value: &Value) -> usize {
    1 + match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int(i) => i.to_string().len(),
        Value::Float(f) => f.to_string().len(),
        Value::Decimal(text)
        | Value::Date(text)
        | Value::Time(text)
        | Value::DateTime(text)
        | Value::DateTimeOffset(text)
        | Value::Uuid(text) => text.len(),
        // Quotes around text that needs them.
        Value::Text(text) => text.chars().count() + 2,
        Value::Bytes(bytes) => 2 + 2 * bytes.len().min(32) + 1,
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
    let database = match v.database(req.database.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            let requested = req.database.clone().unwrap_or_default();
            s.log_run(v, &requested, sql, Logged::not_run(RunLogOutcome::Error))
                .await;
            return e;
        }
    };
    let mut log_db = database.clone().unwrap_or_default();

    let driver = match s.driver_for(v) {
        Ok(d) => d,
        Err(e) => {
            s.log_run(v, &log_db, sql, Logged::not_run(RunLogOutcome::Error))
                .await;
            return e;
        }
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
    tokio::pin!(cancelled);
    // Until the query starts, the client giving up just ends the call.
    let gave_up = |outcome: ToolText| async {
        s.log_run(v, &log_db, sql, Logged::not_run(RunLogOutcome::Cancelled))
            .await;
        outcome
    };
    let _slot = tokio::select! {
        slot = s.slot() => slot,
        () = &mut cancelled => return gave_up(ToolText::error(format::CANCELLED)).await,
    };
    let connected = tokio::select! {
        session = driver.connect(&params) => session,
        () = &mut cancelled => return gave_up(ToolText::error(format::CANCELLED)).await,
    };
    let mut session = match connected {
        Ok(session) => session,
        Err(e) => {
            s.log_run(v, &log_db, sql, Logged::not_run(RunLogOutcome::Error))
                .await;
            return ToolText::error(format::connect_failed(&v.label, &e));
        }
    };
    if log_db.is_empty() {
        let current = tokio::select! {
            current = session.current_database() => current,
            () = &mut cancelled => return gave_up(ToolText::error(format::CANCELLED)).await,
        };
        if let Ok(current) = current {
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

    let mut exec = session.execute_read_only(sql, max_rows, tx);
    let deadline = sleep(Duration::from_secs(u64::from(timeout_s)));
    tokio::pin!(deadline);
    let grace = sleep(CANCEL_GRACE);
    tokio::pin!(grace);
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

    let head = Head {
        label: &v.label,
        environment: environment_name(v),
        database: &log_db,
        elapsed_ms,
    };
    // Why the query stopped (or its error), after whatever was collected.
    let (outcome, is_error, trailer) = match (stop, &result) {
        (Some(Stop::Timeout), _) => (RunLogOutcome::Cancelled, true, format::timed_out(timeout_s)),
        (Some(Stop::Client), _) => (
            RunLogOutcome::Cancelled,
            true,
            format::CANCELLED.to_string(),
        ),
        (Some(Stop::OutputCut), _) => (RunLogOutcome::Ok, false, String::new()),
        (None, Some(Err(e))) => {
            let outcome = match e {
                DriverError::Disconnected(_) => RunLogOutcome::Disconnected,
                DriverError::Cancelled => RunLogOutcome::Cancelled,
                _ => RunLogOutcome::Error,
            };
            (outcome, true, format::on_connection(&v.label, e))
        }
        _ if collector.disconnected => (
            RunLogOutcome::Disconnected,
            true,
            format::on_connection(&v.label, &DriverError::Disconnected(String::new())),
        ),
        _ => match collector.error.clone() {
            Some(error) => (RunLogOutcome::Error, true, error),
            None => (RunLogOutcome::Ok, false, String::new()),
        },
    };
    let rendered = if is_error && !collector.has_output() {
        Rendered {
            text: capped(&trailer),
            rows: 0,
        }
    } else {
        collector.render(&head, &trailer)
    };
    let logged = Logged {
        outcome,
        rows: Some(rendered.rows),
        elapsed_ms: Some(elapsed_ms),
    };
    s.log_run(v, &log_db, sql, logged).await;
    ToolText {
        text: rendered.text,
        is_error,
    }
}
