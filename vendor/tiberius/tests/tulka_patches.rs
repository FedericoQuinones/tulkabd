//! Integration tests for the tulka patches (see `TULKA_PATCHES.md`).
//!
//! They need a live SQL Server and are skipped (they return early and pass)
//! unless `TULKA_MSSQL_TEST` is set, e.g.
//! `TULKA_MSSQL_TEST="server=127.0.0.1;port=14333;user=sa;password=..."`.
//! The server certificate is trusted unconditionally (test containers use a
//! self-signed one).

use std::time::{Duration, Instant};

use futures_util::stream::TryStreamExt;
use tiberius::error::Error;
use tiberius::{AuthMethod, Client, Config, DoneInfo, InfoMessage, QueryItem};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

type TestClient = Client<Compat<TcpStream>>;

/// Builds a `Config` from `TULKA_MSSQL_TEST`, or `None` when it is not set.
fn config() -> Option<Config> {
    let raw = std::env::var("TULKA_MSSQL_TEST").ok()?;

    let mut config = Config::new();
    let (mut user, mut password) = (String::new(), String::new());

    for part in raw.split(';').filter(|p| !p.trim().is_empty()) {
        let (key, value) = part
            .split_once('=')
            .expect("TULKA_MSSQL_TEST entries must be key=value");

        match key.trim().to_lowercase().as_str() {
            "server" => config.host(value.trim()),
            "port" => config.port(value.trim().parse().expect("port must be a number")),
            "user" => user = value.to_string(),
            "password" => password = value.to_string(),
            _ => {}
        }
    }

    config.authentication(AuthMethod::sql_server(user, password));
    config.trust_cert();

    Some(config)
}

/// Connects to the test server, or returns `None` (after printing "skipped")
/// when `TULKA_MSSQL_TEST` is not set.
async fn connect(test: &str) -> Option<TestClient> {
    let Some(config) = config() else {
        println!("{test}: skipped (TULKA_MSSQL_TEST not set)");
        return None;
    };

    let tcp = TcpStream::connect(config.get_addr())
        .await
        .expect("tcp connect");
    tcp.set_nodelay(true).expect("set_nodelay");

    Some(
        Client::connect(config, tcp.compat_write())
            .await
            .expect("login"),
    )
}

/// Runs a batch and collects every item of its stream.
async fn collect(client: &mut TestClient, sql: &str) -> tiberius::Result<Vec<QueryItem>> {
    client.simple_query(sql).await?.try_collect().await
}

fn dones(items: &[QueryItem]) -> Vec<DoneInfo> {
    items
        .iter()
        .filter_map(|item| match item {
            QueryItem::Done(done) => Some(*done),
            _ => None,
        })
        .collect()
}

async fn select_one(client: &mut TestClient) -> i32 {
    client
        .simple_query("SELECT 1")
        .await
        .expect("SELECT 1 after cancel")
        .into_row()
        .await
        .expect("SELECT 1 row")
        .expect("SELECT 1 returned no row")
        .get::<i32, _>(0)
        .expect("SELECT 1 returned NULL")
}

#[tokio::test]
async fn print_yields_info_item() {
    let Some(mut client) = connect("print_yields_info_item").await else {
        return;
    };

    let items = collect(&mut client, "PRINT 'hola'").await.unwrap();

    let infos: Vec<&InfoMessage> = items
        .iter()
        .filter_map(|item| match item {
            QueryItem::Info(info) => Some(info),
            _ => None,
        })
        .collect();

    assert_eq!(infos.len(), 1, "items: {items:?}");
    assert_eq!(infos[0].message, "hola");
    assert_eq!(infos[0].number, 0);
    assert_eq!(infos[0].class, 0);
    assert_eq!(infos[0].line, 1);

    // RAISERROR with severity <= 10 is an informational message as well, and
    // carries its number, state and class.
    let items = collect(&mut client, "SELECT 1;\nRAISERROR('aviso', 10, 3)")
        .await
        .unwrap();
    let info = items
        .iter()
        .find_map(|item| match item {
            QueryItem::Info(info) => Some(info),
            _ => None,
        })
        .expect("RAISERROR info item");
    assert_eq!(info.message, "aviso");
    assert_eq!(info.number, 50000);
    // SQL Server reports severity 10 as class 0 on the wire.
    assert_eq!(info.class, 0);
    assert_eq!(info.state, 3);
    assert_eq!(info.line, 2);
}

#[tokio::test]
async fn update_yields_rows_affected() {
    let Some(mut client) = connect("update_yields_rows_affected").await else {
        return;
    };

    collect(
        &mut client,
        "CREATE TABLE #t (id INT); INSERT INTO #t VALUES (1), (2), (3), (4);",
    )
    .await
    .unwrap();

    let items = collect(&mut client, "UPDATE #t SET id = id + 10 WHERE id <= 3")
        .await
        .unwrap();

    assert_eq!(
        dones(&items),
        vec![DoneInfo {
            rows_affected: Some(3)
        }],
        "items: {items:?}"
    );
}

/// Every statement of a batch produces exactly one `Done`, in order; the ones
/// without a row count (DDL, SET, ...) carry `None`.
#[tokio::test]
async fn every_statement_yields_one_done() {
    let Some(mut client) = connect("every_statement_yields_one_done").await else {
        return;
    };

    let items = collect(
        &mut client,
        "SET NOCOUNT OFF;\n\
         CREATE TABLE #d (id INT);\n\
         INSERT INTO #d VALUES (1), (2);\n\
         SELECT id FROM #d;\n\
         PRINT 'between';\n\
         DELETE FROM #d;",
    )
    .await
    .unwrap();

    let counts: Vec<Option<u64>> = dones(&items).iter().map(|d| d.rows_affected).collect();
    // SET, CREATE, INSERT, SELECT, PRINT, DELETE.
    assert_eq!(
        counts,
        vec![None, None, Some(2), Some(2), None, Some(2)],
        "items: {items:?}"
    );

    // The item order follows the server: metadata and rows of the SELECT come
    // before its Done, and the PRINT message before the PRINT's Done.
    let shape: Vec<&str> = items
        .iter()
        .map(|item| match item {
            QueryItem::Metadata(_) => "meta",
            QueryItem::Row(_) => "row",
            QueryItem::Info(_) => "info",
            QueryItem::Done(_) => "done",
        })
        .collect();
    assert_eq!(
        shape,
        vec!["done", "done", "done", "meta", "row", "row", "done", "info", "done", "done"]
    );

    // `query` (sp_executesql) yields the same statement-level items: the
    // envelope DONEPROC of the RPC itself is not reported.
    let items: Vec<QueryItem> = client
        .query("PRINT 'p'; SELECT @P1 AS v", &[&7i32])
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let counts: Vec<Option<u64>> = dones(&items).iter().map(|d| d.rows_affected).collect();
    assert_eq!(counts, vec![None, Some(1)], "items: {items:?}");

    // In a batch, an EXEC statement ends with its own Done (from DONEPROC)
    // after the Done of each statement run inside the procedure, whether or
    // not it is the last statement of the batch.
    for (sql, expected) in [
        (
            "EXEC sp_executesql N'SELECT 1 AS a; SELECT 2 AS b'",
            vec![Some(1), Some(1), None],
        ),
        (
            "EXEC sp_executesql N'SELECT 1 AS a'; SELECT 3 AS c",
            vec![Some(1), None, Some(1)],
        ),
    ] {
        let items = collect(&mut client, sql).await.unwrap();
        let counts: Vec<Option<u64>> = dones(&items).iter().map(|d| d.rows_affected).collect();
        assert_eq!(counts, expected, "{sql}: {items:?}");
    }

    // The non-streaming helpers still return only rows.
    let rows = client
        .simple_query("PRINT 'x'; UPDATE #d SET id = 1; SELECT 5")
        .await
        .unwrap()
        .into_results()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].get::<i32, _>(0), Some(5));
}

#[tokio::test]
async fn cancel_stops_waitfor_and_client_survives() {
    let Some(mut client) = connect("cancel_stops_waitfor_and_client_survives").await else {
        return;
    };

    let handle = client.cancel_handle();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        handle.cancel().await
    });

    let started = Instant::now();
    let outcome = collect(&mut client, "WAITFOR DELAY '00:00:30'").await;
    let elapsed = started.elapsed();

    canceller.await.unwrap().unwrap();
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
    assert!(
        matches!(outcome, Err(Error::Cancelled)),
        "expected Error::Cancelled, got {outcome:?}"
    );

    assert_eq!(select_one(&mut client).await, 1);
    // Responses stay aligned for later requests too.
    assert_eq!(select_one(&mut client).await, 1);
}

/// Cancelling while rows are being streamed stops the stream after the
/// Attention is acknowledged, and the connection stays in sync.
#[tokio::test]
async fn cancel_mid_stream_keeps_client_usable() {
    let Some(mut client) = connect("cancel_mid_stream_keeps_client_usable").await else {
        return;
    };

    let handle = client.cancel_handle();
    let mut seen = 0usize;
    let outcome: tiberius::Result<()> = async {
        let mut stream = client
            .simple_query(
                "SELECT a.object_id, b.object_id, REPLICATE('x', 200) AS pad \
                 FROM sys.all_objects a CROSS JOIN sys.all_objects b",
            )
            .await?;
        while let Some(item) = stream.try_next().await? {
            if let QueryItem::Row(_) = item {
                seen += 1;
                if seen == 100 {
                    handle.cancel().await?;
                }
            }
        }
        Ok(())
    }
    .await;

    assert!(
        matches!(outcome, Err(Error::Cancelled)),
        "expected Error::Cancelled, got {outcome:?} after {seen} rows"
    );
    assert_eq!(select_one(&mut client).await, 1);
}

/// A cancel requested while no request is in flight is a no-op: it must not
/// cancel the next request.
#[tokio::test]
async fn idle_cancel_does_not_affect_next_request() {
    let Some(mut client) = connect("idle_cancel_does_not_affect_next_request").await else {
        return;
    };

    client.cancel_handle().cancel().await.unwrap();
    assert_eq!(select_one(&mut client).await, 1);
}

/// The pre-existing `Client::cancel_query` (after dropping the request
/// future) returns `Ok` and leaves the connection in sync.
#[tokio::test]
async fn cancel_query_after_dropped_future_keeps_client_usable() {
    let Some(mut client) = connect("cancel_query_after_dropped_future_keeps_client_usable").await
    else {
        return;
    };

    let dropped = tokio::time::timeout(
        Duration::from_millis(500),
        collect(&mut client, "WAITFOR DELAY '00:00:30'"),
    )
    .await;
    assert!(dropped.is_err(), "the WAITFOR should still be running");

    let started = Instant::now();
    client.cancel_query().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));

    assert_eq!(select_one(&mut client).await, 1);
    assert_eq!(select_one(&mut client).await, 1);
}

/// A cancel requested through the handle after the request future was
/// dropped is carried out by the next request, which then runs normally.
#[tokio::test]
async fn handle_cancel_after_dropped_future_is_applied_on_next_request() {
    let Some(mut client) =
        connect("handle_cancel_after_dropped_future_is_applied_on_next_request").await
    else {
        return;
    };

    let dropped = tokio::time::timeout(
        Duration::from_millis(500),
        collect(&mut client, "WAITFOR DELAY '00:00:30'"),
    )
    .await;
    assert!(dropped.is_err(), "the WAITFOR should still be running");

    client.cancel_handle().cancel().await.unwrap();

    let started = Instant::now();
    assert_eq!(select_one(&mut client).await, 1);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(select_one(&mut client).await, 1);
}

/// `Client::execute` (no result stream) is cancelled the same way.
#[tokio::test]
async fn cancel_stops_execute_and_client_survives() {
    let Some(mut client) = connect("cancel_stops_execute_and_client_survives").await else {
        return;
    };

    let handle = client.cancel_handle();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.cancel().await
    });

    let started = Instant::now();
    let outcome = client.execute("WAITFOR DELAY '00:00:30'", &[]).await;

    canceller.await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        matches!(outcome, Err(Error::Cancelled)),
        "expected Error::Cancelled, got {outcome:?}"
    );
    assert_eq!(select_one(&mut client).await, 1);
}
