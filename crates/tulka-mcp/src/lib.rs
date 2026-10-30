//! MCP server for tulkabd: the AI tools of `tulka_app::ai` over stdio.
//!
//! Standard output belongs to the protocol. Nothing else may write to it;
//! diagnostics go to standard error.

mod server;

use std::io::Write;
use std::sync::Arc;

use std::time::Duration;

use rmcp::{
    service::{QuitReason, ServerInitializeError},
    transport::{stdio, IntoTransport},
    RoleServer, ServiceExt,
};
use tulka_app::ai::AiState;

pub use server::{InFlight, TulkaServer};

/// How long calls still running when the client leaves get to cancel their
/// queries and write their run log entries.
const DRAIN_LIMIT: Duration = Duration::from_secs(10);

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
        serve(server, stdio()).await
    })
}

/// Serves `server` on `transport` until the client leaves, then lets the
/// calls still running finish their bookkeeping. Returns the exit code.
#[doc(hidden)]
pub async fn serve<E, A>(
    server: TulkaServer,
    transport: impl IntoTransport<RoleServer, E, A>,
) -> i32
where
    E: std::error::Error + Send + Sync + 'static,
{
    let in_flight = server.in_flight();
    let running = match server.serve(transport).await {
        Ok(running) => running,
        // The client left before it said hello.
        Err(ServerInitializeError::ConnectionClosed(_)) => return 0,
        // Not the error's own text: it would print the client's message,
        // which could carry a query.
        Err(ServerInitializeError::ExpectedInitializeRequest(_)) => {
            return fail("the client did not start with an MCP initialize request")
        }
        Err(e) => return fail(&format!("cannot start the MCP session: {e}")),
    };
    let quit = running.waiting().await;
    in_flight.drained(DRAIN_LIMIT).await;
    match quit {
        // rmcp reports a failed read as the pipe closing, so a transport
        // error shows up as `Closed`; only a crashed service task lands here.
        Ok(QuitReason::JoinError(e)) | Err(e) => fail(&format!("the server stopped: {e}")),
        // The client closed the pipe, or asked the server to stop.
        Ok(_) => 0,
    }
}

fn fail(message: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "tulkabd: {message}");
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

const USAGE: &str =
    "tulkabd mcp speaks MCP over stdin/stdout; add it to an MCP client (see the README).\n";

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
