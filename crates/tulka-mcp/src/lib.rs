//! MCP server for tulkabd: the AI tools of `tulka_app::ai` over stdio.
//!
//! Standard output belongs to the protocol. Nothing else may write to it;
//! diagnostics go to standard error.

mod server;

use std::io::Write;
use std::sync::Arc;

use rmcp::{transport::stdio, ServerHandler, ServiceExt};
use tulka_app::ai::AiState;

pub use server::TulkaServer;

/// Runs the server on standard input and output until the client closes
/// them. Returns the process exit code: 0 when the client left, 1 when the
/// transport failed, 2 when there is no standard input and output to serve.
pub fn main_stdio() -> i32 {
    if !has_stdio() {
        report_missing_stdio();
        return 2;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return fail(&format!("cannot start the async runtime: {e}")),
    };
    runtime.block_on(async {
        let server = TulkaServer::new(Arc::new(AiState::open_default()));
        serve(server).await
    })
}

/// Serves `server` on standard input and output.
async fn serve(server: impl ServerHandler) -> i32 {
    let running = match server.serve(stdio()).await {
        Ok(running) => running,
        // The client left before it said hello.
        Err(rmcp::service::ServerInitializeError::ConnectionClosed(_)) => return 0,
        Err(e) => return fail(&format!("cannot start the MCP session: {e}")),
    };
    match running.waiting().await {
        Ok(rmcp::service::QuitReason::JoinError(e)) | Err(e) => {
            fail(&format!("the server stopped: {e}"))
        }
        // The client closed the pipe, or asked the server to stop.
        Ok(_) => 0,
    }
}

fn fail(message: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "tulka-mcp: {message}");
    1
}

#[cfg(not(windows))]
fn has_stdio() -> bool {
    true
}

/// A Windows program built for the GUI subsystem starts without standard
/// handles unless its parent passes them, so a double click leaves them null
/// or invalid.
#[cfg(windows)]
fn has_stdio() -> bool {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};

    [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE].into_iter().all(|id| {
        // SAFETY: GetStdHandle only reads the process's handle table.
        let handle = unsafe { GetStdHandle(id) };
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    })
}

const USAGE: &str = "tulka-mcp is an MCP server and talks over standard input and output.\n\
Do not start it by hand: add it to the MCP configuration of your AI assistant.\n";

#[cfg(not(windows))]
fn report_missing_stdio() {
    let _ = std::io::stderr().write_all(USAGE.as_bytes());
}

/// Without standard handles the usage text has nowhere to go, so borrow the
/// console of the program that started this one, if it has any.
#[cfg(windows)]
fn report_missing_stdio() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

    // SAFETY: no pointers are involved; failure just means no console.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    if let Ok(mut console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        let _ = console.write_all(USAGE.as_bytes());
    }
}
