//! The in-app chat: runs the user's own Claude Code in the background with
//! only the tulkabd tools and turns its `stream-json` output into
//! [`ChatEvent`]s.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

use super::agent::{self, AiFiles, CLAUDE_NOT_FOUND};
use super::setup::mcp_setup;

/// Prefix of the names Claude Code gives to the tulkabd tools.
const TOOL_PREFIX: &str = "mcp__tulkabd__";
/// Longest tool result preview, in characters.
const PREVIEW_CHARS: usize = 2000;
/// How much of what Claude Code wrote to stderr is kept, in bytes.
const STDERR_KEEP: usize = 4096;

/// One step of a chat run, as the window receives it.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatEvent {
    #[serde(rename_all = "camelCase")]
    Started {
        session_id: String,
    },
    /// A piece of the answer being written.
    TextDelta {
        text: String,
    },
    /// A whole text block; it replaces the deltas since the last `Text` or
    /// `ToolCall`.
    Text {
        text: String,
    },
    /// `tool` is the tool's name without the `mcp__tulkabd__` prefix.
    ToolCall {
        id: String,
        tool: String,
        input: Value,
    },
    ToolResult {
        id: String,
        ok: bool,
        preview: String,
    },
    #[serde(rename_all = "camelCase")]
    Done {
        duration_ms: u64,
        is_error: bool,
        result: String,
    },
    Failed {
        message: String,
    },
}

fn preview(text: &str) -> String {
    text.chars().take(PREVIEW_CHARS).collect()
}

/// The events one line of Claude Code's output stands for. Lines that are not
/// understood stand for none.
pub fn parse_line(line: &str) -> Vec<ChatEvent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let str_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            str_of(&v, "session_id")
                .map(|session_id| vec![ChatEvent::Started { session_id }])
                .unwrap_or_default()
        }
        Some("stream_event") => {
            let event = &v["event"];
            let delta = &event["delta"];
            if event["type"] == "content_block_delta" && delta["type"] == "text_delta" {
                if let Some(text) = str_of(delta, "text") {
                    return vec![ChatEvent::TextDelta { text }];
                }
            }
            Vec::new()
        }
        Some("assistant") => blocks(&v)
            .iter()
            .filter_map(|b| match b["type"].as_str()? {
                "text" => Some(ChatEvent::Text {
                    text: str_of(b, "text")?,
                }),
                "tool_use" => {
                    let name = str_of(b, "name")?;
                    Some(ChatEvent::ToolCall {
                        id: str_of(b, "id")?,
                        tool: name.strip_prefix(TOOL_PREFIX).unwrap_or(&name).to_string(),
                        input: b.get("input").cloned().unwrap_or(Value::Null),
                    })
                }
                _ => None,
            })
            .collect(),
        Some("user") => blocks(&v)
            .iter()
            .filter(|b| b["type"] == "tool_result")
            .filter_map(|b| {
                let text = match &b["content"] {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                Some(ChatEvent::ToolResult {
                    id: str_of(b, "tool_use_id")?,
                    ok: !b["is_error"].as_bool().unwrap_or(false),
                    preview: preview(&text),
                })
            })
            .collect(),
        Some("result") => vec![ChatEvent::Done {
            duration_ms: v["duration_ms"].as_u64().unwrap_or(0),
            is_error: v["is_error"].as_bool().unwrap_or(false),
            result: str_of(&v, "result").unwrap_or_default(),
        }],
        _ => Vec::new(),
    }
}

fn blocks(v: &Value) -> Vec<Value> {
    v["message"]["content"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The arguments Claude Code is started with. The user's text is not among
/// them: it goes through stdin.
pub fn claude_args(files: &AiFiles, session_id: Option<&str>) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(files.mcp_config.clone().into_os_string());
    args.extend(
        ["--allowedTools", "mcp__tulkabd", "--setting-sources", ""]
            .into_iter()
            .map(OsString::from),
    );
    args.push("--append-system-prompt-file".into());
    args.push(files.system_prompt.clone().into_os_string());
    if let Some(id) = session_id {
        args.push("--resume".into());
        args.push(id.into());
    }
    args
}

/// The chats that are running, by chat id.
#[derive(Default)]
pub struct ChatRuns {
    running: Mutex<HashMap<String, Arc<Notify>>>,
}

impl ChatRuns {
    fn start(&self, chat_id: &str) -> Result<Arc<Notify>, String> {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if running.contains_key(chat_id) {
            return Err("busy".to_string());
        }
        let stop = Arc::new(Notify::new());
        running.insert(chat_id.to_string(), stop.clone());
        Ok(stop)
    }

    fn finish(&self, chat_id: &str) {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(chat_id);
    }
}

/// Stops the run of `chat_id`, if there is one.
pub async fn stop(runs: &ChatRuns, chat_id: &str) {
    let stop = runs
        .running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(chat_id)
        .cloned();
    if let Some(stop) = stop {
        stop.notify_one();
    }
}

/// Sends `text` to Claude Code for the chat `chat_id` (continuing
/// `session_id` when given) and reports what happens to `on_event` until the
/// run ends. Fails with `busy` while the chat is already running.
pub async fn send(
    runs: &ChatRuns,
    chat_id: String,
    session_id: Option<String>,
    text: String,
    on_event: impl Fn(ChatEvent) + Send + 'static,
) -> Result<(), String> {
    let stop = runs.start(&chat_id)?;
    let Some(program) = agent::find_claude() else {
        runs.finish(&chat_id);
        on_event(ChatEvent::Failed {
            message: CLAUDE_NOT_FOUND.to_string(),
        });
        return Ok(());
    };
    let Some(dir) = agent::ai_dir() else {
        runs.finish(&chat_id);
        on_event(ChatEvent::Failed {
            message: "could not create the folder for the assistant's files".to_string(),
        });
        return Ok(());
    };
    run_registered(
        runs,
        &stop,
        &program,
        &dir,
        &mcp_setup().command,
        chat_id,
        session_id,
        text,
        on_event,
    )
    .await
}

/// Runs a chat already registered in `runs` (with `stop` as its stop signal)
/// and removes it when it ends.
#[allow(clippy::too_many_arguments)]
async fn run_registered(
    runs: &ChatRuns,
    stop: &Notify,
    program: &Path,
    dir: &Path,
    exe: &str,
    chat_id: String,
    session_id: Option<String>,
    text: String,
    on_event: impl Fn(ChatEvent) + Send + 'static,
) -> Result<(), String> {
    let outcome = run(
        program,
        dir,
        exe,
        session_id.as_deref(),
        text,
        stop,
        &on_event,
    )
    .await;
    runs.finish(&chat_id);
    if let Err(message) = outcome {
        on_event(ChatEvent::Failed { message });
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[allow(clippy::too_many_arguments)]
async fn send_with(
    runs: &ChatRuns,
    program: &Path,
    dir: &Path,
    exe: &str,
    chat_id: String,
    session_id: Option<String>,
    text: String,
    on_event: impl Fn(ChatEvent) + Send + 'static,
) -> Result<(), String> {
    let stop = runs.start(&chat_id)?;
    run_registered(
        runs, &stop, program, dir, exe, chat_id, session_id, text, on_event,
    )
    .await
}

/// Runs Claude Code to its end. `Err` is a failure not yet reported.
async fn run(
    program: &Path,
    dir: &Path,
    exe: &str,
    session_id: Option<&str>,
    text: String,
    stop: &Notify,
    on_event: &impl Fn(ChatEvent),
) -> Result<(), String> {
    let files = agent::write_ai_files(dir, exe)
        .map_err(|e| format!("could not write the assistant's files: {e}"))?;
    let mut cmd = agent::command(program);
    cmd.args(claude_args(&files, session_id))
        .current_dir(dir)
        .stdin(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start Claude Code: {e}"))?;

    // The prompt goes through stdin, from its own task so a full pipe cannot
    // stall the reading below.
    if let Some(mut stdin) = child.stdin.take() {
        tokio::spawn(async move {
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let stderr = child.stderr.take();
    let stderr_tail = tokio::spawn(async move {
        let mut kept: Vec<u8> = Vec::new();
        if let Some(mut stderr) = stderr {
            let mut buf = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                kept.extend_from_slice(&buf[..n]);
                if kept.len() > STDERR_KEEP {
                    kept.drain(..kept.len() - STDERR_KEEP);
                }
            }
        }
        String::from_utf8_lossy(&kept).trim().to_string()
    });

    let mut out = BufReader::new(child.stdout.take().ok_or("no output pipe")?);
    let mut done = false;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        tokio::select! {
            () = stop.notified() => {
                let _ = child.kill().await;
                return if done { Ok(()) } else { Err("stopped".to_string()) };
            }
            read = out.read_until(b'\n', &mut buf) => match read {
                Ok(n) if n > 0 => {
                    for ev in parse_line(&String::from_utf8_lossy(&buf)) {
                        done |= matches!(ev, ChatEvent::Done { .. });
                        on_event(ev);
                    }
                }
                _ => break,
            }
        }
    }
    drop(out);
    let status = tokio::select! {
        () = stop.notified() => {
            let _ = child.kill().await;
            return if done { Ok(()) } else { Err("stopped".to_string()) };
        }
        status = child.wait() => status,
    };
    let tail = tokio::time::timeout(Duration::from_secs(1), stderr_tail)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    if done {
        return Ok(());
    }
    if !tail.is_empty() {
        return Err(tail);
    }
    let code = status
        .ok()
        .and_then(|s| s.code())
        .map_or("none".to_string(), |c| c.to_string());
    Err(format!(
        "Claude Code stopped without an answer (exit code {code})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude-stream");

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()
    }

    fn parse_all(text: &str) -> Vec<ChatEvent> {
        text.lines().flat_map(parse_line).collect()
    }

    #[test]
    fn parses_init_text_tool_and_result() {
        assert_eq!(
            parse_all(&fixture("conversation.jsonl")),
            vec![
                ChatEvent::Started {
                    session_id: "sess-0001".into()
                },
                ChatEvent::TextDelta {
                    text: "Let me ".into()
                },
                ChatEvent::TextDelta {
                    text: "check.".into()
                },
                ChatEvent::Text {
                    text: "Let me check.".into()
                },
                ChatEvent::ToolCall {
                    id: "toolu_01".into(),
                    tool: "run_query".into(),
                    input: serde_json::json!({"connection":"Sales","database":"shop","sql":"SELECT 1"}),
                },
                ChatEvent::ToolResult {
                    id: "toolu_01".into(),
                    ok: true,
                    preview: "1 row\n1".into()
                },
                ChatEvent::Done {
                    duration_ms: 1234,
                    is_error: false,
                    result: "Done: one row.".into()
                },
            ]
        );
    }

    #[test]
    fn tool_result_string_content_and_error() {
        let line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_02","content":"boom","is_error":true}]}}"#;
        assert_eq!(
            parse_line(line),
            vec![ChatEvent::ToolResult {
                id: "toolu_02".into(),
                ok: false,
                preview: "boom".into()
            }]
        );
        let long = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t","content":"{}"}}]}}}}"#,
            "x".repeat(5000)
        );
        match &parse_line(&long)[0] {
            ChatEvent::ToolResult { ok, preview, .. } => {
                assert!(ok);
                assert_eq!(preview.chars().count(), PREVIEW_CHARS);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn unknown_lines_are_ignored() {
        for line in [
            r#"{"type":"rate_limit_event"}"#,
            r#"{"type":"system","subtype":"hook_started"}"#,
            "not json",
            "",
        ] {
            assert_eq!(parse_line(line), Vec::new(), "{line}");
        }
    }

    #[test]
    fn args_resume_and_no_user_text() {
        let files = AiFiles {
            mcp_config: Path::new("d").join("mcp.json"),
            system_prompt: Path::new("d").join("system-prompt.md"),
        };
        let text = |a: Vec<OsString>| {
            a.iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let base: Vec<String> = [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
        ]
        .iter()
        .map(|s| s.to_string())
        .chain([
            files.mcp_config.to_string_lossy().into_owned(),
            "--allowedTools".into(),
            "mcp__tulkabd".into(),
            "--setting-sources".into(),
            "".into(),
            "--append-system-prompt-file".into(),
            files.system_prompt.to_string_lossy().into_owned(),
        ])
        .collect();
        assert_eq!(text(claude_args(&files, None)), base);
        let mut resumed = base;
        resumed.extend(["--resume".into(), "abc".into()]);
        assert_eq!(text(claude_args(&files, Some("abc"))), resumed);
    }

    #[cfg(unix)]
    mod run {
        use super::*;
        use std::path::PathBuf;
        use std::sync::mpsc;

        /// Starting a program right after writing it can fail with "text file
        /// busy" when another test forks at that moment, so those tests take
        /// turns.
        static SPAWN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

        fn script(dir: &Path, body: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("claude");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        async fn go(
            runs: &ChatRuns,
            program: &Path,
            dir: &Path,
            text: &str,
        ) -> (Result<(), String>, Vec<ChatEvent>) {
            let (tx, rx) = mpsc::channel();
            let out = send_with(
                runs,
                program,
                &dir.join("ai"),
                "tulkabd",
                "chat".into(),
                None,
                text.into(),
                move |e| {
                    let _ = tx.send(e);
                },
            )
            .await;
            (out, rx.try_iter().collect())
        }

        #[tokio::test]
        async fn send_writes_prompt_to_stdin() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let seen = dir.path().join("stdin.txt");
            let fixture = Path::new(FIXTURES).join("stream-only.jsonl");
            let program = script(
                dir.path(),
                &format!("cat > '{}'\ncat '{}'", seen.display(), fixture.display()),
            );
            let (out, events) = go(&ChatRuns::default(), &program, dir.path(), "hello there").await;
            assert_eq!(out, Ok(()));
            assert_eq!(std::fs::read_to_string(&seen).unwrap(), "hello there");
            assert_eq!(
                events,
                vec![
                    ChatEvent::Started {
                        session_id: "sess-0002".into()
                    },
                    ChatEvent::Done {
                        duration_ms: 5,
                        is_error: false,
                        result: "ok".into()
                    }
                ]
            );
        }

        #[tokio::test]
        async fn nonzero_exit_without_result_is_failed() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let program = script(dir.path(), "echo 'Not logged in' >&2\nexit 1");
            let (_, events) = go(&ChatRuns::default(), &program, dir.path(), "hi").await;
            match events.as_slice() {
                [ChatEvent::Failed { message }] => assert!(message.contains("Not logged in")),
                other => panic!("unexpected {other:?}"),
            }
        }

        #[tokio::test]
        async fn exit_without_output_names_the_code() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let program = script(dir.path(), "exit 3");
            let (_, events) = go(&ChatRuns::default(), &program, dir.path(), "hi").await;
            assert_eq!(
                events,
                vec![ChatEvent::Failed {
                    message: "Claude Code stopped without an answer (exit code 3)".into()
                }]
            );
        }

        #[tokio::test]
        async fn stop_kills_the_child() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let program = script(dir.path(), "sleep 30");
            let runs = Arc::new(ChatRuns::default());
            let stopper = runs.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                stop(&stopper, "chat").await;
            });
            let started = std::time::Instant::now();
            let (_, events) = go(&runs, &program, dir.path(), "hi").await;
            handle.await.unwrap();
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(
                events,
                vec![ChatEvent::Failed {
                    message: "stopped".into()
                }]
            );
        }

        #[tokio::test]
        async fn stop_after_the_answer_stays_quiet() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let program = script(
                dir.path(),
                r#"cat >/dev/null &
echo '{"type":"result","is_error":false,"result":"ok","duration_ms":1}'
sleep 30"#,
            );
            let runs = Arc::new(ChatRuns::default());
            let stopper = runs.clone();
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                stop(&stopper, "chat").await;
            });
            let (out, events) = go(&runs, &program, dir.path(), "hi").await;
            handle.await.unwrap();
            assert_eq!(out, Ok(()));
            assert_eq!(
                events,
                vec![ChatEvent::Done {
                    duration_ms: 1,
                    is_error: false,
                    result: "ok".into()
                }]
            );
        }

        #[tokio::test]
        async fn stop_before_the_run_starts_is_kept() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let program = script(dir.path(), "sleep 30");
            let runs = ChatRuns::default();
            let signal = runs.start("chat").unwrap();
            stop(&runs, "chat").await;
            let (tx, rx) = mpsc::channel();
            run_registered(
                &runs,
                &signal,
                &program,
                &dir.path().join("ai"),
                "tulkabd",
                "chat".into(),
                None,
                "hi".into(),
                move |e| {
                    let _ = tx.send(e);
                },
            )
            .await
            .unwrap();
            assert_eq!(
                rx.try_iter().collect::<Vec<_>>(),
                vec![ChatEvent::Failed {
                    message: "stopped".into()
                }]
            );
            assert!(runs.start("chat").is_ok(), "the entry is removed");
        }

        #[tokio::test]
        async fn busy_while_running() {
            let _turn = SPAWN.lock().await;
            let dir = tempfile::tempdir().unwrap();
            let program = script(dir.path(), "sleep 30");
            let runs = Arc::new(ChatRuns::default());
            let first = {
                let runs = runs.clone();
                let program = program.clone();
                let dir = dir.path().to_path_buf();
                tokio::spawn(async move { go(&runs, &program, &dir, "hi").await })
            };
            tokio::time::sleep(Duration::from_millis(300)).await;
            let (second, events) = go(&runs, &program, dir.path(), "again").await;
            assert_eq!(second, Err("busy".to_string()));
            assert!(events.is_empty());
            stop(&runs, "chat").await;
            let (_, events) = first.await.unwrap();
            assert_eq!(events.len(), 1);
        }
    }
}
