// Prevents additional console window on Windows in release, DO NOT REMOVE!!
// Also keeps `tulkabd mcp` from opening a console window; MCP clients pass
// pipes, which a GUI process inherits.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `tulkabd mcp`: an MCP server over stdin/stdout for AI assistants. It
    // must decide before anything else runs, so no window, webview or GTK
    // is ever created in that mode.
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|a| a == "mcp") {
        if args.next().is_some() {
            eprintln!("usage: tulkabd mcp");
            std::process::exit(2);
        }
        std::process::exit(tulka_mcp::main_stdio(env!("CARGO_PKG_VERSION")));
    }
    // On Wayland with NVIDIA, WebKitGTK's DMABUF renderer aborts with
    // "Gdk Error 71 (Protocol error)", so turn it off unless the user chose.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: first statement of main, so no other thread exists yet.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    // GTK's own GL context is unused (WebKit renders in its web process) but
    // pulls in the GL driver stack; turning it off saves ~10 MB of idle PSS.
    #[cfg(target_os = "linux")]
    if std::env::var_os("GDK_GL").is_none() {
        // SAFETY: still single-threaded, see above.
        unsafe { std::env::set_var("GDK_GL", "disable") };
    }
    tulkabd_lib::run()
}
