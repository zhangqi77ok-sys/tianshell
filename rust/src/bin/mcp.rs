use tianshell_core::mcp::{run_stdio_loop, McpHandler};
use tianshell_core::mcp_http::run_stdio_attach;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--attach") {
        eprintln!("TianShell MCP 正在附着到已启动的工作台");
        return run_stdio_attach();
    }
    eprintln!("TianShell MCP (Rust stdio) 已启动");
    let handler = McpHandler::new()?;
    run_stdio_loop(handler)?;
    Ok(())
}
