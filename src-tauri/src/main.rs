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
        std::process::exit(tulka_mcp::main_stdio());
    }
    // On Wayland with NVIDIA, WebKitGTK's DMABUF renderer aborts with
    // "Gdk Error 71 (Protocol error)", so turn it off unless the user chose.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // SAFETY: first statement of main, so no other thread exists yet.
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
    tulkabd_lib::run()
}
