//! The services behind the MCP server: what an AI assistant may see and do.
//!
//! Every call reads `config.json` again, so a change of "AI access" made in
//! the app applies to the next call. Only connections with read-only AI
//! access exist for the assistant: it names them by a label, and one without
//! access answers exactly like one that does not exist. Each call that
//! touches a server opens its own session, reported to the server as
//! [`AI_APPLICATION_NAME`], and closes it when done. At most four calls talk
//! to servers at once. The assistant never sees a password and is never
//! asked for one.

mod format;
mod query;
mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{Semaphore, SemaphorePermit};
use tulka_core::{
    AiAccess, ConfigStore, ConnectParams, ConnectionConfig, DatabaseScope, Driver, Folder,
    KeyringStore, Preferences, SecretStore, AI_APPLICATION_NAME,
};
use tulka_mssql::MssqlDriver;
use tulka_store::{MetaCache, SharedRunLog};

use crate::connections::{blocking, connect_params, saved_password};
use crate::types::PASSWORD_REQUIRED;

pub use query::{run_query, QueryRequest};
pub use tools::{describe_table, list_connections, list_databases, search_objects, SearchRequest};

/// Longest text of a table cell given to the assistant, in characters.
pub const AI_CELL_CHARS: usize = 2000;
/// Longest text one tool answers with, in characters.
pub const AI_MAX_OUTPUT_CHARS: usize = 100_000;
/// Matches `search_objects` returns when the assistant gives no limit.
pub const AI_SEARCH_LIMIT_DEFAULT: u32 = 50;
/// Most matches `search_objects` returns.
pub const AI_SEARCH_LIMIT_MAX: u32 = 200;

/// Calls that may talk to servers at the same time.
const MAX_SERVER_CALLS: usize = 4;
/// Longest client name kept, in characters.
const CLIENT_NAME_CHARS: usize = 100;

/// What a tool answers: text for the model, and whether it is an error the
/// model should fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolText {
    pub text: String,
    pub is_error: bool,
}

impl ToolText {
    pub(crate) fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// State behind the tools.
pub struct AiState {
    config: ConfigStore,
    /// Why the config location is unknown, when it is.
    config_error: Option<String>,
    secrets: Arc<dyn SecretStore>,
    drivers: HashMap<&'static str, Arc<dyn Driver>>,
    /// The app's metadata cache, if it exists.
    cache: Mutex<Option<Arc<MetaCache>>>,
    /// Where to look for the cache while it does not exist yet (the app
    /// creates it the first time it runs).
    cache_path: Option<PathBuf>,
    /// Where queries are recorded.
    runlog: Arc<SharedRunLog>,
    slots: Semaphore,
    /// The MCP client's name, recorded with each run.
    client: Mutex<Option<String>>,
}

impl AiState {
    pub fn new(
        config: ConfigStore,
        secrets: Arc<dyn SecretStore>,
        drivers: Vec<Arc<dyn Driver>>,
        cache: Option<Arc<MetaCache>>,
        runlog: Arc<SharedRunLog>,
    ) -> Self {
        Self {
            config,
            config_error: None,
            secrets,
            drivers: drivers.into_iter().map(|d| (d.id(), d)).collect(),
            cache: Mutex::new(cache),
            cache_path: None,
            runlog,
            slots: Semaphore::new(MAX_SERVER_CALLS),
            client: Mutex::new(None),
        }
    }

    /// The per-user config, the OS keychain, the SQL Server driver, the app's
    /// metadata cache (when it exists) and the run log. Never fails, and
    /// never creates or changes the config file or the cache.
    pub fn open_default() -> Self {
        let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore::new());
        let drivers: Vec<Arc<dyn Driver>> = vec![Arc::new(MssqlDriver)];
        let cache_path = MetaCache::default_path();
        let cache = cache_path
            .as_deref()
            .and_then(MetaCache::open_existing)
            .map(Arc::new);
        let runlog = Arc::new(SharedRunLog::open_default());
        let state = match ConfigStore::open_default() {
            Ok(config) => Self::new(config, secrets, drivers, cache, runlog),
            Err(e) => Self {
                config_error: Some(e.to_string()),
                ..Self::new(ConfigStore::at(""), secrets, drivers, cache, runlog)
            },
        };
        Self {
            cache_path,
            ..state
        }
    }

    /// The MCP client's name, recorded with each run (cut to 100 characters).
    pub fn set_client(&self, name: Option<String>) {
        let name = name
            .map(|n| n.trim().chars().take(CLIENT_NAME_CHARS).collect::<String>())
            .filter(|n| !n.is_empty());
        *self.client.lock().unwrap_or_else(PoisonError::into_inner) = name;
    }

    /// The connections the assistant may use, read from the config file now.
    async fn visible(&self) -> Result<Vec<Visible>, ToolText> {
        self.load().await.map(|(visible, _)| visible)
    }

    /// Like [`Self::visible`], with the preferences of the same read.
    async fn load(&self) -> Result<(Vec<Visible>, Preferences), ToolText> {
        if let Some(e) = &self.config_error {
            return Err(ToolText::error(format::config_unreadable(e)));
        }
        let store = self.config.clone();
        let config = blocking(move || store.load())
            .await
            .map_err(|e| ToolText::error(format::config_unreadable(&e)))?
            .map_err(|e| ToolText::error(format::config_unreadable(&e.to_string())))?;
        let connections: Vec<&ConnectionConfig> = config
            .connections
            .iter()
            .filter(|c| c.ai_access == AiAccess::ReadOnly)
            .collect();
        let lower = |s: &str| s.to_lowercase();

        // The plain name when it is unique; else the folders plus the name;
        // else, if even that repeats, the id's first characters too.
        let mut labels: Vec<String> = connections
            .iter()
            .map(|c| {
                let same = connections
                    .iter()
                    .filter(|o| lower(&o.name) == lower(&c.name))
                    .count();
                match (same, folder_path(&config.folders, c.folder_id.as_deref())) {
                    (1, _) | (_, None) => c.name.clone(),
                    (_, Some(path)) => format!("{path}/{}", c.name),
                }
            })
            .collect();
        let counts: Vec<usize> = labels
            .iter()
            .map(|l| labels.iter().filter(|o| lower(o) == lower(l)).count())
            .collect();
        for ((label, count), c) in labels.iter_mut().zip(counts).zip(&connections) {
            if count > 1 {
                let short: String = c.id.chars().take(8).collect();
                label.push_str(&format!(" #{short}"));
            }
        }
        let visible = connections
            .into_iter()
            .zip(labels)
            .map(|(conn, label)| Visible {
                label,
                conn: conn.clone(),
            })
            .collect();
        Ok((visible, config.preferences.clone()))
    }

    /// One of the connections of `visible` by its label (any letter case).
    fn find<'a>(visible: &'a [Visible], name: &str) -> Result<&'a Visible, ToolText> {
        let wanted = name.trim().to_lowercase();
        visible
            .iter()
            .find(|v| v.label.to_lowercase() == wanted)
            .ok_or_else(|| ToolText::error(format::not_available(name.trim())))
    }

    /// The driver of `v`'s connection.
    fn driver_for(&self, v: &Visible) -> Result<Arc<dyn Driver>, ToolText> {
        self.drivers
            .get(v.conn.driver.as_str())
            .cloned()
            .ok_or_else(|| ToolText::error(format!("unknown driver `{}`", v.conn.driver)))
    }

    /// The driver and the login parameters for `v`, with the saved password.
    async fn prepare(
        &self,
        v: &Visible,
        database: Option<String>,
    ) -> Result<(Arc<dyn Driver>, ConnectParams), ToolText> {
        let driver = self.driver_for(v)?;
        let password = match saved_password(self.secrets.clone(), &v.conn).await {
            Ok(password) => password,
            Err(e) if e == PASSWORD_REQUIRED => {
                return Err(ToolText::error(format::password_not_saved(&v.label)))
            }
            Err(e) => return Err(ToolText::error(e)),
        };
        let mut params = connect_params(&v.conn, password, database);
        params.application_name = AI_APPLICATION_NAME.to_string();
        Ok((driver, params))
    }

    /// The MCP client's name, as set by [`Self::set_client`].
    fn client(&self) -> Option<String> {
        self.client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Waits for one of the slots that limit how many calls use servers.
    async fn slot(&self) -> SemaphorePermit<'_> {
        self.slots
            .acquire()
            .await
            .expect("the semaphore is never closed")
    }

    /// The cache, opened now if the app created it after this process began.
    async fn current_cache(&self) -> Option<Arc<MetaCache>> {
        let known = self
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if known.is_some() {
            return known;
        }
        let path = self.cache_path.clone()?;
        let opened = blocking(move || MetaCache::open_existing(&path))
            .await
            .ok()
            .flatten()
            .map(Arc::new)?;
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(opened.clone());
        Some(opened)
    }

    /// Runs `f` on the metadata cache, off the async runtime. `None` without
    /// a cache.
    async fn with_cache<T: Send + 'static>(
        &self,
        f: impl FnOnce(&MetaCache) -> T + Send + 'static,
    ) -> Option<T> {
        let cache = self.current_cache().await?;
        blocking(move || f(&cache)).await.ok()
    }
}

/// A connection the assistant may use, with the name it uses for it.
pub(crate) struct Visible {
    pub label: String,
    pub conn: ConnectionConfig,
}

impl Visible {
    /// The database to use: the one asked for (which must be among the
    /// selected ones, if the connection selects some), else the first
    /// selected one. `None` means the login's own database.
    fn database(&self, requested: Option<&str>) -> Result<Option<String>, ToolText> {
        let requested = requested.map(str::trim).filter(|d| !d.is_empty());
        match &self.conn.databases {
            DatabaseScope::All => Ok(requested.map(str::to_string)),
            DatabaseScope::Selected { names } if names.is_empty() => {
                Err(ToolText::error(format::no_databases_selected(&self.label)))
            }
            DatabaseScope::Selected { names } => match requested {
                None => Ok(names.first().cloned()),
                Some(wanted) => names
                    .iter()
                    .find(|n| n.to_lowercase() == wanted.to_lowercase())
                    .cloned()
                    .map(Some)
                    .ok_or_else(|| {
                        ToolText::error(format::database_not_available(wanted, &self.label, names))
                    }),
            },
        }
    }
}

/// `Parent/Child` for a folder, `None` for the root (or a broken chain).
fn folder_path(folders: &[Folder], folder_id: Option<&str>) -> Option<String> {
    let mut names = Vec::new();
    let mut next = folder_id;
    while let Some(id) = next {
        if names.len() > folders.len() {
            return None;
        }
        let folder = folders.iter().find(|f| f.id == id)?;
        names.push(folder.name.clone());
        next = folder.parent_id.as_deref();
    }
    if names.is_empty() {
        return None;
    }
    names.reverse();
    Some(names.join("/"))
}
