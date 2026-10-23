use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

use crate::meta::{DatabaseSnapshot, TableStructure};
use crate::{DriverError, Value};

/// Application name reported to the server at login.
pub const APPLICATION_NAME: &str = "tulkabd";

/// A result-set column.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// The server's type name, for display (`int`, `nvarchar`, ...).
    pub type_name: String,
}

/// Severity of a message produced while executing.
///
/// Serialized as a plain string: `"info"`, `"warning"`, `"error"`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MessageKind {
    Info,
    Warning,
    Error,
}

/// Events streamed while a script executes, in server order.
///
/// Serialized internally tagged: `{"kind": "rowsAffected", "count": 3}`.
/// Because the tag is `kind`, the severity of `Message` is serialized as
/// `messageKind`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExecEvent {
    /// A new result set starts. `index` counts result sets from 0 across the
    /// whole script.
    ResultStart { index: u32, columns: Vec<Column> },
    /// A chunk of rows of result set `index`.
    Rows { index: u32, rows: Vec<Vec<Value>> },
    /// A server message. `line` is relative to the editor text.
    Message {
        #[serde(rename = "messageKind")]
        kind: MessageKind,
        text: String,
        line: Option<u32>,
    },
    /// A statement affected `count` rows.
    RowsAffected { count: u64 },
    /// Batch number `batch` (from 0) finished successfully.
    BatchDone { batch: u32 },
    /// The connection was lost.
    Disconnected { reason: String },
}

/// Connection parameters for a server.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectParams {
    /// `host` or `host\instance`. A named instance's port is looked up
    /// through SQL Server Browser when `port` is the default 1433.
    pub server: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: Option<String>,
    pub encrypt: bool,
    pub trust_server_certificate: bool,
    /// Bounds the TCP connection and login only; `0` waits indefinitely.
    /// Queries have no timeout.
    pub connect_timeout_s: u32,
    /// Always [`APPLICATION_NAME`]; optional in JSON.
    #[serde(default = "default_application_name")]
    pub application_name: String,
}

fn default_application_name() -> String {
    APPLICATION_NAME.to_string()
}

impl Default for ConnectParams {
    fn default() -> Self {
        Self {
            server: String::new(),
            port: 1433,
            user: String::new(),
            password: String::new(),
            database: None,
            encrypt: true,
            trust_server_certificate: false,
            connect_timeout_s: 15,
            application_name: default_application_name(),
        }
    }
}

// Hand-written so the password never ends up in logs.
impl fmt::Debug for ConnectParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectParams")
            .field("server", &self.server)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("database", &self.database)
            .field("encrypt", &self.encrypt)
            .field("trust_server_certificate", &self.trust_server_certificate)
            .field("connect_timeout_s", &self.connect_timeout_s)
            .field("application_name", &self.application_name)
            .finish()
    }
}

/// Information about the connected server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    /// Product and version, for example `Microsoft SQL Server 2022 (RTM-CU..) ...`.
    pub version: String,
    /// The login name of the session.
    pub user: String,
    /// True on Azure SQL Database, where a connection is bound to one database.
    pub single_database: bool,
}

/// An open connection to a server.
#[async_trait]
pub trait Session: Send {
    async fn server_info(&mut self) -> Result<ServerInfo, DriverError>;
    async fn list_databases(&mut self) -> Result<Vec<String>, DriverError>;
    async fn use_database(&mut self, name: &str) -> Result<(), DriverError>;
    async fn current_database(&mut self) -> Result<String, DriverError>;
    /// Every object of `database` with the columns of its tables and views.
    async fn snapshot(&mut self, database: &str) -> Result<DatabaseSnapshot, DriverError>;
    /// Columns, foreign keys and indexes of one table.
    async fn structure(
        &mut self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<TableStructure, DriverError>;
    /// The source text of a view or procedure.
    async fn definition(
        &mut self,
        database: &str,
        schema: &str,
        name: &str,
    ) -> Result<String, DriverError>;
    /// Runs a script, streaming its events to `tx`. Returns once the script
    /// finished, failed, was cancelled or the connection was lost.
    async fn execute(&mut self, sql: &str, tx: Sender<ExecEvent>) -> Result<(), DriverError>;
    /// A handle that cancels the running `execute` from another task.
    fn cancel_handle(&self) -> Box<dyn CancelHandle>;
}

/// Cancels the request running on a session.
#[async_trait]
pub trait CancelHandle: Send + Sync {
    async fn cancel(&self) -> Result<(), DriverError>;
}

/// A database driver.
#[async_trait]
pub trait Driver: Send + Sync {
    fn id(&self) -> &'static str;
    async fn connect(&self, p: &ConnectParams) -> Result<Box<dyn Session>, DriverError>;
}
