use async_trait::async_trait;
use chrono::Datelike;
use serde_json::json;

use crate::{Tool, ToolContext, ToolOutput};

const DESCRIPTION: &str = include_str!("websearch.txt");

pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "websearch"
    }

    fn description(&self) -> &str {
        static DESCRIPTION_CACHED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        DESCRIPTION_CACHED
            .get_or_init(|| DESCRIPTION.replace("{{year}}", &chrono::Utc::now().year().to_string()))
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "numResults": {"type": "number"},
                "livecrawl": {"type": "string", "enum": ["fallback", "preferred"]},
                "type": {"type": "string", "enum": ["auto", "fast", "deep"]},
                "contextMaxCharacters": {"type": "number"}
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let query = input["query"].as_str().unwrap_or("");
        let client = reqwest::Client::new();
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query,
                    "type": input["type"].as_str().unwrap_or("auto"),
                    "numResults": input["numResults"].as_u64().unwrap_or(8),
                    "livecrawl": input["livecrawl"].as_str().unwrap_or("fallback"),
                    "contextMaxCharacters": input["contextMaxCharacters"].as_u64()
                }
            }
        });
        let res = client
            .post("https://mcp.exa.ai/mcp")
            .json(&payload)
            .send()
            .await?;
        if !res.status().is_success() {
            return Ok(ToolOutput::error(format!(
                "Search error ({})",
                res.status()
            )));
        }
        let text = res.text().await?;
        Ok(ToolOutput::success(text))
    }
}
