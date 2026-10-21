//! Integration tests against a live SQL Server.
//!
//! They are skipped (they print "skipped" and pass) unless `TULKA_MSSQL_TEST`
//! is set, e.g. `eval "$(bash scripts/mssql-test.sh)"`, which exports
//! `TULKA_MSSQL_TEST="server=127.0.0.1;port=14333;user=sa;password=..."`.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tulka_core::{ConnectParams, Driver, DriverError, ExecEvent, MessageKind, Session, Value};
use tulka_mssql::MssqlDriver;

/// Connection parameters from `TULKA_MSSQL_TEST`, or `None` (after printing
/// "skipped") when it is not set.
fn test_params() -> Option<ConnectParams> {
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

async fn connect(p: &ConnectParams) -> Box<dyn Session> {
    MssqlDriver.connect(p).await.expect("connect")
}

/// Runs `sql` and returns its result together with every emitted event.
async fn collect(
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

fn rows(events: &[ExecEvent]) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::Rows { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[tokio::test]
async fn connect_and_list_databases() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let dbs = s.list_databases().await.unwrap();
    assert!(dbs.iter().any(|d| d == "master"), "{dbs:?}");

    let info = s.server_info().await.unwrap();
    assert!(info.version.starts_with("Microsoft SQL Server"), "{info:?}");
    assert_eq!(info.user, p.user);

    s.use_database("tempdb").await.unwrap();
    assert_eq!(s.current_database().await.unwrap(), "tempdb");
    assert!(matches!(
        s.use_database("no_such_db]x").await,
        Err(DriverError::Query { .. })
    ));
}

#[tokio::test]
async fn driver_id_is_mssql() {
    assert_eq!(MssqlDriver.id(), "mssql");
}

#[tokio::test]
async fn wrong_password_is_auth_failed() {
    let Some(mut p) = test_params() else { return };
    p.password = "definitely-wrong".into();
    let result = MssqlDriver.connect(&p).await;
    assert!(
        matches!(result, Err(DriverError::AuthFailed(_))),
        "{:?}",
        result.err()
    );
}

#[tokio::test]
async fn unreachable_host_is_unreachable() {
    let Some(mut p) = test_params() else { return };
    p.port = 1;
    p.connect_timeout_s = 2;
    let started = Instant::now();
    let result = MssqlDriver.connect(&p).await;
    assert!(
        matches!(result, Err(DriverError::Unreachable(_))),
        "{:?}",
        result.err()
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Answers one SQL Server Browser (SSRP) request for `instance` on
/// `127.0.0.1:1434` with `port`. `None` if the UDP port is taken.
async fn fake_sql_browser(instance: &str, port: u16) -> Option<tokio::task::JoinHandle<()>> {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:1434").await.ok()?;
    let instance = instance.to_string();
    Some(tokio::spawn(async move {
        let mut buf = [0u8; 256];
        let (len, from) = socket.recv_from(&mut buf).await.unwrap();
        // CLNT_UCAST_INST (0x04) followed by the instance name.
        assert_eq!(&buf[..len], [&[4u8][..], instance.as_bytes()].concat());
        let data = format!(
            "ServerName;TULKA;InstanceName;{instance};IsClustered;No;Version;16.0;tcp;{port};;"
        );
        let mut reply = vec![5u8];
        reply.extend_from_slice(&(data.len() as u16).to_le_bytes());
        reply.extend_from_slice(data.as_bytes());
        socket.send_to(&reply, from).await.unwrap();
    }))
}

#[tokio::test]
async fn named_instance_resolves_through_sql_browser() {
    let Some(mut p) = test_params() else { return };
    // The login sends the instance name, which the server checks against its
    // own: the test container is the default instance, MSSQLSERVER.
    let Some(browser) = fake_sql_browser("MSSQLSERVER", p.port).await else {
        println!("skipped (UDP 127.0.0.1:1434 is in use)");
        return;
    };
    p.server = format!("{}\\MSSQLSERVER", p.server);
    p.port = 1433;
    let mut s = connect(&p).await;
    browser.await.unwrap();
    let (result, events) = collect(s.as_mut(), "SELECT 1").await;
    result.unwrap();
    assert_eq!(rows(&events), vec![vec![Value::Int(1)]]);
}

#[tokio::test]
async fn named_instance_without_sql_browser_is_unreachable() {
    // Needs no server: nothing answers SSRP on 127.0.0.2.
    let p = ConnectParams {
        server: "127.0.0.2\\NOPE".into(),
        connect_timeout_s: 5,
        ..ConnectParams::default()
    };
    let started = Instant::now();
    let result = MssqlDriver.connect(&p).await;
    match result {
        Err(DriverError::Unreachable(message)) => {
            assert!(message.contains("SQL Server Browser"), "{message}");
            assert!(message.starts_with("127.0.0.2\\NOPE"), "{message}");
        }
        other => panic!("{:?}", other.err()),
    }
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn maps_types_and_unicode() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "SELECT CAST(NULL AS int) a, N'Ferretería López 🐟' b, \
         CAST(8912450.50 AS decimal(18,2)) c, \
         CAST('6F9619FF-8B86-D011-B42D-00C04FC964FF' AS uniqueidentifier) d, \
         CAST('2026-10-20T14:03:00' AS datetime2(0)) e, CAST(12.5 AS money) f, \
         0x0A0B g, CAST(1 AS bit) h",
    )
    .await;
    result.unwrap();

    assert_eq!(
        rows(&events),
        vec![vec![
            Value::Null,
            Value::Text("Ferretería López 🐟".into()),
            Value::Decimal("8912450.50".into()),
            Value::Uuid("6F9619FF-8B86-D011-B42D-00C04FC964FF".into()),
            Value::DateTime("2026-10-20 14:03:00".into()),
            Value::Decimal("12.5000".into()),
            Value::Bytes(vec![10, 11]),
            Value::Bool(true),
        ]]
    );
}

#[tokio::test]
async fn maps_temporal_and_numeric_types() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "SELECT CAST('2026-10-20' AS date), CAST('14:03:00.123' AS time(3)), \
         CAST('2026-10-20 14:03:00.123' AS datetime), \
         CAST('2026-10-20 14:03:00 -03:00' AS datetimeoffset(0)), \
         CAST('2026-10-20 14:03' AS smalldatetime), \
         CAST(-0.05 AS decimal(5,2)), CAST(42 AS decimal(10,0)), \
         CAST(-3.25 AS smallmoney), CAST(1.5 AS float), CAST(0.1 AS real), \
         CAST(255 AS tinyint), CAST(-9000000000 AS bigint), \
         CAST('abc' AS varchar(10)), CAST('<a/>' AS xml)",
    )
    .await;
    result.unwrap();

    assert_eq!(
        rows(&events),
        vec![vec![
            Value::Date("2026-10-20".into()),
            Value::Time("14:03:00.123".into()),
            Value::DateTime("2026-10-20 14:03:00.123".into()),
            Value::DateTimeOffset("2026-10-20 14:03:00 -03:00".into()),
            Value::DateTime("2026-10-20 14:03:00".into()),
            Value::Decimal("-0.05".into()),
            Value::Decimal("42".into()),
            Value::Decimal("-3.2500".into()),
            Value::Float(1.5),
            Value::Float(0.1),
            Value::Int(255),
            Value::Int(-9_000_000_000),
            Value::Text("abc".into()),
            Value::Text("<a/>".into()),
        ]]
    );
}

#[tokio::test]
async fn multiple_result_sets() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(s.as_mut(), "SELECT 1 AS a; SELECT 2 AS b, 3 AS c;").await;
    result.unwrap();

    let starts: Vec<(u32, Vec<String>)> = events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::ResultStart { index, columns } => {
                Some((*index, columns.iter().map(|c| c.name.clone()).collect()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![
            (0, vec!["a".to_string()]),
            (1, vec!["b".to_string(), "c".to_string()])
        ]
    );
    let row_indexes: Vec<u32> = events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::Rows { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(row_indexes, vec![0, 1]);
}

#[tokio::test]
async fn print_and_rows_affected() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "CREATE TABLE #t(x int); INSERT #t VALUES (1),(2),(3); PRINT 'hola';",
    )
    .await;
    result.unwrap();

    assert!(
        events.contains(&ExecEvent::RowsAffected { count: 3 }),
        "{events:?}"
    );
    assert!(
        events.iter().any(|ev| matches!(
            ev,
            ExecEvent::Message { kind: MessageKind::Info, text, .. } if text == "hola"
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn messages_report_editor_lines() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(s.as_mut(), "SELECT 1\nGO\n\nPRINT 'x'").await;
    result.unwrap();
    assert!(
        events.contains(&ExecEvent::Message {
            kind: MessageKind::Info,
            text: "x".into(),
            line: Some(4)
        }),
        "{events:?}"
    );
}

#[tokio::test]
async fn error_reports_editor_line() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(s.as_mut(), "SELECT 1\nGO\nSELECT 1\nSELECT 1/0").await;

    assert!(
        matches!(result, Err(DriverError::Query { line: Some(4), .. })),
        "{result:?}"
    );
    let done = events
        .iter()
        .position(|ev| *ev == ExecEvent::BatchDone { batch: 0 })
        .expect("BatchDone 0");
    let error = events
        .iter()
        .position(|ev| {
            matches!(
                ev,
                ExecEvent::Message {
                    kind: MessageKind::Error,
                    line: Some(4),
                    ..
                }
            )
        })
        .expect("error message");
    assert!(done < error, "{events:?}");
    assert!(!events.contains(&ExecEvent::BatchDone { batch: 1 }));

    // The session is still usable after a query error.
    let (result, _) = collect(s.as_mut(), "SELECT 1").await;
    result.unwrap();
}

#[tokio::test]
async fn failed_batch_stops_the_script() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "CREATE TABLE #s(x int)\nGO\nSELECT * FROM no_such_table\nGO\nINSERT #s VALUES (1)",
    )
    .await;
    assert!(
        matches!(result, Err(DriverError::Query { line: Some(3), .. })),
        "{result:?}"
    );
    assert!(!events.contains(&ExecEvent::RowsAffected { count: 1 }));
}

#[tokio::test]
async fn go_batches_run_in_order() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "CREATE TABLE #g(x int)\nGO\nINSERT #g VALUES (7)\nGO\nSELECT x FROM #g",
    )
    .await;
    result.unwrap();

    assert_eq!(rows(&events), vec![vec![Value::Int(7)]]);
    let dones: Vec<u32> = events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::BatchDone { batch } => Some(*batch),
            _ => None,
        })
        .collect();
    assert_eq!(dones, vec![0, 1, 2]);
}

#[tokio::test]
async fn cancel_waitfor() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;
    let handle = s.cancel_handle();

    let started = Instant::now();
    let ((result, _events), cancelled) =
        tokio::join!(collect(s.as_mut(), "WAITFOR DELAY '00:00:30'"), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            handle.cancel().await
        });
    cancelled.unwrap();
    assert!(matches!(result, Err(DriverError::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(3));

    let (result, events) = collect(s.as_mut(), "SELECT 1").await;
    result.unwrap();
    assert_eq!(rows(&events), vec![vec![Value::Int(1)]]);
}

#[tokio::test]
async fn cancel_between_batches_stops_the_script() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;
    let handle = s.cancel_handle();

    // The cancel lands while the second batch waits, so the third batch must
    // not run.
    let script = "SELECT 1\nGO\nWAITFOR DELAY '00:00:30'\nGO\nSELECT 3";
    let ((result, events), cancelled) = tokio::join!(collect(s.as_mut(), script), async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        handle.cancel().await
    });
    cancelled.unwrap();
    assert!(matches!(result, Err(DriverError::Cancelled)), "{result:?}");
    assert!(!rows(&events).contains(&vec![Value::Int(3)]));
}

#[tokio::test]
async fn killed_session_reports_disconnected() {
    let Some(p) = test_params() else { return };
    let mut victim = connect(&p).await;
    let mut killer = connect(&p).await;

    let (result, events) = collect(victim.as_mut(), "SELECT @@SPID").await;
    result.unwrap();
    let spid = match rows(&events).as_slice() {
        [row] => match row.as_slice() {
            [Value::Int(spid)] => *spid,
            other => panic!("unexpected row {other:?}"),
        },
        other => panic!("unexpected rows {other:?}"),
    };

    let (result, _) = collect(killer.as_mut(), &format!("KILL {spid}")).await;
    result.unwrap();

    let (result, events) = collect(victim.as_mut(), "SELECT 1").await;
    assert!(
        matches!(result, Err(DriverError::Disconnected(_))),
        "{result:?}"
    );
    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, ExecEvent::Disconnected { .. })),
        "{events:?}"
    );

    // The session stays unusable.
    assert!(matches!(
        victim.current_database().await,
        Err(DriverError::Disconnected(_))
    ));
}

#[tokio::test]
async fn large_result_streams_in_batches() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "SELECT TOP 2000 a.object_id, b.name \
         FROM sys.all_objects a CROSS JOIN sys.all_objects b",
    )
    .await;
    result.unwrap();

    let sizes: Vec<usize> = events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::Rows { rows, .. } => Some(rows.len()),
            _ => None,
        })
        .collect();
    assert!(sizes.len() > 1, "{sizes:?}");
    assert!(sizes.iter().all(|&n| n <= 500), "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), 2000);
    assert!(events.contains(&ExecEvent::RowsAffected { count: 2000 }));
}
