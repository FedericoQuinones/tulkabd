//! The Explorer: one metadata session per connection, background indexing
//! into the SQLite cache and the in-memory search index, and the queries of
//! the tree, the Ctrl+P palette and autocompletion.
//!
//! # Sessions
//!
//! `connect_explorer` opens one session per connection, separate from the
//! query tabs. On a single-database server (Azure SQL Database) a session is
//! bound to one database, so the Explorer keeps at most one session and
//! rebinds it: to use database `d` it first drops the current session, then
//! connects with `database = d`. There are never two sessions at once for
//! one connection, and metadata of `d` is never read from a session bound to
//! another database.
//!
//! # Indexing
//!
//! Each connected Explorer has a queue of databases and at most one tokio
//! task draining it, one database at a time: `snapshot`, the cache write
//! (`spawn_blocking`), the search index update, then an `indexProgress`
//! event. Databases open in a query tab of the connection go first, then
//! the rest alphabetically. `refresh_metadata` adds to the queue (one
//! database jumps to its front) and starts the task only if none is running.
//! A database the login cannot use (`NoAccess`) is marked `noAccess` in the
//! cache, so the tree shows it dimmed; any other error on one database (for
//! example one that went offline after the access check) leaves what was
//! cached for it untouched. Either way indexing goes on. Only a lost
//! connection or `disconnect_explorer` stops it: what was saved stays, and
//! the Explorer never reconnects on its own.
//!
//! # Connecting
//!
//! `connect_explorer` registers its attempt before connecting;
//! `disconnect_explorer` and `delete_connection` cancel it. Once connected
//! it registers the Explorer (and writes the database list to the cache)
//! under the connections lock, after checking that the connection still
//! exists and that its attempt was not cancelled or superseded. So a delete
//! or a disconnect that comes in while connecting always wins: the new
//! session is dropped and nothing is cached for a deleted connection.
//!
//! # Locks
//!
//! `conns` is a `std::sync::Mutex` held only for short, non-async sections
//! (queue and bookkeeping). Each connection's session sits behind its own
//! `tokio::sync::Mutex` ("slot"), held across the I/O of one snapshot or one
//! live call: a live call waits for at most the snapshot in progress. The
//! search index is a `std::sync::RwLock` taken only on blocking threads: the
//! read lock just for `search`, the write lock just for one
//! `replace_database` or `remove_connection`. None of these is ever held
//! together with another one, nor with the tabs or connections locks.
//!
//! Stopping a connection's Explorer drops its entry (whose `stop` sender
//! wakes the task), waits for the task to end, then closes the slot. The task
//! can only be interrupted while talking to the server; a cache and index
//! write in progress always finishes, so the two stay consistent.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use tulka_core::{
    AppConfig, ColumnInfo, ConnectParams, DatabaseScope, DatabaseSnapshot, DbObject, Driver,
    DriverError, Folder, ObjectKind, Session, TableStructure,
};
use tulka_store::{CachedDatabase, MetaCache, SearchEntry, SearchIndex, SearchScope};

use crate::connections::{blocking, connect_params, resolve_password};
use crate::state::AppState;
use crate::types::{
    AppEvent, AppEvents, CompletionMetadata, ConnectionStatus, ExplorerInfo, NoopEvents,
    PaletteHit, SearchResponse, CLOSED, DISCONNECTED, NOT_CONNECTED, NOT_FOUND,
};

/// Explorer state shared by the commands and the indexing tasks.
pub(crate) struct Explorer {
    cache: Arc<MetaCache>,
    events: Arc<dyn AppEvents>,
    index: Arc<RwLock<SearchIndex>>,
    /// `true` once the startup index was built from the cache.
    loaded: watch::Sender<bool>,
    conns: std::sync::Mutex<HashMap<String, Conn>>,
    /// The epoch of the latest `connect_explorer` of each connection;
    /// removed by a disconnect or a delete. Locked before `conns` when both
    /// are needed.
    attempts: std::sync::Mutex<HashMap<String, u64>>,
    next_epoch: AtomicU64,
}

/// A connected Explorer. Present in `conns` exactly while connected.
struct Conn {
    /// Unique per `connect_explorer`, so a stopped task never touches the
    /// entry of a newer connect.
    epoch: u64,
    slot: Arc<AsyncMutex<Slot>>,
    /// Databases the connection exposes.
    databases: Vec<String>,
    queue: VecDeque<String>,
    /// The database being indexed.
    current: Option<String>,
    /// An indexing task is draining `queue`.
    running: bool,
    task: Option<JoinHandle<()>>,
    /// Dropped (or set) to stop the task.
    stop: watch::Sender<bool>,
}

/// The Explorer session of one connection.
struct Slot {
    session: Option<Box<dyn Session>>,
    /// Single-database servers only: how to open a session bound to another
    /// database (the password stays in memory, never in events or errors),
    /// and the database the current session is bound to.
    rebind: Option<(Arc<dyn Driver>, ConnectParams)>,
    bound: Option<String>,
    /// Set by a disconnect or a lost connection; the slot is never used again.
    closed: bool,
}

enum LiveError {
    /// The Explorer was disconnected.
    Closed,
    Driver(DriverError),
}

impl Slot {
    /// The session to read metadata of `database` with, rebinding first on a
    /// single-database server.
    async fn session_for(&mut self, database: &str) -> Result<&mut Box<dyn Session>, LiveError> {
        if self.closed {
            return Err(LiveError::Closed);
        }
        if let Some((driver, params)) = &self.rebind {
            if self.bound.as_deref() != Some(database) {
                // Drop the old session before opening the new one.
                self.session = None;
                self.bound = None;
                let mut params = params.clone();
                params.database = Some(database.to_string());
                let session = driver.connect(&params).await.map_err(|e| match e {
                    // The old session is already gone, so failing to reach
                    // the server means the connection is lost, not one database.
                    DriverError::Unreachable(_) | DriverError::Disconnected(_) => {
                        LiveError::Driver(DriverError::Disconnected(e.to_string()))
                    }
                    e => LiveError::Driver(e),
                })?;
                self.session = Some(session);
                self.bound = Some(database.to_string());
            }
        }
        self.session.as_mut().ok_or(LiveError::Closed)
    }

    /// Closes the slot when `result` says the connection was lost.
    fn check<T>(&mut self, result: Result<T, LiveError>) -> Result<T, LiveError> {
        if let Err(LiveError::Driver(DriverError::Disconnected(_))) = &result {
            self.close();
        }
        result
    }

    fn close(&mut self) {
        self.closed = true;
        self.session = None;
        self.bound = None;
        self.rebind = None;
    }
}

/// Marks the startup index as loaded when dropped.
struct MarkLoaded<'a>(&'a Explorer);

impl Drop for MarkLoaded<'_> {
    fn drop(&mut self) {
        self.0.loaded.send_replace(true);
    }
}

/// What the indexing task does next.
enum Next {
    Index(String),
    Idle,
    /// The entry was removed or replaced.
    Gone,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Explorer {
    /// An Explorer over an empty in-memory cache that drops its events.
    pub(crate) fn empty() -> Arc<Self> {
        let explorer = Self::new(Arc::new(MetaCache::in_memory()), Arc::new(NoopEvents));
        explorer.loaded.send_replace(true);
        explorer
    }

    /// An Explorer over `cache`, building the search index from it on a
    /// background thread.
    pub(crate) fn start(cache: Arc<MetaCache>, events: Arc<dyn AppEvents>) -> Arc<Self> {
        let explorer = Self::new(cache, events);
        let loader = explorer.clone();
        let spawned = std::thread::Builder::new()
            .name("tulka-index-load".into())
            .spawn(move || {
                // Set even if loading panics, or every index update would
                // wait forever.
                let _loaded = MarkLoaded(&loader);
                let entries = loader
                    .cache
                    .all_objects()
                    .into_iter()
                    .map(|(connection_id, database, object)| SearchEntry {
                        connection_id,
                        database,
                        object,
                    })
                    .collect();
                let built = SearchIndex::build(entries);
                *loader.index.write().unwrap_or_else(PoisonError::into_inner) = built;
            });
        if spawned.is_err() {
            // Search starts empty and fills up as connections are indexed.
            explorer.loaded.send_replace(true);
        }
        explorer
    }

    fn new(cache: Arc<MetaCache>, events: Arc<dyn AppEvents>) -> Arc<Self> {
        Arc::new(Explorer {
            cache,
            events,
            index: Arc::new(RwLock::new(SearchIndex::default())),
            loaded: watch::Sender::new(false),
            conns: std::sync::Mutex::new(HashMap::new()),
            attempts: std::sync::Mutex::new(HashMap::new()),
            next_epoch: AtomicU64::new(1),
        })
    }

    fn conns(&self) -> std::sync::MutexGuard<'_, HashMap<String, Conn>> {
        self.conns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn attempts(&self) -> std::sync::MutexGuard<'_, HashMap<String, u64>> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a new connect attempt, superseding any earlier one.
    fn begin_attempt(&self, connection_id: &str) -> u64 {
        let epoch = self.next_epoch.fetch_add(1, Ordering::Relaxed);
        self.attempts().insert(connection_id.to_string(), epoch);
        epoch
    }

    /// True while `epoch` is the latest connect attempt of `connection_id`
    /// (no disconnect, delete or newer connect since).
    fn is_current_attempt(&self, connection_id: &str, epoch: u64) -> bool {
        self.attempts().get(connection_id) == Some(&epoch)
    }

    /// Cancels the connect attempt in progress, if any.
    fn cancel_attempt(&self, connection_id: &str) {
        self.attempts().remove(connection_id);
    }

    pub(crate) fn emit(&self, ev: AppEvent) {
        self.events.emit(ev);
    }

    fn status(&self, connection_id: &str, status: ConnectionStatus, message: Option<String>) {
        self.emit(AppEvent::ExplorerStatus {
            connection_id: connection_id.to_string(),
            status,
            message,
        });
    }

    fn progress(&self, connection_id: &str, database: &str, done: bool, no_access: bool) {
        self.emit(AppEvent::IndexProgress {
            connection_id: connection_id.to_string(),
            database: database.to_string(),
            done,
            no_access,
        });
    }

    /// Waits until the startup index is built, so updates are not
    /// overwritten by it.
    async fn wait_loaded(&self) {
        let mut rx = self.loaded.subscribe();
        let _ = rx.wait_for(|loaded| *loaded).await;
    }

    /// Runs `f` on the search index under the write lock, on a blocking
    /// thread, once the startup index is built.
    async fn update_index(&self, f: impl FnOnce(&mut SearchIndex) + Send + 'static) {
        self.wait_loaded().await;
        let index = self.index.clone();
        let _ =
            blocking(move || f(&mut index.write().unwrap_or_else(PoisonError::into_inner))).await;
    }

    /// Runs `f` on the cache on a blocking thread; `T::default()` if the
    /// thread failed.
    async fn with_cache<T: Default + Send + 'static>(
        &self,
        f: impl FnOnce(&MetaCache) -> T + Send + 'static,
    ) -> T {
        let cache = self.cache.clone();
        blocking(move || f(&cache)).await.unwrap_or_default()
    }

    /// The live slot of a connected Explorer.
    fn slot(&self, connection_id: &str) -> Result<(u64, Arc<AsyncMutex<Slot>>), String> {
        self.conns()
            .get(connection_id)
            .map(|c| (c.epoch, c.slot.clone()))
            .ok_or_else(|| NOT_CONNECTED.to_string())
    }

    /// Removes the entry of `connection_id` (only of `epoch`, if given).
    fn take(&self, connection_id: &str, epoch: Option<u64>) -> Option<Conn> {
        let mut conns = self.conns();
        match conns.get(connection_id) {
            Some(c) if epoch.is_none_or(|e| e == c.epoch) => conns.remove(connection_id),
            _ => None,
        }
    }

    /// Stops a removed entry: wakes its task, waits for it and closes the
    /// session.
    async fn stop(conn: Conn) {
        let Conn {
            slot, task, stop, ..
        } = conn;
        stop.send_replace(true);
        drop(stop);
        if let Some(task) = task {
            let _ = task.await;
        }
        slot.lock().await.close();
    }

    /// The connection was lost (seen with `epoch`): forgets the Explorer and
    /// reports `error`. The caller already closed the slot.
    fn lost(&self, connection_id: &str, epoch: u64, error: &DriverError) {
        // Dropping the entry also stops its task.
        if self.take(connection_id, Some(epoch)).is_some() {
            self.status(
                connection_id,
                ConnectionStatus::Error,
                Some(error.to_string()),
            );
        }
    }

    /// Maps the result of a live call to the command result.
    fn live_result<T>(
        &self,
        connection_id: &str,
        epoch: u64,
        result: Result<T, LiveError>,
    ) -> Result<T, String> {
        match result {
            Ok(v) => Ok(v),
            Err(LiveError::Closed) => Err(NOT_CONNECTED.into()),
            Err(LiveError::Driver(e @ DriverError::Disconnected(_))) => {
                self.lost(connection_id, epoch, &e);
                Err(DISCONNECTED.into())
            }
            Err(LiveError::Driver(e)) => Err(e.to_string()),
        }
    }

    /// Adds `order` to the queue of a connected Explorer (skipping what is
    /// queued already; `front` moves them to the front) and starts the
    /// indexing task if none is running.
    fn enqueue(
        self: &Arc<Self>,
        connection_id: &str,
        order: Vec<String>,
        front: bool,
    ) -> Result<(), String> {
        let mut conns = self.conns();
        let conn = conns
            .get_mut(connection_id)
            .ok_or_else(|| NOT_CONNECTED.to_string())?;
        if front {
            conn.queue.retain(|d| !order.contains(d));
            for db in order.into_iter().rev() {
                conn.queue.push_front(db);
            }
        } else {
            for db in order {
                if !conn.queue.contains(&db) {
                    conn.queue.push_back(db);
                }
            }
        }
        if !conn.running {
            conn.running = true;
            let task = tokio::spawn(index_task(
                self.clone(),
                connection_id.to_string(),
                conn.epoch,
                conn.slot.clone(),
                conn.stop.subscribe(),
            ));
            conn.task = Some(task);
        }
        Ok(())
    }

    fn next_database(&self, connection_id: &str, epoch: u64) -> Next {
        let mut conns = self.conns();
        let Some(conn) = conns.get_mut(connection_id).filter(|c| c.epoch == epoch) else {
            return Next::Gone;
        };
        conn.current = conn.queue.pop_front();
        match &conn.current {
            Some(db) => Next::Index(db.clone()),
            None => {
                conn.running = false;
                Next::Idle
            }
        }
    }

    /// Saves a snapshot to the cache, then to the search index.
    async fn store(&self, connection_id: &str, database: &str, snap: DatabaseSnapshot) {
        let (id, db) = (connection_id.to_string(), database.to_string());
        let objects = self
            .with_cache(move |cache| {
                // A failed cache write still updates the search index.
                let _ = cache.replace_database(&id, &db, &snap, now_ms());
                snap.objects
            })
            .await;
        let (id, db) = (connection_id.to_string(), database.to_string());
        self.update_index(move |index| index.replace_database(&id, &db, &objects))
            .await;
    }

    /// Marks a database the login cannot use and drops what was known of it.
    async fn mark_no_access(&self, connection_id: &str, database: &str) {
        let (id, db) = (connection_id.to_string(), database.to_string());
        self.with_cache(move |cache| cache.mark_no_access(&id, &db))
            .await;
        let (id, db) = (connection_id.to_string(), database.to_string());
        self.update_index(move |index| index.replace_database(&id, &db, &[]))
            .await;
    }

    /// Databases of connected Explorers still to be indexed, plus one while
    /// the startup index is loading.
    fn indexing(&self) -> usize {
        let pending: usize = self
            .conns()
            .values()
            .map(|c| c.queue.len() + usize::from(c.current.is_some()))
            .sum();
        pending + usize::from(!*self.loaded.borrow())
    }

    fn connected(&self) -> HashSet<String> {
        self.conns().keys().cloned().collect()
    }
}

/// Drains the queue of one connection. See the module docs.
async fn index_task(
    explorer: Arc<Explorer>,
    connection_id: String,
    epoch: u64,
    slot: Arc<AsyncMutex<Slot>>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let database = match explorer.next_database(&connection_id, epoch) {
            Next::Index(db) => db,
            Next::Idle => {
                explorer.emit(AppEvent::IndexIdle {
                    connection_id: connection_id.clone(),
                });
                return;
            }
            Next::Gone => return,
        };
        explorer.progress(&connection_id, &database, false, false);
        let fetched = tokio::select! {
            biased;
            // Set or dropped: either way, stop.
            _ = stop.wait_for(|stopped| *stopped) => return,
            r = snapshot(&slot, &database) => r,
        };
        let no_access = match fetched {
            Ok(snap) => {
                explorer.store(&connection_id, &database, snap).await;
                false
            }
            Err(LiveError::Closed) => return,
            Err(LiveError::Driver(e @ DriverError::Disconnected(_))) => {
                explorer.lost(&connection_id, epoch, &e);
                return;
            }
            Err(LiveError::Driver(DriverError::NoAccess(_))) => {
                explorer.mark_no_access(&connection_id, &database).await;
                true
            }
            // Keep what was cached; a later refresh may succeed.
            Err(LiveError::Driver(_)) => false,
        };
        explorer.progress(&connection_id, &database, true, no_access);
    }
}

async fn snapshot(slot: &AsyncMutex<Slot>, database: &str) -> Result<DatabaseSnapshot, LiveError> {
    let mut slot = slot.lock().await;
    let result = match slot.session_for(database).await {
        Ok(session) => session.snapshot(database).await.map_err(LiveError::Driver),
        Err(e) => Err(e),
    };
    slot.check(result)
}

/// The databases of `connection_id` selected in its query tabs.
fn tab_databases(state: &AppState, connection_id: &str) -> HashSet<String> {
    state
        .tabs()
        .open
        .values()
        .filter(|t| t.connection_id == connection_id)
        .map(|t| t.database.clone())
        .collect()
}

/// Indexing order: databases open in a tab, then the rest; alphabetical
/// (case-insensitive) within each group.
fn index_order(state: &AppState, connection_id: &str, databases: &[String]) -> Vec<String> {
    let in_tabs = tab_databases(state, connection_id);
    let mut order = databases.to_vec();
    order.sort_by_cached_key(|d| (!in_tabs.contains(d), d.to_lowercase(), d.clone()));
    order
}

/// Opens the Explorer session of a saved connection and starts indexing its
/// databases in the background. An Explorer already open for it is closed
/// first. Password semantics as `open_session` (`"password_required"`).
/// Emits `explorerStatus` "connecting", then "connected" or "error". If the
/// connection is deleted while connecting, fails with `"not_found"`; if
/// `disconnect_explorer` (or a newer `connect_explorer`) comes in meanwhile,
/// fails with `"closed"`, even if the connect itself failed. In both cases
/// the new session is dropped and this call emits no "connected" or "error".
pub async fn connect_explorer(
    state: &AppState,
    connection_id: String,
    password: Option<String>,
) -> Result<ExplorerInfo, String> {
    let explorer = &state.explorer;
    let conn = state
        .connections
        .lock()
        .await
        .connections
        .iter()
        .find(|c| c.id == connection_id)
        .cloned()
        .ok_or_else(|| "the connection does not exist".to_string())?;
    let password = resolve_password(state, &conn, password).await?;
    let driver = state.driver(&conn.driver)?;
    let epoch = explorer.begin_attempt(&connection_id);
    if let Some(old) = explorer.take(&connection_id, None) {
        Explorer::stop(old).await;
    }

    explorer.status(&connection_id, ConnectionStatus::Connecting, None);
    let params = connect_params(&conn, password, None);
    let opened = async {
        let mut session = driver.connect(&params).await?;
        let server = session.server_info().await?;
        let mut databases = session.list_databases().await?;
        if let DatabaseScope::Selected { names } = &conn.databases {
            databases.retain(|d| names.contains(d));
        }
        let bound = match server.single_database {
            true => Some(session.current_database().await?),
            false => None,
        };
        Ok::<_, DriverError>((session, server, databases, bound))
    }
    .await;
    let (session, server, databases, bound) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            // A disconnect or a newer connect came in meanwhile: its status
            // stands, this attempt reports nothing.
            if !explorer.is_current_attempt(&connection_id, epoch) {
                return Err(CLOSED.into());
            }
            let message = e.to_string();
            explorer.status(
                &connection_id,
                ConnectionStatus::Error,
                Some(message.clone()),
            );
            return Err(message);
        }
    };

    let slot = Slot {
        session: Some(session),
        rebind: server.single_database.then_some((driver, params)),
        bound,
        closed: false,
    };
    let entry = Conn {
        epoch,
        slot: Arc::new(AsyncMutex::new(slot)),
        databases: databases.clone(),
        queue: VecDeque::new(),
        current: None,
        running: false,
        task: None,
        stop: watch::Sender::new(false),
    };

    // Register under the connections lock, so a delete either happens before
    // (and is seen here) or after (and its cleanup sees the Explorer and the
    // cached rows). On failure `entry` is dropped with its session.
    let registered = {
        let config = state.connections.lock().await;
        if !config.connections.iter().any(|c| c.id == connection_id) {
            Err(NOT_FOUND)
        } else {
            let attempts = explorer.attempts();
            if attempts.get(&connection_id) == Some(&epoch) {
                Ok(explorer.conns().insert(connection_id.clone(), entry))
            } else {
                Err(CLOSED)
            }
        }
        .map(|replaced| (config, replaced))
    };
    let (config, replaced) = registered.map_err(String::from)?;
    // The cache keeps exactly the exposed databases. Still under the
    // connections lock, so a delete's cleanup comes after this write.
    let (id, names) = (connection_id.clone(), databases.clone());
    let vanished: Vec<String> = explorer
        .with_cache(move |cache| {
            let before = cache.databases(&id);
            cache.set_databases(&id, &names);
            before
                .into_iter()
                .map(|d| d.name)
                .filter(|d| !names.contains(d))
                .collect()
        })
        .await;
    drop(config);
    // A disconnect or a delete may have taken the entry since it was
    // registered; then its own status stands.
    let still_ours = explorer
        .conns()
        .get(&connection_id)
        .is_some_and(|c| c.epoch == epoch);
    if still_ours {
        explorer.status(&connection_id, ConnectionStatus::Connected, None);
    }
    if let Some(replaced) = replaced {
        Explorer::stop(replaced).await;
    }
    if !still_ours {
        return Err(CLOSED.into());
    }
    if !vanished.is_empty() {
        let id = connection_id.clone();
        explorer
            .update_index(move |index| {
                for db in &vanished {
                    index.replace_database(&id, db, &[]);
                }
            })
            .await;
    }
    let order = index_order(state, &connection_id, &databases);
    // Fails only if a disconnect came in meanwhile, which is fine.
    let _ = explorer.enqueue(&connection_id, order, false);
    Ok(ExplorerInfo { server, databases })
}

/// Stops the Explorer of a connection: indexing stops (what was saved
/// stays) and its session is closed. Emits `explorerStatus` "disconnected".
pub async fn disconnect_explorer(state: &AppState, connection_id: String) -> Result<(), String> {
    state.explorer.cancel_attempt(&connection_id);
    if let Some(conn) = state.explorer.take(&connection_id, None) {
        Explorer::stop(conn).await;
    }
    state
        .explorer
        .status(&connection_id, ConnectionStatus::Disconnected, None);
    Ok(())
}

/// Indexes `database` again (ahead of anything queued), or every database
/// when `None`. Never runs two indexing tasks for one connection: this only
/// queues. Fails with `"not_connected"`, or `"not_found"` for a database
/// the connection does not expose.
pub async fn refresh_metadata(
    state: &AppState,
    connection_id: String,
    database: Option<String>,
) -> Result<(), String> {
    let databases = state
        .explorer
        .conns()
        .get(&connection_id)
        .map(|c| c.databases.clone())
        .ok_or_else(|| NOT_CONNECTED.to_string())?;
    match database {
        Some(db) if !databases.contains(&db) => Err(NOT_FOUND.into()),
        Some(db) => state.explorer.enqueue(&connection_id, vec![db], true),
        None => {
            let order = index_order(state, &connection_id, &databases);
            state.explorer.enqueue(&connection_id, order, false)
        }
    }
}

/// Empties the metadata cache and the search index, then queues a new
/// indexing of every connected Explorer. Disconnected connections are filled
/// again when they connect.
pub async fn clear_metadata_cache(state: &AppState) {
    state.explorer.with_cache(MetaCache::clear).await;
    state
        .explorer
        .update_index(|index| *index = SearchIndex::default())
        .await;
    let connected: Vec<(String, Vec<String>)> = state
        .explorer
        .conns()
        .iter()
        .map(|(id, c)| (id.clone(), c.databases.clone()))
        .collect();
    for (id, databases) in connected {
        let order = index_order(state, &id, &databases);
        // Fails only if a disconnect came in meanwhile, which is fine.
        let _ = state.explorer.enqueue(&id, order, false);
    }
}

/// Cached databases of a connection, by name; works without a connection.
pub async fn explorer_databases(state: &AppState, connection_id: String) -> Vec<CachedDatabase> {
    state
        .explorer
        .with_cache(move |cache| cache.databases(&connection_id))
        .await
}

/// Cached objects of one kind, by schema and name.
pub async fn explorer_objects(
    state: &AppState,
    connection_id: String,
    database: String,
    kind: ObjectKind,
) -> Vec<DbObject> {
    state
        .explorer
        .with_cache(move |cache| cache.objects(&connection_id, &database, kind))
        .await
}

/// Cached columns of a table or view, in ordinal order.
pub async fn explorer_columns(
    state: &AppState,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
) -> Vec<ColumnInfo> {
    state
        .explorer
        .with_cache(move |cache| cache.columns(&connection_id, &database, &schema, &name))
        .await
}

/// Columns, foreign keys and indexes of a table, read live through the
/// Explorer session. Fails with `"not_connected"` without one and with
/// `"disconnected"` when the connection is lost (the Explorer then reports
/// `explorerStatus` "error").
pub async fn table_structure(
    state: &AppState,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
) -> Result<TableStructure, String> {
    let (epoch, slot) = state.explorer.slot(&connection_id)?;
    let result = {
        let mut slot = slot.lock().await;
        let result = match slot.session_for(&database).await {
            Ok(s) => s
                .structure(&database, &schema, &name)
                .await
                .map_err(LiveError::Driver),
            Err(e) => Err(e),
        };
        slot.check(result)
    };
    state.explorer.live_result(&connection_id, epoch, result)
}

/// The source text of a view or procedure, read live; errors as
/// [`table_structure`].
pub async fn object_definition(
    state: &AppState,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
) -> Result<String, String> {
    let (epoch, slot) = state.explorer.slot(&connection_id)?;
    let result = {
        let mut slot = slot.lock().await;
        let result = match slot.session_for(&database).await {
            Ok(s) => s
                .definition(&database, &schema, &name)
                .await
                .map_err(LiveError::Driver),
            Err(e) => Err(e),
        };
        slot.check(result)
    };
    state.explorer.live_result(&connection_id, epoch, result)
}

/// Schemas, objects and column names of one database for autocompletion,
/// from the cache.
pub async fn completion_metadata(
    state: &AppState,
    connection_id: String,
    database: String,
) -> CompletionMetadata {
    state
        .explorer
        .with_cache(move |cache| {
            let mut objects = Vec::new();
            for kind in [ObjectKind::Table, ObjectKind::View, ObjectKind::Procedure] {
                objects.extend(cache.objects(&connection_id, &database, kind));
            }
            let mut schemas: Vec<String> = objects.iter().map(|o| o.schema.clone()).collect();
            schemas.sort();
            schemas.dedup();
            let columns = cache
                .column_names(&connection_id, &database)
                .into_iter()
                .map(|(schema, name, columns)| (format!("{schema}.{name}"), columns))
                .collect();
            CompletionMetadata {
                schemas,
                objects,
                columns,
            }
        })
        .await
}

/// Fuzzy search over every cached object of every connection, for the
/// palette (up to `limit` hits).
///
/// A query `"Folder/…/text"` searches `text` only in the connections inside
/// the folders whose path *ends with* the part before the last `/`
/// (case-insensitive, segment by segment) and in their subfolders. So
/// `"Proyecto X/ped"` and `"Cliente A/Proyecto X/ped"` both work. When no
/// folder matches, the whole query is searched, slashes included.
///
/// Saved queries are matched by name with the same matcher, with their own
/// limit of 20; a folder-scoped query finds none (they belong to a
/// database, not to a connection).
pub async fn search_objects(state: &AppState, query: String, limit: usize) -> SearchResponse {
    let config = {
        let c = state.connections.lock().await;
        AppConfig {
            connections: c.connections.clone(),
            folders: c.folders.clone(),
            ..AppConfig::default()
        }
    };
    let tree = FolderTree::new(&config.folders);
    let (text, scope) = match folder_scope(&query, &tree, &config) {
        Some((text, ids)) => (text.to_string(), Some(ids)),
        None => (query.clone(), None),
    };

    let index = state.explorer.index.clone();
    let saved = state.queries.clone();
    let (results, queries) = blocking(move || {
        let queries = match scope {
            Some(_) => Vec::new(),
            None => saved.search(&text),
        };
        let scope = SearchScope {
            connection_ids: scope,
        };
        // The read lock covers the search only.
        let index = index.read().unwrap_or_else(PoisonError::into_inner);
        (index.search(&text, &scope, limit), queries)
    })
    .await
    .unwrap_or_default();

    let connected = state.explorer.connected();
    let hits = results
        .hits
        .into_iter()
        .filter_map(|h| {
            let conn = config
                .connections
                .iter()
                .find(|c| c.id == h.connection_id)?;
            Some(PaletteHit {
                connected: connected.contains(&h.connection_id),
                connection_name: conn.name.clone(),
                folder_path: tree.path(conn.folder_id.as_deref()),
                connection_id: h.connection_id,
                database: h.database,
                object: h.object,
                indices: h.indices,
            })
        })
        .collect();
    SearchResponse {
        hits,
        total_matches: results.total_matches,
        databases_matched: results.databases_matched,
        indexing: state.explorer.indexing(),
        queries,
    }
}

/// Folder lookups bounded by the folder count, so a corrupt parent loop in
/// the config cannot spin.
struct FolderTree<'a> {
    by_id: HashMap<&'a str, &'a Folder>,
}

impl<'a> FolderTree<'a> {
    fn new(folders: &'a [Folder]) -> Self {
        FolderTree {
            by_id: folders.iter().map(|f| (f.id.as_str(), f)).collect(),
        }
    }

    /// `folder` and its ancestors, innermost first.
    fn ancestors(&self, folder: Option<&'a str>) -> Vec<&'a Folder> {
        let mut out = Vec::new();
        let mut cursor = folder;
        while let Some(id) = cursor {
            let Some(f) = self.by_id.get(id) else { break };
            if out.len() > self.by_id.len() {
                break;
            }
            out.push(*f);
            cursor = f.parent_id.as_deref();
        }
        out
    }

    /// Folder names from the root down to `folder`.
    fn path(&self, folder: Option<&'a str>) -> Vec<String> {
        let mut names: Vec<String> = self
            .ancestors(folder)
            .into_iter()
            .map(|f| f.name.clone())
            .collect();
        names.reverse();
        names
    }
}

/// For `"A/B/text"`: `text` and the ids of the connections in the folders
/// whose path ends with `A/B`, and below. `None` when the query has no `/`
/// or no folder matches.
fn folder_scope<'q>(
    query: &'q str,
    tree: &FolderTree<'_>,
    config: &AppConfig,
) -> Option<(&'q str, HashSet<String>)> {
    let (prefix, text) = query.rsplit_once('/')?;
    let wanted: Vec<String> = prefix.split('/').map(|s| s.trim().to_lowercase()).collect();
    if wanted.iter().any(String::is_empty) {
        return None;
    }
    let matched: HashSet<&str> = config
        .folders
        .iter()
        .filter(|f| {
            let path = tree.ancestors(Some(&f.id));
            // `path` is innermost first; compare from the end of `wanted`.
            path.len() >= wanted.len()
                && wanted
                    .iter()
                    .rev()
                    .zip(&path)
                    .all(|(w, f)| f.name.trim().to_lowercase() == *w)
        })
        .map(|f| f.id.as_str())
        .collect();
    if matched.is_empty() {
        return None;
    }
    let ids = config
        .connections
        .iter()
        .filter(|c| {
            tree.ancestors(c.folder_id.as_deref())
                .iter()
                .any(|f| matched.contains(f.id.as_str()))
        })
        .map(|c| c.id.clone())
        .collect();
    Some((text, ids))
}

/// Forgets everything of a deleted connection: stops its Explorer and drops
/// its cache and search entries.
pub(crate) async fn forget_connection(state: &AppState, connection_id: &str) {
    let explorer = &state.explorer;
    explorer.cancel_attempt(connection_id);
    if let Some(conn) = explorer.take(connection_id, None) {
        Explorer::stop(conn).await;
    }
    let id = connection_id.to_string();
    explorer
        .with_cache(move |cache| cache.forget_connection(&id))
        .await;
    let id = connection_id.to_string();
    explorer
        .update_index(move |index| index.remove_connection(&id))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, parent: Option<&str>) -> Folder {
        Folder {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(String::from),
        }
    }

    #[test]
    fn folder_paths_are_bounded_on_parent_loops() {
        let folders = vec![
            folder("a", "A", Some("b")),
            folder("b", "B", Some("a")),
            folder("c", "C", None),
        ];
        let tree = FolderTree::new(&folders);
        assert!(tree.path(Some("a")).len() <= folders.len() + 1);
        assert_eq!(tree.path(Some("c")), ["C"]);
        assert!(tree.path(None).is_empty());
        assert!(tree.path(Some("missing")).is_empty());
    }

    #[test]
    fn empty_folder_segments_search_the_whole_text() {
        let config = AppConfig {
            folders: vec![folder("a", "A", None)],
            ..AppConfig::default()
        };
        let tree = FolderTree::new(&config.folders);
        assert!(folder_scope("/x", &tree, &config).is_none());
        assert!(folder_scope("A//x", &tree, &config).is_none());
        assert!(folder_scope("x", &tree, &config).is_none());
        let (text, ids) = folder_scope(" a /x", &tree, &config).unwrap();
        assert_eq!(text, "x");
        assert!(ids.is_empty());
    }
}
