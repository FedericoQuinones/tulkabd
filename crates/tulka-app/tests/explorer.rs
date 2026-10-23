//! Explorer sessions, background indexing and metadata queries against a
//! scripted fake driver.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;
use tokio::sync::mpsc::Sender;
use tokio::sync::Notify;
use tulka_app::tulka_store::MetaCache;
use tulka_app::*;
use tulka_core::{
    CancelHandle, ColumnInfo, ConfigStore, ConnectParams, ConnectionConfig, DatabaseScope,
    DatabaseSnapshot, DbObject, Driver, DriverError, Environment, ExecEvent, Folder, MemoryStore,
    ObjectKind, ServerInfo, Session, TableStructure,
};

const PASSWORD: &str = "s3cret-pw";
const TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------- fake driver

/// What `snapshot` does for one database.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    Ok,
    NoAccess,
    /// A generic error, like a database that went offline after the check.
    Fail,
    Disconnect,
    /// Signals `gate_entered`, waits for `gate_release`, then succeeds.
    Gate,
}

#[derive(Default)]
struct Inner {
    databases: Vec<String>,
    single_database: bool,
    behaviors: Mutex<HashMap<String, Behavior>>,
    /// Databases passed to `snapshot`, in call order.
    snapshots: Mutex<Vec<String>>,
    /// `database` of every `connect`, in order.
    connects: Mutex<Vec<Option<String>>>,
    open: AtomicUsize,
    max_open: AtomicUsize,
    in_snapshot: AtomicUsize,
    max_in_snapshot: AtomicUsize,
    /// Set when a call used a database the session is not bound to.
    wrong_database: AtomicBool,
    gate_entered: Notify,
    gate_release: Notify,
    /// When set, `connect` signals `connect_entered` and waits for
    /// `connect_release`.
    gate_connect: AtomicBool,
    connect_entered: Notify,
    connect_release: Notify,
    /// When set as a `connect` starts, that connect fails (after the gate).
    fail_connect: AtomicBool,
}

impl Inner {
    fn new(databases: &[&str]) -> Self {
        Inner {
            databases: databases.iter().map(|d| d.to_string()).collect(),
            ..Inner::default()
        }
    }

    fn set(&self, db: &str, b: Behavior) {
        self.behaviors.lock().unwrap().insert(db.into(), b);
    }

    fn snapshots(&self) -> Vec<String> {
        self.snapshots.lock().unwrap().clone()
    }
}

struct MetaDriver(Arc<Inner>);

#[async_trait]
impl Driver for MetaDriver {
    fn id(&self) -> &'static str {
        "meta"
    }

    async fn connect(&self, p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        if p.password != PASSWORD {
            return Err(DriverError::AuthFailed("Login failed".into()));
        }
        let fail = self.0.fail_connect.load(Ordering::SeqCst);
        if self.0.gate_connect.load(Ordering::SeqCst) {
            self.0.connect_entered.notify_one();
            self.0.connect_release.notified().await;
        }
        if fail {
            return Err(DriverError::Unreachable("connect timed out".into()));
        }
        self.0.connects.lock().unwrap().push(p.database.clone());
        let open = self.0.open.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.max_open.fetch_max(open, Ordering::SeqCst);
        Ok(Box::new(MetaSession {
            inner: self.0.clone(),
            database: p.database.clone().unwrap_or_else(|| "master".into()),
        }))
    }
}

struct MetaSession {
    inner: Arc<Inner>,
    database: String,
}

impl Drop for MetaSession {
    fn drop(&mut self) {
        self.inner.open.fetch_sub(1, Ordering::SeqCst);
    }
}

impl MetaSession {
    /// On a single-database server, every metadata call must target the
    /// database the session is bound to.
    fn check_bound(&self, database: &str) {
        if self.inner.single_database && database != self.database {
            self.inner.wrong_database.store(true, Ordering::SeqCst);
        }
    }
}

fn obj(schema: &str, name: &str, kind: ObjectKind) -> DbObject {
    DbObject {
        schema: schema.into(),
        name: name.into(),
        kind,
    }
}

fn column(name: &str, pk: bool) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        type_name: "int".into(),
        nullable: !pk,
        primary_key: pk,
        default: None,
    }
}

/// `dbo.Pedidos` (table), `dbo.V_{db}` (view) and `sales.sp_{db}`
/// (procedure) with columns for the table and the view.
fn snapshot_of(db: &str) -> DatabaseSnapshot {
    DatabaseSnapshot {
        objects: vec![
            obj("dbo", "Pedidos", ObjectKind::Table),
            obj("dbo", &format!("V_{db}"), ObjectKind::View),
            obj("sales", &format!("sp_{db}"), ObjectKind::Procedure),
        ],
        columns: vec![
            (0, vec![column("id", true), column("total", false)]),
            (1, vec![column("id", true)]),
        ],
    }
}

#[async_trait]
impl Session for MetaSession {
    async fn server_info(&mut self) -> Result<ServerInfo, DriverError> {
        Ok(ServerInfo {
            version: "Fake SQL 1".into(),
            user: "sa".into(),
            single_database: self.inner.single_database,
        })
    }

    async fn list_databases(&mut self) -> Result<Vec<String>, DriverError> {
        Ok(self.inner.databases.clone())
    }

    async fn use_database(&mut self, name: &str) -> Result<(), DriverError> {
        self.database = name.into();
        Ok(())
    }

    async fn current_database(&mut self) -> Result<String, DriverError> {
        Ok(self.database.clone())
    }

    async fn snapshot(&mut self, database: &str) -> Result<DatabaseSnapshot, DriverError> {
        self.check_bound(database);
        self.inner.snapshots.lock().unwrap().push(database.into());
        let n = self.inner.in_snapshot.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.max_in_snapshot.fetch_max(n, Ordering::SeqCst);
        let behavior = self
            .inner
            .behaviors
            .lock()
            .unwrap()
            .get(database)
            .copied()
            .unwrap_or(Behavior::Ok);
        if behavior == Behavior::Gate {
            self.inner.gate_entered.notify_one();
            self.inner.gate_release.notified().await;
        }
        // Yield so concurrent indexers, if any, would overlap.
        tokio::time::sleep(Duration::from_millis(2)).await;
        self.inner.in_snapshot.fetch_sub(1, Ordering::SeqCst);
        match behavior {
            Behavior::Ok | Behavior::Gate => Ok(snapshot_of(database)),
            Behavior::NoAccess => Err(DriverError::NoAccess(database.into())),
            Behavior::Fail => Err(DriverError::Other(format!(
                "Database '{database}' cannot be opened."
            ))),
            Behavior::Disconnect => Err(DriverError::Disconnected("connection reset".into())),
        }
    }

    async fn structure(
        &mut self,
        database: &str,
        _schema: &str,
        _name: &str,
    ) -> Result<TableStructure, DriverError> {
        self.check_bound(database);
        Ok(TableStructure {
            columns: vec![column("id", true)],
            foreign_keys: vec![],
            indexes: vec![],
        })
    }

    async fn definition(
        &mut self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<String, DriverError> {
        self.check_bound(database);
        Ok(format!(
            "CREATE VIEW {schema}.{name} AS SELECT 1 -- {database}"
        ))
    }

    async fn execute(&mut self, _sql: &str, _tx: Sender<ExecEvent>) -> Result<(), DriverError> {
        Ok(())
    }

    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(NoCancel)
    }
}

struct NoCancel;

#[async_trait]
impl CancelHandle for NoCancel {
    async fn cancel(&self) -> Result<(), DriverError> {
        Ok(())
    }
}

// ------------------------------------------------------------------- helpers

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AppEvent>>,
}

impl AppEvents for Recorder {
    fn emit(&self, ev: AppEvent) {
        self.events.lock().unwrap().push(ev);
    }
}

impl Recorder {
    fn all(&self) -> Vec<AppEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Waits until an event matches `pred`.
    async fn wait(&self, pred: impl Fn(&AppEvent) -> bool) {
        tokio::time::timeout(TIMEOUT, async {
            loop {
                if self.all().iter().any(&pred) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("event never came; got {:?}", self.all()));
    }

    async fn wait_idle(&self, id: &str) {
        self.wait(|e| matches!(e, AppEvent::IndexIdle { connection_id } if connection_id == id))
            .await;
    }

    /// `(database, noAccess)` of every finished `indexProgress`, in order.
    fn finished(&self, id: &str) -> Vec<(String, bool)> {
        self.all()
            .into_iter()
            .filter_map(|e| match e {
                AppEvent::IndexProgress {
                    connection_id,
                    database,
                    done: true,
                    no_access,
                } if connection_id == id => Some((database, no_access)),
                _ => None,
            })
            .collect()
    }
}

struct Fx {
    state: Arc<AppState>,
    events: Arc<Recorder>,
    driver: Arc<Inner>,
    _dir: TempDir,
}

fn fx(driver: Inner) -> Fx {
    fx_with_cache(driver, MetaCache::in_memory())
}

fn fx_with_cache(driver: Inner, cache: MetaCache) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let events = Arc::new(Recorder::default());
    let state = AppState::new(
        ConfigStore::at(dir.path().join("config.json")),
        Arc::new(MemoryStore::default()),
        vec![Arc::new(MetaDriver(driver.clone()))],
    )
    .unwrap()
    .with_metadata(Arc::new(cache), events.clone());
    Fx {
        state: Arc::new(state),
        events,
        driver,
        _dir: dir,
    }
}

fn conn(id: &str, name: &str, folder: Option<&str>) -> ConnectionConfig {
    let mut c = ConnectionConfig::new(name, "fake-host", 1433, "sa", Environment::Local);
    c.id = id.into();
    c.driver = "meta".into();
    c.folder_id = folder.map(String::from);
    c
}

async fn save(f: &Fx, c: ConnectionConfig) -> ConnectionConfig {
    save_connection(&f.state, c, Some(PASSWORD.into()))
        .await
        .unwrap()
        .connection
}

/// A fixture with one saved connection `c1`, Explorer connected and the
/// first indexing finished.
async fn indexed(driver: Inner) -> Fx {
    let f = fx(driver);
    save(&f, conn("c1", "Main", None)).await;
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    f
}

async fn search(f: &Fx, query: &str) -> SearchResponse {
    search_objects(&f.state, query.into(), 50).await
}

/// Polls `search` until it reports nothing left to index and at least
/// `min_hits` hits (the startup index is built in the background).
async fn search_settled(f: &Fx, query: &str, min_hits: usize) -> SearchResponse {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let r = search(f, query).await;
            if r.indexing == 0 && r.hits.len() >= min_hits {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("search never settled")
}

fn status_events(f: &Fx, id: &str) -> Vec<(ConnectionStatus, Option<String>)> {
    f.events
        .all()
        .into_iter()
        .filter_map(|e| match e {
            AppEvent::ExplorerStatus {
                connection_id,
                status,
                message,
            } if connection_id == id => Some((status, message)),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ indexing

#[tokio::test]
async fn indexing_stores_every_database_and_reports_progress() {
    let f = fx(Inner::new(&["b", "a", "c"]));
    save(&f, conn("c1", "Main", None)).await;
    let info = connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    assert_eq!(info.server.version, "Fake SQL 1");
    assert_eq!(info.databases, ["b", "a", "c"]);
    f.events.wait_idle("c1").await;

    // Alphabetical without open tabs; one finished progress per database,
    // then idle as the last event.
    assert_eq!(f.driver.snapshots(), ["a", "b", "c"]);
    assert_eq!(
        f.events.finished("c1"),
        [
            ("a".into(), false),
            ("b".into(), false),
            ("c".into(), false)
        ]
    );
    assert!(matches!(
        f.events.all().last(),
        Some(AppEvent::IndexIdle { .. })
    ));
    assert_eq!(
        status_events(&f, "c1"),
        [
            (ConnectionStatus::Connecting, None),
            (ConnectionStatus::Connected, None)
        ]
    );

    let dbs = explorer_databases(&f.state, "c1".into()).await;
    assert_eq!(dbs.len(), 3);
    for d in &dbs {
        assert!(d.indexed_at.is_some() && !d.no_access, "{d:?}");
        assert_eq!((d.tables, d.views, d.procedures), (1, 1, 1));
    }
    assert_eq!(
        explorer_objects(&f.state, "c1".into(), "b".into(), ObjectKind::View).await,
        [obj("dbo", "V_b", ObjectKind::View)]
    );
    let cols = explorer_columns(
        &f.state,
        "c1".into(),
        "b".into(),
        "dbo".into(),
        "Pedidos".into(),
    )
    .await;
    assert_eq!(cols, [column("id", true), column("total", false)]);

    let r = search(&f, "pedidos").await;
    assert_eq!(r.total_matches, 3);
    assert_eq!(r.databases_matched, 3);
    assert_eq!(r.indexing, 0);
    assert!(r
        .hits
        .iter()
        .all(|h| h.connected && h.connection_name == "Main"));
    // The session is still open, and only one.
    assert_eq!(f.driver.open.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn databases_without_access_are_marked_and_indexing_goes_on() {
    let driver = Inner::new(&["a", "b", "c", "d"]);
    driver.set("b", Behavior::NoAccess);
    driver.set("c", Behavior::Fail);
    let f = indexed(driver).await;
    // Only NoAccess marks a database; another error leaves it as it was.
    assert_eq!(
        f.events.finished("c1"),
        [
            ("a".into(), false),
            ("b".into(), true),
            ("c".into(), false),
            ("d".into(), false)
        ]
    );
    let dbs = explorer_databases(&f.state, "c1".into()).await;
    let flags: Vec<_> = dbs
        .iter()
        .map(|d| (d.name.as_str(), d.no_access, d.indexed_at.is_some()))
        .collect();
    assert_eq!(
        flags,
        [
            ("a", false, true),
            ("b", true, false),
            ("c", false, false),
            ("d", false, true)
        ]
    );
    assert_eq!(search(&f, "pedidos").await.total_matches, 2);
    // Still connected: a database error is not a connection error.
    assert!(table_structure(
        &f.state,
        "c1".into(),
        "a".into(),
        "dbo".into(),
        "Pedidos".into()
    )
    .await
    .is_ok());
}

#[tokio::test]
async fn other_database_errors_keep_the_cached_objects() {
    let driver = Inner::new(&["db"]);
    driver.set("db", Behavior::Fail);
    let f = fx_with_cache(driver, cache_with(&["c1"]));
    save(&f, conn("c1", "Main", None)).await;
    search_settled(&f, "pedidos", 1).await;
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    assert_eq!(f.events.finished("c1"), [("db".into(), false)]);
    let dbs = explorer_databases(&f.state, "c1".into()).await;
    assert_eq!(dbs.len(), 1);
    assert!(!dbs[0].no_access && dbs[0].indexed_at == Some(1));
    assert_eq!(dbs[0].tables, 2);
    assert_eq!(
        explorer_objects(&f.state, "c1".into(), "db".into(), ObjectKind::Table)
            .await
            .len(),
        2
    );
    let r = search(&f, "pedidos").await;
    assert_eq!(r.hits.len(), 1);
    assert!(r.hits[0].connected);
}

/// Drives a future that never waits (no `.await` that can pend) to its end.
fn poll_ready<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match f.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(v) => v,
        std::task::Poll::Pending => panic!("the future waited"),
    }
}

/// A fixture whose next `connect` blocks; returns once it is blocked.
async fn connect_blocked() -> (Fx, tokio::task::JoinHandle<Result<ExplorerInfo, String>>) {
    let f = fx(Inner::new(&["a", "b"]));
    save(&f, conn("c1", "Main", None)).await;
    f.driver.gate_connect.store(true, Ordering::SeqCst);
    let state = f.state.clone();
    let connecting = tokio::spawn(async move { connect_explorer(&state, "c1".into(), None).await });
    tokio::time::timeout(TIMEOUT, f.driver.connect_entered.notified())
        .await
        .unwrap();
    (f, connecting)
}

/// No Explorer, no session and nothing indexed is left for `c1`.
async fn assert_nothing_left(f: &Fx) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(f.driver.open.load(Ordering::SeqCst), 0);
    assert!(f.driver.snapshots().is_empty());
    assert_eq!(
        refresh_metadata(&f.state, "c1".into(), None)
            .await
            .unwrap_err(),
        NOT_CONNECTED
    );
    assert!(explorer_databases(&f.state, "c1".into()).await.is_empty());
    assert_eq!(search(f, "pedidos").await.indexing, 0);
}

#[tokio::test]
async fn a_delete_during_connect_leaves_nothing_behind() {
    let (f, connecting) = connect_blocked().await;
    delete_connection(&f.state, "c1".into()).await.unwrap();
    f.driver.connect_release.notify_one();
    let result = tokio::time::timeout(TIMEOUT, connecting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), NOT_FOUND);
    assert_nothing_left(&f).await;
}

#[tokio::test]
async fn a_disconnect_during_connect_wins() {
    let (f, connecting) = connect_blocked().await;
    disconnect_explorer(&f.state, "c1".into()).await.unwrap();
    f.driver.connect_release.notify_one();
    let result = tokio::time::timeout(TIMEOUT, connecting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), CLOSED);
    assert_eq!(
        status_events(&f, "c1").last().unwrap(),
        &(ConnectionStatus::Disconnected, None)
    );
    assert_nothing_left(&f).await;

    // A later connect works normally.
    f.driver.gate_connect.store(false, Ordering::SeqCst);
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    assert_eq!(f.driver.snapshots(), ["a", "b"]);
}

#[tokio::test]
async fn a_disconnect_during_a_failing_connect_reports_no_error() {
    let f = fx(Inner::new(&["a"]));
    save(&f, conn("c1", "Main", None)).await;
    f.driver.gate_connect.store(true, Ordering::SeqCst);
    f.driver.fail_connect.store(true, Ordering::SeqCst);
    let state = f.state.clone();
    let connecting = tokio::spawn(async move { connect_explorer(&state, "c1".into(), None).await });
    tokio::time::timeout(TIMEOUT, f.driver.connect_entered.notified())
        .await
        .unwrap();
    disconnect_explorer(&f.state, "c1".into()).await.unwrap();
    f.driver.connect_release.notify_one();
    let result = tokio::time::timeout(TIMEOUT, connecting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status_events(&f, "c1"),
        [
            (ConnectionStatus::Connecting, None),
            (ConnectionStatus::Disconnected, None)
        ]
    );
    assert_eq!(result.unwrap_err(), CLOSED);
}

#[tokio::test]
async fn a_superseded_connect_failing_late_reports_no_error() {
    let f = fx(Inner::new(&["a"]));
    save(&f, conn("c1", "Main", None)).await;
    f.driver.gate_connect.store(true, Ordering::SeqCst);
    f.driver.fail_connect.store(true, Ordering::SeqCst);
    let state = f.state.clone();
    let first = tokio::spawn(async move { connect_explorer(&state, "c1".into(), None).await });
    tokio::time::timeout(TIMEOUT, f.driver.connect_entered.notified())
        .await
        .unwrap();
    // A newer connect goes through while the first one hangs.
    f.driver.gate_connect.store(false, Ordering::SeqCst);
    f.driver.fail_connect.store(false, Ordering::SeqCst);
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    // Then the first one fails.
    f.driver.connect_release.notify_one();
    let result = tokio::time::timeout(TIMEOUT, first).await.unwrap().unwrap();
    assert!(!status_events(&f, "c1")
        .iter()
        .any(|(s, _)| *s == ConnectionStatus::Error));
    assert_eq!(
        status_events(&f, "c1").last().unwrap(),
        &(ConnectionStatus::Connected, None)
    );
    assert_eq!(result.unwrap_err(), CLOSED);
    // The newer Explorer is still registered.
    assert!(refresh_metadata(&f.state, "c1".into(), None).await.is_ok());
}

#[tokio::test]
async fn connected_is_reported_once_the_explorer_is_registered() {
    #[derive(Default)]
    struct Probe {
        state: std::sync::OnceLock<std::sync::Weak<AppState>>,
        seen: Mutex<Vec<bool>>,
    }
    impl AppEvents for Probe {
        fn emit(&self, ev: AppEvent) {
            if let AppEvent::ExplorerStatus {
                status: ConnectionStatus::Connected,
                ..
            } = ev
            {
                let state = self.state.get().unwrap().upgrade().unwrap();
                // A refresh succeeds only for a registered Explorer.
                let registered =
                    poll_ready(refresh_metadata(&state, "c1".into(), Some("a".into()))).is_ok();
                self.seen.lock().unwrap().push(registered);
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(Inner::new(&["a"]));
    let probe = Arc::new(Probe::default());
    let state = Arc::new(
        AppState::new(
            ConfigStore::at(dir.path().join("config.json")),
            Arc::new(MemoryStore::default()),
            vec![Arc::new(MetaDriver(driver.clone()))],
        )
        .unwrap()
        .with_metadata(Arc::new(MetaCache::in_memory()), probe.clone()),
    );
    probe.state.set(Arc::downgrade(&state)).unwrap();
    save_connection(&state, conn("c1", "Main", None), Some(PASSWORD.into()))
        .await
        .unwrap();
    connect_explorer(&state, "c1".into(), None).await.unwrap();
    assert_eq!(*probe.seen.lock().unwrap(), [true]);
}

#[tokio::test]
async fn databases_open_in_a_tab_are_indexed_first() {
    let f = fx(Inner::new(&["a", "b", "c", "d"]));
    save(&f, conn("c1", "Main", None)).await;
    open_session(&f.state, "t1".into(), "c1".into(), Some("c".into()), None)
        .await
        .unwrap();
    open_session(&f.state, "t2".into(), "c1".into(), None, None)
        .await
        .unwrap();
    use_database(&f.state, "t2".into(), "b".into())
        .await
        .unwrap();
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    assert_eq!(f.driver.snapshots(), ["b", "c", "a", "d"]);
}

#[tokio::test]
async fn a_lost_connection_stops_indexing_and_keeps_what_was_saved() {
    let driver = Inner::new(&["a", "b", "c"]);
    driver.set("b", Behavior::Disconnect);
    let f = fx(driver);
    save(&f, conn("c1", "Main", None)).await;
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events
        .wait(|e| {
            matches!(
                e,
                AppEvent::ExplorerStatus {
                    status: ConnectionStatus::Error,
                    ..
                }
            )
        })
        .await;
    let (_, message) = status_events(&f, "c1").pop().unwrap();
    assert!(message.unwrap().contains("connection reset"));
    // `c` was never tried, and no idle follows an error.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(f.driver.snapshots(), ["a", "b"]);
    assert!(!f
        .events
        .all()
        .iter()
        .any(|e| matches!(e, AppEvent::IndexIdle { .. })));

    let dbs = explorer_databases(&f.state, "c1".into()).await;
    let indexed: Vec<_> = dbs
        .iter()
        .map(|d| (d.name.as_str(), d.indexed_at.is_some()))
        .collect();
    assert_eq!(indexed, [("a", true), ("b", false), ("c", false)]);

    // The palette keeps working from the cache, as not connected.
    let r = search(&f, "pedidos").await;
    assert_eq!(r.hits.len(), 1);
    assert!(!r.hits[0].connected);
    assert_eq!(r.indexing, 0);
    // Never reconnects on its own; live calls need a new connect.
    assert_eq!(f.driver.open.load(Ordering::SeqCst), 0);
    assert_eq!(
        table_structure(
            &f.state,
            "c1".into(),
            "a".into(),
            "dbo".into(),
            "Pedidos".into()
        )
        .await
        .unwrap_err(),
        NOT_CONNECTED
    );
}

#[tokio::test]
async fn disconnect_explorer_stops_indexing_and_closes_the_session() {
    let driver = Inner::new(&["a", "b", "c"]);
    driver.set("b", Behavior::Gate);
    let f = fx(driver);
    save(&f, conn("c1", "Main", None)).await;
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    tokio::time::timeout(TIMEOUT, f.driver.gate_entered.notified())
        .await
        .unwrap();
    // While `b` is pending, the palette counts it (and `c`) as indexing.
    assert_eq!(search(&f, "pedidos").await.indexing, 2);

    tokio::time::timeout(TIMEOUT, disconnect_explorer(&f.state, "c1".into()))
        .await
        .expect("disconnect waited for the gate")
        .unwrap();
    assert_eq!(f.driver.open.load(Ordering::SeqCst), 0);
    assert_eq!(
        status_events(&f, "c1").last().unwrap(),
        &(ConnectionStatus::Disconnected, None)
    );
    f.driver.gate_release.notify_one();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(f.driver.snapshots(), ["a", "b"]);
    let dbs = explorer_databases(&f.state, "c1".into()).await;
    assert!(dbs[0].indexed_at.is_some() && dbs[1].indexed_at.is_none());
    assert_eq!(search(&f, "pedidos").await.indexing, 0);
    assert_eq!(
        refresh_metadata(&f.state, "c1".into(), None)
            .await
            .unwrap_err(),
        NOT_CONNECTED
    );
}

#[tokio::test]
async fn refresh_queues_instead_of_running_two_indexers() {
    let driver = Inner::new(&["a", "b", "c"]);
    driver.set("a", Behavior::Gate);
    let f = fx(driver);
    save(&f, conn("c1", "Main", None)).await;
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    tokio::time::timeout(TIMEOUT, f.driver.gate_entered.notified())
        .await
        .unwrap();
    f.driver.set("a", Behavior::Ok);
    refresh_metadata(&f.state, "c1".into(), Some("c".into()))
        .await
        .unwrap();
    refresh_metadata(&f.state, "c1".into(), None).await.unwrap();
    f.driver.gate_release.notify_one();
    f.events.wait_idle("c1").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(f.driver.max_in_snapshot.load(Ordering::SeqCst), 1);
    // `c` jumped the queue; the full refresh re-queued `a` behind the rest.
    let snaps = f.driver.snapshots();
    assert_eq!(snaps[..3], ["a", "c", "b"]);
    assert_eq!(snaps.len(), 4, "{snaps:?}");
    let idles = f
        .events
        .all()
        .iter()
        .filter(|e| matches!(e, AppEvent::IndexIdle { .. }))
        .count();
    assert_eq!(idles, 1);

    // After idle, a refresh of one database indexes just that one.
    refresh_metadata(&f.state, "c1".into(), Some("b".into()))
        .await
        .unwrap();
    f.events.wait(|_| f.driver.snapshots().len() == 5).await;
    assert_eq!(f.driver.snapshots()[4], "b");
}

#[tokio::test]
async fn selected_databases_limit_what_is_indexed() {
    let f = fx(Inner::new(&["a", "b", "c"]));
    let mut c = conn("c1", "Main", None);
    c.databases = DatabaseScope::Selected {
        names: vec!["c".into(), "a".into()],
    };
    save(&f, c).await;
    let info = connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    assert_eq!(info.databases, ["a", "c"]);
    f.events.wait_idle("c1").await;
    assert_eq!(f.driver.snapshots(), ["a", "c"]);
}

#[tokio::test]
async fn connect_explorer_needs_a_password() {
    let f = fx(Inner::new(&["a"]));
    let mut c = conn("c1", "Main", None);
    c.password_saved = false;
    save_connection(&f.state, c, None).await.unwrap();
    assert_eq!(
        connect_explorer(&f.state, "c1".into(), None)
            .await
            .unwrap_err(),
        PASSWORD_REQUIRED
    );
    connect_explorer(&f.state, "c1".into(), Some(PASSWORD.into()))
        .await
        .unwrap();
    f.events.wait_idle("c1").await;

    let err = connect_explorer(&f.state, "c1".into(), Some("wrong-pw".into()))
        .await
        .unwrap_err();
    assert!(err.starts_with("authentication failed"), "{err}");
    let (status, message) = status_events(&f, "c1").pop().unwrap();
    assert_eq!(status, ConnectionStatus::Error);
    assert!(!message.unwrap().contains("wrong-pw"));
    for e in f.events.all() {
        assert!(!serde_json::to_string(&e).unwrap().contains(PASSWORD));
    }
}

// --------------------------------------------------------- single database

#[tokio::test]
async fn single_database_servers_get_one_session_per_database() {
    let mut driver = Inner::new(&["master", "a", "b"]);
    driver.single_database = true;
    let f = indexed(driver).await;

    assert!(!f.driver.wrong_database.load(Ordering::SeqCst));
    assert_eq!(f.driver.max_open.load(Ordering::SeqCst), 1);
    assert_eq!(f.driver.snapshots(), ["a", "b", "master"]);
    let connects = f.driver.connects.lock().unwrap().clone();
    assert_eq!(
        connects,
        [
            None,
            Some("a".into()),
            Some("b".into()),
            Some("master".into())
        ]
    );
    assert_eq!(
        f.events.finished("c1"),
        [
            ("a".into(), false),
            ("b".into(), false),
            ("master".into(), false)
        ]
    );

    // Live calls go through a session bound to the requested database.
    table_structure(
        &f.state,
        "c1".into(),
        "a".into(),
        "dbo".into(),
        "Pedidos".into(),
    )
    .await
    .unwrap();
    let def = object_definition(
        &f.state,
        "c1".into(),
        "a".into(),
        "dbo".into(),
        "V_a".into(),
    )
    .await
    .unwrap();
    assert!(def.contains("-- a"));
    assert!(!f.driver.wrong_database.load(Ordering::SeqCst));
    assert_eq!(f.driver.max_open.load(Ordering::SeqCst), 1);
}

// ------------------------------------------------------------- live queries

#[tokio::test]
async fn live_queries_need_an_explorer_session() {
    let f = fx(Inner::new(&["a"]));
    save(&f, conn("c1", "Main", None)).await;
    let structure = || {
        table_structure(
            &f.state,
            "c1".into(),
            "a".into(),
            "dbo".into(),
            "Pedidos".into(),
        )
    };
    assert_eq!(structure().await.unwrap_err(), NOT_CONNECTED);
    assert_eq!(
        object_definition(
            &f.state,
            "c1".into(),
            "a".into(),
            "dbo".into(),
            "V_a".into()
        )
        .await
        .unwrap_err(),
        NOT_CONNECTED
    );
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    assert_eq!(structure().await.unwrap().columns, [column("id", true)]);
    assert!(object_definition(
        &f.state,
        "c1".into(),
        "a".into(),
        "dbo".into(),
        "V_a".into()
    )
    .await
    .unwrap()
    .starts_with("CREATE VIEW dbo.V_a"));
    disconnect_explorer(&f.state, "c1".into()).await.unwrap();
    assert_eq!(structure().await.unwrap_err(), NOT_CONNECTED);
}

#[tokio::test]
async fn completion_metadata_comes_from_the_cache() {
    let f = indexed(Inner::new(&["a"])).await;
    disconnect_explorer(&f.state, "c1".into()).await.unwrap();
    let m = completion_metadata(&f.state, "c1".into(), "a".into()).await;
    assert_eq!(m.schemas, ["dbo", "sales"]);
    assert_eq!(m.objects.len(), 3);
    assert!(m
        .objects
        .contains(&obj("sales", "sp_a", ObjectKind::Procedure)));
    assert_eq!(m.columns.len(), 2);
    assert_eq!(m.columns["dbo.Pedidos"], ["id", "total"]);
    assert_eq!(m.columns["dbo.V_a"], ["id"]);
    let empty = completion_metadata(&f.state, "c1".into(), "nope".into()).await;
    assert!(empty.schemas.is_empty() && empty.objects.is_empty() && empty.columns.is_empty());
}

// ------------------------------------------------------------------- search

/// A cache holding `dbo.Pedidos` and `dbo.In/Out` for each connection id.
fn cache_with(ids: &[&str]) -> MetaCache {
    let cache = MetaCache::in_memory();
    for id in ids {
        let snap = DatabaseSnapshot {
            objects: vec![
                obj("dbo", "Pedidos", ObjectKind::Table),
                obj("dbo", "In/Out", ObjectKind::Table),
            ],
            columns: vec![],
        };
        cache.replace_database(id, "db", &snap, 1).unwrap();
    }
    cache
}

#[tokio::test]
async fn the_index_is_built_from_the_cache_at_startup() {
    let f = fx_with_cache(Inner::new(&[]), cache_with(&["c1"]));
    save(&f, conn("c1", "Main", None)).await;
    let r = search_settled(&f, "pedidos", 1).await;
    assert_eq!(r.hits.len(), 1);
    let hit = &r.hits[0];
    assert_eq!(hit.connection_id, "c1");
    assert_eq!(hit.connection_name, "Main");
    assert!(!hit.connected);
    assert_eq!(hit.database, "db");
    assert_eq!(hit.object, obj("dbo", "Pedidos", ObjectKind::Table));
    assert!(hit.folder_path.is_empty());
    // Character positions in `dbo.Pedidos`.
    assert_eq!(hit.indices, [4, 5, 6, 7, 8, 9, 10]);
}

#[tokio::test]
async fn a_folder_prefix_limits_the_search_to_that_folder_and_below() {
    let f = fx_with_cache(Inner::new(&[]), cache_with(&["c1", "c2", "c3", "c4"]));
    let fa = create_folder(&f.state, "Cliente A".into(), None)
        .await
        .unwrap();
    let fx_ = create_folder(&f.state, "Proyecto X".into(), Some(fa.id.clone()))
        .await
        .unwrap();
    let fs = create_folder(&f.state, "Sub".into(), Some(fx_.id.clone()))
        .await
        .unwrap();
    let fo = create_folder(&f.state, "Otro".into(), None).await.unwrap();
    save(&f, conn("c1", "Uno", Some(&fx_.id))).await;
    save(&f, conn("c2", "Dos", Some(&fs.id))).await;
    save(&f, conn("c3", "Tres", Some(&fa.id))).await;
    save(&f, conn("c4", "Cuatro", Some(&fo.id))).await;
    search_settled(&f, "pedidos", 4).await;

    let conns = |r: &SearchResponse| {
        let mut v: Vec<String> = r.hits.iter().map(|h| h.connection_name.clone()).collect();
        v.sort();
        v
    };
    // Just the folder name, any case.
    let r = search(&f, "proyecto x/ped").await;
    assert_eq!(conns(&r), ["Dos", "Uno"]);
    assert_eq!(r.total_matches, 2);
    let dos = r.hits.iter().find(|h| h.connection_name == "Dos").unwrap();
    assert_eq!(dos.folder_path, ["Cliente A", "Proyecto X", "Sub"]);
    // The full path from the root, and a parent folder.
    assert_eq!(
        conns(&search(&f, "Cliente A/Proyecto X/ped").await),
        ["Dos", "Uno"]
    );
    assert_eq!(
        conns(&search(&f, "Cliente A/ped").await),
        ["Dos", "Tres", "Uno"]
    );
    // A path that does not exist searches the whole text, slashes included.
    assert!(search(&f, "Otro/Proyecto X/ped").await.hits.is_empty());
    let r = search(&f, "In/Out").await;
    assert_eq!(r.total_matches, 4);
    assert!(r.hits.iter().all(|h| h.object.name == "In/Out"));
}

#[tokio::test]
async fn a_folder_prefix_survives_a_corrupt_parent_loop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut c1 = conn("c1", "Uno", Some("l1"));
    c1.password_saved = false;
    let config = tulka_core::AppConfig {
        connections: vec![c1],
        folders: vec![
            Folder {
                id: "l1".into(),
                name: "L1".into(),
                parent_id: Some("l2".into()),
            },
            Folder {
                id: "l2".into(),
                name: "L2".into(),
                parent_id: Some("l1".into()),
            },
        ],
        ..tulka_core::AppConfig::default()
    };
    ConfigStore::at(&path).save(&config).unwrap();
    let state = AppState::new(
        ConfigStore::at(&path),
        Arc::new(MemoryStore::default()),
        vec![],
    )
    .unwrap()
    .with_metadata(Arc::new(cache_with(&["c1"])), Arc::new(NoopEvents));
    let r = tokio::time::timeout(TIMEOUT, async {
        loop {
            let r = search_objects(&state, "L2/ped".into(), 10).await;
            if !r.hits.is_empty() {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("search hung or never found the object");
    assert_eq!(r.hits[0].connection_id, "c1");
}

#[tokio::test]
async fn search_marks_which_connections_are_connected() {
    let f = fx_with_cache(Inner::new(&["db"]), cache_with(&["c2"]));
    save(&f, conn("c1", "Live", None)).await;
    save(&f, conn("c2", "Cached", None)).await;
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    let r = search_settled(&f, "pedidos", 2).await;
    let connected: HashMap<_, _> = r
        .hits
        .iter()
        .map(|h| (h.connection_id.as_str(), h.connected))
        .collect();
    assert_eq!(connected, HashMap::from([("c1", true), ("c2", false)]));
}

// --------------------------------------------------------------- deletion

#[tokio::test]
async fn delete_connection_closes_its_sessions_and_forgets_its_cache() {
    let f = fx_with_cache(Inner::new(&["a", "b"]), cache_with(&["other"]));
    save(&f, conn("other", "Other", None)).await;
    save(&f, conn("c1", "Main", None)).await;
    open_session(&f.state, "t1".into(), "c1".into(), None, None)
        .await
        .unwrap();
    connect_explorer(&f.state, "c1".into(), None).await.unwrap();
    f.events.wait_idle("c1").await;
    search_settled(&f, "pedidos", 3).await;
    assert_eq!(f.driver.open.load(Ordering::SeqCst), 2);

    delete_connection(&f.state, "c1".into()).await.unwrap();
    assert_eq!(f.driver.open.load(Ordering::SeqCst), 0);
    assert!(explorer_databases(&f.state, "c1".into()).await.is_empty());
    let r = search(&f, "pedidos").await;
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.hits[0].connection_id, "other");
    assert!(run_query(&f.state, "t1".into(), "SELECT 1".into(), |_| {})
        .await
        .is_err());
    assert_eq!(explorer_databases(&f.state, "other".into()).await.len(), 1);
}
