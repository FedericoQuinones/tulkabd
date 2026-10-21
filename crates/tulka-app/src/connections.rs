//! Saved connections and their passwords.

use std::sync::Arc;

use tulka_core::{
    AppConfig, ConnectParams, ConnectionConfig, SecretError, SecretStore, ServerInfo,
};

use crate::state::AppState;
use crate::types::{SaveConnectionResult, PASSWORD_REQUIRED};

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
/// not prevent the deletion.
pub async fn delete_connection(state: &AppState, id: String) -> Result<(), String> {
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
    Ok(())
}

/// Connects with `conn`, reads the server information and disconnects.
/// Without `password`, uses the saved one.
pub async fn test_connection(
    state: &AppState,
    conn: ConnectionConfig,
    password: Option<String>,
) -> Result<ServerInfo, String> {
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
