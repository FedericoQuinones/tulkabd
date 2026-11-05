# tulkabd

A lightweight, cross-platform desktop client for Microsoft SQL Server, built with Tauri 2, Rust and Svelte.

## Requirements

- [Rust](https://rustup.rs) 1.88 or newer (stable toolchain)
- [Node.js](https://nodejs.org) 22
- [pnpm](https://pnpm.io)
- Docker (only for integration tests)
- Tauri system dependencies for your OS:
  - **Fedora:** `sudo dnf install webkit2gtk4.1-devel libsoup3-devel javascriptcoregtk4.1-devel libappindicator-gtk3-devel librsvg2-devel libxdo-devel openssl-devel curl wget file gcc gcc-c++`
  - **Debian/Ubuntu:** `sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev build-essential curl wget file`
  - **macOS:** Xcode Command Line Tools (`xcode-select --install`)
  - **Windows:** Microsoft Edge WebView2 runtime and the MSVC C++ build tools (Visual Studio Build Tools)

See the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for details.

## Use with AI assistants (MCP)

tulkabd can act as an MCP server so Claude Code, Cursor or VS Code Copilot can list your connections, find tables and run read-only queries. The assistant only sees connections whose **AI access** is **Read only** (Production connections and connections saved before this version start at **No access**). tulkabd adds the saved password itself; it never reaches the assistant.

### 1. Allow a connection

Edit connection › **AI access** › **Read only**. New Local, Test and Staging connections default to **Read only**; Production connections, and connections saved before this version, start at **No access**.

### 2. Register tulkabd in your assistant

Settings › **AI assistants** shows the exact path of your executable and copies the texts below. The examples use `/usr/bin/tulkabd`; replace it with your path:

- macOS: `/Applications/tulkabd.app/Contents/MacOS/tulkabd`
- Windows (MSI): `"C:\Program Files\tulkabd\tulkabd.exe"`
- Windows (NSIS): `"%LOCALAPPDATA%\tulkabd\tulkabd.exe"`

**Claude Code**

```bash
claude mcp add --scope user tulkabd -- /usr/bin/tulkabd mcp
```

Check it with `claude mcp list`.

**Cursor** (`~/.cursor/mcp.json`, or `.cursor/mcp.json` in a project) and **Claude Desktop** (`claude_desktop_config.json`):

```json
{ "mcpServers": { "tulkabd": { "command": "/usr/bin/tulkabd", "args": ["mcp"] } } }
```

**VS Code** (`.vscode/mcp.json`, or the "MCP: Add Server" command):

```json
{ "servers": { "tulkabd": { "type": "stdio", "command": "/usr/bin/tulkabd", "args": ["mcp"] } } }
```

Use the tools from Copilot Chat in Agent mode.

### Tools

| Tool | What it does |
| --- | --- |
| `list_connections` | Lists the connections that allow AI access. |
| `list_databases` | Lists the databases of a connection. |
| `search_objects` | Finds tables, views and procedures by name. |
| `describe_table` | Shows the columns, keys and indexes of a table or view. |
| `run_query` | Runs a read-only query and returns the rows. |

### Limits

Results are capped at 500 rows and queries at 30 seconds by default; both are configurable in Settings. Each cell is cut at 2000 characters and a whole answer at 100 000 characters. Only `SELECT` and `WITH … SELECT` are accepted (several statements separated by `;` are fine if each one is a query). The check is lexical and refuses a query that contains any of these words anywhere:

- Writes: `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `TRUNCATE`, `INTO` (so `SELECT … INTO`), `BULK`.
- Schema and permissions: `ALTER`, `CREATE`, `DROP`, `GRANT`, `REVOKE`, `DENY`, `BACKUP`, `RESTORE`.
- Running code: `EXEC`, `EXECUTE`, `OPENQUERY`, `OPENROWSET`, `OPENDATASOURCE`.
- State and flow: `SET`, `USE`, `DECLARE`, transactions, cursors, `IF`, `WHILE`, `WAITFOR`, `PRINT`, `THROW`, `KILL`, `DBCC`, `NEXT VALUE FOR` and similar.
- Table hints that lock: `UPDLOCK`, `XLOCK`, `TABLOCK`, `TABLOCKX`, `PAGLOCK`, `HOLDLOCK`, `SERIALIZABLE`, `REPEATABLEREAD`.
- Anything it cannot read with certainty (control characters, unusual symbols, a name with three or more dots, as in `server.db.dbo.t`).

The read-only check is conservative and has known limits:

- Columns named like a forbidden word that is not reserved (`Send`, `Get`, `Move`, `Open`, `Close`, `Save`, `Load`) must be bracketed, for example `[Open]`.
- Non-ASCII identifiers must be bracketed too, for example `[año]`.
- Names with three or more dots are taken as linked-server names and refused, so `db.dbo.t.col` and `dbo.t.xmlcol.value(…)` fail.
- The `MERGE` join hint is refused.
- The check cannot see inside user functions (including CLR) with external effects, or synonyms and views that point to linked servers.

### Safety notes

- Read only is enforced by tulkabd before anything is sent, and every AI query runs inside a transaction that is rolled back.
- A function or a view on the server can still have effects tulkabd cannot see. For a hard guarantee, give the connection a login that only has `db_datareader`.
- Use a least-privilege login for AI connections. With an administrator login, tulkabd cannot stop everything a plain `SELECT` can reach: it refuses the best-known file, password-hash and SQL-text sources (`sys.fn_xe_file_target_read_file`, `sys.fn_get_audit_file`, `fn_trace_gettable`, `fn_dump_dblog`, `fn_dblog`, `sys.dm_os_enumerate_filesystem`, `sys.dm_os_file_exists`, `sys.sql_logins`, `sys.dm_exec_sql_text`, `sys.dm_exec_input_buffer`, `fn_get_sql`), but others may exist. On Windows, a UNC path in such a function makes the service account authenticate over SMB. `list_connections` warns when a connection's login is a sysadmin.
- "Selected databases" only shapes what tulkabd lists and checks in names: `run_query` refuses `db.schema.object` names outside the selection, but `sys.databases` still lists every database and functions such as `DB_NAME(5)` still answer. A login mapped only to those databases is the real limit.
- Hints that can starve the server (`MAXDOP`, `MIN_GRANT_PERCENT`, `MAX_GRANT_PERCENT`, `QUERYTRACEON` inside `OPTION (…)`) are refused. The query timeout cannot be set above 300 seconds.
- Error text from the driver hides the server and the login, but a query can still `SELECT @@SERVERNAME`, `SUSER_SNAME()` or `HOST_NAME()`.
- A single huge cell is read whole from the server before being cut to 2000 characters, so it can use memory until the query ends or times out.
- Query text and results go to the AI provider: share only what you would paste into a chat.
- Every AI query is in the Run log, marked AI.

### Try it without a client

```bash
npx @modelcontextprotocol/inspector /usr/bin/tulkabd mcp
```

## Development

```bash
pnpm install
pnpm tauri dev      # run the desktop app
pnpm vitest run      # frontend tests
pnpm check           # svelte-check
cargo test           # Rust tests (whole workspace)
```

## Integration tests

Integration tests run against a real SQL Server 2022 instance in Docker. The script starts the container (reusing it if it already exists) and prints the connection string to export:

```bash
eval "$(bash scripts/mssql-test.sh)"
cargo test
```

`TULKA_MSSQL_TEST` has the form `server=127.0.0.1;port=14333;user=sa;password=...`. Tests that need a database are skipped when it is not set.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

The vendored Tiberius fork in `vendor/tiberius` keeps its own MIT/Apache-2.0 licenses.
