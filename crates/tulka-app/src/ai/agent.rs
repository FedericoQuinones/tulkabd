//! Claude Code on this computer: finding it, connecting it to tulkabd and
//! opening it in a terminal with the tulkabd tools ready.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;

use super::setup::mcp_setup;

const SERVER_NAME: &str = "tulkabd";
/// Answer of the commands that need Claude Code but it is not installed.
pub const CLAUDE_NOT_FOUND: &str = "claude_not_found";
/// How long `claude --version` may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the other `claude` commands may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The system prompt given to Claude Code when it runs inside tulkabd.
pub const SYSTEM_PROMPT: &str = "You are a database diagnostics assistant inside tulkabd, a SQL Server client. You can only use the tulkabd tools, which are read-only. Start with list_connections. Use search_objects and describe_table to find the right tables instead of guessing names. When you reach a conclusion, show the SQL that supports it. Never claim to have changed data. Answer in the language the user writes in.";

/// What is known about Claude Code and tulkabd.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatus {
    pub installed: bool,
    pub version: Option<String>,
    /// tulkabd is registered in Claude Code and points at this executable.
    pub connected: bool,
    /// tulkabd is registered in Claude Code but points at another executable.
    pub connected_elsewhere: bool,
    pub path: Option<String>,
}

/// A command that runs a program in `dir`; on Windows it hides the console
/// window.
pub(crate) fn command(program: &Path) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program);
    cmd.kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn is_runnable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Looks for Claude Code in the `PATH` and then in the usual install places.
pub fn find_claude_in(
    path_var: Option<&OsStr>,
    home: Option<&Path>,
    windows: bool,
) -> Option<PathBuf> {
    let names: &[&str] = if windows {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let mut dirs: Vec<PathBuf> = path_var
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    if let Some(home) = home {
        if windows {
            dirs.push(home.join(".local").join("bin"));
            if let Some(appdata) = std::env::var_os("APPDATA") {
                dirs.push(PathBuf::from(appdata).join("npm"));
            }
        } else {
            dirs.push(home.join(".local/bin"));
            dirs.push(home.join(".claude/local"));
            dirs.push(PathBuf::from("/usr/local/bin"));
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
        }
    } else if !windows {
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
    }
    dirs.iter()
        .filter(|d| !d.as_os_str().is_empty())
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|c| is_runnable(c))
}

/// Claude Code on this computer, if it is installed.
pub fn find_claude() -> Option<PathBuf> {
    let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf());
    find_claude_in(
        std::env::var_os("PATH").as_deref(),
        home.as_deref(),
        cfg!(windows),
    )
}

/// What `program` prints and whether it succeeded, or `None` when it did not
/// run or took longer than `timeout`.
async fn run(program: &Path, args: &[&OsStr], timeout: Duration) -> Option<(bool, String)> {
    let mut cmd = command(program);
    cmd.args(args);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some((out.status.success(), text))
}

async fn status_with(program: &Path, exe: &str) -> ClaudeStatus {
    let mut status = ClaudeStatus {
        path: Some(program.to_string_lossy().into_owned()),
        ..ClaudeStatus::default()
    };
    let Some((ok, text)) = run(program, &[OsStr::new("--version")], VERSION_TIMEOUT).await else {
        return status;
    };
    if !ok {
        return status;
    }
    status.installed = true;
    status.version = text
        .lines()
        .next()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let get = [
        OsStr::new("mcp"),
        OsStr::new("get"),
        OsStr::new(SERVER_NAME),
    ];
    if let Some((true, text)) = run(program, &get, COMMAND_TIMEOUT).await {
        if text.contains(exe) {
            status.connected = true;
        } else {
            status.connected_elsewhere = true;
        }
    }
    status
}

/// Whether Claude Code is installed and connected to this tulkabd.
pub async fn claude_status() -> ClaudeStatus {
    match find_claude() {
        Some(program) => status_with(&program, &mcp_setup().command).await,
        None => ClaudeStatus::default(),
    }
}

async fn connect_with(program: &Path, exe: &str) -> Result<ClaudeStatus, String> {
    let status = status_with(program, exe).await;
    if !status.installed {
        return Err(CLAUDE_NOT_FOUND.to_string());
    }
    if status.connected {
        return Ok(status);
    }
    if status.connected_elsewhere {
        let remove = ["mcp", "remove", "--scope", "user", SERVER_NAME].map(OsStr::new);
        run(program, &remove, COMMAND_TIMEOUT).await;
    }
    let add = [
        OsStr::new("mcp"),
        OsStr::new("add"),
        OsStr::new("--scope"),
        OsStr::new("user"),
        OsStr::new(SERVER_NAME),
        OsStr::new("--"),
        OsStr::new(exe),
        OsStr::new("mcp"),
    ];
    match run(program, &add, COMMAND_TIMEOUT).await {
        Some((true, _)) => {}
        Some((false, text)) => {
            let text = text.trim();
            return Err(if text.is_empty() {
                "claude mcp add failed".to_string()
            } else {
                text.to_string()
            });
        }
        None => return Err("claude mcp add did not finish".to_string()),
    }
    Ok(status_with(program, exe).await)
}

/// Registers this tulkabd in Claude Code (replacing a registration that
/// points at another executable).
pub async fn connect_claude() -> Result<ClaudeStatus, String> {
    let program = find_claude().ok_or_else(|| CLAUDE_NOT_FOUND.to_string())?;
    connect_with(&program, &mcp_setup().command).await
}

/// `<data dir>/ai`, created when missing.
pub fn ai_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("dev", "tulkabd", "tulkabd")?;
    let dir = dirs.data_dir().join("ai");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The files Claude Code is started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiFiles {
    pub mcp_config: PathBuf,
    pub system_prompt: PathBuf,
}

/// Writes the MCP config that points at the executable `exe`, and the system
/// prompt, into `dir`.
pub fn write_ai_files(dir: &Path, exe: &str) -> std::io::Result<AiFiles> {
    std::fs::create_dir_all(dir)?;
    let files = AiFiles {
        mcp_config: dir.join("mcp.json"),
        system_prompt: dir.join("system-prompt.md"),
    };
    let config = json!({
        "mcpServers": { SERVER_NAME: { "command": exe, "args": ["mcp"] } }
    });
    std::fs::write(
        &files.mcp_config,
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    )?;
    std::fs::write(&files.system_prompt, SYSTEM_PROMPT)?;
    Ok(files)
}

/// How a terminal is told what to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalStyle {
    /// `<program> <lead...> <command...>`, started in the folder.
    Exec,
    /// Windows Terminal: `wt.exe -d <dir> <command...>`.
    WindowsTerminal,
    /// `cmd /C start "" /D <dir> cmd /K <command...>`.
    CmdStart,
    /// Terminal.app through `osascript`.
    AppleScript,
}

/// A way to open a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalCmd {
    pub program: String,
    /// Arguments between the program and the command to run.
    pub lead: Vec<String>,
    pub style: TerminalStyle,
}

fn exec(program: &str, lead: &[&str]) -> TerminalCmd {
    TerminalCmd {
        program: program.to_string(),
        lead: lead.iter().map(|s| s.to_string()).collect(),
        style: TerminalStyle::Exec,
    }
}

/// The terminals to try on `os` (as in `std::env::consts::OS`), best first.
pub fn terminal_candidates(os: &str, env_terminal: Option<String>) -> Vec<TerminalCmd> {
    match os {
        "windows" => vec![
            TerminalCmd {
                program: "wt.exe".into(),
                lead: Vec::new(),
                style: TerminalStyle::WindowsTerminal,
            },
            TerminalCmd {
                program: "cmd.exe".into(),
                lead: Vec::new(),
                style: TerminalStyle::CmdStart,
            },
        ],
        "macos" => vec![TerminalCmd {
            program: "osascript".into(),
            lead: Vec::new(),
            style: TerminalStyle::AppleScript,
        }],
        _ => {
            let mut list = Vec::new();
            if let Some(t) = env_terminal.filter(|t| !t.trim().is_empty()) {
                list.push(exec(t.trim(), &["-e"]));
            }
            list.extend([
                exec("x-terminal-emulator", &["-e"]),
                exec("ptyxis", &["--"]),
                exec("gnome-terminal", &["--"]),
                exec("konsole", &["-e"]),
                exec("xfce4-terminal", &["-x"]),
                exec("kitty", &[]),
                exec("alacritty", &["-e"]),
                exec("xterm", &["-e"]),
            ]);
            list
        }
    }
}

/// Whether `program` is a file in one of the `PATH` folders (or a path to
/// one).
fn on_path(program: &str, path_var: Option<&OsStr>) -> bool {
    let p = Path::new(program);
    if p.components().count() > 1 {
        return is_runnable(p);
    }
    path_var
        .map(|v| std::env::split_paths(v).any(|d| is_runnable(&d.join(program))))
        .unwrap_or(false)
}

/// Single-quotes `s` for a POSIX shell or AppleScript text.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The program and arguments that open `terminal` running Claude Code with
/// the tulkabd tools.
fn terminal_invocation(
    terminal: &TerminalCmd,
    dir: &Path,
    claude: &Path,
    mcp_config: &Path,
) -> Vec<OsString> {
    let claude_args = [
        claude.as_os_str().to_owned(),
        OsString::from("--mcp-config"),
        mcp_config.as_os_str().to_owned(),
    ];
    let mut args: Vec<OsString> = Vec::new();
    match terminal.style {
        TerminalStyle::Exec => {
            args.extend(terminal.lead.iter().map(OsString::from));
            args.extend(claude_args);
        }
        TerminalStyle::WindowsTerminal => {
            args.extend([OsString::from("-d"), dir.as_os_str().to_owned()]);
            args.extend(claude_args);
        }
        TerminalStyle::CmdStart => {
            args.extend(
                ["/C", "start", "", "/D"]
                    .iter()
                    .map(OsString::from)
                    .chain([dir.as_os_str().to_owned(), OsString::from("cmd")])
                    .chain([OsString::from("/K")]),
            );
            args.extend(claude_args);
        }
        TerminalStyle::AppleScript => {
            let script = format!(
                "cd {} && {} --mcp-config {}",
                sh_quote(&dir.to_string_lossy()),
                sh_quote(&claude.to_string_lossy()),
                sh_quote(&mcp_config.to_string_lossy())
            );
            let script = script.replace('\\', "\\\\").replace('"', "\\\"");
            args.extend([
                OsString::from("-e"),
                OsString::from(format!(
                    "tell application \"Terminal\" to do script \"{script}\""
                )),
                OsString::from("-e"),
                OsString::from("tell application \"Terminal\" to activate"),
            ]);
        }
    }
    args
}

/// Opens a terminal running Claude Code with the tulkabd tools. The error is
/// [`CLAUDE_NOT_FOUND`], or the text of what went wrong (with the command to
/// run by hand when no terminal was found).
pub fn open_claude_terminal() -> Result<(), String> {
    let claude = find_claude().ok_or_else(|| CLAUDE_NOT_FOUND.to_string())?;
    let dir = ai_dir().ok_or("could not find the data folder")?;
    let files = write_ai_files(&dir, &mcp_setup().command)
        .map_err(|e| format!("could not write the files for Claude Code: {e}"))?;
    let path_var = std::env::var_os("PATH");
    let candidates = terminal_candidates(std::env::consts::OS, std::env::var("TERMINAL").ok());
    let Some(terminal) = candidates
        .iter()
        .find(|t| on_path(&t.program, path_var.as_deref()))
    else {
        return Err(format!(
            "no terminal found; run this in one: cd {} && {} --mcp-config {}",
            sh_quote(&dir.to_string_lossy()),
            sh_quote(&claude.to_string_lossy()),
            sh_quote(&files.mcp_config.to_string_lossy())
        ));
    };
    let args = terminal_invocation(terminal, &dir, &claude, &files.mcp_config);
    let mut cmd = std::process::Command::new(&terminal.program);
    cmd.args(args)
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn()
        .map(drop)
        .map_err(|e| format!("could not open {}: {e}", terminal.program))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starting a program right after writing it can fail with "text file
    /// busy" when another test forks at that moment, so those tests take turns.
    #[cfg(unix)]
    static SPAWN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn finds_claude_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let claude = script(dir.path(), "claude", "true");
        let found = find_claude_in(Some(dir.path().as_os_str()), None, false);
        assert_eq!(found, Some(claude));
    }

    #[cfg(unix)]
    #[test]
    fn finds_claude_in_fallback_dirs() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let claude = script(&bin, "claude", "true");
        let found = find_claude_in(Some(OsStr::new("")), Some(home.path()), false);
        assert_eq!(found, Some(claude));
    }

    #[cfg(unix)]
    #[test]
    fn ignores_files_that_cannot_run() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("claude"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        assert_eq!(
            find_claude_in(Some(dir.path().as_os_str()), None, false),
            None
        );
    }

    #[test]
    fn prefers_exe_over_cmd_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["claude.cmd", "claude.exe"] {
            #[cfg(unix)]
            script(dir.path(), name, "true");
            #[cfg(not(unix))]
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        let found = find_claude_in(Some(dir.path().as_os_str()), None, true).unwrap();
        assert_eq!(found, dir.path().join("claude.exe"));
    }

    #[test]
    fn missing_claude_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            find_claude_in(Some(dir.path().as_os_str()), Some(home.path()), false),
            None
        );
    }

    #[test]
    fn writes_mcp_config_and_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let files = write_ai_files(dir.path(), "/opt/tulkabd").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&files.mcp_config).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["tulkabd"]["command"], "/opt/tulkabd");
        assert_eq!(v["mcpServers"]["tulkabd"]["args"], json!(["mcp"]));
        let prompt = std::fs::read_to_string(&files.system_prompt).unwrap();
        assert!(prompt.starts_with("You are a database diagnostics assistant"));
    }

    #[test]
    fn linux_terminals_start_with_env_terminal() {
        let list = terminal_candidates("linux", Some("foot".into()));
        assert_eq!(list[0].program, "foot");
        assert_eq!(list[1].program, "x-terminal-emulator");
        let list = terminal_candidates("linux", None);
        assert_eq!(list[0].program, "x-terminal-emulator");
    }

    #[test]
    fn windows_terminal_prefers_wt() {
        let list = terminal_candidates("windows", None);
        assert_eq!(list[0].program, "wt.exe");
        assert_eq!(list[1].style, TerminalStyle::CmdStart);
    }

    #[test]
    fn macos_script_quotes_paths() {
        let t = &terminal_candidates("macos", None)[0];
        let args = terminal_invocation(
            t,
            Path::new("/Users/o'neil/ai"),
            Path::new("/bin/claude"),
            Path::new("/Users/o'neil/ai/mcp.json"),
        );
        let script = args[1].to_string_lossy();
        assert!(script.contains(r"cd '/Users/o'\\''neil/ai' && '/bin/claude' --mcp-config"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn status_reads_version_and_registration() {
        let _turn = SPAWN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let claude = script(
            dir.path(),
            "claude",
            r#"case "$1" in --version) echo "2.0.1 (Claude Code)";; mcp) echo "Command: /opt/tulkabd";; esac"#,
        );
        let here = status_with(&claude, "/opt/tulkabd").await;
        assert!(here.installed && here.connected && !here.connected_elsewhere);
        assert_eq!(here.version.as_deref(), Some("2.0.1 (Claude Code)"));
        let other = status_with(&claude, "/elsewhere/tulkabd").await;
        assert!(other.connected_elsewhere && !other.connected);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn status_of_an_unregistered_server() {
        let _turn = SPAWN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let claude = script(
            dir.path(),
            "claude",
            r#"case "$1" in --version) echo 2.0.1;; mcp) exit 1;; esac"#,
        );
        let s = status_with(&claude, "/opt/tulkabd").await;
        assert!(s.installed && !s.connected && !s.connected_elsewhere);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_replaces_a_registration_elsewhere() {
        let _turn = SPAWN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("calls.log");
        let state = dir.path().join("registered");
        let claude = script(
            dir.path(),
            "claude",
            &format!(
                r#"echo "$@" >> '{log}'
case "$1" in
  --version) echo 2.0.1;;
  mcp) case "$2" in
    get) if [ -f '{state}' ]; then cat '{state}'; else echo "Command: /elsewhere/tulkabd"; fi;;
    remove) ;;
    add) echo "Command: $7" > '{state}';;
  esac;;
esac"#,
                log = log.display(),
                state = state.display()
            ),
        );
        let status = connect_with(&claude, "/opt/tulkabd").await.unwrap();
        assert!(status.connected);
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("mcp remove --scope user tulkabd"));
        assert!(calls.contains("mcp add --scope user tulkabd -- /opt/tulkabd mcp"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_reports_a_failed_add() {
        let _turn = SPAWN.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let claude = script(
            dir.path(),
            "claude",
            r#"case "$2" in add) echo boom >&2; exit 1;; get) exit 1;; esac; case "$1" in --version) echo 2.0.1;; esac"#,
        );
        let err = connect_with(&claude, "/opt/tulkabd").await.unwrap_err();
        assert_eq!(err, "boom");
    }
}
