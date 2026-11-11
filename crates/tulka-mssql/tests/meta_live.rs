//! Live tests of the metadata queries (objects, columns, structure and
//! definitions). Like `integration.rs`, they are skipped unless
//! `TULKA_MSSQL_TEST` is set.

use tokio::sync::{mpsc, OnceCell};
use tulka_core::{ConnectParams, Driver, DriverError, ForeignKey, IndexInfo, ObjectKind, Session};
use tulka_mssql::MssqlDriver;

#[path = "support/params.rs"]
#[allow(dead_code)]
mod params;
use params::test_params;

async fn connect(p: &ConnectParams) -> Box<dyn Session> {
    MssqlDriver.connect(p).await.expect("connect")
}

/// Runs a script, panicking on any error.
async fn run(s: &mut dyn Session, sql: &str) {
    let (tx, mut rx) = mpsc::channel(16);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    s.execute(sql, tx)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
    drain.await.unwrap();
}

const SETUP: &str = "
IF DB_ID('tulka_meta') IS NOT NULL
BEGIN
  ALTER DATABASE tulka_meta SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
  DROP DATABASE tulka_meta;
END
GO
IF DB_ID('tulka_meta_other') IS NOT NULL
BEGIN
  ALTER DATABASE tulka_meta_other SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
  DROP DATABASE tulka_meta_other;
END
GO
CREATE DATABASE tulka_meta_other
GO
CREATE TABLE tulka_meta_other.dbo.Remota(Id int NOT NULL)
GO
CREATE DATABASE tulka_meta
GO
USE tulka_meta
GO
CREATE TABLE dbo.Pedidos(
  Id int NOT NULL CONSTRAINT PK_Pedidos PRIMARY KEY,
  Cliente nvarchar(50) NOT NULL,
  Total decimal(18,2) NULL CONSTRAINT DF_Pedidos_Total DEFAULT 0,
  Notas nvarchar(max) NULL,
  Creado datetime2(7) NULL)
GO
CREATE UNIQUE INDEX IX_Pedidos_Cliente ON dbo.Pedidos(Cliente)
GO
CREATE TABLE dbo.[Order Details](
  PedidoId int NOT NULL CONSTRAINT FK_Details_Pedidos REFERENCES dbo.Pedidos(Id),
  Linea int NOT NULL,
  CONSTRAINT PK_Details PRIMARY KEY (PedidoId, Linea))
GO
CREATE SCHEMA ventas
GO
CREATE TABLE ventas.[Pedidos_Año](Id int NOT NULL)
GO
CREATE VIEW dbo.vPedidos AS SELECT Id, Cliente FROM dbo.Pedidos
GO
CREATE PROCEDURE dbo.pTotal AS SELECT SUM(Total) FROM dbo.Pedidos
GO
CREATE FUNCTION dbo.fnDoble(@n int) RETURNS int AS BEGIN RETURN @n * 2 END
GO
CREATE FUNCTION dbo.fnDeCliente(@c nvarchar(50)) RETURNS TABLE
AS RETURN SELECT Id, Total FROM dbo.Pedidos WHERE Cliente = @c
GO
CREATE FUNCTION dbo.fnLista() RETURNS @t TABLE(Id int, Cliente nvarchar(50), Total decimal(18,2))
AS BEGIN INSERT @t SELECT Id, Cliente, Total FROM dbo.Pedidos; RETURN END
GO
CREATE PROCEDURE dbo.pSecreto WITH ENCRYPTION AS SELECT 1
GO
CREATE SYNONYM dbo.synPedidos FOR dbo.Pedidos
GO
CREATE SYNONYM dbo.synVista FOR dbo.vPedidos
GO
CREATE SYNONYM dbo.synProc FOR dbo.pTotal
GO
CREATE SYNONYM dbo.synEscalar FOR dbo.fnDoble
GO
CREATE SYNONYM dbo.synTabla FOR dbo.fnDeCliente
GO
CREATE SYNONYM dbo.synFalta FOR dbo.NoExiste
GO
CREATE SYNONYM dbo.synRemota FOR tulka_meta_other.dbo.Remota
GO
CREATE SYNONYM dbo.synRemotaFalta FOR tulka_meta_other.dbo.NoExiste
GO
CREATE SYNONYM dbo.synBaseFalta FOR tulka_no_existe.dbo.Nada
GO
CREATE SYNONYM dbo.synEnlazado FOR [un_servidor].tulka_meta.dbo.Pedidos
GO
CREATE SYNONYM dbo.synSinEsquema FOR Pedidos
GO
CREATE SYNONYM [dbo].[syn.Con Punto] FOR [dbo].[Order Details]
GO
CREATE SYNONYM dbo.synDeSyn FOR dbo.synPedidos
";

/// Creates the `tulka_meta` database once per test process.
async fn setup(p: &ConnectParams) {
    static DONE: OnceCell<()> = OnceCell::const_new();
    DONE.get_or_init(|| async {
        let mut s = connect(p).await;
        run(s.as_mut(), SETUP).await;
    })
    .await;
}

#[tokio::test]
async fn snapshot_lists_objects_and_columns() {
    let Some(p) = test_params() else { return };
    setup(&p).await;
    let mut s = connect(&p).await;

    let snap = s.snapshot("tulka_meta").await.unwrap();
    let mut objects: Vec<(String, String, ObjectKind)> = snap
        .objects
        .iter()
        .map(|o| (o.schema.clone(), o.name.clone(), o.kind))
        .collect();
    objects.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    assert_eq!(
        objects,
        vec![
            ("dbo".into(), "Order Details".into(), ObjectKind::Table),
            ("dbo".into(), "Pedidos".into(), ObjectKind::Table),
            (
                "dbo".into(),
                "fnDeCliente".into(),
                ObjectKind::TableFunction
            ),
            ("dbo".into(), "fnDoble".into(), ObjectKind::Function),
            ("dbo".into(), "fnLista".into(), ObjectKind::TableFunction),
            ("dbo".into(), "pSecreto".into(), ObjectKind::Procedure),
            ("dbo".into(), "pTotal".into(), ObjectKind::Procedure),
            ("dbo".into(), "syn.Con Punto".into(), ObjectKind::Synonym),
            ("dbo".into(), "synBaseFalta".into(), ObjectKind::Synonym),
            ("dbo".into(), "synDeSyn".into(), ObjectKind::Synonym),
            ("dbo".into(), "synEnlazado".into(), ObjectKind::Synonym),
            ("dbo".into(), "synEscalar".into(), ObjectKind::Synonym),
            ("dbo".into(), "synFalta".into(), ObjectKind::Synonym),
            ("dbo".into(), "synPedidos".into(), ObjectKind::Synonym),
            ("dbo".into(), "synProc".into(), ObjectKind::Synonym),
            ("dbo".into(), "synRemota".into(), ObjectKind::Synonym),
            ("dbo".into(), "synRemotaFalta".into(), ObjectKind::Synonym),
            ("dbo".into(), "synSinEsquema".into(), ObjectKind::Synonym),
            ("dbo".into(), "synTabla".into(), ObjectKind::Synonym),
            ("dbo".into(), "synVista".into(), ObjectKind::Synonym),
            ("dbo".into(), "vPedidos".into(), ObjectKind::View),
            ("ventas".into(), "Pedidos_Año".into(), ObjectKind::Table),
        ]
    );

    let index_of = |name: &str| snap.objects.iter().position(|o| o.name == name).unwrap();
    let cols_of = |name: &str| {
        let i = index_of(name);
        &snap.columns.iter().find(|(k, _)| *k == i).unwrap().1
    };

    let pedidos = cols_of("Pedidos");
    let by_name = |n: &str| pedidos.iter().find(|c| c.name == n).unwrap();
    assert_eq!(pedidos.len(), 5);
    assert_eq!(pedidos[0].name, "Id");
    assert!(by_name("Id").primary_key && !by_name("Id").nullable);
    assert_eq!(by_name("Id").type_name, "int");
    assert_eq!(by_name("Cliente").type_name, "nvarchar(50)");
    assert!(!by_name("Cliente").nullable && !by_name("Cliente").primary_key);
    assert_eq!(by_name("Total").type_name, "decimal(18,2)");
    assert!(by_name("Total").nullable);
    assert_eq!(by_name("Total").default.as_deref(), Some("((0))"));
    assert_eq!(by_name("Notas").type_name, "nvarchar(max)");
    assert_eq!(by_name("Creado").type_name, "datetime2(7)");

    // Views have columns too; procedures have none.
    assert_eq!(cols_of("vPedidos").len(), 2);
    assert!(snap.columns.iter().all(|(k, _)| *k != index_of("pTotal")));

    // Table-valued functions expose their result columns; scalar functions
    // and synonyms have none.
    let names =
        |name: &str| -> Vec<String> { cols_of(name).iter().map(|c| c.name.clone()).collect() };
    assert_eq!(names("fnDeCliente"), ["Id", "Total"]);
    assert_eq!(names("fnLista"), ["Id", "Cliente", "Total"]);
    for none in ["fnDoble", "synPedidos", "synProc"] {
        assert!(snap.columns.iter().all(|(k, _)| *k != index_of(none)));
    }
}

#[tokio::test]
async fn structure_reports_foreign_keys_and_indexes() {
    let Some(p) = test_params() else { return };
    setup(&p).await;
    let mut s = connect(&p).await;

    let st = s
        .structure("tulka_meta", "dbo", "Order Details")
        .await
        .unwrap();
    assert_eq!(st.columns.len(), 2);
    assert!(st.columns.iter().all(|c| c.primary_key));
    assert_eq!(
        st.foreign_keys,
        vec![ForeignKey {
            name: "FK_Details_Pedidos".into(),
            columns: vec!["PedidoId".into()],
            ref_schema: "dbo".into(),
            ref_table: "Pedidos".into(),
            ref_columns: vec!["Id".into()],
        }]
    );
    assert_eq!(
        st.indexes,
        vec![IndexInfo {
            name: "PK_Details".into(),
            columns: vec!["PedidoId".into(), "Linea".into()],
            unique: true,
            primary_key: true,
            clustered: true,
        }]
    );

    let st = s.structure("tulka_meta", "dbo", "Pedidos").await.unwrap();
    let ix = st
        .indexes
        .iter()
        .find(|i| i.name == "IX_Pedidos_Cliente")
        .unwrap();
    assert!(ix.unique && !ix.primary_key && !ix.clustered);
    assert_eq!(ix.columns, vec!["Cliente".to_string()]);
}

#[tokio::test]
async fn definition_reads_views_procedures_functions_and_synonyms() {
    let Some(p) = test_params() else { return };
    setup(&p).await;
    let mut s = connect(&p).await;

    let view = s.definition("tulka_meta", "dbo", "vPedidos").await.unwrap();
    assert!(view.starts_with("CREATE VIEW"), "{view}");
    let proc = s.definition("tulka_meta", "dbo", "pTotal").await.unwrap();
    assert!(proc.starts_with("CREATE PROCEDURE"), "{proc}");
    for name in ["fnDoble", "fnDeCliente", "fnLista"] {
        let text = s.definition("tulka_meta", "dbo", name).await.unwrap();
        assert!(text.starts_with("CREATE FUNCTION"), "{text}");
    }
    // An encrypted module says so instead of blaming a missing permission.
    match s.definition("tulka_meta", "dbo", "pSecreto").await {
        Err(DriverError::Other(m)) => assert!(m.contains("encrypted"), "{m}"),
        other => panic!("{other:?}"),
    }
    // A synonym has no source: it yields the statement naming its base object.
    let synonym = s
        .definition("tulka_meta", "dbo", "synPedidos")
        .await
        .unwrap();
    assert_eq!(
        synonym,
        "CREATE SYNONYM [dbo].[synPedidos] FOR [dbo].[Pedidos]"
    );
    // A table has no definition.
    assert!(s.definition("tulka_meta", "dbo", "Pedidos").await.is_err());
}

#[tokio::test]
async fn reading_metadata_does_not_change_the_current_database() {
    let Some(p) = test_params() else { return };
    setup(&p).await;
    let mut s = connect(&p).await;
    let before = s.current_database().await.unwrap();
    s.snapshot("tulka_meta").await.unwrap();
    s.definition("tulka_meta", "dbo", "vPedidos").await.unwrap();
    assert_eq!(s.current_database().await.unwrap(), before);
    assert!(!s.server_info().await.unwrap().single_database);
}

#[tokio::test]
async fn offline_database_is_no_access() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;
    run(
        s.as_mut(),
        "IF DB_ID('tulka_off') IS NOT NULL
         BEGIN
           ALTER DATABASE tulka_off SET ONLINE;
           ALTER DATABASE tulka_off SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
           DROP DATABASE tulka_off;
         END
         GO
         CREATE DATABASE tulka_off
         GO
         ALTER DATABASE tulka_off SET OFFLINE WITH ROLLBACK IMMEDIATE",
    )
    .await;

    let result = s.snapshot("tulka_off").await;
    let structure = s.structure("tulka_off", "dbo", "x").await;
    let definition = s.definition("tulka_off", "dbo", "x").await;

    run(
        s.as_mut(),
        "ALTER DATABASE tulka_off SET ONLINE
         GO
         DROP DATABASE tulka_off",
    )
    .await;

    match result {
        Err(DriverError::NoAccess(message)) => assert!(message.contains("tulka_off"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(structure, Err(DriverError::NoAccess(_))));
    assert!(matches!(definition, Err(DriverError::NoAccess(_))));
}

#[tokio::test]
async fn missing_database_is_no_access() {
    let Some(p) = test_params() else { return };
    let mut s = connect(&p).await;
    assert!(matches!(
        s.snapshot("tulka_no_such_db").await,
        Err(DriverError::NoAccess(_))
    ));
}

#[tokio::test]
async fn login_without_access_is_no_access() {
    let Some(p) = test_params() else { return };
    setup(&p).await;
    let mut admin = connect(&p).await;
    let password = "Tulka_NoAccess_2026!";
    let drop_login = "IF EXISTS (SELECT 1 FROM sys.server_principals WHERE name = 'tulka_noaccess')
                      DROP LOGIN tulka_noaccess";
    run(admin.as_mut(), drop_login).await;
    run(
        admin.as_mut(),
        &format!("CREATE LOGIN tulka_noaccess WITH PASSWORD = '{password}', CHECK_POLICY = OFF"),
    )
    .await;

    let limited = ConnectParams {
        user: "tulka_noaccess".into(),
        password: password.into(),
        ..p.clone()
    };
    let mut s = connect(&limited).await;
    let result = s.snapshot("tulka_meta").await;
    let definition = s.definition("tulka_meta", "dbo", "vPedidos").await;
    drop(s);
    run(admin.as_mut(), drop_login).await;

    match result {
        Err(DriverError::NoAccess(message)) => {
            assert!(message.contains("tulka_meta"), "{message}");
            assert!(!message.contains(password), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(definition, Err(DriverError::NoAccess(_))));
}

#[tokio::test]
async fn snapshot_is_not_blocked_by_an_open_transaction() {
    let Some(p) = test_params() else { return };
    let mut admin = connect(&p).await;
    run(
        admin.as_mut(),
        "IF DB_ID('tulka_meta_lock') IS NOT NULL
         BEGIN
           ALTER DATABASE tulka_meta_lock SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
           DROP DATABASE tulka_meta_lock;
         END
         GO
         CREATE DATABASE tulka_meta_lock",
    )
    .await;
    // A transaction that created a table and has not committed yet.
    let mut writer = connect(&p).await;
    run(
        writer.as_mut(),
        "USE tulka_meta_lock
         GO
         BEGIN TRANSACTION
         CREATE TABLE dbo.Pending(Id int)",
    )
    .await;

    let mut s = connect(&p).await;
    let snap = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        s.snapshot("tulka_meta_lock"),
    )
    .await
    .expect("the catalog scan was blocked by an open transaction");
    snap.unwrap();

    run(writer.as_mut(), "ROLLBACK").await;
    drop(writer);
    drop(s);
    run(
        admin.as_mut(),
        "ALTER DATABASE tulka_meta_lock SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
         DROP DATABASE tulka_meta_lock",
    )
    .await;
}

#[tokio::test]
async fn a_synonym_reports_the_kind_of_its_target() {
    let Some(p) = test_params() else { return };
    setup(&p).await;
    let mut s = connect(&p).await;

    let snap = s.snapshot("tulka_meta").await.unwrap();
    let target = |name: &str| {
        let o = snap.objects.iter().find(|o| o.name == name).unwrap();
        assert_eq!(o.kind == ObjectKind::Synonym, name.starts_with("syn"));
        o.target
    };
    for (synonym, expected) in [
        ("synPedidos", Some(ObjectKind::Table)),
        ("synVista", Some(ObjectKind::View)),
        ("synProc", Some(ObjectKind::Procedure)),
        ("synEscalar", Some(ObjectKind::Function)),
        ("synTabla", Some(ObjectKind::TableFunction)),
        ("synSinEsquema", Some(ObjectKind::Table)),
        ("syn.Con Punto", Some(ObjectKind::Table)),
        // Another database of the server.
        ("synRemota", Some(ObjectKind::Table)),
        // Missing object, missing database, linked server, synonym chain.
        ("synFalta", None),
        ("synRemotaFalta", None),
        ("synBaseFalta", None),
        ("synEnlazado", None),
        ("synDeSyn", None),
    ] {
        assert_eq!(target(synonym), expected, "{synonym}");
    }
    // Objects that are not synonyms carry no target.
    assert_eq!(target("Pedidos"), None);
    assert_eq!(target("pTotal"), None);
}
