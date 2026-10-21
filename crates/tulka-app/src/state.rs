//! Application state shared by every command.
//!
//! # Concurrency
//!
//! `tabs` is a plain `std::sync::Mutex` that is only held for short,
//! non-async critical sections: no command holds it across an `.await`, let
//! alone while talking to a server. A command that needs the session
//! (`run_query`, `use_database`) *checks it out*: it takes the
//! `Box<dyn Session>` out of the tab under the lock, releases the lock, does
//! its I/O on the owned session and checks it back in afterwards. While it is
//! checked out the tab is "busy" (`session == None && !disconnected`) and a
//! second run or database switch on the same tab fails with `"busy"` instead
//! of waiting. The checkout is a drop guard: if the command panics (driver or
//! event sink) or its future is dropped before the check-in, the guard marks
//! the tab disconnected (the session is gone with the command), so the UI
//! gets the `"disconnected"` sentinel and reconnects instead of staying busy.
//!
//! During a run, `cancel` holds the session's [`CancelHandle`] (an `Arc`), so
//! `cancel_query` clones it under the lock and cancels after releasing it.
//! Each result buffer sits behind its own `Arc<tokio::sync::RwLock<_>>`: the
//! tabs lock is held only to clone that `Arc`, then rows are appended (one
//! chunk of at most 500 rows per write), read by `fetch_rows`, or formatted by
//! `copy_results` on a blocking thread, under the buffer's own lock. So
//! `fetch_rows` and `copy_results` see the rows already received while the
//! run goes on, and a large copy never blocks other tabs or the runtime.
//!
//! `close_session` removes the tab, records when it was closed and cancels a
//! running query; `open_session` replaces the tab (cancelling a run on the old
//! one). Generations come from one increasing counter: each `open_session`
//! takes one when it starts (it becomes the tab's `generation`) and each
//! `close_session` takes one. An `open_session` whose tab was closed, or
//! opened by a newer `open_session`, while it was connecting drops its new
//! session and fails with `"closed"` instead of resurrecting the tab. A run
//! only writes rows to, and checks its session back into, the tab with its own
//! generation. A run whose tab was closed or replaced keeps draining the driver
//! until it ends (a cancelled request must be read to its end) and then drops
//! its session, which closes that connection.
//!
//! `connections` is a separate async mutex held across the keychain and
//! config-file writes of `save_connection` / `delete_connection`, so those
//! read-modify-write cycles are serialized. Keychain and file I/O run on
//! `spawn_blocking`. The two mutexes are never held together.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard, PoisonError};

use tokio::sync::{Mutex, RwLock};
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
    /// Set when the config file could not be read at startup: the state is
    /// read-only (nothing is ever written to the file) and this is shown to
    /// the user.
    startup_error: Option<String>,
    tabs: std::sync::Mutex<Tabs>,
    next_generation: AtomicU64,
}

/// A result buffer shared between the run that fills it and the commands
/// that read it.
pub type SharedBuffer = Arc<RwLock<ResultBuffer>>;

/// The query tabs, guarded by [`AppState::tabs`].
#[derive(Default)]
pub(crate) struct Tabs {
    pub open: HashMap<String, TabState>,
    /// Generation taken by the last `close_session` of each tab id.
    pub closed_at: HashMap<String, u64>,
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
    pub results: Vec<SharedBuffer>,
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
            startup_error: None,
            tabs: std::sync::Mutex::new(Tabs::default()),
            next_generation: AtomicU64::new(1),
        })
    }

    /// State with an empty in-memory config that never touches the config
    /// file: `save_connection` / `delete_connection` fail with
    /// "the connections file is read-only: {error}". Used when the file cannot
    /// be read, so the app still opens and the file is never overwritten.
    pub fn new_read_only(
        error: String,
        secrets: Arc<dyn SecretStore>,
        drivers: Vec<Arc<dyn Driver>>,
    ) -> Self {
        Self {
            config: ConfigStore::at(""),
            connections: Mutex::new(AppConfig::default()),
            secrets,
            drivers: drivers.into_iter().map(|d| (d.id(), d)).collect(),
            startup_error: Some(error),
            tabs: std::sync::Mutex::new(Tabs::default()),
            next_generation: AtomicU64::new(1),
        }
    }

    /// Why the config file could not be read at startup, if it could not.
    pub fn startup_error(&self) -> Option<String> {
        self.startup_error.clone()
    }

    /// State over `config`; an unreadable file gives a read-only state
    /// (see [`AppState::new_read_only`]) instead of an error.
    pub fn open(
        config: ConfigStore,
        secrets: Arc<dyn SecretStore>,
        drivers: Vec<Arc<dyn Driver>>,
    ) -> Self {
        match Self::new(config, secrets.clone(), drivers.clone()) {
            Ok(state) => state,
            Err(e) => Self::new_read_only(e, secrets, drivers),
        }
    }

    /// The production state: the per-user config file, the OS keychain and
    /// the SQL Server driver. Never fails.
    pub fn open_default() -> Self {
        let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore::new());
        let drivers: Vec<Arc<dyn Driver>> = vec![Arc::new(MssqlDriver)];
        match ConfigStore::open_default() {
            Ok(config) => Self::open(config, secrets, drivers),
            Err(e) => Self::new_read_only(e.to_string(), secrets, drivers),
        }
    }

    /// Fails when the state is read-only.
    pub(crate) fn ensure_writable(&self) -> Result<(), String> {
        match &self.startup_error {
            Some(e) => Err(format!("the connections file is read-only: {e}")),
            None => Ok(()),
        }
    }

    pub(crate) fn driver(&self, id: &str) -> Result<Arc<dyn Driver>, String> {
        self.drivers
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown driver `{id}`"))
    }

    /// Locks the tabs. Never hold the guard across an `.await`. A panic
    /// while it was held does not leave the tabs unusable.
    pub(crate) fn tabs(&self) -> MutexGuard<'_, Tabs> {
        self.tabs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn new_generation(&self) -> u64 {
        self.next_generation.fetch_add(1, Ordering::Relaxed)
    }
}
