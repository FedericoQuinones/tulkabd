//! Integration tests against a live SQL Server.
//!
//! They are skipped (they print "skipped" and pass) unless `TULKA_MSSQL_TEST`
//! is set, e.g. `eval "$(bash scripts/mssql-test.sh)"`, which exports
//! `TULKA_MSSQL_TEST="server=127.0.0.1;port=14333;user=sa;password=..."`.

use std::time::{Duration, Instant};

use tulka_core::{ConnectParams, Driver, DriverError, ExecEvent, MessageKind, Session, Value};
use tulka_mssql::MssqlDriver;

#[path = "support/params.rs"]
mod params;
use params::{collect, test_params};

async fn connect(p: &ConnectParams) -> Box<dyn Session> {
    MssqlDriver.connect(p).await.expect("connect")
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
async fn money_is_exact_at_the_extremes() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "SELECT CAST(922337203685477.5807 AS money), CAST(-922337203685477.5808 AS money), \
         CAST(-0.0001 AS money), CAST(214748.3647 AS smallmoney), \
         CAST(-214748.3648 AS smallmoney), CAST(-1.5 AS smallmoney), \
         CAST(NULL AS money), \
         ISNULL(CAST(922337203685477.5807 AS money), 0), ISNULL(CAST(-214748.3648 AS smallmoney), 0)",
    )
    .await;
    result.unwrap();
    let dec = |s: &str| Value::Decimal(s.into());
    assert_eq!(
        rows(&events),
        vec![vec![
            dec("922337203685477.5807"),
            dec("-922337203685477.5808"),
            dec("-0.0001"),
            dec("214748.3647"),
            dec("-214748.3648"),
            dec("-1.5000"),
            Value::Null,
            dec("922337203685477.5807"),
            dec("-214748.3648"),
        ]]
    );
}

#[tokio::test]
async fn nullable_small_types_keep_their_names() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "CREATE TABLE #n(a smalldatetime NULL, b smallmoney NULL, c datetime NULL, d money NULL, \
         e smalldatetime NOT NULL, f smallmoney NOT NULL); SELECT * FROM #n",
    )
    .await;
    result.unwrap();
    let names: Vec<String> = events
        .iter()
        .find_map(|ev| match ev {
            ExecEvent::ResultStart { columns, .. } => {
                Some(columns.iter().map(|c| c.type_name.clone()).collect())
            }
            _ => None,
        })
        .expect("a result set");
    assert_eq!(
        names,
        [
            "smalldatetime",
            "smallmoney",
            "datetime",
            "money",
            "smalldatetime",
            "smallmoney"
        ]
    );
}

#[tokio::test]
async fn varchar_in_oem_collations_decodes() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    // CP437 (sort id 32) and CP850 (sort id 42), also through char and text.
    let (result, events) = collect(
        s.as_mut(),
        "SELECT CAST(N'x\u{e7}\u{2591}' COLLATE SQL_Latin1_General_CP437_CI_AS AS varchar(10)), \
         CAST(N'x\u{e7}\u{2591}' COLLATE SQL_Latin1_General_CP850_CI_AS AS varchar(10)), \
         CAST(N'\u{f8}\u{a2}' COLLATE SQL_Latin1_General_CP850_CS_AS AS char(3)), \
         CAST(N'\u{a2}\u{a5}' COLLATE SQL_Latin1_General_CP437_BIN AS varchar(10)), \
         CAST(CAST(N'\u{e9}' COLLATE SQL_Latin1_General_CP850_CI_AS AS varchar(5)) AS sql_variant)",
    )
    .await;
    result.unwrap();
    assert_eq!(
        rows(&events),
        vec![vec![
            Value::Text("x\u{e7}\u{2591}".into()),
            Value::Text("x\u{e7}\u{2591}".into()),
            Value::Text("\u{f8}\u{a2} ".into()),
            Value::Text("\u{a2}\u{a5}".into()),
            Value::Text("\u{e9}".into()),
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
async fn module_errors_name_the_procedure_and_do_not_jump() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    // The error is on line 3 of the procedure, which the batch calls on its
    // line 7: neither number may be offered as an editor line.
    let sql = "CREATE PROCEDURE #tulka_fails AS\nBEGIN\n  SELECT 1/0\nEND\nGO\nSELECT 1\nEXEC #tulka_fails";
    let (result, events) = collect(s.as_mut(), sql).await;

    let error = events
        .iter()
        .find_map(|ev| match ev {
            ExecEvent::Message {
                kind: MessageKind::Error,
                text,
                line,
            } => Some((text.clone(), *line)),
            _ => None,
        })
        .expect("error message");
    assert_eq!(error.1, None, "{events:?}");
    assert!(error.0.contains("Divide by zero"), "{}", error.0);
    assert!(
        error.0.contains("Procedure #tulka_fails") && error.0.contains(", line 3"),
        "{}",
        error.0
    );
    assert!(
        matches!(result, Err(DriverError::Query { .. })),
        "{result:?}"
    );
}

/// The text and editor line of the first error message of `events`.
fn first_error(events: &[ExecEvent]) -> (String, Option<u32>) {
    events
        .iter()
        .find_map(|ev| match ev {
            ExecEvent::Message {
                kind: MessageKind::Error,
                text,
                line,
            } => Some((text.clone(), *line)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no error message in {events:?}"))
}

#[tokio::test]
async fn module_ddl_errors_keep_the_editor_line() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    // The server names the module here, but counts lines from the batch start.
    let cases = [
        (
            "-- leading comment\nCREATE PROCEDURE dbo.tulka_p AS\nBEGIN\n  SELECT 1 FROM\nEND",
            5,
        ),
        (
            "SELECT 1\nGO\nCREATE OR ALTER PROCEDURE dbo.tulka_p2 AS\nBEGIN\n  SELECT nope FROM sys.objects\nEND",
            5,
        ),
        (
            "SELECT 1\nGO\n/* c */ create view dbo.tulka_v as\nSELECT nope FROM sys.objects",
            4,
        ),
        (
            "SELECT 1\nGO\nCREATE TRIGGER dbo.tulka_trg ON dbo.tulka_no_table AFTER INSERT AS\nSELECT 1",
            3,
        ),
    ];
    for (sql, line) in cases {
        let (result, events) = collect(s.as_mut(), sql).await;
        assert!(result.is_err(), "{sql}");
        let error = first_error(&events);
        assert_eq!(error.1, Some(line), "{sql}\n{events:?}");
    }
}

#[tokio::test]
async fn module_ddl_errors_survive_unclosed_text_and_a_late_create() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let cases = [
        // An unclosed string or comment only fails the batch itself.
        (
            "CREATE PROCEDURE dbo.tulka_px AS\nBEGIN\n  SELECT 'abc\nEND",
            3,
        ),
        (
            "CREATE PROCEDURE dbo.tulka_px AS\nBEGIN\n  SELECT 1 /* x\nEND",
            4,
        ),
        // "must be the first statement in a query batch".
        ("SELECT 1\nCREATE PROCEDURE dbo.tulka_px AS SELECT 1", 2),
    ];
    for (sql, line) in cases {
        let (result, events) = collect(s.as_mut(), sql).await;
        assert!(result.is_err(), "{sql}");
        assert_eq!(first_error(&events).1, Some(line), "{sql}\n{events:?}");
    }
}

#[tokio::test]
async fn messages_of_a_ddl_trigger_keep_their_own_module() {
    let Some(admin) = test_params() else { return };
    params::with_temp_db(admin, |p| async move {
        let mut s = connect(&p).await;
        let trigger = "CREATE TRIGGER tulka_ddl ON DATABASE FOR CREATE_PROCEDURE AS\nBEGIN\n  PRINT 'trigger says'\n  RAISERROR('trigger error', 16, 1)\nEND";
        let (result, _) = collect(s.as_mut(), trigger).await;
        result.unwrap();

        // Two lines of comment, so the module's own text starts on line 3.
        let (_, events) = collect(
            s.as_mut(),
            "-- one\n-- two\nCREATE PROCEDURE dbo.tulka_px AS\nSELECT 1",
        )
        .await;
        let print = events.iter().find_map(|ev| match ev {
            ExecEvent::Message {
                kind: MessageKind::Info,
                text,
                line,
            } if text == "trigger says" => Some(*line),
            _ => None,
        });
        assert_eq!(print, Some(None), "{events:?}");
        let (text, line) = first_error(&events);
        assert_eq!(line, None, "{events:?}");
        assert!(
            text.contains("trigger error") && text.contains("Procedure tulka_ddl, line 4"),
            "{text}"
        );
    })
    .await;
}

#[tokio::test]
async fn messages_from_inside_a_module_have_no_editor_line() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let sql = "CREATE PROCEDURE #tulka_talks AS\nBEGIN\n  PRINT 'inside'\n  RAISERROR('low', 10, 1)\nEND\nGO\nPRINT 'outside'\nEXEC #tulka_talks";
    let (result, events) = collect(s.as_mut(), sql).await;
    result.unwrap();
    let info = |wanted: &str| {
        events.iter().find_map(|ev| match ev {
            ExecEvent::Message {
                kind: MessageKind::Info,
                text,
                line,
            } if text == wanted => Some(*line),
            _ => None,
        })
    };
    assert_eq!(info("outside"), Some(Some(7)), "{events:?}");
    assert_eq!(info("inside"), Some(None), "{events:?}");
    assert_eq!(info("low"), Some(None), "{events:?}");
}

#[tokio::test]
async fn dynamic_sql_errors_past_the_batch_have_no_editor_line() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let sql = "DECLARE @s nvarchar(max) = N'SELECT 1' + CHAR(10) + CHAR(10) + CHAR(10) + N'SELECT 1/0'; EXEC(@s)";
    let (result, events) = collect(s.as_mut(), sql).await;
    assert!(result.is_err());
    assert_eq!(first_error(&events).1, None, "{events:?}");
}

#[tokio::test]
async fn announced_database_uses_the_catalog_spelling() {
    let Some(mut p) = test_params() else { return };
    p.database = Some("TEMPDB".into());
    let mut s = connect(&p).await;

    // As open_session does right after connecting.
    assert_eq!(s.current_database().await.unwrap(), "tempdb");
    assert_eq!(s.announced_database().as_deref(), Some("tempdb"));

    let (result, _) = collect(s.as_mut(), "USE master").await;
    result.unwrap();
    assert_eq!(s.announced_database().as_deref(), Some("master"));
}

#[tokio::test]
async fn use_inside_a_script_is_announced_without_a_query() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let announced = s.announced_database();
    assert_eq!(announced.as_deref(), Some("master"));

    let (result, _) = collect(s.as_mut(), "USE model\nGO\nSELECT 1\nGO\nUSE tempdb").await;
    result.unwrap();
    assert_eq!(s.announced_database().as_deref(), Some("tempdb"));
    assert_eq!(s.current_database().await.unwrap(), "tempdb");
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

/// The `(kind, text)` of the messages in `events`, in order.
fn messages(events: &[ExecEvent]) -> Vec<(MessageKind, String)> {
    events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::Message { kind, text, .. } => Some((*kind, text.clone())),
            _ => None,
        })
        .collect()
}

fn error_count(events: &[ExecEvent]) -> usize {
    messages(events)
        .iter()
        .filter(|(k, _)| *k == MessageKind::Error)
        .count()
}

#[tokio::test]
async fn every_error_arrives_in_order() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;

    let (result, events) = collect(
        s.as_mut(),
        "PRINT 'a'; RAISERROR('e1',16,1); PRINT 'b'; SELECT 1; RAISERROR('e2',16,1); SELECT 2",
    )
    .await;
    // The batch failed with its first error.
    assert!(
        matches!(&result, Err(DriverError::Query { message, .. }) if message == "e1"),
        "{result:?}"
    );
    let all: Vec<String> = events
        .iter()
        .filter_map(|ev| match ev {
            ExecEvent::Message { kind, text, .. } => Some(format!("{kind:?} {text}")),
            ExecEvent::ResultStart { index, .. } => Some(format!("result {index}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        all,
        ["Info a", "Error e1", "Info b", "result 0", "Error e2", "result 1"]
    );

    let (result, events) = collect(s.as_mut(), "SELECT 1/0; SELECT 2").await;
    assert!(
        matches!(result, Err(DriverError::Query { .. })),
        "{result:?}"
    );
    assert_eq!(rows(&events), vec![vec![Value::Int(2)]]);
    assert_eq!(error_count(&events), 1);
    // The session is still usable.
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

#[tokio::test]
async fn transaction_count_and_end() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;
    assert_eq!(s.transaction_count().await.unwrap(), 0);
    collect(s.as_mut(), "BEGIN TRAN").await.0.unwrap();
    assert_eq!(s.transaction_count().await.unwrap(), 1);
    collect(s.as_mut(), "BEGIN TRAN").await.0.unwrap();
    assert_eq!(s.transaction_count().await.unwrap(), 2);
    s.end_transactions(false).await.unwrap();
    assert_eq!(s.transaction_count().await.unwrap(), 0);
    // Nothing open: ending is not an error.
    s.end_transactions(false).await.unwrap();
    s.end_transactions(true).await.unwrap();

    // Commit keeps the work, rollback discards it.
    let table = format!("##tulka_tx_{}", std::process::id());
    collect(s.as_mut(), &format!("CREATE TABLE {table} (a int)"))
        .await
        .0
        .unwrap();
    collect(
        s.as_mut(),
        &format!("BEGIN TRAN; INSERT {table} VALUES (1)"),
    )
    .await
    .0
    .unwrap();
    s.end_transactions(true).await.unwrap();
    assert_eq!(s.transaction_count().await.unwrap(), 0);
    collect(
        s.as_mut(),
        &format!("BEGIN TRAN; INSERT {table} VALUES (2)"),
    )
    .await
    .0
    .unwrap();
    s.end_transactions(false).await.unwrap();
    let (r, events) = collect(s.as_mut(), &format!("SELECT a FROM {table}")).await;
    r.unwrap();
    assert_eq!(rows(&events), vec![vec![Value::Int(1)]]);
    collect(s.as_mut(), &format!("DROP TABLE {table}"))
        .await
        .0
        .unwrap();
}

#[tokio::test]
async fn end_transactions_on_a_lost_connection_is_disconnected() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;
    let mut killer = connect(&p).await;
    let (_, events) = collect(s.as_mut(), "SELECT @@SPID").await;
    let spid = match rows(&events).as_slice() {
        [row] => match row.as_slice() {
            [Value::Int(spid)] => *spid,
            other => panic!("unexpected row {other:?}"),
        },
        other => panic!("unexpected rows {other:?}"),
    };
    collect(killer.as_mut(), &format!("KILL {spid}"))
        .await
        .0
        .unwrap();
    assert!(matches!(
        s.end_transactions(true).await,
        Err(DriverError::Disconnected(_))
    ));
}

// Read-only execution. Each test uses a temporary database with `dbo.w`.

/// Runs `sql` with `execute_read_only` and returns the result with the events.
async fn collect_read_only(
    session: &mut dyn Session,
    sql: &str,
    max_rows: u32,
) -> (Result<(), DriverError>, Vec<ExecEvent>) {
    params::collect_with(|tx| session.execute_read_only(sql, max_rows, tx)).await
}

/// `SELECT @@TRANCOUNT` on `session`.
async fn trancount(session: &mut dyn Session) -> Value {
    let (result, events) = collect(session, "SELECT @@TRANCOUNT").await;
    result.unwrap();
    rows(&events).remove(0).remove(0)
}

#[tokio::test]
async fn read_only_returns_rows_and_caps_with_rowcount() {
    let Some(admin) = test_params() else { return };
    params::with_temp_db(admin, |p| async move {
        let mut s = connect(&p).await;
        let (result, events) =
            collect_read_only(s.as_mut(), "SELECT * FROM sys.all_objects", 10).await;
        result.unwrap();
        assert_eq!(rows(&events).len(), 11);
    })
    .await;
}

#[tokio::test]
async fn read_only_runs_inside_one_transaction() {
    let Some(admin) = test_params() else { return };
    params::with_temp_db(admin, |p| async move {
        let mut s = connect(&p).await;
        let (result, events) =
            collect_read_only(s.as_mut(), "SELECT @@TRANCOUNT, @@LOCK_TIMEOUT", 10).await;
        result.unwrap();
        assert_eq!(rows(&events), vec![vec![Value::Int(1), Value::Int(5000)]]);
    })
    .await;
}

#[tokio::test]
async fn read_only_refuses_writes_without_touching_the_server() {
    let Some(admin) = test_params() else { return };
    params::with_temp_db(admin, |p| async move {
        let mut s = connect(&p).await;
        let (result, events) = collect_read_only(s.as_mut(), "DELETE FROM dbo.w", 10).await;
        assert!(matches!(result, Err(DriverError::Other(_))), "{result:?}");
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(params::count_w(&p).await, 3);
    })
    .await;
}

#[tokio::test]
async fn query_error_is_reported_and_rolled_back() {
    let Some(admin) = test_params() else { return };
    params::with_temp_db(admin, |p| async move {
        let mut s = connect(&p).await;
        let (result, _) = collect_read_only(s.as_mut(), "SELECT 1/0", 10).await;
        assert!(
            matches!(result, Err(DriverError::Query { .. })),
            "{result:?}"
        );
        // The closing statement ran and left no transaction open.
        assert_eq!(trancount(s.as_mut()).await, Value::Int(0));
    })
    .await;
}

#[tokio::test]
async fn cancel_during_read_only_is_cancelled() {
    let Some(admin) = test_params() else { return };
    params::with_temp_db(admin, |p| async move {
        let mut s = connect(&p).await;
        let cancel = s.cancel_handle();
        // The sort has to read every row before it can emit the first, so the
        // row cap of the transaction does not make it finish early.
        let sql = "SELECT a.object_id FROM sys.all_objects a CROSS JOIN sys.all_objects b \
                   CROSS JOIN sys.all_objects c ORDER BY NEWID()";
        let (result, _) = tokio::join!(collect_read_only(s.as_mut(), sql, 10), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            cancel.cancel().await.unwrap();
        });
        assert!(
            matches!(result.0, Err(DriverError::Cancelled)),
            "{:?}",
            result.0
        );
        // The closing batch ran after the cancel: the session is usable and
        // has no transaction left.
        assert_eq!(trancount(s.as_mut()).await, Value::Int(0));
    })
    .await;
}
