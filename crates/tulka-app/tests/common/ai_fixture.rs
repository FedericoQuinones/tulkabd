//! A fake SQL Server driver and a ready-made `AiState` for testing the AI
//! tools. Self-contained, so another crate can include it with `#[path]`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc::Sender;
use tokio::sync::Notify;
use tulka_app::ai::AiState;
use tulka_app::tulka_store::{MetaCache, SharedRunLog};
use tulka_core::{
    AiAccess, AppConfig, CancelHandle, Column, ConfigStore, ConnectParams, ConnectionConfig,
    DatabaseScope, DatabaseSnapshot, DbObject, Driver, DriverError, Environment, ExecEvent, Folder,
    MemoryStore, MessageKind, ObjectKind, Preferences, ReadOnlyViolation, SecretStore, ServerInfo,
    Session, TableStructure, Value,
};

/// The password every connection of the fixture has saved.
pub const PASSWORD: &str = "S3cret!pw";

/// What the fake driver was asked, and what it answers.
#[derive(Default)]
pub struct Script {
    /// Every login, in order.
    pub connects: Mutex<Vec<ConnectParams>>,
    /// Every script given to `execute_read_only`.
    pub sqls: Mutex<Vec<String>>,
    /// Every `structure` call as `(database, schema, name)`.
    pub structures_asked: Mutex<Vec<(String, String, String)>>,
    /// Every `snapshot` call.
    pub snapshots_asked: Mutex<Vec<String>>,
    /// What `list_databases` answers; `None` makes the login fail.
    pub databases: Mutex<Option<Vec<String>>>,
    /// The database of a login that names none.
    pub default_database: Mutex<String>,
    pub snapshots: Mutex<HashMap<String, DatabaseSnapshot>>,
    pub structures: Mutex<HashMap<(String, String, String), TableStructure>>,
    /// Sessions alive now, and the most there ever were at once.
    pub live: AtomicUsize,
    pub peak: AtomicUsize,
}

pub struct FakeDriver {
    pub script: Arc<Script>,
}

impl FakeDriver {
    pub fn new() -> Arc<Self> {
        let script = Script::default();
        *script.databases.lock().unwrap() = Some(vec!["master".into(), "sales".into()]);
        *script.default_database.lock().unwrap() = "master".into();
        Arc::new(Self {
            script: Arc::new(script),
        })
    }
}

#[async_trait]
impl Driver for FakeDriver {
    fn id(&self) -> &'static str {
        "fake"
    }

    async fn connect(&self, p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        self.script.connects.lock().unwrap().push(p.clone());
        if p.password != PASSWORD {
            return Err(DriverError::AuthFailed(format!(
                "Login failed for user {}@{}",
                p.user, p.server
            )));
        }
        if self.script.databases.lock().unwrap().is_none() {
            return Err(DriverError::Unreachable(format!(
                "{}:{}: connection refused for {}",
                p.server, p.port, p.user
            )));
        }
        let now = self.script.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.script.peak.fetch_max(now, Ordering::SeqCst);
        Ok(Box::new(FakeSession {
            script: self.script.clone(),
            database: p.database.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
        }))
    }

    fn read_only_violation(&self, sql: &str) -> Option<ReadOnlyViolation> {
        tulka_mssql::readonly::violation(sql)
    }
}

struct FakeSession {
    script: Arc<Script>,
    database: Option<String>,
    cancelled: Arc<AtomicBool>,
    wake: Arc<Notify>,
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.script.live.fetch_sub(1, Ordering::SeqCst);
    }
}

struct FakeCancel {
    cancelled: Arc<AtomicBool>,
    wake: Arc<Notify>,
}

#[async_trait]
impl CancelHandle for FakeCancel {
    async fn cancel(&self) -> Result<(), DriverError> {
        self.cancelled.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
        Ok(())
    }
}

fn column(name: &str) -> Column {
    Column {
        name: name.into(),
        type_name: "int".into(),
    }
}

#[async_trait]
impl Session for FakeSession {
    async fn server_info(&mut self) -> Result<ServerInfo, DriverError> {
        Ok(ServerInfo {
            version: "Fake".into(),
            user: "ai".into(),
            single_database: false,
        })
    }

    async fn list_databases(&mut self) -> Result<Vec<String>, DriverError> {
        Ok(self
            .script
            .databases
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default())
    }

    async fn use_database(&mut self, name: &str) -> Result<(), DriverError> {
        self.database = Some(name.into());
        Ok(())
    }

    async fn current_database(&mut self) -> Result<String, DriverError> {
        Ok(self
            .database
            .clone()
            .unwrap_or_else(|| self.script.default_database.lock().unwrap().clone()))
    }

    async fn snapshot(&mut self, database: &str) -> Result<DatabaseSnapshot, DriverError> {
        self.script
            .snapshots_asked
            .lock()
            .unwrap()
            .push(database.into());
        self.script
            .snapshots
            .lock()
            .unwrap()
            .get(database)
            .cloned()
            .ok_or_else(|| DriverError::NoAccess(database.into()))
    }

    async fn structure(
        &mut self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<TableStructure, DriverError> {
        if name == "lost" {
            return Err(DriverError::Disconnected(
                "socket closed by db.example.com".into(),
            ));
        }
        let key = (database.to_string(), schema.to_string(), name.to_string());
        self.script
            .structures_asked
            .lock()
            .unwrap()
            .push(key.clone());
        Ok(self
            .script
            .structures
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or(TableStructure {
                columns: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
            }))
    }

    async fn definition(&mut self, _: &str, _: &str, _: &str) -> Result<String, DriverError> {
        Ok(String::new())
    }

    async fn execute(&mut self, _: &str, _: Sender<ExecEvent>) -> Result<(), DriverError> {
        Err(DriverError::Other("the AI never runs plain scripts".into()))
    }

    /// Scripted by the text: `SELECT big` gives 2000 rows, `SELECT slow`
    /// waits until cancelled, `SELECT err` fails at line 2, anything else
    /// gives one row.
    async fn execute_read_only(
        &mut self,
        sql: &str,
        max_rows: u32,
        tx: Sender<ExecEvent>,
    ) -> Result<(), DriverError> {
        self.script.sqls.lock().unwrap().push(sql.to_string());
        let send = |event| async { tx.send(event).await.map_err(|_| DriverError::Cancelled) };
        match sql.trim() {
            "SELECT slow" => {
                let woken = self.wake.notified();
                tokio::pin!(woken);
                woken.as_mut().enable();
                if !self.cancelled.load(Ordering::SeqCst) {
                    woken.await;
                }
                Err(DriverError::Cancelled)
            }
            "SELECT err" => Err(DriverError::Query {
                message: "Invalid column name 'nope'.".into(),
                line: Some(2),
            }),
            "SELECT big" => {
                send(ExecEvent::ResultStart {
                    index: 0,
                    columns: vec![column("n")],
                })
                .await?;
                let total = 2000.min(max_rows.saturating_add(1));
                let rows = (0..total).map(|n| vec![Value::Int(i64::from(n))]).collect();
                send(ExecEvent::Rows { index: 0, rows }).await
            }
            "SELECT format" => {
                send(ExecEvent::ResultStart {
                    index: 0,
                    columns: vec![column("id"), column("name"), column("note")],
                })
                .await?;
                send(ExecEvent::Rows {
                    index: 0,
                    rows: vec![
                        vec![Value::Int(1), Value::Text("Ana".into()), Value::Null],
                        vec![
                            Value::Int(2),
                            Value::Text(String::new()),
                            Value::Text("a|b".into()),
                        ],
                    ],
                })
                .await
            }
            "SELECT sets" => {
                send(ExecEvent::ResultStart {
                    index: 0,
                    columns: vec![column("a")],
                })
                .await?;
                send(ExecEvent::Rows {
                    index: 0,
                    rows: vec![vec![Value::Int(1)]],
                })
                .await?;
                send(ExecEvent::Message {
                    kind: MessageKind::Info,
                    text: "hello".into(),
                    line: None,
                })
                .await?;
                send(ExecEvent::ResultStart {
                    index: 1,
                    columns: vec![column("b")],
                })
                .await?;
                send(ExecEvent::Rows {
                    index: 1,
                    rows: vec![vec![Value::Int(2)], vec![Value::Int(3)]],
                })
                .await?;
                send(ExecEvent::Message {
                    kind: MessageKind::Warning,
                    text: "careful".into(),
                    line: Some(3),
                })
                .await
            }
            "SELECT long" => {
                send(ExecEvent::ResultStart {
                    index: 0,
                    columns: vec![column("t")],
                })
                .await?;
                send(ExecEvent::Rows {
                    index: 0,
                    rows: vec![vec![Value::Text("é".repeat(5000))]],
                })
                .await
            }
            "SELECT wide" => {
                send(ExecEvent::ResultStart {
                    index: 0,
                    columns: vec![column("t")],
                })
                .await?;
                for _ in 0..100 {
                    send(ExecEvent::Rows {
                        index: 0,
                        rows: vec![vec![Value::Text("x".repeat(1900))]],
                    })
                    .await?;
                    if self.cancelled.load(Ordering::SeqCst) {
                        return Err(DriverError::Cancelled);
                    }
                }
                Ok(())
            }
            _ => {
                send(ExecEvent::ResultStart {
                    index: 0,
                    columns: vec![column("n")],
                })
                .await?;
                send(ExecEvent::Rows {
                    index: 0,
                    rows: vec![vec![Value::Int(1)]],
                })
                .await
            }
        }
    }

    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(FakeCancel {
            cancelled: self.cancelled.clone(),
            wake: self.wake.clone(),
        })
    }
}

/// A connection of the fake driver with a saved password.
pub fn connection(name: &str, environment: Environment, access: AiAccess) -> ConnectionConfig {
    let mut conn = ConnectionConfig::new(name, "db.example.com", 1433, "ai", environment);
    conn.driver = "fake".into();
    conn.password_saved = true;
    conn.ai_access = access;
    conn
}

pub struct Fixture {
    pub state: AiState,
    pub driver: Arc<FakeDriver>,
    pub cache: Arc<MetaCache>,
    pub runlog: Arc<SharedRunLog>,
    pub config: ConfigStore,
    pub secrets: Arc<MemoryStore>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// Prod without AI access and Local with read-only access (all of its
    /// databases), both with their password saved.
    pub fn standard() -> Self {
        Self::with(
            vec![
                connection("Prod", Environment::Production, AiAccess::None),
                connection("Local", Environment::Local, AiAccess::ReadOnly),
            ],
            Vec::new(),
        )
    }

    pub fn with(connections: Vec<ConnectionConfig>, folders: Vec<Folder>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigStore::at(dir.path().join("config.json"));
        let secrets = Arc::new(MemoryStore::default());
        for c in &connections {
            secrets.set(&c.id, PASSWORD).unwrap();
        }
        let driver = FakeDriver::new();
        let cache = Arc::new(MetaCache::in_memory());
        let runlog = Arc::new(SharedRunLog::in_memory());
        let state = AiState::new(
            config.clone(),
            secrets.clone(),
            vec![driver.clone()],
            Some(cache.clone()),
            runlog.clone(),
        );
        let fixture = Self {
            state,
            driver,
            cache,
            runlog,
            config,
            secrets,
            _dir: dir,
        };
        fixture.write_config(connections, folders);
        fixture
    }

    /// Replaces `config.json`, as the app does when the user edits it.
    pub fn write_config(&self, connections: Vec<ConnectionConfig>, folders: Vec<Folder>) {
        self.config
            .save(&AppConfig {
                version: 1,
                connections,
                folders,
                expanded: Vec::new(),
                preferences: Preferences::default(),
            })
            .unwrap();
    }

    /// Changes the preferences in `config.json`.
    pub fn set_preferences(&self, change: impl FnOnce(&mut Preferences)) {
        let mut config = self.config.load().unwrap();
        change(&mut config.preferences);
        self.config.save(&config).unwrap();
    }

    /// The connections now in `config.json`.
    pub fn connections(&self) -> Vec<ConnectionConfig> {
        self.config.load().unwrap().connections
    }

    pub fn config_path(&self) -> PathBuf {
        self.config.path().to_path_buf()
    }

    /// Caches the given tables of `database` for a connection.
    pub fn seed(&self, connection_id: &str, database: &str, tables: &[(&str, &str)]) {
        let objects = tables
            .iter()
            .map(|(schema, name)| DbObject {
                schema: (*schema).into(),
                name: (*name).into(),
                kind: ObjectKind::Table,
            })
            .collect();
        self.cache
            .replace_database(
                connection_id,
                database,
                &DatabaseSnapshot {
                    objects,
                    columns: Vec::new(),
                },
                1,
            )
            .unwrap();
    }
}

pub fn selected(names: &[&str]) -> DatabaseScope {
    DatabaseScope::Selected {
        names: names.iter().map(|n| (*n).to_string()).collect(),
    }
}
