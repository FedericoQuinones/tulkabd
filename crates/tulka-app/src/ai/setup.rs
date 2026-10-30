//! The commands and JSON a user pastes into an AI assistant to register
//! tulkabd as an MCP server.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::json;

use crate::types::{Clipboard, NOT_FOUND};

const SERVER_NAME: &str = "tulkabd";

/// What to paste into each assistant.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct McpSetup {
    /// Absolute path of this executable (the AppImage itself under `$APPIMAGE`).
    pub command: String,
    pub args: Vec<String>,
    /// `claude mcp add tulkabd -- <command> mcp`, with the command quoted
    /// only when it contains whitespace.
    pub claude_code: String,
    /// `{"mcpServers": {"tulkabd": {"command": ..., "args": ["mcp"]}}}`, pretty,
    /// for Cursor and Claude Desktop.
    pub mcp_servers_json: String,
    /// `{"servers": {"tulkabd": {"type": "stdio", ...}}}`, for VS Code `.vscode/mcp.json`.
    pub vscode_json: String,
}

/// The executable to register: the AppImage file when running from one (the
/// mounted binary disappears when the app closes), otherwise the current one.
fn exe_for(current: PathBuf, appimage: Option<String>) -> String {
    match appimage {
        Some(p) if !p.is_empty() => p,
        _ => current.to_string_lossy().into_owned(),
    }
}

/// The setup for this running executable.
pub fn mcp_setup() -> McpSetup {
    let current = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(SERVER_NAME));
    mcp_setup_for(&exe_for(current, std::env::var("APPIMAGE").ok()))
}

/// The setup for the executable at `exe`.
pub fn mcp_setup_for(exe: &str) -> McpSetup {
    let args = vec!["mcp".to_string()];
    let shown = if exe.chars().any(char::is_whitespace) {
        format!("\"{exe}\"")
    } else {
        exe.to_string()
    };
    let claude_code = format!("claude mcp add {SERVER_NAME} -- {shown} mcp");
    let pretty = |v: serde_json::Value| serde_json::to_string_pretty(&v).unwrap_or_default();
    McpSetup {
        command: exe.to_string(),
        mcp_servers_json: pretty(json!({
            "mcpServers": { SERVER_NAME: { "command": exe, "args": args } }
        })),
        vscode_json: pretty(json!({
            "servers": { SERVER_NAME: { "type": "stdio", "command": exe, "args": args } }
        })),
        args,
        claude_code,
    }
}

/// Copies one of the setups: `"claude"`, `"json"` or `"vscode"`. Any other
/// kind fails with [`NOT_FOUND`].
pub fn copy_mcp_setup(kind: &str, clipboard: Arc<dyn Clipboard>) -> Result<(), String> {
    let setup = mcp_setup();
    let text = match kind {
        "claude" => setup.claude_code,
        "json" => setup.mcp_servers_json,
        "vscode" => setup.vscode_json,
        _ => return Err(NOT_FOUND.to_string()),
    };
    clipboard.write_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nop;
    impl Clipboard for Nop {
        fn write_text(&self, _: String) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn setup_quotes_paths_with_spaces() {
        let s = mcp_setup_for(r"C:\Program Files\tulkabd\tulkabd.exe");
        assert_eq!(
            s.claude_code,
            r#"claude mcp add tulkabd -- "C:\Program Files\tulkabd\tulkabd.exe" mcp"#
        );
        assert_eq!(
            mcp_setup_for("/usr/bin/tulkabd").claude_code,
            "claude mcp add tulkabd -- /usr/bin/tulkabd mcp"
        );
    }

    #[test]
    fn setup_json_escapes_windows_paths() {
        let s = mcp_setup_for(r"C:\Program Files\tulkabd\tulkabd.exe");
        assert!(s
            .mcp_servers_json
            .contains(r#""command": "C:\\Program Files\\tulkabd\\tulkabd.exe""#));
        let v: serde_json::Value = serde_json::from_str(&s.vscode_json).unwrap();
        assert_eq!(
            v["servers"]["tulkabd"]["command"],
            r"C:\Program Files\tulkabd\tulkabd.exe"
        );
        assert_eq!(v["servers"]["tulkabd"]["type"], "stdio");
        let v: serde_json::Value = serde_json::from_str(&s.mcp_servers_json).unwrap();
        assert_eq!(v["mcpServers"]["tulkabd"]["args"][0], "mcp");
    }

    #[test]
    fn setup_prefers_appimage_env() {
        let cur = PathBuf::from("/tmp/.mount_x/usr/bin/tulkabd");
        assert_eq!(
            exe_for(cur.clone(), Some("/home/u/tulkabd.AppImage".into())),
            "/home/u/tulkabd.AppImage"
        );
        assert_eq!(exe_for(cur.clone(), None), "/tmp/.mount_x/usr/bin/tulkabd");
        assert_eq!(
            exe_for(cur, Some(String::new())),
            "/tmp/.mount_x/usr/bin/tulkabd"
        );
    }

    #[test]
    fn copy_unknown_kind_is_not_found() {
        assert_eq!(
            copy_mcp_setup("nope", Arc::new(Nop)),
            Err("not_found".into())
        );
    }
}
