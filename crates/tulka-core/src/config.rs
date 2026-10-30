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

    /// Default AI access for connections in this environment.
    pub fn default_ai_access(&self) -> AiAccess {
        match self {
            Environment::Production => AiAccess::None,
            Environment::Staging | Environment::Test | Environment::Local => AiAccess::ReadOnly,
        }
    }
}

/// What AI assistants (through `tulkabd mcp`) may do on a connection.
///
/// Serialized as a plain string: `"none"` or `"readOnly"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AiAccess {
    #[default]
    None,
    ReadOnly,
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
    /// The folder holding this connection; `None` is the root.
    #[serde(default)]
    pub folder_id: Option<String>,
    /// What AI assistants may do here; a config without it grants none.
    #[serde(default, deserialize_with = "lenient")]
    pub ai_access: AiAccess,
}

/// A folder of the connections tree (nestable).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// UUID v4.
    pub id: String,
    pub name: String,
    /// The containing folder; `None` is the root.
    #[serde(default)]
    pub parent_id: Option<String>,
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
            folder_id: None,
            ai_access: environment.default_ai_access(),
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
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// Ids of the folders and connections open in the tree.
    #[serde(default)]
    pub expanded: Vec<String>,
    /// User preferences; a config written before they existed gets the defaults.
    #[serde(default, deserialize_with = "lenient")]
    pub preferences: Preferences,
}

/// Lowest zoom of the editor and the grid, in percent.
pub const ZOOM_MIN: u16 = 70;
/// Highest zoom of the editor and the grid, in percent.
pub const ZOOM_MAX: u16 = 200;

/// Brings a zoom level into `ZOOM_MIN..=ZOOM_MAX`.
pub fn clamp_zoom(zoom: u16) -> u16 {
    zoom.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Smallest and largest editor and grid font size, in pixels.
pub const FONT_SIZE_MIN: u8 = 10;
pub const FONT_SIZE_MAX: u8 = 24;
/// Smallest and largest number of rows kept in memory per result.
pub const MAX_ROWS_MIN: u32 = 1_000;
pub const MAX_ROWS_MAX: u32 = 1_000_000;
/// Default number of rows kept in memory per result.
pub const MAX_ROWS_DEFAULT: u32 = 100_000;
/// Largest query timeout in seconds (one day); `0` means no limit.
pub const QUERY_TIMEOUT_MAX_S: u32 = 86_400;
/// Smallest and largest number of days the run log is kept.
pub const RUN_LOG_DAYS_MIN: u16 = 1;
pub const RUN_LOG_DAYS_MAX: u16 = 365;
/// Smallest, largest and default number of rows returned to an AI assistant.
pub const AI_MAX_ROWS_MIN: u32 = 1;
pub const AI_MAX_ROWS_MAX: u32 = 10_000;
pub const AI_MAX_ROWS_DEFAULT: u32 = 500;
/// Smallest, largest and default query timeout for an AI assistant, in seconds.
pub const AI_TIMEOUT_MIN_S: u32 = 1;
pub const AI_TIMEOUT_MAX_S: u32 = 3_600;
pub const AI_TIMEOUT_DEFAULT_S: u32 = 30;

/// Whether `color` is written as `#rrggbb`.
fn is_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// How numbers are written by the "Copy" (Excel) command.
///
/// Serialized as a plain string: `"system"` or `"invariant"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NumberFormat {
    /// The decimal separator of the operating system's locale.
    #[default]
    System,
    /// Always a dot.
    Invariant,
}

/// The color and guard level new connections of one environment start with.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentStyle {
    /// Accent color as `#rrggbb`.
    pub color: String,
    pub guard: GuardLevel,
    pub ai_access: AiAccess,
}

impl EnvironmentStyle {
    /// The factory style of `environment`.
    pub fn factory(environment: Environment) -> Self {
        Self {
            color: environment.default_color().into(),
            guard: environment.default_guard(),
            ai_access: environment.default_ai_access(),
        }
    }

    /// Reads a style from loose JSON: each field that is missing or invalid
    /// takes the factory value of `environment`.
    fn from_value(value: Option<&serde_json::Value>, environment: Environment) -> Self {
        let mut style = Self::factory(environment);
        if let Some(color) = value.and_then(|v| v.get("color")).and_then(|c| c.as_str()) {
            if is_hex_color(color) {
                style.color = color.into();
            }
        }
        if let Some(guard) = value
            .and_then(|v| v.get("guard"))
            .and_then(|g| serde_json::from_value(g.clone()).ok())
        {
            style.guard = guard;
        }
        if let Some(ai_access) = value
            .and_then(|v| v.get("aiAccess"))
            .and_then(|a| serde_json::from_value(a.clone()).ok())
        {
            style.ai_access = ai_access;
        }
        style
    }
}

/// The style new connections start with, for each environment.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDefaults {
    pub production: EnvironmentStyle,
    pub staging: EnvironmentStyle,
    pub test: EnvironmentStyle,
    pub local: EnvironmentStyle,
}

impl EnvironmentDefaults {
    /// The style of `environment`.
    pub fn get(&self, environment: Environment) -> &EnvironmentStyle {
        match environment {
            Environment::Production => &self.production,
            Environment::Staging => &self.staging,
            Environment::Test => &self.test,
            Environment::Local => &self.local,
        }
    }
}

impl Default for EnvironmentDefaults {
    fn default() -> Self {
        Self {
            production: EnvironmentStyle::factory(Environment::Production),
            staging: EnvironmentStyle::factory(Environment::Staging),
            test: EnvironmentStyle::factory(Environment::Test),
            local: EnvironmentStyle::factory(Environment::Local),
        }
    }
}

impl<'de> Deserialize<'de> for EnvironmentDefaults {
    /// Never fails on content: anything unusable becomes the factory style.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let style = |key: &str, env| EnvironmentStyle::from_value(value.get(key), env);
        Ok(Self {
            production: style("production", Environment::Production),
            staging: style("staging", Environment::Staging),
            test: style("test", Environment::Test),
            local: style("local", Environment::Local),
        })
    }
}

/// Deserializes a field leniently: a value of the wrong type yields the
/// field's default instead of failing the whole file.
fn lenient<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn default_zoom() -> u16 {
    100
}
fn default_font_size() -> u8 {
    13
}
fn default_true() -> bool {
    true
}
fn default_max_rows() -> u32 {
    MAX_ROWS_DEFAULT
}
fn default_run_log_days() -> u16 {
    30
}
fn default_ai_max_rows() -> u32 {
    AI_MAX_ROWS_DEFAULT
}
fn default_ai_timeout_s() -> u32 {
    AI_TIMEOUT_DEFAULT_S
}

/// Like [`lenient`], for a field whose default is not the type's default.
macro_rules! lenient_with {
    ($name:ident, $ty:ty, $default:path) => {
        fn $name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<$ty, D::Error> {
            let value = serde_json::Value::deserialize(d)?;
            Ok(serde_json::from_value(value).unwrap_or_else(|_| $default()))
        }
    };
}
lenient_with!(lenient_zoom, u16, default_zoom);
lenient_with!(lenient_font_size, u8, default_font_size);
lenient_with!(lenient_true, bool, default_true);
lenient_with!(lenient_max_rows, u32, default_max_rows);
lenient_with!(lenient_run_log_days, u16, default_run_log_days);
lenient_with!(lenient_ai_max_rows, u32, default_ai_max_rows);
lenient_with!(lenient_ai_timeout_s, u32, default_ai_timeout_s);

/// Settings of the interface that survive restarts.
///
/// Loading is forgiving: a missing field takes its default, a field of the
/// wrong type too, and unknown fields are ignored. Call [`Preferences::clamped`]
/// to bring values into their ranges.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Preferences {
    /// Zoom of the editor and the result grid, in percent (70 to 200).
    #[serde(deserialize_with = "lenient_zoom")]
    pub zoom: u16,
    /// Font size of the editor and the grid at 100 % zoom, in pixels (10 to 24).
    #[serde(deserialize_with = "lenient_font_size")]
    pub font_size: u8,
    /// Whether the tabs of the last session are reopened at startup.
    #[serde(deserialize_with = "lenient_true")]
    pub restore_session: bool,
    /// Rows kept in memory per result set (1,000 to 1,000,000).
    #[serde(deserialize_with = "lenient_max_rows")]
    pub max_rows: u32,
    /// Seconds after which a running query is cancelled; `0` is no limit.
    #[serde(deserialize_with = "lenient")]
    pub query_timeout_s: u32,
    /// How "Copy" writes numbers.
    #[serde(deserialize_with = "lenient")]
    pub copy_numbers: NumberFormat,
    /// Days the run log keeps its entries (1 to 365).
    #[serde(deserialize_with = "lenient_run_log_days")]
    pub run_log_days: u16,
    /// Color and guard level new connections start with, per environment.
    #[serde(deserialize_with = "lenient")]
    pub environments: EnvironmentDefaults,
    /// Rows an AI assistant gets per query (1 to 10,000).
    #[serde(deserialize_with = "lenient_ai_max_rows")]
    pub ai_max_rows: u32,
    /// Seconds after which an AI query is cancelled (1 to 3,600).
    #[serde(deserialize_with = "lenient_ai_timeout_s")]
    pub ai_query_timeout_s: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            zoom: default_zoom(),
            font_size: default_font_size(),
            restore_session: true,
            max_rows: MAX_ROWS_DEFAULT,
            query_timeout_s: 0,
            copy_numbers: NumberFormat::System,
            run_log_days: default_run_log_days(),
            environments: EnvironmentDefaults::default(),
            ai_max_rows: AI_MAX_ROWS_DEFAULT,
            ai_query_timeout_s: AI_TIMEOUT_DEFAULT_S,
        }
    }
}

impl Preferences {
    /// The preferences with every value brought into its range and every
    /// environment color that is not `#rrggbb` reset to the factory color.
    pub fn clamped(mut self) -> Self {
        self.zoom = clamp_zoom(self.zoom);
        self.font_size = self.font_size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
        self.max_rows = self.max_rows.clamp(MAX_ROWS_MIN, MAX_ROWS_MAX);
        self.query_timeout_s = self.query_timeout_s.min(QUERY_TIMEOUT_MAX_S);
        self.run_log_days = self.run_log_days.clamp(RUN_LOG_DAYS_MIN, RUN_LOG_DAYS_MAX);
        self.ai_max_rows = self.ai_max_rows.clamp(AI_MAX_ROWS_MIN, AI_MAX_ROWS_MAX);
        self.ai_query_timeout_s = self
            .ai_query_timeout_s
            .clamp(AI_TIMEOUT_MIN_S, AI_TIMEOUT_MAX_S);
        let envs = [
            (&mut self.environments.production, Environment::Production),
            (&mut self.environments.staging, Environment::Staging),
            (&mut self.environments.test, Environment::Test),
            (&mut self.environments.local, Environment::Local),
        ];
        for (style, env) in envs {
            if !is_hex_color(&style.color) {
                style.color = env.default_color().into();
            }
        }
        self
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            connections: Vec::new(),
            folders: Vec::new(),
            expanded: Vec::new(),
            preferences: Preferences::default(),
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
        let mut config: AppConfig = serde_json::from_value(value)?;
        config.preferences = std::mem::take(&mut config.preferences).clamped();
        Ok(config)
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
    fn config_without_preferences_loads_with_zoom_100() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"version": 1, "connections": []}"#).unwrap();
        assert_eq!(ConfigStore::at(&path).load().unwrap().preferences.zoom, 100);
    }

    #[test]
    fn stored_zoom_is_clamped_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for (stored, expected) in [(500, 200), (10, 70), (130, 130)] {
            let text = format!(r#"{{"version": 1, "preferences": {{"zoom": {stored}}}}}"#);
            fs::write(&path, text).unwrap();
            assert_eq!(
                ConfigStore::at(&path).load().unwrap().preferences.zoom,
                expected
            );
        }
    }

    fn load_text(text: &str) -> AppConfig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, text).unwrap();
        ConfigStore::at(&path).load().unwrap()
    }

    #[test]
    fn old_config_gets_defaults() {
        let cfg = load_text(r#"{"version": 1, "preferences": {"zoom": 130}}"#);
        let expected = Preferences {
            zoom: 130,
            ..Preferences::default()
        };
        assert_eq!(cfg.preferences, expected);
        assert_eq!(expected.font_size, 13);
        assert!(expected.restore_session);
        assert_eq!((expected.max_rows, expected.query_timeout_s), (100_000, 0));
        assert_eq!(expected.run_log_days, 30);
        assert_eq!(
            (expected.ai_max_rows, expected.ai_query_timeout_s),
            (500, 30)
        );
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let cfg = load_text(
            r#"{"version": 1, "preferences": {"zoom": 5, "fontSize": 99, "maxRows": 1,
                "queryTimeoutS": 999999, "runLogDays": 9999}}"#,
        );
        let p = cfg.preferences;
        assert_eq!((p.zoom, p.font_size, p.max_rows), (70, 24, 1_000));
        assert_eq!((p.query_timeout_s, p.run_log_days), (86_400, 365));
        let low = Preferences {
            run_log_days: 0,
            font_size: 1,
            ..Preferences::default()
        }
        .clamped();
        assert_eq!((low.run_log_days, low.font_size), (1, 10));
    }

    #[test]
    fn wrong_types_fall_back_to_defaults_without_losing_connections() {
        let conn = ConnectionConfig::new("A", "h", 1433, "u", Environment::Test);
        let text = json!({
            "version": 1,
            "connections": [conn],
            "preferences": {
                "runLogDays": "abc", "zoom": null, "maxRows": [1], "copyNumbers": "weird",
                "restoreSession": "yes", "environments": 7, "fontSize": 15
            }
        })
        .to_string();
        let cfg = load_text(&text);
        assert_eq!(cfg.connections, vec![conn]);
        let expected = Preferences {
            font_size: 15,
            ..Preferences::default()
        };
        assert_eq!(cfg.preferences, expected);
        // A preferences section that is not an object at all is also tolerated.
        let cfg = load_text(r#"{"version": 1, "connections": [], "preferences": "oops"}"#);
        assert_eq!(cfg.preferences, Preferences::default());
    }

    #[test]
    fn bad_env_color_falls_back() {
        let cfg = load_text(
            r##"{"version": 1, "preferences": {"environments": {
                "production": {"color": "red", "guard": "block"},
                "staging": {"color": "#12345", "guard": 3},
                "test": {"color": "#ABCDEF"},
                "local": "nope"}}}"##,
        );
        let e = cfg.preferences.environments;
        assert_eq!(e.production.color, "#e5566f");
        assert_eq!(e.production.guard, GuardLevel::Block);
        assert_eq!(e.staging, EnvironmentStyle::factory(Environment::Staging));
        assert_eq!(e.test.color, "#ABCDEF");
        assert_eq!(e.local, EnvironmentStyle::factory(Environment::Local));
        // Set programmatically, the color is also repaired by `clamped`.
        let mut p = Preferences::default();
        p.environments.test.color = "blue".into();
        assert_eq!(p.clamped().environments.test.color, "#44a282");
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let cfg = load_text(
            r##"{"version": 1, "futureThing": 1, "preferences": {"zoom": 120, "theme": "x",
                "environments": {"local": {"color": "#112233", "icon": "x"}, "qa": {}}}}"##,
        );
        assert_eq!(cfg.preferences.zoom, 120);
        assert_eq!(cfg.preferences.environments.local.color, "#112233");
    }

    #[test]
    fn widened_preferences_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(dir.path().join("config.json"));
        let mut cfg = AppConfig::default();
        cfg.preferences.copy_numbers = NumberFormat::Invariant;
        cfg.preferences.max_rows = 5_000;
        cfg.preferences.environments.local.guard = GuardLevel::Block;
        store.save(&cfg).unwrap();
        assert_eq!(store.load().unwrap(), cfg);
    }

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
                "passwordSaved": false, "folderId": null, "aiAccess": "readOnly"
            })
        );
        assert_eq!(
            serde_json::to_value(AppConfig::default()).unwrap(),
            json!({"version": 1, "connections": [], "folders": [], "expanded": [], "preferences": {
                "zoom": 100, "fontSize": 13, "restoreSession": true, "maxRows": 100000,
                "queryTimeoutS": 0, "copyNumbers": "system", "runLogDays": 30,
                "environments": {
                    "production": {"color": "#e5566f", "guard": "confirm", "aiAccess": "none"},
                    "staging": {"color": "#d9a441", "guard": "off", "aiAccess": "readOnly"},
                    "test": {"color": "#44a282", "guard": "off", "aiAccess": "readOnly"},
                    "local": {"color": "#4a8ad6", "guard": "off", "aiAccess": "readOnly"}
                },
                "aiMaxRows": 500, "aiQueryTimeoutS": 30
            }})
        );
    }

    fn conn_json(id: &str, extra: &str) -> String {
        format!(
            r##"{{"id": "{id}", "name": "{id}", "driver": "mssql", "server": "h", "port": 1433,
            "user": "u", "databases": {{"kind": "all"}}, "environment": "local",
            "color": "#4a8ad6", "guard": "off", "encrypt": true,
            "trustServerCertificate": false, "connectTimeoutS": 15,
            "passwordSaved": false{extra}}}"##
        )
    }

    #[test]
    fn connection_without_ai_access_loads_no_access() {
        let cfg = load_text(&format!(
            r#"{{"version": 1, "connections": [{}]}}"#,
            conn_json("a", "")
        ));
        assert_eq!(cfg.connections[0].ai_access, AiAccess::None);
    }

    #[test]
    fn unknown_ai_access_loads_no_access_and_keeps_other_connections() {
        let cfg = load_text(&format!(
            r#"{{"version": 1, "connections": [{}, {}]}}"#,
            conn_json("a", r#", "aiAccess": "admin""#),
            conn_json("b", r#", "aiAccess": "readOnly""#)
        ));
        assert_eq!(cfg.connections.len(), 2);
        assert_eq!(cfg.connections[0].ai_access, AiAccess::None);
        assert_eq!(cfg.connections[1].ai_access, AiAccess::ReadOnly);
        assert_eq!(cfg.connections[1].id, "b");
    }

    #[test]
    fn ai_access_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(dir.path().join("config.json"));
        let mut c = ConnectionConfig::new("A", "h", 1, "u", Environment::Production);
        c.ai_access = AiAccess::ReadOnly;
        let cfg = AppConfig {
            connections: vec![c],
            ..AppConfig::default()
        };
        store.save(&cfg).unwrap();
        assert_eq!(
            store.load().unwrap().connections[0].ai_access,
            AiAccess::ReadOnly
        );
    }

    #[test]
    fn new_connection_takes_the_environment_default() {
        let prod = ConnectionConfig::new("A", "h", 1, "u", Environment::Production);
        let local = ConnectionConfig::new("A", "h", 1, "u", Environment::Local);
        assert_eq!(prod.ai_access, AiAccess::None);
        assert_eq!(local.ai_access, AiAccess::ReadOnly);
    }

    #[test]
    fn environment_style_ai_access_is_lenient() {
        let cfg = load_text(
            r#"{"version": 1, "preferences": {"environments": {
                "production": {"aiAccess": "admin"},
                "local": {"aiAccess": "none"}
            }}}"#,
        );
        let e = cfg.preferences.environments;
        assert_eq!(e.production.ai_access, AiAccess::None);
        assert_eq!(e.local.ai_access, AiAccess::None);
        assert_eq!(e.staging.ai_access, AiAccess::ReadOnly);
        let cfg = load_text(r#"{"version": 1, "preferences": {"environments": {"local": {}}}}"#);
        assert_eq!(
            cfg.preferences.environments.local.ai_access,
            AiAccess::ReadOnly
        );
    }

    #[test]
    fn ai_preferences_default_and_clamp() {
        let p = load_text(r#"{"version": 1, "preferences": {}}"#).preferences;
        assert_eq!((p.ai_max_rows, p.ai_query_timeout_s), (500, 30));
        let p =
            load_text(r#"{"version": 1, "preferences": {"aiMaxRows": 0, "aiQueryTimeoutS": 0}}"#)
                .preferences;
        assert_eq!((p.ai_max_rows, p.ai_query_timeout_s), (1, 1));
        let p = load_text(
            r#"{"version": 1, "preferences": {"aiMaxRows": 99999, "aiQueryTimeoutS": 99999}}"#,
        )
        .preferences;
        assert_eq!((p.ai_max_rows, p.ai_query_timeout_s), (10_000, 3_600));
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

    #[test]
    fn plan1_file_without_folders_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r##"{"version": 1, "connections": [{
                "id": "c1", "name": "A", "driver": "mssql", "server": "h", "port": 1433,
                "user": "u", "databases": {"kind": "all"}, "environment": "test",
                "color": "#44a282", "guard": "off", "encrypt": true,
                "trustServerCertificate": false, "connectTimeoutS": 15,
                "passwordSaved": false}]}"##,
        )
        .unwrap();
        let cfg = ConfigStore::at(path).load().unwrap();
        assert_eq!(cfg.connections.len(), 1);
        assert_eq!(cfg.connections[0].folder_id, None);
        assert!(cfg.folders.is_empty() && cfg.expanded.is_empty());
    }

    #[test]
    fn folders_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(dir.path().join("config.json"));
        let mut cfg = AppConfig::default();
        cfg.folders.push(Folder {
            id: "f1".into(),
            name: "Cliente A".into(),
            parent_id: None,
        });
        cfg.expanded.push("f1".into());
        store.save(&cfg).unwrap();
        assert_eq!(store.load().unwrap(), cfg);
        assert_eq!(
            serde_json::to_value(&cfg.folders[0]).unwrap(),
            json!({"id": "f1", "name": "Cliente A", "parentId": null})
        );
    }
}
