//! The AI tools against a live SQL Server.
//!
//! Skipped (they print "skipped" and pass) unless `TULKA_MSSQL_TEST` is set,
//! e.g. `eval "$(bash scripts/mssql-test.sh)"`.

use std::future::pending;
use std::sync::Arc;

use tempfile::TempDir;
use tulka_app::ai::{describe_table, run_query, AiState, QueryRequest, ToolText};
use tulka_app::tulka_store::{MetaCache, SharedRunLog};
use tulka_core::{
    AiAccess, AppConfig, ConfigStore, ConnectParams, ConnectionConfig, Driver, Environment,
    MemoryStore, Preferences, SecretStore as _,
};
use tulka_mssql::MssqlDriver;

struct Live {
    state: AiState,
    params: ConnectParams,
    database: String,
    _dir: TempDir,
}

fn live_params() -> Option<(ConnectionConfig, String)> {
    let Ok(raw) = std::env::var("TULKA_MSSQL_TEST") else {
        println!("skipped (TULKA_MSSQL_TEST not set)");
        return None;
    };
    let mut conn = ConnectionConfig::new("Live", "127.0.0.1", 1433, "", Environment::Test);
    conn.trust_server_certificate = true;
    conn.password_saved = true;
    conn.ai_access = AiAccess::ReadOnly;
    let mut password = String::new();
    for part in raw.split(';').filter(|s| !s.trim().is_empty()) {
        let (key, value) = part
            .split_once('=')
            .expect("TULKA_MSSQL_TEST entries must be key=value");
        match key.trim().to_lowercase().as_str() {
            "server" => conn.server = value.trim().to_string(),
            "port" => conn.port = value.trim().parse().expect("port must be a number"),
            "user" => conn.user = value.to_string(),
            "password" => password = value.to_string(),
            _ => {}
        }
    }
    Some((conn, password))
}

impl Live {
    /// Runs setup SQL as the app would, outside the AI tools.
    async fn setup(&self, sql: &str) {
        let mut session = MssqlDriver.connect(&self.params).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        session.execute(sql, tx).await.unwrap();
        drain.await.unwrap();
    }

    async fn run(&self, sql: &str, max_rows: Option<u32>) -> ToolText {
        let req = QueryRequest {
            connection: "Live".into(),
            database: Some(self.database.clone()),
            sql: sql.into(),
            max_rows,
        };
        run_query(&self.state, req, pending()).await
    }

    async fn drop_database(&self) {
        let mut params = self.params.clone();
        params.database = None;
        let mut session = MssqlDriver.connect(&params).await.unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let sql = format!(
            "ALTER DATABASE [{0}] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; DROP DATABASE [{0}]",
            self.database
        );
        let _ = session.execute(&sql, tx).await;
        let _ = drain.await;
    }
}

/// A fresh database with `dbo.clientes` (a foreign key and an index), or
/// `None` when no server is configured.
async fn live() -> Option<Live> {
    let (conn, password) = live_params()?;
    let dir = tempfile::tempdir().unwrap();
    let config = ConfigStore::at(dir.path().join("config.json"));
    config
        .save(&AppConfig {
            version: 1,
            connections: vec![conn.clone()],
            folders: Vec::new(),
            expanded: Vec::new(),
            preferences: Preferences::default(),
        })
        .unwrap();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set(&conn.id, &password).unwrap();
    let state = AiState::new(
        config,
        secrets,
        vec![Arc::new(MssqlDriver)],
        Some(Arc::new(MetaCache::in_memory())),
        Arc::new(SharedRunLog::in_memory()),
    );
    let params = ConnectParams {
        server: conn.server.clone(),
        port: conn.port,
        user: conn.user.clone(),
        password,
        trust_server_certificate: true,
        ..ConnectParams::default()
    };
    let database = format!("tulka_ai_{}", uuid::Uuid::new_v4().simple());
    let live = Live {
        state,
        params,
        database: database.clone(),
        _dir: dir,
    };
    live.setup(&format!("CREATE DATABASE [{database}]")).await;
    let mut inner = live.params.clone();
    inner.database = Some(database.clone());
    let probe = Live {
        params: inner,
        state: AiState::new(
            ConfigStore::at(""),
            Arc::new(MemoryStore::default()),
            Vec::new(),
            None,
            Arc::new(SharedRunLog::in_memory()),
        ),
        database: database.clone(),
        _dir: tempfile::tempdir().unwrap(),
    };
    probe
        .setup(
            "CREATE TABLE dbo.zonas(id int NOT NULL PRIMARY KEY, nombre nvarchar(50) NULL)\n\
             GO\n\
             CREATE TABLE dbo.clientes(id int NOT NULL PRIMARY KEY, nombre nvarchar(50) NULL, \
             zona_id int NULL CONSTRAINT fk_cli_zona REFERENCES dbo.zonas(id))\n\
             GO\n\
             CREATE INDEX ix_clientes_nombre ON dbo.clientes(nombre)\n\
             GO\n\
             INSERT INTO dbo.zonas VALUES (1, N'Norte')\n\
             INSERT INTO dbo.clientes VALUES (1, N'Ana', 1), (2, N'Beto', 1), (3, N'Carla', NULL), (4, N'Dani', 1)",
        )
        .await;
    Some(live)
}

#[tokio::test]
async fn live_select_and_describe() {
    let Some(l) = live().await else { return };
    let described = describe_table(
        &l.state,
        "Live".into(),
        Some(l.database.clone()),
        "clientes".into(),
    )
    .await;
    assert!(!described.is_error, "{}", described.text);
    for part in [
        "dbo.clientes",
        "zona_id",
        "fk_cli_zona",
        "ix_clientes_nombre",
        "PK",
    ] {
        assert!(described.text.contains(part), "{part}: {}", described.text);
    }

    let out = l
        .run(
            "SELECT TOP 3 id, nombre FROM dbo.clientes ORDER BY id",
            None,
        )
        .await;
    assert_eq!(
        out,
        ToolText {
            text: "id|nombre\n1|Ana\n2|Beto\n3|Carla\n".into(),
            is_error: false,
        }
    );
    l.drop_database().await;
}

#[tokio::test]
async fn live_attempted_writes_change_nothing() {
    let Some(l) = live().await else { return };
    let attempts = [
        "INSERT INTO dbo.clientes VALUES (9, N'X', NULL)",
        "UPDATE dbo.clientes SET nombre = N'X'",
        "DELETE FROM dbo.clientes",
        "TRUNCATE TABLE dbo.clientes",
        "DROP TABLE dbo.clientes",
        "ALTER TABLE dbo.clientes ADD extra int",
        "SELECT * INTO dbo.copia FROM dbo.clientes",
        "EXEC sp_who",
        "EXEC('DELETE FROM dbo.clientes')",
        "SELECT 1; DELETE FROM dbo.clientes",
        "WITH c AS (SELECT id FROM dbo.clientes) DELETE FROM c",
        "SELECT * FROM OPENROWSET('SQLNCLI', 'Server=x;', 'SELECT 1')",
        "SELECT * FROM a.b.c.d",
    ];
    for sql in attempts {
        let out = l.run(sql, None).await;
        assert!(out.is_error, "{sql}: {}", out.text);
    }
    let count = l.run("SELECT COUNT(*) AS n FROM dbo.clientes", None).await;
    assert_eq!(count.text, "n\n4\n");
    let tables = l
        .run("SELECT name FROM sys.tables ORDER BY name", None)
        .await;
    assert_eq!(tables.text, "name\nclientes\nzonas\n");
    l.drop_database().await;
}

#[tokio::test]
async fn live_rowcount_limit() {
    let Some(l) = live().await else { return };
    let out = l.run("SELECT * FROM sys.all_objects", Some(5)).await;
    assert!(!out.is_error, "{}", out.text);
    // Header, 5 rows and the mark.
    assert_eq!(out.text.lines().count(), 7, "{}", out.text);
    assert!(out.text.contains("Limit reached: only the first 5 rows"));
    l.drop_database().await;
}
