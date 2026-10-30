//! Shared by the live tests: the `TULKA_MSSQL_TEST` parser and a temporary
//! database. Included with `#[path]`, so it is not a test target of its own.

use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tulka_core::{ConnectParams, Driver, DriverError, ExecEvent, Session, Value};
use tulka_mssql::MssqlDriver;

/// Connection parameters from `TULKA_MSSQL_TEST`, or `None` (after printing
/// "skipped") when it is not set.
pub fn test_params() -> Option<ConnectParams> {
    let Ok(raw) = std::env::var("TULKA_MSSQL_TEST") else {
        println!("skipped (TULKA_MSSQL_TEST not set)");
        return None;
    };

    let mut p = ConnectParams {
        server: "127.0.0.1".into(),
        port: 1433,
        encrypt: true,
        trust_server_certificate: true,
        connect_timeout_s: 15,
        ..ConnectParams::default()
    };
    for part in raw.split(';').filter(|s| !s.trim().is_empty()) {
        let (key, value) = part
            .split_once('=')
            .expect("TULKA_MSSQL_TEST entries must be key=value");
        match key.trim().to_lowercase().as_str() {
            "server" => p.server = value.trim().to_string(),
            "port" => p.port = value.trim().parse().expect("port must be a number"),
            "user" => p.user = value.to_string(),
            "password" => p.password = value.to_string(),
            _ => {}
        }
    }
    Some(p)
}

/// Runs `sql` and returns its result together with every emitted event.
pub async fn collect(
    session: &mut dyn Session,
    sql: &str,
) -> (Result<(), DriverError>, Vec<ExecEvent>) {
    let (tx, mut rx) = mpsc::channel(16);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        events
    });
    let result = session.execute(sql, tx).await;
    (result, collector.await.unwrap())
}

/// Runs `sql` on a new session and panics if it fails.
async fn run(p: &ConnectParams, sql: &str) {
    let mut s = MssqlDriver.connect(p).await.expect("connect");
    let (result, _) = collect(s.as_mut(), sql).await;
    result.expect(sql);
}

/// The number of rows of `dbo.w` as seen by a new session of `p`.
pub async fn count_w(p: &ConnectParams) -> i64 {
    let mut s = MssqlDriver.connect(p).await.expect("connect");
    let (result, events) = collect(s.as_mut(), "SELECT COUNT(*) FROM dbo.w").await;
    result.expect("count");
    events
        .iter()
        .find_map(|ev| match ev {
            ExecEvent::Rows { rows, .. } => match rows[0][0] {
                Value::Int(n) => Some(n),
                _ => None,
            },
            _ => None,
        })
        .expect("a count")
}

/// Creates a database `tulka_ro_<pid>_<nanos>` holding `dbo.w(id int)` with
/// three rows, runs `body` with parameters that connect to it, and drops the
/// database afterwards, also when `body` panics.
pub async fn with_temp_db<F, Fut>(admin: ConnectParams, body: F)
where
    F: FnOnce(ConnectParams) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("tulka_ro_{}_{nanos}", std::process::id());
    run(&admin, &format!("CREATE DATABASE [{name}]")).await;

    let mut p = admin.clone();
    p.database = Some(name.clone());
    // Spawned so that a panic in `body` still reaches the cleanup below.
    let outcome = tokio::spawn(prepare_and_run(p, body)).await;
    run(
        &admin,
        &format!(
            "ALTER DATABASE [{name}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; \
             DROP DATABASE [{name}]"
        ),
    )
    .await;
    if let Err(e) = outcome {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn prepare_and_run<F, Fut>(p: ConnectParams, body: F)
where
    F: FnOnce(ConnectParams) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    run(
        &p,
        "CREATE TABLE dbo.w(id int); INSERT INTO dbo.w VALUES (1),(2),(3)",
    )
    .await;
    body(p).await;
}
