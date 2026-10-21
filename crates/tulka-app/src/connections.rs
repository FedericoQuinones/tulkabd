//! Saved connections and their passwords.

use std::sync::Arc;

use tulka_core::{
    AppConfig, ConnectParams, ConnectionConfig, Folder, SecretError, SecretStore, ServerInfo,
};

use crate::explorer;
use crate::query::close_session;
use crate::state::AppState;
use crate::types::{SaveConnectionResult, CYCLE, EMPTY_NAME, NOT_FOUND, PASSWORD_REQUIRED};

/// Runs blocking work (keychain, config file) off the async runtime.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| format!("internal error: {e}"))
}

/// Writes `config` to disk; the caller replaces its in-memory copy only on
/// success.
async fn persist(state: &AppState, config: AppConfig) -> Result<AppConfig, String> {
    let store = state.config.clone();
    blocking(move || store.save(&config).map(|()| config))
        .await?
        .map_err(|e| e.to_string())
}

/// The saved connections.
pub async fn list_connections(state: &AppState) -> Vec<ConnectionConfig> {
    state.connections.lock().await.connections.clone()
}

/// Creates or updates a connection (matched by id).
///
/// With a password, stores it in the keychain and sets `password_saved`; if
/// there is no usable keychain the connection is still saved, with
/// `password_saved = false` and `keyring_unavailable = true`. Without one,
/// an existing connection keeps its stored secret and `password_saved`
/// (rename or edit without retyping); a new one gets `password_saved = false`.
pub async fn save_connection(
    state: &AppState,
    mut conn: ConnectionConfig,
    password: Option<String>,
) -> Result<SaveConnectionResult, String> {
    state.ensure_writable()?;
    if conn.id.trim().is_empty() {
        return Err("the connection has no id".into());
    }
    let mut current = state.connections.lock().await;
    let existing = current.connections.iter().position(|c| c.id == conn.id);

    let mut keyring_unavailable = false;
    match password {
        Some(password) => {
            let secrets: Arc<dyn SecretStore> = state.secrets.clone();
            let id = conn.id.clone();
            match blocking(move || secrets.set(&id, &password)).await? {
                Ok(()) => conn.password_saved = true,
                Err(SecretError::Unavailable(_)) => {
                    conn.password_saved = false;
                    keyring_unavailable = true;
                }
                Err(e) => return Err(format!("could not store the password: {e}")),
            }
        }
        None => {
            conn.password_saved = existing.is_some_and(|i| current.connections[i].password_saved);
        }
    }

    // The folder is changed only by `move_connection`: an edit keeps the
    // stored one, and a new connection must name an existing folder.
    conn.folder_id = match existing {
        Some(i) => current.connections[i].folder_id.clone(),
        None => conn.folder_id.take().filter(|f| has_folder(&current, f)),
    };

    let mut next = current.clone();
    match existing {
        Some(i) => next.connections[i] = conn.clone(),
        None => next.connections.push(conn.clone()),
    }
    *current = persist(state, next).await?;
    Ok(SaveConnectionResult {
        connection: conn,
        keyring_unavailable,
    })
}

/// Deletes a connection and its stored password. A missing keychain does
/// not prevent the deletion. Then closes its query tabs and its Explorer and
/// forgets its cached metadata.
pub async fn delete_connection(state: &AppState, id: String) -> Result<(), String> {
    state.ensure_writable()?;
    {
        let mut current = state.connections.lock().await;
        let secrets = state.secrets.clone();
        let key = id.clone();
        match blocking(move || secrets.delete(&key)).await? {
            Ok(()) | Err(SecretError::Unavailable(_)) => {}
            Err(e) => return Err(format!("could not delete the password: {e}")),
        }
        let mut next = current.clone();
        next.connections.retain(|c| c.id != id);
        *current = persist(state, next).await?;
    }
    let tabs: Vec<String> = state
        .tabs()
        .open
        .iter()
        .filter(|(_, t)| t.connection_id == id)
        .map(|(tab_id, _)| tab_id.clone())
        .collect();
    for tab_id in tabs {
        close_session(state, tab_id).await?;
    }
    explorer::forget_connection(state, &id).await;
    Ok(())
}

/// Applies `f` to a copy of the config and persists it; the in-memory config
/// is replaced only when both succeed.
async fn mutate<T>(
    state: &AppState,
    f: impl FnOnce(&mut AppConfig) -> Result<T, String>,
) -> Result<T, String> {
    state.ensure_writable()?;
    let mut current = state.connections.lock().await;
    let mut next = current.clone();
    let out = f(&mut next)?;
    *current = persist(state, next).await?;
    Ok(out)
}

fn has_folder(config: &AppConfig, id: &str) -> bool {
    config.folders.iter().any(|f| f.id == id)
}

/// Checks that `parent` (when given) is an existing folder.
fn check_parent(config: &AppConfig, parent: &Option<String>) -> Result<(), String> {
    match parent {
        Some(p) if !has_folder(config, p) => Err(NOT_FOUND.into()),
        _ => Ok(()),
    }
}

fn clean_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        Err(EMPTY_NAME.into())
    } else {
        Ok(name.into())
    }
}

/// The folders of the connections tree.
pub async fn list_folders(state: &AppState) -> Vec<Folder> {
    state.connections.lock().await.folders.clone()
}

/// Creates a folder under `parent_id` (the root when `None`). Fails with
/// [`EMPTY_NAME`] or [`NOT_FOUND`] (unknown parent).
pub async fn create_folder(
    state: &AppState,
    name: String,
    parent_id: Option<String>,
) -> Result<Folder, String> {
    mutate(state, |config| {
        let name = clean_name(&name)?;
        check_parent(config, &parent_id)?;
        let folder = Folder {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            parent_id,
        };
        config.folders.push(folder.clone());
        Ok(folder)
    })
    .await
}

/// Renames a folder. Fails with [`EMPTY_NAME`] or [`NOT_FOUND`].
pub async fn rename_folder(state: &AppState, id: String, name: String) -> Result<(), String> {
    mutate(state, |config| {
        let name = clean_name(&name)?;
        let folder = config
            .folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or(NOT_FOUND)?;
        folder.name = name;
        Ok(())
    })
    .await
}

/// Deletes a folder; its child folders and connections move to its parent
/// (or the root). Fails with [`NOT_FOUND`].
pub async fn delete_folder(state: &AppState, id: String) -> Result<(), String> {
    mutate(state, |config| {
        let parent = config
            .folders
            .iter()
            .find(|f| f.id == id)
            .ok_or(NOT_FOUND)?
            .parent_id
            .clone();
        config.folders.retain(|f| f.id != id);
        for f in config
            .folders
            .iter_mut()
            .filter(|f| f.parent_id.as_ref() == Some(&id))
        {
            f.parent_id = parent.clone();
        }
        for c in config
            .connections
            .iter_mut()
            .filter(|c| c.folder_id.as_ref() == Some(&id))
        {
            c.folder_id = parent.clone();
        }
        config.expanded.retain(|e| *e != id);
        Ok(())
    })
    .await
}

/// Moves a connection into a folder (the root when `None`). Fails with
/// [`NOT_FOUND`].
pub async fn move_connection(
    state: &AppState,
    connection_id: String,
    folder_id: Option<String>,
) -> Result<(), String> {
    mutate(state, |config| {
        check_parent(config, &folder_id)?;
        let conn = config
            .connections
            .iter_mut()
            .find(|c| c.id == connection_id)
            .ok_or(NOT_FOUND)?;
        conn.folder_id = folder_id;
        Ok(())
    })
    .await
}

/// Moves a folder under `parent_id` (the root when `None`). Fails with
/// [`NOT_FOUND`], or [`CYCLE`] when the target is the folder itself or one of
/// its descendants.
pub async fn move_folder(
    state: &AppState,
    id: String,
    parent_id: Option<String>,
) -> Result<(), String> {
    mutate(state, |config| {
        if !has_folder(config, &id) {
            return Err(NOT_FOUND.into());
        }
        check_parent(config, &parent_id)?;
        // Walk up from the target; meeting `id` means it is a descendant.
        // Bounded by the folder count so a corrupt parent loop cannot spin.
        let mut cursor = parent_id.clone();
        let mut steps = 0;
        while let Some(current) = cursor {
            steps += 1;
            if current == id || steps > config.folders.len() {
                return Err(CYCLE.into());
            }
            cursor = config
                .folders
                .iter()
                .find(|f| f.id == current)
                .and_then(|f| f.parent_id.clone());
        }
        let folder = config
            .folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or(NOT_FOUND)?;
        folder.parent_id = parent_id;
        Ok(())
    })
    .await
}

/// The tree nodes left open, as stored by [`set_expanded`].
pub async fn list_expanded(state: &AppState) -> Vec<String> {
    state.connections.lock().await.expanded.clone()
}

/// Stores which tree nodes are open. The ids are the UI's; duplicates are
/// dropped.
pub async fn set_expanded(state: &AppState, ids: Vec<String>) -> Result<(), String> {
    mutate(state, |config| {
        let mut seen = std::collections::HashSet::new();
        config.expanded = ids.into_iter().filter(|i| seen.insert(i.clone())).collect();
        Ok(())
    })
    .await
}

/// Connects with `conn` just to list the server's databases (for the
/// "Selected" picker) and disconnects; no tab session is opened. Password
/// semantics as [`test_connection`].
pub async fn list_server_databases(
    state: &AppState,
    mut conn: ConnectionConfig,
    password: Option<String>,
) -> Result<Vec<String>, String> {
    if password.is_none() {
        conn.password_saved = saved_password_flag(state, &conn.id).await;
    }
    let password = resolve_password(state, &conn, password).await?;
    let driver = state.driver(&conn.driver)?;
    let mut session = driver
        .connect(&connect_params(&conn, password, None))
        .await
        .map_err(|e| e.to_string())?;
    session.list_databases().await.map_err(|e| e.to_string())
}

async fn saved_password_flag(state: &AppState, id: &str) -> bool {
    state
        .connections
        .lock()
        .await
        .connections
        .iter()
        .any(|c| c.id == id && c.password_saved)
}

/// Connects with `conn`, reads the server information and disconnects.
/// Without `password`, uses the one saved for the connection with the same
/// id (the stored `password_saved` decides, not the one sent by the UI,
/// which may be stale).
pub async fn test_connection(
    state: &AppState,
    mut conn: ConnectionConfig,
    password: Option<String>,
) -> Result<ServerInfo, String> {
    if password.is_none() {
        conn.password_saved = saved_password_flag(state, &conn.id).await;
    }
    let password = resolve_password(state, &conn, password).await?;
    let driver = state.driver(&conn.driver)?;
    let mut session = driver
        .connect(&connect_params(&conn, password, None))
        .await
        .map_err(|e| e.to_string())?;
    session.server_info().await.map_err(|e| e.to_string())
}

/// The password to log in with: `given`, else the saved one. Fails with
/// [`PASSWORD_REQUIRED`] when neither exists (a keychain that is unavailable
/// counts as nothing saved).
pub(crate) async fn resolve_password(
    state: &AppState,
    conn: &ConnectionConfig,
    given: Option<String>,
) -> Result<String, String> {
    if let Some(password) = given {
        return Ok(password);
    }
    if !conn.password_saved {
        return Err(PASSWORD_REQUIRED.into());
    }
    let secrets = state.secrets.clone();
    let id = conn.id.clone();
    match blocking(move || secrets.get(&id)).await? {
        Ok(Some(password)) => Ok(password),
        Ok(None) | Err(SecretError::Unavailable(_)) => Err(PASSWORD_REQUIRED.into()),
        Err(e) => Err(format!("could not read the saved password: {e}")),
    }
}

pub(crate) fn connect_params(
    conn: &ConnectionConfig,
    password: String,
    database: Option<String>,
) -> ConnectParams {
    ConnectParams {
        server: conn.server.clone(),
        port: conn.port,
        user: conn.user.clone(),
        password,
        database,
        encrypt: conn.encrypt,
        trust_server_certificate: conn.trust_server_certificate,
        connect_timeout_s: conn.connect_timeout_s,
        ..ConnectParams::default()
    }
}
