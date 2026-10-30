//! The metadata tools of the AI services, against a fake driver.

mod common;

use common::ai_fixture::*;
use tulka_app::ai::{
    describe_table, list_connections, list_databases, search_objects, SearchRequest, ToolText,
};
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
    }
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
