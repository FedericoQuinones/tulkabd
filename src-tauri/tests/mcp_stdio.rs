//! Runs the real `tulkabd mcp` binary over stdio against a throwaway config.

// `ProjectDirs` honors `XDG_*` only on Linux, which is what keeps this test
// away from the developer's real config, data and keychain.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tulka_app::tulka_store::{RunLogOutcome, RunSource, SharedRunLog};

fn config() -> Value {
    let conn = |name: &str, environment: &str, ai_access: &str| {
        json!({
            "id": format!("id-{name}"),
            "name": name,
            "driver": "mssql",
            "server": "127.0.0.1",
            "port": 1,
            "user": "sa",
            "databases": { "kind": "all" },
            "environment": environment,
            "color": "#4a8ad6",
            "guard": "off",
            "encrypt": false,
            "trustServerCertificate": true,
            "connectTimeoutS": 1,
            "passwordSaved": false,
            "aiAccess": ai_access,
        })
    };
    json!({
        "version": 1,
        "connections": [
            conn("Visible", "local", "readOnly"),
            conn("Hidden", "production", "none"),
        ],
    })
}

struct Client {
    stdin: Option<std::process::ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<String>,
}

impl Client {
    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn next_line(&mut self, within: Duration) -> String {
        let line = match self.lines.recv_timeout(within) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => panic!("no reply within {within:?}"),
            Err(RecvTimeoutError::Disconnected) => panic!("stdout closed early"),
        };
        self.seen.push(line.clone());
        line
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let reply: Value = serde_json::from_str(&self.next_line(Duration::from_secs(10)))
            .expect("stdout line is JSON");
        assert_eq!(reply["id"], id, "{reply}");
        reply
    }

    fn call(&mut self, id: u64, tool: &str, arguments: Value) -> Value {
        let reply = self.request(
            id,
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        );
        reply["result"].clone()
    }
}

fn text(result: &Value) -> String {
    result["content"]
        .as_array()
        .expect("content")
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn spawn(dir: &Path) -> (std::process::Child, Client) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tulkabd"))
        .arg("mcp")
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("HOME", dir.join("home"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tulkabd mcp");
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let client = Client {
        stdin: child.stdin.take(),
        lines,
        seen: Vec::new(),
    };
    (child, client)
}

#[test]
fn serves_the_tools_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config/tulkabd");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.json"), config().to_string()).unwrap();

    let started = Instant::now();
    let (mut child, mut client) = spawn(dir.path());

    let init = client.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "process-test", "version": "0" },
        }),
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "slow initialize"
    );
    assert!(init["result"]["serverInfo"].is_object(), "{init}");
    assert!(init["result"]["instructions"].is_string(), "{init}");

    client.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    let list = client.request(2, "tools/list", json!({}));
    let mut names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
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

    let connections = client.call(3, "list_connections", json!({}));
    let listed = text(&connections);
    assert!(listed.contains("Visible"), "{listed}");
    assert!(!listed.contains("Hidden"), "{listed}");

    let unsaved = client.call(
        4,
        "run_query",
        json!({ "connection": "Visible", "sql": "SELECT 1" }),
    );
    assert_eq!(unsaved["isError"], true, "{unsaved}");
    assert!(text(&unsaved).contains("not saved"), "{unsaved}");

    let hidden = client.call(
        5,
        "run_query",
        json!({ "connection": "Hidden", "sql": "SELECT 1" }),
    );
    assert_eq!(hidden["isError"], true, "{hidden}");
    assert!(
        text(&hidden).contains("No connection named \"Hidden\""),
        "{hidden}"
    );

    let closed = Instant::now();
    drop(client.stdin.take());
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(closed.elapsed() < Duration::from_secs(5), "did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(0));

    // The reader thread ends with the pipe; whatever is left is still stdout.
    while let Ok(line) = client.lines.recv_timeout(Duration::from_millis(200)) {
        client.seen.push(line);
    }
    for line in &client.seen {
        serde_json::from_str::<Value>(line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
    }

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains('{'), "{stderr}");
    assert!(!stderr.contains("passwordSaved"), "{stderr}");
    assert!(!stderr.to_lowercase().contains("password"), "{stderr}");

    let log = SharedRunLog::open(&dir.path().join("data/tulkabd/runlog.db"));
    let entries = log.list(None, 10);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].source, RunSource::Ai);
    assert_eq!(entries[0].outcome, RunLogOutcome::Error);
    assert_eq!(entries[0].connection_name, "Visible");
}

#[test]
fn mcp_with_extra_args_exits_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_tulkabd"))
        .args(["mcp", "extra"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
