//! The server and a real rmcp client, talking over an in-memory pipe.

#[path = "../../tulka-app/tests/common/ai_fixture.rs"]
mod ai_fixture;

use std::sync::Arc;
use std::time::Duration;

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, ClientRequest,
        Implementation, Request,
    },
    service::{PeerRequestOptions, RunningService},
    RoleClient, ServiceExt,
};
use tulka_app::ai;
use tulka_app::tulka_store::RunLogOutcome;
use tulka_mcp::TulkaServer;

use ai_fixture::Fixture;

const INSTRUCTIONS: &str = "tulkabd gives read-only access to the SQL Server connections the user shared with AI assistants. Start with list_connections, then search_objects or describe_table, then run_query. Results are compact tables: a header row, fields separated by |, an empty field is NULL, \"\" is an empty string, and fields with |, line breaks or quotes are in double quotes. Passwords are handled by tulkabd; never ask the user for one.";

struct Session {
    client: RunningService<RoleClient, ClientConfig>,
    fixture: Fixture,
    state: Arc<ai::AiState>,
}

async fn connect(client_name: &str) -> Session {
    let mut fixture = Fixture::standard();
    // The fixture keeps its other parts (run log, driver) for the test.
    let state = Arc::new(std::mem::replace(
        &mut fixture.state,
        ai::AiState::new(
            fixture.config.clone(),
            fixture.secrets.clone(),
            vec![fixture.driver.clone()],
            Some(fixture.cache.clone()),
            fixture.runlog.clone(),
        ),
    ));
    let server = TulkaServer::new(state.clone());
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let running = server.serve(server_io).await.expect("server starts");
        let _ = running.waiting().await;
    });
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new(client_name, "1.0.0"),
    );
    let client = config.serve(client_io).await.expect("client starts");
    Session {
        client,
        fixture,
        state,
    }
}

fn args(value: serde_json::Value) -> rmcp::model::JsonObject {
    value.as_object().expect("an object").clone()
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn initialize_reports_tools_and_instructions() {
    let s = connect("test-client").await;
    let info = s.client.peer_info().expect("server info");
    assert_eq!(info.instructions.as_deref(), Some(INSTRUCTIONS));
    assert!(info.capabilities.tools.is_some());
}

#[tokio::test]
async fn lists_the_five_tools_with_schemas() {
    let s = connect("test-client").await;
    let tools = s.client.list_all_tools().await.expect("tools");
    let required = |name: &str| -> Vec<String> {
        let tool = tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let mut required: Vec<String> = tool
            .input_schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|r| {
                r.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        required.sort();
        required
    };
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "describe_table",
            "list_connections",
            "list_databases",
            "run_query",
            "search_objects"
        ]
    );
    assert!(required("list_connections").is_empty());
    assert_eq!(required("list_databases"), ["connection"]);
    assert_eq!(required("search_objects"), ["query"]);
    assert_eq!(required("describe_table"), ["connection", "table"]);
    assert_eq!(required("run_query"), ["connection", "sql"]);
    for tool in &tools {
        assert!(
            tool.description.as_deref().is_some_and(|d| !d.is_empty()),
            "{} has no description",
            tool.name
        );
    }
}

#[tokio::test]
async fn call_list_connections_returns_text() {
    let s = connect("test-client").await;
    let result = s
        .client
        .call_tool(CallToolRequestParams::new("list_connections"))
        .await
        .expect("call");
    assert_eq!(text_of(&result), ai::list_connections(&s.state).await.text);
    assert!(matches!(result.is_error, Some(false) | None));
}

#[tokio::test]
async fn bad_sql_is_a_tool_error_not_a_protocol_error() {
    let s = connect("test-client").await;
    let result = s
        .client
        .call_tool(CallToolRequestParams::new("run_query").with_arguments(args(
            serde_json::json!({
                "connection": "Local",
                "sql": "DELETE FROM dbo.clientes"
            }),
        )))
        .await
        .expect("a tool error is still a result");
    assert_eq!(result.is_error, Some(true));
    assert!(
        text_of(&result).starts_with("Not run:"),
        "{}",
        text_of(&result)
    );
}

/// rmcp 3.8 answers arguments that do not fit the schema as a tool error the
/// model can read and fix (not as a JSON-RPC error), and never runs the tool.
#[tokio::test]
async fn missing_required_argument_is_invalid_params() {
    let s = connect("test-client").await;
    let result = s
        .client
        .call_tool(
            CallToolRequestParams::new("run_query")
                .with_arguments(args(serde_json::json!({ "connection": "Local" }))),
        )
        .await
        .expect("rmcp answers with a tool error");
    assert_eq!(result.is_error, Some(true));
    assert!(
        text_of(&result).contains("missing field `sql`"),
        "{}",
        text_of(&result)
    );
    assert!(s.fixture.runlog.list(None, 1).is_empty());
    assert!(s.fixture.driver.script.sqls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn client_name_is_recorded_in_the_run_log() {
    let s = connect("claude-code").await;
    let result = s
        .client
        .call_tool(CallToolRequestParams::new("run_query").with_arguments(args(
            serde_json::json!({
                "connection": "Local",
                "sql": "SELECT 1"
            }),
        )))
        .await
        .expect("call");
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    let log = s.fixture.runlog.list(None, 10);
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].client.as_deref(), Some("claude-code"));
}

#[tokio::test]
async fn cancelling_a_call_cancels_the_query() {
    let s = connect("test-client").await;
    let request = ClientRequest::CallToolRequest(Request::new(
        CallToolRequestParams::new("run_query").with_arguments(args(serde_json::json!({
            "connection": "Local",
            "sql": "SELECT slow"
        }))),
    ));
    let handle = s
        .client
        .send_cancellable_request(request, PeerRequestOptions::no_options())
        .await
        .expect("sent");
    // Wait until the query reached the driver, then give up on it.
    for _ in 0..200 {
        if !s.fixture.driver.script.sqls.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!s.fixture.driver.script.sqls.lock().unwrap().is_empty());
    handle
        .cancel(Some("test".into()))
        .await
        .expect("cancel sent");
    for _ in 0..200 {
        if let Some(entry) = s.fixture.runlog.list(None, 1).first() {
            assert_eq!(entry.outcome, RunLogOutcome::Cancelled);
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the cancelled query was never logged");
}
