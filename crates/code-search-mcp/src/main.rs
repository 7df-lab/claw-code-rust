//! Stdio entrypoint for the bundled `code_search` MCP server.

use devo_code_search_mcp::CodeSearchMcpServer;
use rmcp::ServiceExt;

fn stdio() -> (tokio::io::Stdin, tokio::io::Stdout) {
    (tokio::io::stdin(), tokio::io::stdout())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = CodeSearchMcpServer::production()?;
    // Serve the MCP handshake first. Prefetching/indexing must not block
    // `initialize` / `tools/list`, or clients time out and report 0 tools.
    let prewarm_server = server.clone();
    tokio::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || prewarm_server.prewarm()).await;
    });
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
