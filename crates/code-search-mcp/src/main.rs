//! Stdio entrypoint for the bundled `code_search` MCP server.

use devo_code_search_mcp::CodeSearchMcpServer;
use rmcp::ServiceExt;

fn stdio() -> (tokio::io::Stdin, tokio::io::Stdout) {
    (tokio::io::stdin(), tokio::io::stdout())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = CodeSearchMcpServer::production()?;
    server.prewarm();
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
