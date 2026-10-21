//! Application state shared by every command.
//!
//! # Concurrency
//!
//! `tabs` is a single async mutex, and no command holds it while talking to
//! a server. A command that needs the session (`run_query`, `use_database`)
//! *checks it out*: it takes the `Box<dyn Session>` out of the tab under the
//! lock, releases the lock, does its I/O on the owned session and checks it
//! back in under the lock afterwards. While it is checked out the tab is
//! "busy" (`session == None && !disconnected`) and a second run or database
//! switch on the same tab is refused instead of waiting.
//!
//! During a run, `cancel` holds the session's [`CancelHandle`] (an `Arc`), so
//! `cancel_query` clones it under the lock and cancels after releasing it.
//! Rows streamed by the driver are pushed into the tab's [`ResultBuffer`]s in
//! one short critical section per chunk (at most 500 rows), so `fetch_rows`
//! and `copy_results` read the rows already received while the run goes on.
//!
//! `close_session` removes the tab and cancels a running query; `open_session`
//! replaces the tab (cancelling a run on the old one). Each tab carries a
//! `generation`, unique per `open_session`, and a run only writes rows to, and
//! checks its session back into, the tab with its own generation. A run whose
//! tab was closed or replaced keeps draining the driver until it ends (a
//! cancelled request must be read to its end) and then drops its session,
//! which closes that connection.
//!
//! `connections` is a separate async mutex held across the keychain and
//! config-file writes of `save_connection` / `delete_connection`, so those
//! read-modify-write cycles are serialized. Keychain and file I/O run on
//! `spawn_blocking`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use tulka_core::{
    AppConfig, CancelHandle, ConfigStore, Driver, KeyringStore, SecretStore, Session,
};
use tulka_mssql::MssqlDriver;

use crate::results::ResultBuffer;

/// State behind every command.
pub struct AppState {
    pub(crate) config: ConfigStore,
    pub(crate) connections: Mutex<AppConfig>,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) drivers: HashMap<&'static str, Arc<dyn Driver>>,
    pub(crate) tabs: Mutex<HashMap<String, TabState>>,
    next_generation: AtomicU64,
}

/// What a query tab holds between commands.
pub struct TabState {
    /// Unique per `open_session`; see the module docs.
    pub generation: u64,
    pub connection_id: String,
    /// `None` while checked out by a running command, or after a disconnect.
    pub session: Option<Box<dyn Session>>,
    /// The running query's cancel handle; `Some` only during `run_query`.
    pub cancel: Option<Arc<dyn CancelHandle>>,
    /// One buffer per result index of the last run.
    pub results: Vec<ResultBuffer>,
    /// The connection was lost; commands fail with `"disconnected"` until
    /// `open_session` replaces the tab. Never reconnects on its own.
    pub disconnected: bool,
}

impl AppState {
    /// State over `config`, loading the saved connections now.
    pub fn new(
        config: ConfigStore,
        secrets: Arc<dyn SecretStore>,
        drivers: Vec<Arc<dyn Driver>>,
    ) -> Result<Self, String> {
        let loaded = config.load().map_err(|e| e.to_string())?;
        Ok(Self {
            config,
            connections: Mutex::new(loaded),
            secrets,
            drivers: drivers.into_iter().map(|d| (d.id(), d)).collect(),
            tabs: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
        })
    }

    /// The production state: the per-user config file, the OS keychain and
    /// the SQL Server driver.
    pub fn open_default() -> Result<Self, String> {
        let config = ConfigStore::open_default().map_err(|e| e.to_string())?;
        Self::new(
            config,
            Arc::new(KeyringStore::new()),
            vec![Arc::new(MssqlDriver)],
        )
    }

    pub(crate) fn driver(&self, id: &str) -> Result<Arc<dyn Driver>, String> {
        self.drivers
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown driver `{id}`"))
    }

    pub(crate) fn new_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}
