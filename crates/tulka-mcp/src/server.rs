//! The MCP server: five tools that forward to `tulka_app::ai`.

use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router, RoleServer, ServerHandler,
};
use serde::Deserialize;
use tokio::sync::watch;
use tulka_app::ai::{self, AiState, QueryRequest, SearchRequest, ToolText};
use tulka_app::tulka_core::ObjectKind;

/// What the assistant reads before it calls any tool.
const INSTRUCTIONS: &str = "tulkabd gives read-only access to the SQL Server connections the user shared with AI assistants. Start with list_connections, then search_objects or describe_table, then run_query. Results are compact tables: a header row, fields separated by |, an empty field is NULL, \"\" is an empty string, and fields with |, line breaks or quotes are in double quotes. Passwords are handled by tulkabd; never ask the user for one.";

/// The kind of object `search_objects` looks for.
#[derive(Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
enum KindParam {
    Table,
    View,
    Procedure,
}

impl From<KindParam> for ObjectKind {
    fn from(kind: KindParam) -> Self {
        match kind {
            KindParam::Table => ObjectKind::Table,
            KindParam::View => ObjectKind::View,
            KindParam::Procedure => ObjectKind::Procedure,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListDatabasesParams {
    /// Name of the connection, as list_connections shows it.
    connection: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchObjectsParams {
    /// Words to look for, separated by spaces. Every word must match.
    query: String,
    /// Search only this connection. Default: all of them.
    connection: Option<String>,
    /// Search only this database. Default: all the cached ones.
    database: Option<String>,
    /// Only objects of this kind. Default: tables, views and procedures.
    kind: Option<KindParam>,
    /// Look in column names instead of object names.
    in_columns: Option<bool>,
    /// Most matches to return. Default 50, at most 200.
    limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct DescribeTableParams {
    /// Name of the connection, as list_connections shows it.
    connection: String,
    /// Database of the table. Default: the connection's first database.
    database: Option<String>,
    /// The table or view, as schema.name or just name.
    table: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunQueryParams {
    /// Name of the connection, as list_connections shows it.
    connection: String,
    /// Database to run the query in. Default: the connection's first database.
    database: Option<String>,
    /// The T-SQL query: a single SELECT, or WITH followed by SELECT.
    sql: String,
    /// Most rows to return. Capped at the row limit the user set in tulkabd.
    max_rows: Option<u32>,
}

/// The server. Cheap to build; all state lives in [`AiState`].
pub struct TulkaServer {
    state: Arc<AiState>,
    tool_router: ToolRouter<Self>,
    /// How many tool calls are running now.
    in_flight: Arc<watch::Sender<usize>>,
}

/// Counts a running tool call for as long as it lives.
struct InFlightGuard(Arc<watch::Sender<usize>>);

impl InFlightGuard {
    fn new(counter: &Arc<watch::Sender<usize>>) -> Self {
        counter.send_modify(|n| *n += 1);
        Self(counter.clone())
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n -= 1);
    }
}

/// Lets the owner wait for the tool calls that are still running.
#[derive(Clone)]
pub struct InFlight(Arc<watch::Sender<usize>>);

impl InFlight {
    /// Waits until no tool call is running, at most `limit`. Calls whose
    /// client left are cancelled and log themselves as they finish.
    pub async fn drained(&self, limit: Duration) {
        let mut rx = self.0.subscribe();
        let _ = tokio::time::timeout(limit, rx.wait_for(|n| *n == 0)).await;
    }
}

impl TulkaServer {
    pub fn new(state: Arc<AiState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            in_flight: Arc::new(watch::channel(0).0),
        }
    }

    /// A handle to wait for running calls once the session is over.
    pub fn in_flight(&self) -> InFlight {
        InFlight(self.in_flight.clone())
    }

    /// Starts a call: remembers the client's name for the run log (read on
    /// every call, so it does not matter how the client introduced itself)
    /// and counts the call as running until the guard drops.
    fn begin(&self, context: &RequestContext<RoleServer>) -> InFlightGuard {
        let guard = InFlightGuard::new(&self.in_flight);
        if let Some(info) = context.client_info() {
            self.state.set_client(Some(info.name));
        }
        guard
    }
}

/// A tool's text as a protocol result. Problems with the data are results the
/// model can read and fix, never protocol errors.
fn reply(text: ToolText) -> CallToolResult {
    let content = vec![ContentBlock::text(text.text)];
    if text.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

#[tool_router]
impl TulkaServer {
    #[tool(
        description = "List the SQL Server connections the user shared with you (read only), with their environment and databases. Call this first."
    )]
    async fn list_connections(&self, context: RequestContext<RoleServer>) -> CallToolResult {
        let _running = self.begin(&context);
        reply(ai::list_connections(&self.state).await)
    }

    #[tool(description = "List the databases of a connection.")]
    async fn list_databases(
        &self,
        Parameters(p): Parameters<ListDatabasesParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let _running = self.begin(&context);
        reply(ai::list_databases(&self.state, p.connection).await)
    }

    #[tool(
        description = "Find tables, views and procedures whose schema.name contains every word of the query (or, with in_columns, columns whose name does), from tulkabd's metadata cache."
    )]
    async fn search_objects(
        &self,
        Parameters(p): Parameters<SearchObjectsParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let _running = self.begin(&context);
        let request = SearchRequest {
            query: p.query,
            connection: p.connection,
            database: p.database,
            kind: p.kind.map(ObjectKind::from),
            in_columns: p.in_columns.unwrap_or(false),
            limit: p.limit,
        };
        reply(ai::search_objects(&self.state, request).await)
    }

    #[tool(
        description = "Columns (type, nullability, primary key, default), foreign keys and indexes of a table or view. table is schema.name or name."
    )]
    async fn describe_table(
        &self,
        Parameters(p): Parameters<DescribeTableParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let _running = self.begin(&context);
        reply(ai::describe_table(&self.state, p.connection, p.database, p.table).await)
    }

    #[tool(
        description = "Run one read-only T-SQL query: SELECT or WITH … SELECT only (no INTO, EXEC, SET, DML or DDL). Returns at most the row limit; use TOP and WHERE."
    )]
    async fn run_query(
        &self,
        Parameters(p): Parameters<RunQueryParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let _running = self.begin(&context);
        let request = QueryRequest {
            connection: p.connection,
            database: p.database,
            sql: p.sql,
            max_rows: p.max_rows,
        };
        reply(ai::run_query(&self.state, request, context.ct.cancelled()).await)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TulkaServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tulkabd", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}
