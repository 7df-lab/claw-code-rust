use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;

pub struct WebSearchHandler;

#[async_trait]
impl ToolHandler for WebSearchHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::WebSearch
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let query = invocation.input["query"].as_str().unwrap_or("");
        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query,
                    "type": invocation.input["type"].as_str().unwrap_or("auto"),
                    "numResults": invocation.input["numResults"].as_u64().unwrap_or(8),
                    "livecrawl": invocation.input["livecrawl"].as_str().unwrap_or("fallback"),
                    "contextMaxCharacters": invocation.input["contextMaxCharacters"].as_u64()
                }
            }
        });

        let res = client
            .post("https://mcp.exa.ai/mcp")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Search request failed: {e}"),
            })?;

        if !res.status().is_success() {
            return Ok(Box::new(FunctionToolOutput::error(format!(
                "Search error ({})",
                res.status()
            ))));
        }

        let text = res
            .text()
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to read search response: {e}"),
            })?;

        Ok(Box::new(FunctionToolOutput::success(text)))
    }
}
