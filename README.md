# tulkabd

A lightweight, cross-platform desktop client for Microsoft SQL Server, built with Tauri 2, Rust and Svelte.

## Requirements

- [Rust](https://rustup.rs) (stable toolchain)
- [Node.js](https://nodejs.org) 22
- [pnpm](https://pnpm.io)
- Docker (only for integration tests)
- Tauri system dependencies for your OS:
  - **Fedora:** `sudo dnf install webkit2gtk4.1-devel libsoup3-devel javascriptcoregtk4.1-devel libappindicator-gtk3-devel librsvg2-devel openssl-devel curl wget file gcc gcc-c++`
  - **Debian/Ubuntu:** `sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev build-essential curl wget file`
  - **macOS:** Xcode Command Line Tools (`xcode-select --install`)
  - **Windows:** Microsoft Edge WebView2 runtime and the MSVC C++ build tools (Visual Studio Build Tools)

See the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for details.

## Development

```bash
pnpm install
pnpm tauri dev
```

## Integration tests

Integration tests run against a real SQL Server 2022 instance in Docker. The script starts the container (reusing it if it already exists) and prints the connection string to export:

```bash
eval "$(bash scripts/mssql-test.sh)"
cargo test
```

`TULKA_MSSQL_TEST` has the form `server=127.0.0.1;port=14333;user=sa;password=...`. Tests that need a database are skipped when it is not set.
