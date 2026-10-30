//! The metadata tools of the AI services, against a fake driver.

mod common;

use common::ai_fixture::*;
use std::future::pending;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tulka_app::ai::{
    describe_table, list_connections, list_databases, run_query, search_objects, QueryRequest,
    SearchRequest, ToolText,
};
use tulka_app::tulka_store::{RunLogOutcome, RunSource};
use tulka_core::SecretStore as _;
use tulka_core::{
    AiAccess, ColumnInfo, DatabaseScope, DatabaseSnapshot, DbObject, Environment, Folder,
    ForeignKey, IndexInfo, ObjectKind, TableStructure,
};

fn not_available(name: &str) -> ToolText {
    ToolText {
        text: format!(
            "No connection named \"{name}\" is available to the AI. Use list_connections."
        ),
        is_error: true,
    }
}

fn search(query: &str) -> SearchRequest {
    SearchRequest {
        query: query.into(),
        ..SearchRequest::default()
    }
}

fn id_of(f: &Fixture, name: &str) -> String {
    f.connections()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap()
        .id
}

fn clientes() -> TableStructure {
    TableStructure {
        columns: vec![
            ColumnInfo {
                name: "id".into(),
                type_name: "int".into(),
                nullable: false,
                primary_key: true,
                default: None,
            },
            ColumnInfo {
                name: "nombre".into(),
                type_name: "varchar(50)".into(),
                nullable: true,
                primary_key: false,
                default: Some("('n/a')".into()),
            },
        ],
        foreign_keys: vec![ForeignKey {
            name: "fk_cli_zona".into(),
            columns: vec!["zona_id".into()],
            ref_schema: "dbo".into(),
            ref_table: "zonas".into(),
            ref_columns: vec!["id".into()],
        }],
        indexes: vec![IndexInfo {
            name: "pk_clientes".into(),
            columns: vec!["id".into()],
            unique: true,
            primary_key: true,
            clustered: true,
        }],
    }
}

#[tokio::test]
async fn lists_only_read_only_connections() {
    let f = Fixture::standard();
    let out = list_connections(&f.state).await;
    assert_eq!(
        out,
        ToolText {
            text: "connection|environment|databases\nLocal|local|all (not indexed)\n".into(),
            is_error: false,
        }
    );
    assert!(!out.text.contains("Prod"));
    assert!(!out.text.contains("db.example.com"));
}

#[tokio::test]
async fn lists_selected_databases_and_cached_counts() {
    let mut a = connection("A", Environment::Test, AiAccess::ReadOnly);
    a.databases = selected(&["x", "y"]);
    let b = connection("B", Environment::Staging, AiAccess::ReadOnly);
    let f = Fixture::with(vec![a, b.clone()], Vec::new());
    f.seed(&b.id, "sales", &[("dbo", "t")]);
    assert_eq!(
        list_connections(&f.state).await.text,
        "connection|environment|databases\nA|test|selected: x, y\nB|staging|all (1 cached)\n"
    );
}

#[tokio::test]
async fn no_access_and_missing_give_the_same_error() {
    let f = Fixture::standard();
    for name in ["Prod", "Ghost"] {
        let expected = not_available(name);
        assert_eq!(list_databases(&f.state, name.into()).await, expected);
        let mut req = search("x");
        req.connection = Some(name.into());
        assert_eq!(search_objects(&f.state, req).await, expected);
        assert_eq!(
            describe_table(&f.state, name.into(), None, "dbo.t".into()).await,
            expected
        );
        assert_eq!(
            run_query(&f.state, query(name, "SELECT 1"), pending()).await,
            expected
        );
    }
    assert!(f.runlog.list(None, 10).is_empty());
    assert!(f.driver.script.connects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn duplicate_names_get_folder_paths() {
    let folders = vec![
        Folder {
            id: "fx".into(),
            name: "Proyecto X".into(),
            parent_id: None,
        },
        Folder {
            id: "fy".into(),
            name: "Sub".into(),
            parent_id: Some("fx".into()),
        },
    ];
    let mut one = connection("Sales", Environment::Test, AiAccess::ReadOnly);
    one.folder_id = Some("fx".into());
    let mut two = connection("sales", Environment::Test, AiAccess::ReadOnly);
    two.folder_id = Some("fy".into());
    let mut three = connection("Twin", Environment::Test, AiAccess::ReadOnly);
    three.id = "aaaaaaaa-1".into();
    let mut four = connection("Twin", Environment::Test, AiAccess::ReadOnly);
    four.id = "bbbbbbbb-2".into();
    let f = Fixture::with(vec![one, two, three, four], folders);
    let text = list_connections(&f.state).await.text;
    let labels: Vec<&str> = text
        .lines()
        .skip(1)
        .map(|l| l.split('|').next().unwrap())
        .collect();
    assert_eq!(
        labels,
        [
            "Proyecto X/Sales",
            "Proyecto X/Sub/sales",
            "Twin #aaaaaaaa",
            "Twin #bbbbbbbb"
        ]
    );
    let out = list_databases(&f.state, "proyecto x/sub/SALES".into()).await;
    assert!(!out.is_error, "{}", out.text);
}

#[tokio::test]
async fn labels_resolve_case_insensitively() {
    let mut local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    local.databases = selected(&["a", "b"]);
    let f = Fixture::with(vec![local], Vec::new());
    let out = list_databases(&f.state, " lOCAL ".into()).await;
    assert_eq!(
        out,
        ToolText {
            text: "Local › databases (selected)\ndatabase\na\nb\n".into(),
            is_error: false,
        }
    );
}

#[tokio::test]
async fn config_change_applies_on_the_next_call() {
    let f = Fixture::standard();
    assert!(!list_databases(&f.state, "Local".into()).await.is_error);
    let mut conns = f.connections();
    conns[1].ai_access = AiAccess::None;
    f.write_config(conns, Vec::new());
    assert_eq!(
        list_databases(&f.state, "Local".into()).await,
        not_available("Local")
    );
    assert!(!list_connections(&f.state).await.text.contains("Local"));
}

#[tokio::test]
async fn unreadable_config_is_reported() {
    let f = Fixture::standard();
    std::fs::write(f.config_path(), "{ not json").unwrap();
    let out = list_connections(&f.state).await;
    assert!(out.is_error);
    assert!(
        out.text
            .starts_with("tulkabd could not read its configuration: "),
        "{}",
        out.text
    );
}

#[tokio::test]
async fn all_scope_lists_the_server_databases_and_marks_no_access() {
    let f = Fixture::standard();
    let id = id_of(&f, "Local");
    f.cache
        .set_databases(&id, &["master".into(), "sales".into()]);
    f.cache.mark_no_access(&id, "master");
    let out = list_databases(&f.state, "Local".into()).await;
    assert_eq!(
        out.text,
        "Local › databases (all)\ndatabase|access\nmaster|no access\nsales|\n"
    );
    assert_eq!(
        f.driver
            .script
            .live
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn unreachable_server_falls_back_to_the_cache() {
    let f = Fixture::standard();
    let id = id_of(&f, "Local");
    *f.driver.script.databases.lock().unwrap() = None;
    let out = list_databases(&f.state, "Local".into()).await;
    assert_eq!(
        out,
        ToolText {
            text: "Could not connect to \"Local\": the server could not be reached.".into(),
            is_error: true,
        }
    );
    f.cache.set_databases(&id, &["sales".into()]);
    let out = list_databases(&f.state, "Local".into()).await;
    assert!(!out.is_error);
    assert!(out.text.contains("could not be reached"), "{}", out.text);
    assert!(out.text.ends_with("sales|\n"), "{}", out.text);
}

#[tokio::test]
async fn search_never_returns_objects_of_hidden_connections() {
    let f = Fixture::standard();
    f.seed(&id_of(&f, "Prod"), "erp", &[("dbo", "secret_clientes")]);
    f.seed(&id_of(&f, "Local"), "erp", &[("dbo", "clientes")]);
    let out = search_objects(&f.state, search("clientes")).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object\nLocal|erp|table|dbo.clientes\n1 of 1 matches"
    );
    let mut req = search("secret");
    req.connection = Some("Prod".into());
    assert_eq!(search_objects(&f.state, req).await, not_available("Prod"));
    let all = search_objects(&f.state, search("secret")).await;
    assert!(all
        .text
        .starts_with("connection|database|kind|object\n0 of 0 matches"));
    assert!(!all.text.contains("Prod"));
}

#[tokio::test]
async fn search_reports_connections_without_cache() {
    let a = connection("A", Environment::Test, AiAccess::ReadOnly);
    let b = connection("B", Environment::Test, AiAccess::ReadOnly);
    let f = Fixture::with(vec![a.clone(), b], Vec::new());
    f.seed(&a.id, "erp", &[("dbo", "clientes")]);
    let out = search_objects(&f.state, search("cli")).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object\nA|erp|table|dbo.clientes\n\
         1 of 1 matches\nNot indexed: B (open it once in tulkabd to index it)."
    );
}

#[tokio::test]
async fn search_in_columns_and_limit() {
    let a = connection("A", Environment::Test, AiAccess::ReadOnly);
    let f = Fixture::with(vec![a.clone()], Vec::new());
    let objects = (0..5)
        .map(|i| DbObject {
            schema: "dbo".into(),
            name: format!("t{i}"),
            kind: ObjectKind::Table,
        })
        .collect::<Vec<_>>();
    let columns = (0..5)
        .map(|i| {
            (
                i,
                vec![ColumnInfo {
                    name: "email".into(),
                    type_name: "varchar".into(),
                    nullable: true,
                    primary_key: false,
                    default: None,
                }],
            )
        })
        .collect();
    f.cache
        .replace_database(&a.id, "erp", &DatabaseSnapshot { objects, columns }, 1)
        .unwrap();
    let mut req = search("EMAIL");
    req.in_columns = true;
    req.limit = Some(2);
    let out = search_objects(&f.state, req).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object|column\nA|erp|table|dbo.t0|email\n\
         A|erp|table|dbo.t1|email\n2 of 5 matches"
    );
    assert!(search_objects(&f.state, search("  ")).await.is_error);
}

#[tokio::test]
async fn search_live_fallback_reads_one_database() {
    let f = Fixture::standard();
    f.driver.script.snapshots.lock().unwrap().insert(
        "sales".into(),
        DatabaseSnapshot {
            objects: vec![
                DbObject {
                    schema: "dbo".into(),
                    name: "clientes".into(),
                    kind: ObjectKind::Table,
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "pedidos".into(),
                    kind: ObjectKind::Table,
                },
            ],
            columns: Vec::new(),
        },
    );
    let mut req = search("cli");
    req.connection = Some("Local".into());
    req.database = Some("sales".into());
    let out = search_objects(&f.state, req).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object\nLocal|sales|table|dbo.clientes\n1 of 1 matches"
    );
    assert_eq!(*f.driver.script.snapshots_asked.lock().unwrap(), ["sales"]);
    let params = f.driver.script.connects.lock().unwrap().clone();
    assert_eq!(params[0].database.as_deref(), Some("sales"));
}

#[tokio::test]
async fn describe_prints_columns_keys_and_indexes() {
    let f = Fixture::standard();
    f.driver.script.structures.lock().unwrap().insert(
        ("master".into(), "dbo".into(), "clientes".into()),
        clientes(),
    );
    let out = describe_table(&f.state, "Local".into(), None, "[dbo].[clientes]".into()).await;
    assert_eq!(
        out.text,
        "Local › master › dbo.clientes\n\
         Columns:\ncolumn|type|nullable|key|default\nid|int|no|PK|\nnombre|varchar(50)|yes||('n/a')\n\
         Foreign keys:\nname|columns|references\nfk_cli_zona|zona_id|dbo.zonas (id)\n\
         Indexes:\nname|columns|unique|clustered|primary\npk_clientes|id|yes|yes|yes\n"
    );
    assert!(!out.is_error);
}

#[tokio::test]
async fn describe_resolves_the_schema_from_the_cache_or_dbo() {
    let f = Fixture::standard();
    let id = id_of(&f, "Local");
    f.seed(&id, "master", &[("sales", "clientes")]);
    {
        let mut structures = f.driver.script.structures.lock().unwrap();
        structures.insert(
            ("master".into(), "sales".into(), "clientes".into()),
            clientes(),
        );
        structures.insert(
            ("master".into(), "dbo".into(), "pedidos".into()),
            clientes(),
        );
    }
    assert!(
        !describe_table(&f.state, "Local".into(), None, "CLIENTES".into())
            .await
            .is_error
    );
    assert!(
        !describe_table(&f.state, "Local".into(), None, "pedidos".into())
            .await
            .is_error
    );
    assert_eq!(
        *f.driver.script.structures_asked.lock().unwrap(),
        [
            (
                "master".to_string(),
                "sales".to_string(),
                "clientes".to_string()
            ),
            (
                "master".to_string(),
                "dbo".to_string(),
                "pedidos".to_string()
            ),
        ]
    );
}

#[tokio::test]
async fn describe_ambiguous_name_lists_the_schemas() {
    let f = Fixture::standard();
    f.seed(
        &id_of(&f, "Local"),
        "master",
        &[("dbo", "items"), ("archive", "items")],
    );
    let out = describe_table(&f.state, "Local".into(), None, "items".into()).await;
    assert_eq!(
        out,
        ToolText {
            text: "\"items\" exists in several schemas of master: archive.items, dbo.items. \
                   Use schema.name."
                .into(),
            is_error: true,
        }
    );
    assert!(f.driver.script.structures_asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn describe_unknown_table_is_an_error() {
    let f = Fixture::standard();
    let out = describe_table(&f.state, "Local".into(), None, "nada".into()).await;
    assert_eq!(
        out,
        ToolText {
            text:
                "No table or view named dbo.nada in master on Local. Use search_objects to find it."
                    .into(),
            is_error: true,
        }
    );
    assert!(
        describe_table(&f.state, "Local".into(), None, "a.b.c".into())
            .await
            .is_error
    );
}

#[tokio::test]
async fn selected_scope_rejects_other_databases_and_lists_the_allowed() {
    let mut local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    local.databases = DatabaseScope::Selected {
        names: vec!["erp".into(), "crm".into()],
    };
    let f = Fixture::with(vec![local], Vec::new());
    let out = describe_table(&f.state, "Local".into(), Some("master".into()), "t".into()).await;
    assert_eq!(
        out,
        ToolText {
            text: "Database \"master\" is not available on \"Local\". Available: erp, crm.".into(),
            is_error: true,
        }
    );
    let mut req = search("t");
    req.connection = Some("Local".into());
    req.database = Some("master".into());
    assert_eq!(search_objects(&f.state, req).await, out);
    assert!(f.driver.script.connects.lock().unwrap().is_empty());
    // Without a database, the first selected one is used.
    describe_table(&f.state, "Local".into(), None, "t".into()).await;
    let params = f.driver.script.connects.lock().unwrap().clone();
    assert_eq!(params[0].database.as_deref(), Some("erp"));
}

#[tokio::test]
async fn missing_password_is_explained_without_asking_for_it() {
    let mut local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    local.password_saved = false;
    let f = Fixture::with(vec![local], Vec::new());
    let expected = ToolText {
        text: "The password of \"Local\" is not saved in tulkabd, so the AI cannot connect. \
               Open tulkabd, edit the connection and save its password."
            .into(),
        is_error: true,
    };
    assert_eq!(list_databases(&f.state, "Local".into()).await, expected);
    assert_eq!(
        describe_table(&f.state, "Local".into(), None, "t".into()).await,
        expected
    );
    assert!(f.driver.script.connects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sessions_use_the_ai_application_name() {
    let f = Fixture::standard();
    list_databases(&f.state, "Local".into()).await;
    describe_table(&f.state, "Local".into(), Some("sales".into()), "t".into()).await;
    let params = f.driver.script.connects.lock().unwrap().clone();
    assert_eq!(params.len(), 2);
    assert!(params.iter().all(|p| p.application_name == "tulkabd-ai"));
    assert_eq!(params[1].database.as_deref(), Some("sales"));
    // Sessions end with the call.
    assert_eq!(
        f.driver
            .script
            .live
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn connect_errors_never_carry_the_host_or_the_login() {
    let mut local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    local.user = "svc_report".into();
    let f = Fixture::with(vec![local.clone()], Vec::new());
    let leaks = |text: &str| text.contains("db.example.com") || text.contains("svc_report");

    *f.driver.script.databases.lock().unwrap() = None;
    let unreachable = list_databases(&f.state, "Local".into()).await;
    let described = describe_table(&f.state, "Local".into(), None, "t".into()).await;
    let mut req = search("t");
    req.database = Some("x".into());
    let live = search_objects(&f.state, req).await;
    f.secrets.set(&local.id, "another password").unwrap();
    let rejected = describe_table(&f.state, "Local".into(), None, "t".into()).await;
    for out in [&unreachable, &described, &live, &rejected] {
        assert!(out.is_error);
        assert!(!leaks(&out.text), "{}", out.text);
        assert!(!out.text.contains("another password"));
    }
    assert_eq!(
        rejected.text,
        "Could not connect to \"Local\": the saved login was rejected; open tulkabd and check the connection."
    );
}

#[tokio::test]
async fn search_ignores_cached_databases_outside_the_selection() {
    let mut a = connection("A", Environment::Test, AiAccess::ReadOnly);
    a.databases = selected(&["erp"]);
    let b = connection("B", Environment::Test, AiAccess::ReadOnly);
    let f = Fixture::with(vec![a.clone(), b.clone()], Vec::new());
    // Indexed while A exposed every database, before it was narrowed.
    f.seed(&a.id, "erp", &[("dbo", "clientes_ok")]);
    f.seed(&a.id, "stale", &[("dbo", "clientes_stale")]);
    f.seed(&b.id, "stale", &[("dbo", "clientes_b")]);

    let out = search_objects(&f.state, search("clientes")).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object\nB|stale|table|dbo.clientes_b\n\
         A|erp|table|dbo.clientes_ok\n2 of 2 matches"
    );
    let mut req = search("clientes");
    req.database = Some("STALE".into());
    let out = search_objects(&f.state, req).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object\nB|stale|table|dbo.clientes_b\n1 of 1 matches"
    );
}

#[tokio::test]
async fn database_names_are_matched_ignoring_case() {
    let f = Fixture::standard();
    f.seed(&id_of(&f, "Local"), "sales", &[("dbo", "clientes")]);
    let mut req = search("cli");
    req.connection = Some("Local".into());
    req.database = Some("SALES".into());
    let out = search_objects(&f.state, req).await;
    assert_eq!(
        out.text,
        "connection|database|kind|object\nLocal|sales|table|dbo.clientes\n1 of 1 matches"
    );
    assert!(f.driver.script.snapshots_asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_selection_with_no_databases_offers_nothing() {
    let mut local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    local.databases = selected(&[]);
    let f = Fixture::with(vec![local], Vec::new());
    let out = describe_table(&f.state, "Local".into(), None, "t".into()).await;
    assert_eq!(
        out,
        ToolText {
            text: "\"Local\" has no databases selected in tulkabd, so there is nothing to use."
                .into(),
            is_error: true,
        }
    );
    assert!(f.driver.script.connects.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_hidden_namesake_does_not_rename_the_visible_connection() {
    let folders = vec![Folder {
        id: "f1".into(),
        name: "Other".into(),
        parent_id: None,
    }];
    let mut hidden = connection("Local", Environment::Production, AiAccess::None);
    hidden.folder_id = Some("f1".into());
    let visible = connection("Local", Environment::Local, AiAccess::ReadOnly);
    let f = Fixture::with(vec![hidden, visible], folders);
    assert_eq!(
        list_connections(&f.state).await.text,
        "connection|environment|databases\nLocal|local|all (not indexed)\n"
    );
}

fn query(connection: &str, sql: &str) -> QueryRequest {
    QueryRequest {
        connection: connection.into(),
        database: None,
        sql: sql.into(),
        max_rows: None,
    }
}

/// The text with the elapsed time of every header-like line replaced by `N ms`.
fn masked(text: &str) -> String {
    text.lines()
        .map(|line| match line.rsplit_once(" · ") {
            Some((rest, tail))
                if tail
                    .strip_suffix(" ms")
                    .is_some_and(|n| n.parse::<u64>().is_ok()) =>
            {
                format!("{rest} · N ms")
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

async fn run(f: &Fixture, sql: &str) -> ToolText {
    run_query(&f.state, query("Local", sql), pending()).await
}

#[tokio::test]
async fn select_is_formatted_like_copy_for_ai() {
    let f = Fixture::standard();
    let out = run(&f, "SELECT format").await;
    assert!(!out.is_error);
    assert_eq!(
        masked(&out.text),
        "Local · LOCAL › master · 2 rows · N ms\nid|name|note\n1|Ana|\n2|\"\"|\"a|b\"\n"
    );
}

#[tokio::test]
async fn two_result_sets_and_messages_are_formatted() {
    let f = Fixture::standard();
    let out = run(&f, "SELECT sets").await;
    assert!(!out.is_error);
    assert_eq!(
        masked(&out.text),
        "Local · LOCAL › master · 2 result sets · N ms\n#1 · 1 row\na\n1\n#2 · 2 rows\nb\n2\n3\n\
         Messages:\nInfo: hello\nWarning (line 3): careful\n"
    );
    let none = run(&f, "SELECT none").await;
    assert_eq!(
        masked(&none.text),
        "Local · LOCAL › master · no rows returned · N ms\n"
    );
}

#[tokio::test]
async fn rows_are_capped_at_the_preference_and_marked() {
    let f = Fixture::standard();
    let mut req = query("Local", "SELECT big");
    req.max_rows = Some(10_000);
    let out = run_query(&f.state, req, pending()).await;
    assert!(!out.is_error);
    // Header, 500 rows and the mark.
    assert_eq!(out.text.lines().count(), 502);
    assert!(masked(&out.text).starts_with(
        "Local · LOCAL › master · 500 rows (limit reached: more rows exist; narrow the query or use TOP) · N ms\nn\n0\n"
    ));
    assert!(out.text.contains("\n499\n") && !out.text.contains("\n500\n"));
}

#[tokio::test]
async fn smaller_max_rows_is_honoured() {
    let f = Fixture::standard();
    let mut req = query("Local", "SELECT big");
    req.max_rows = Some(10);
    let out = run_query(&f.state, req, pending()).await;
    assert_eq!(out.text.lines().count(), 12);
    assert!(out.text.contains("· 10 rows (limit reached"));
    let mut req = query("Local", "SELECT big");
    req.max_rows = Some(0);
    assert_eq!(
        run_query(&f.state, req, pending())
            .await
            .text
            .lines()
            .count(),
        3
    );
}

#[tokio::test]
async fn long_cells_are_cut() {
    let f = Fixture::standard();
    let out = run(&f, "SELECT long").await;
    assert_eq!(
        masked(&out.text),
        format!(
            "Local · LOCAL › master · 1 row · N ms\nt\n{}…\n",
            "é".repeat(2000)
        )
    );
}

#[tokio::test]
async fn output_is_cut_at_the_character_budget_and_the_query_cancelled() {
    let f = Fixture::standard();
    let out = run(&f, "SELECT wide").await;
    assert!(!out.is_error);
    assert!(out
        .text
        .ends_with("Output cut at 100,000 characters; narrow the query.\n"));
    assert!(out.text.chars().count() <= 100_000);
    assert_eq!(f.driver.script.live.load(Ordering::SeqCst), 0);
    // The driver was told to stop long before it sent everything.
    assert_eq!(f.driver.script.cancels.load(Ordering::SeqCst), 1);
    assert!(out.text.lines().count() < 100);
    // The header counts the rows that are in the text.
    let shown = out.text.lines().count() - 3;
    assert!(
        masked(&out.text).starts_with(&format!(
            "Local · LOCAL › master · {shown} rows · N ms\nt\n"
        )),
        "{}",
        out.text.lines().next().unwrap()
    );
    assert_eq!(f.runlog.list(None, 1)[0].rows, Some(shown as u64));
}

#[tokio::test]
async fn rejected_sql_never_reaches_the_driver_and_is_logged_blocked() {
    let f = Fixture::standard();
    let cases = [
        ("DELETE FROM t", "Not run: only SELECT queries (and WITH … SELECT) are allowed; line 1 starts with DELETE."),
        ("SELECT 1\nSELECT * INTO x FROM t", "Not run: INTO is not allowed in a read-only query (line 2). If it is a column or table name, write it in [brackets]."),
        ("SELECT * FROM a.b.c.d", "Not run: four-part names (linked servers) are not allowed (line 1)."),
        ("", "Not run: the query is empty."),
    ];
    for (sql, expected) in cases {
        assert_eq!(
            run(&f, sql).await,
            ToolText {
                text: expected.into(),
                is_error: true
            },
            "{sql}"
        );
    }
    let unparsed = run(&f, "SELECT 'oops").await;
    assert!(unparsed.is_error);
    assert!(unparsed
        .text
        .starts_with("Not run: the query has an unclosed string, identifier or comment."));
    assert!(unparsed.text.contains("[año]"));
    assert!(f.driver.script.sqls.lock().unwrap().is_empty());
    assert!(f.driver.script.connects.lock().unwrap().is_empty());
    let log = f.runlog.list(None, 10);
    assert_eq!(log.len(), 5);
    assert!(log.iter().all(|e| e.outcome == RunLogOutcome::Blocked
        && e.rows.is_none()
        && e.elapsed_ms.is_none()
        && e.source == RunSource::Ai));
}

#[tokio::test]
async fn query_error_is_an_error_with_its_line() {
    let f = Fixture::standard();
    assert_eq!(
        run(&f, "SELECT err").await,
        ToolText {
            text: "Error (line 2): Invalid column name 'nope'.".into(),
            is_error: true,
        }
    );
    assert_eq!(f.runlog.list(None, 1)[0].outcome, RunLogOutcome::Error);
}

#[tokio::test]
async fn timeout_cancels_and_logs_cancelled() {
    let f = Fixture::standard();
    f.set_preferences(|p| p.ai_query_timeout_s = 1);
    let started = std::time::Instant::now();
    let out = run(&f, "SELECT slow").await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        out,
        ToolText {
            text: "Timeout (1 s) reached; the query was cancelled.".into(),
            is_error: true,
        }
    );
    assert_eq!(f.runlog.list(None, 1)[0].rows, Some(0));
    let entry = &f.runlog.list(None, 1)[0];
    assert_eq!(entry.outcome, RunLogOutcome::Cancelled);
    assert_eq!(f.driver.script.live.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn client_cancel_cancels_and_logs_cancelled() {
    let f = Fixture::standard();
    let out = run_query(
        &f.state,
        query("Local", "SELECT slow"),
        tokio::time::sleep(Duration::from_millis(100)),
    )
    .await;
    assert_eq!(
        out,
        ToolText {
            text: "The query was cancelled.".into(),
            is_error: true,
        }
    );
    assert_eq!(f.runlog.list(None, 1)[0].outcome, RunLogOutcome::Cancelled);
    assert_eq!(f.driver.script.live.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn every_run_is_logged_as_ai_with_the_client_name() {
    let mut nopass = connection("NoPass", Environment::Test, AiAccess::ReadOnly);
    nopass.password_saved = false;
    let local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    let f = Fixture::with(vec![local, nopass], Vec::new());
    f.state.set_client(Some("claude-code".into()));
    run(&f, "SELECT 1").await;
    run(&f, "SELECT big").await;
    run(&f, "DROP TABLE t").await;
    run_query(&f.state, query("NoPass", "SELECT 1"), pending()).await;
    let log = f.runlog.list(None, 10);
    assert_eq!(log.len(), 4);
    assert!(log
        .iter()
        .all(|e| e.source == RunSource::Ai && e.client.as_deref() == Some("claude-code")));
    // Newest first.
    let missing = &log[0];
    assert_eq!(missing.connection_name, "NoPass");
    assert_eq!(missing.outcome, RunLogOutcome::Error);
    assert_eq!((missing.rows, missing.elapsed_ms), (None, None));
    assert_eq!(log[1].outcome, RunLogOutcome::Blocked);
    assert_eq!(log[1].sql, "DROP TABLE t");
    assert_eq!(log[2].outcome, RunLogOutcome::Ok);
    assert_eq!(log[2].rows, Some(500));
    assert_eq!(log[3].outcome, RunLogOutcome::Ok);
    assert_eq!(log[3].rows, Some(1));
    assert!(log[3].elapsed_ms.is_some());
    assert_eq!(log[3].database, "master");
}

#[tokio::test]
async fn the_password_is_nowhere() {
    let f = Fixture::standard();
    let id = id_of(&f, "Local");
    let mut outputs = vec![
        list_connections(&f.state).await,
        list_databases(&f.state, "Local".into()).await,
        describe_table(&f.state, "Local".into(), None, "t".into()).await,
        search_objects(&f.state, search("t")).await,
        run(&f, "SELECT format").await,
        run(&f, "SELECT err").await,
        run(&f, "DROP TABLE t").await,
    ];
    f.secrets.set(&id, "another password").unwrap();
    outputs.push(run(&f, "SELECT 1").await);
    assert!(outputs.last().unwrap().is_error);
    let logged = format!("{:?}", f.runlog.list(None, 50));
    for text in outputs
        .iter()
        .map(|o| o.text.as_str())
        .chain([logged.as_str()])
    {
        assert!(
            !text.contains(PASSWORD) && !text.contains("another password"),
            "{text}"
        );
    }
}

#[tokio::test]
async fn at_most_four_calls_touch_servers_at_once() {
    let f = Fixture::standard();
    f.set_preferences(|p| p.ai_query_timeout_s = 1);
    let one = || run(&f, "SELECT slow");
    let outs = tokio::join!(one(), one(), one(), one(), one(), one());
    assert!([&outs.0, &outs.1, &outs.2, &outs.3, &outs.4, &outs.5]
        .iter()
        .all(|o| o.is_error));
    assert_eq!(f.driver.script.peak.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn describe_errors_after_connecting_name_the_connection() {
    let f = Fixture::standard();
    let out = describe_table(&f.state, "Local".into(), None, "lost".into()).await;
    assert_eq!(
        out,
        ToolText {
            text: "Local: the connection was lost.".into(),
            is_error: true,
        }
    );
}

#[tokio::test]
async fn many_narrow_rows_are_never_silently_dropped() {
    let f = Fixture::standard();
    f.set_preferences(|p| p.ai_max_rows = 10_000);
    let mut req = query("Local", "SELECT many");
    req.max_rows = Some(10_000);
    let out = run_query(&f.state, req, pending()).await;
    let shown = out.text.lines().count() - 2;
    let cut = out.text.ends_with("narrow the query.\n");
    assert!(cut || shown == 10_000, "{shown} rows without a notice");
    assert!(out.text.chars().count() <= 100_000);
}

#[tokio::test]
async fn a_timeout_keeps_the_rows_received_before_it() {
    let f = Fixture::standard();
    f.set_preferences(|p| p.ai_query_timeout_s = 1);
    let out = run(&f, "SELECT partial then slow").await;
    assert!(out.is_error);
    assert_eq!(
        masked(&out.text),
        "Local · LOCAL › master · 1 row · N ms\nn\n7\nTimeout (1 s) reached; the query was cancelled.\n"
    );
    let entry = &f.runlog.list(None, 1)[0];
    assert_eq!(
        (entry.outcome, entry.rows),
        (RunLogOutcome::Cancelled, Some(1))
    );
}

#[tokio::test]
async fn cancelling_while_waiting_for_a_slot_is_logged_cancelled() {
    let f = Fixture::standard();
    let one = || {
        run_query(
            &f.state,
            query("Local", "SELECT slow"),
            tokio::time::sleep(Duration::from_millis(300)),
        )
    };
    let outs = tokio::join!(one(), one(), one(), one(), one());
    assert!([&outs.0, &outs.1, &outs.2, &outs.3, &outs.4]
        .iter()
        .all(|o| o.is_error && o.text == "The query was cancelled."));
    let log = f.runlog.list(None, 10);
    assert_eq!(log.len(), 5);
    assert!(log.iter().all(|e| e.outcome == RunLogOutcome::Cancelled));
    // Four started; the fifth never got a slot.
    assert_eq!(log.iter().filter(|e| e.elapsed_ms.is_none()).count(), 1);
    assert_eq!(f.driver.script.connects.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn an_unknown_driver_is_logged_as_an_error() {
    let mut local = connection("Local", Environment::Local, AiAccess::ReadOnly);
    local.driver = "nope".into();
    let f = Fixture::with(vec![local], Vec::new());
    let out = run(&f, "SELECT 1").await;
    assert!(out.is_error);
    let log = f.runlog.list(None, 5);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].outcome, RunLogOutcome::Error);
}

#[tokio::test]
async fn timeout_and_error_after_near_budget_output_stay_in_budget() {
    let f = Fixture::standard();
    f.set_preferences(|p| p.ai_query_timeout_s = 1);
    let out = run(&f, "SELECT wide then slow").await;
    assert!(out.is_error);
    assert!(
        out.text.chars().count() <= 100_000,
        "{}",
        out.text.chars().count()
    );
    assert!(out
        .text
        .ends_with("Timeout (1 s) reached; the query was cancelled.\n"));
    let entry = &f.runlog.list(None, 1)[0];
    assert_eq!(entry.outcome, RunLogOutcome::Cancelled);
    let shown = out.text.lines().filter(|l| l.starts_with("xxx")).count() as u64;
    assert_eq!(entry.rows, Some(shown));

    let out = run(&f, "SELECT wide then err").await;
    assert!(out.is_error);
    assert!(out.text.chars().count() <= 100_000);
    // The server's text is cut to 2000 characters.
    let last = out.text.lines().last().unwrap();
    assert!(last.starts_with("Error (line 2): eee") && last.ends_with('…'));
    assert_eq!(last.chars().count(), 2001);
    // It did not fit beside every row: some were left out, and it says so.
    assert!(out.text.contains("Output cut at 100,000 characters"));
    let shown = out.text.lines().filter(|l| l.starts_with("xxx")).count();
    assert!(shown < 52);
    assert!(out.text.contains(&format!("· {shown} rows · ")));
    assert_eq!(f.runlog.list(None, 1)[0].rows, Some(shown as u64));
}
