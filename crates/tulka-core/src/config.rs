//! Connection configuration: environments, guard levels and the on-disk
//! config store.
//!
//! The password is never part of [`ConnectionConfig`]; it lives in the OS
//! keychain (see [`crate::secrets`]). Only the `password_saved` flag is stored.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current version of the config file format.
pub const CONFIG_VERSION: u32 = 1;

/// The kind of server a connection points at.
///
/// Serialized as a plain string: `"production"`, `"staging"`, `"test"`,
/// `"local"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Environment {
    Production,
    Staging,
    Test,
    Local,
}

impl Environment {
    /// Default accent color (hex) for connections in this environment.
    pub fn default_color(&self) -> &'static str {
        match self {
            Environment::Production => "#e5566f",
            Environment::Staging => "#d9a441",
            Environment::Test => "#44a282",
            Environment::Local => "#4a8ad6",
        }
    }

    /// Default guard level for connections in this environment.
    pub fn default_guard(&self) -> GuardLevel {
        match self {
            Environment::Production => GuardLevel::Confirm,
            Environment::Staging | Environment::Test | Environment::Local => GuardLevel::Off,
        }
    }
}

/// How dangerous statements are handled on a connection. Only stored and
/// displayed for now; enforcement comes later.
///
/// Serialized as a plain string: `"off"`, `"confirm"`, `"block"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum GuardLevel {
    Off,
    Confirm,
    Block,
}

/// Which databases of the server a connection exposes.
///
/// Serialized internally tagged: `{"kind": "all"}` or
/// `{"kind": "selected", "names": ["a", "b"]}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DatabaseScope {
    All,
    Selected { names: Vec<String> },
}

/// A saved connection. It has no password field on purpose.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    /// UUID v4; also the keychain account name of the password.
    pub id: String,
    pub name: String,
    /// Driver identifier, `"mssql"`.
    pub driver: String,
    pub server: String,
    pub port: u16,
    pub user: String,
    pub databases: DatabaseScope,
    pub environment: Environment,
    /// Accent color as a hex string.
    pub color: String,
    pub guard: GuardLevel,
    pub encrypt: bool,
    pub trust_server_certificate: bool,
    pub connect_timeout_s: u32,
    /// Whether a password for this connection is stored in the keychain.
    pub password_saved: bool,
}

impl ConnectionConfig {
    /// A new SQL Server connection with a fresh id and the color and guard
    /// level of `environment`.
    pub fn new(
        name: impl Into<String>,
        server: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        environment: Environment,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            driver: "mssql".into(),
            server: server.into(),
            port,
            user: user.into(),
            databases: DatabaseScope::All,
            environment,
            color: environment.default_color().into(),
            guard: environment.default_guard(),
            encrypt: true,
            trust_server_certificate: false,
            connect_timeout_s: 15,
            password_saved: false,
        }
    }
}

/// The whole config file: `{"version": 1, "connections": [...]}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            connections: Vec::new(),
        }
    }
}

/// Errors reading or writing the config file.
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("config file I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("config file is not valid: {0}")]
    Parse(#[from] serde_json::Error),
    /// The file was written by a newer (or unknown) version of the app.
    #[error("config file version {0} is not supported")]
    UnsupportedVersion(u32),
    #[error("could not determine the config directory")]
    NoConfigDir,
}

/// Reads and writes the config file.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// The store at the platform's per-user config location
    /// (`<config dir>/config.json`).
    pub fn open_default() -> Result<Self, ConfigError> {
        let dirs =
            ProjectDirs::from("dev", "tulkabd", "tulkabd").ok_or(ConfigError::NoConfigDir)?;
        Ok(Self::at(dirs.config_dir().join("config.json")))
    }

    /// A store backed by `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the config. A missing file yields an empty config; a malformed
    /// file or an unknown version is an error (the file is left untouched).
    pub fn load(&self) -> Result<AppConfig, ConfigError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(AppConfig::default()),
            Err(e) => return Err(e.into()),
        };
        let value: serde_json::Value = serde_json::from_str(&text)?;
        // Check the version first: a newer format may not match our shape.
        let version = value.get("version").and_then(|v| v.as_u64());
        if version != Some(u64::from(CONFIG_VERSION)) {
            return Err(ConfigError::UnsupportedVersion(
                version.map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX)),
            ));
        }
        Ok(serde_json::from_value(value)?)
    }

    /// Saves the config atomically: the JSON is written and synced to a
    /// temporary file in the same directory, which then replaces the config.
    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        let dir = match self.path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        fs::create_dir_all(dir)?;
        let mut json = serde_json::to_string_pretty(config)?;
        json.push('\n');

        let file_name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "config.json".into());
        let tmp = dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

        let result = (|| -> io::Result<()> {
            let mut f = File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &self.path)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        // Persist the rename itself; best effort (not possible on Windows).
        #[cfg(unix)]
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn production_defaults() {
        let c = ConnectionConfig::new(
            "Sales · PROD",
            "10.0.3.47",
            1433,
            "federico",
            Environment::Production,
        );
        assert_eq!(
            (c.color.as_str(), c.guard),
            ("#e5566f", GuardLevel::Confirm)
        );
        assert!(c.encrypt && !c.trust_server_certificate && !c.password_saved);
    }

    #[test]
    fn test_env_defaults_off() {
        assert_eq!(Environment::Test.default_guard(), GuardLevel::Off);
        assert_eq!(Environment::Test.default_color(), "#44a282");
    }

    #[test]
    fn config_roundtrip_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(dir.path().join("config.json"));
        assert!(store.load().unwrap().connections.is_empty());
        let mut cfg = AppConfig::default();
        cfg.connections.push(ConnectionConfig::new(
            "A",
            "h",
            1433,
            "u",
            Environment::Local,
        ));
        store.save(&cfg).unwrap();
        assert_eq!(store.load().unwrap(), cfg);
    }

    #[test]
    fn config_file_never_contains_password() {
        let mut c = ConnectionConfig::new("A", "h", 1433, "u", Environment::Staging);
        c.password_saved = true;
        let text = serde_json::to_string(&c).unwrap().to_lowercase();
        assert!(!text.contains("\"password\""));
        assert!(text.contains("\"passwordsaved\":true"));
        assert!(!format!("{c:?}").to_lowercase().contains("secret"));
    }

    #[test]
    fn json_shapes() {
        assert_eq!(
            serde_json::to_value(Environment::Production).unwrap(),
            json!("production")
        );
        assert_eq!(
            serde_json::to_value(GuardLevel::Block).unwrap(),
            json!("block")
        );
        assert_eq!(
            serde_json::to_value(DatabaseScope::All).unwrap(),
            json!({"kind": "all"})
        );
        assert_eq!(
            serde_json::to_value(DatabaseScope::Selected {
                names: vec!["a".into()]
            })
            .unwrap(),
            json!({"kind": "selected", "names": ["a"]})
        );
        let mut c = ConnectionConfig::new("A", "h", 1433, "u", Environment::Test);
        c.id = "id-1".into();
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({
                "id": "id-1", "name": "A", "driver": "mssql", "server": "h", "port": 1433,
                "user": "u", "databases": {"kind": "all"}, "environment": "test",
                "color": "#44a282", "guard": "off", "encrypt": true,
                "trustServerCertificate": false, "connectTimeoutS": 15,
                "passwordSaved": false
            })
        );
        assert_eq!(
            serde_json::to_value(AppConfig::default()).unwrap(),
            json!({"version": 1, "connections": []})
        );
    }

    #[test]
    fn ids_are_unique_uuids() {
        let a = ConnectionConfig::new("A", "h", 1, "u", Environment::Local);
        let b = ConnectionConfig::new("A", "h", 1, "u", Environment::Local);
        assert_ne!(a.id, b.id);
        assert_eq!(a.id.len(), 36);
    }

    #[test]
    fn bad_files_error_and_are_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let store = ConfigStore::at(path.clone());

        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(store.load(), Err(ConfigError::Parse(_))));

        let future = r#"{"version": 99, "connections": []}"#;
        std::fs::write(&path, future).unwrap();
        assert!(matches!(
            store.load(),
            Err(ConfigError::UnsupportedVersion(99))
        ));
        // A failed load leaves the file alone.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), future);
    }

    #[test]
    fn save_creates_missing_directory_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("nested").join("tulkabd");
        let store = ConfigStore::at(sub.join("config.json"));
        store.save(&AppConfig::default()).unwrap();
        store.save(&AppConfig::default()).unwrap();
        let names: Vec<_> = std::fs::read_dir(&sub)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["config.json".to_string()]);
    }
}
