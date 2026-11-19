use tianshell_core::mcp::{run_stdio_loop, McpHandler};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("TianShell MCP (Rust) 已启动");
    let handler = McpHandler::new()?;
    run_stdio_loop(handler)?;
    Ok(())
}
