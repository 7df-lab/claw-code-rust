use std::sync::Arc;

use crate::mcp::McpManager;
use crate::mcp::McpToolInfo;
use async_trait::async_trait;

use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolCapabilityTag;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolPreparationFeedback;
use crate::tool_spec::ToolSpec;

pub struct McpToolHandler {
    manager: Arc<dyn McpManager>,
    info: McpToolInfo,
    spec: ToolSpec,
}

impl McpToolHandler {
    pub fn new(manager: Arc<dyn McpManager>, info: McpToolInfo) -> Self {
        let spec = mcp_tool_spec(&info);
        Self {
            manager,
            info,
            spec,
        }
    }
}

#[async_trait]
impl ToolHandler for McpToolHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let result = tokio::select! {
            biased;
            _ = ctx.cancel_token.cancelled() => return Err(ToolCallError::Cancelled),
            result = self.manager.invoke_tool(&self.info.server_id, &self.info.raw_tool_name, input) => {
                result.map_err(|err| ToolCallError::ExecutionFailed(err.to_string()))?
            }
        };

        Ok(ToolResult::success(
            ToolResultContent::Json(result),
            format!("Called MCP tool {}", self.info.raw_tool_name),
        ))
    }
}

pub fn mcp_tool_spec(info: &McpToolInfo) -> ToolSpec {
    ToolSpec {
        name: info.flat_name.clone(),
        description: format!(
            "MCP tool from {}. {}",
            info.server_display_name,
            info.description()
        ),
        input_schema: serde_json::from_value(info.input_schema())
            .unwrap_or_else(|_| JsonSchema::object(Default::default(), None, Some(true))),
        output_mode: ToolOutputMode::StructuredJson,
        execution_mode: if info.is_read_only() {
            ToolExecutionMode::ReadOnly
        } else {
            ToolExecutionMode::Mutating
        },
        capability_tags: Vec::<ToolCapabilityTag>::new(),
        supports_parallel: info.supports_parallel_tool_calls || info.is_read_only(),
        preparation_feedback: ToolPreparationFeedback::None,
        display_name: Some(info.raw_tool_name.clone()),
        supports_cancellation: None,
        supports_streaming: None,
    }
}

pub fn mcp_search_text(info: &McpToolInfo) -> String {
    let mut schema_properties = info
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    schema_properties.sort_unstable();

    let source_description = info
        .source_description
        .as_deref()
        .map(str::trim)
        .filter(|source_description| !source_description.is_empty());
    let description_len = info
        .description
        .as_deref()
        .map_or("Call MCP tool".len() + info.raw_tool_name.len(), str::len);
    let description_parts = if info.description.is_some() { 1 } else { 2 };
    let part_count =
        5 + description_parts + usize::from(source_description.is_some()) + schema_properties.len();
    let mut text = String::with_capacity(
        info.flat_name.len()
            + info.callable_name.len()
            + info.raw_tool_name.len()
            + info.server_id.0.len()
            + info.server_display_name.len()
            + description_len
            + source_description.map_or(0, str::len)
            + schema_properties
                .iter()
                .map(|property| property.len())
                .sum::<usize>()
            + part_count.saturating_sub(1),
    );
    let mut push_part = |part: &str| {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(part);
    };

    push_part(&info.flat_name);
    push_part(&info.callable_name);
    push_part(&info.raw_tool_name);
    push_part(&info.server_id.0);
    push_part(&info.server_display_name);
    if let Some(description) = info.description.as_deref() {
        push_part(description);
    } else {
        push_part("Call MCP tool");
        push_part(&info.raw_tool_name);
    }
    if let Some(source_description) = source_description {
        push_part(source_description);
    }
    for property in schema_properties {
        push_part(property);
    }
    text
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::hint::black_box;
    use std::time::Instant;

    use super::*;
    use crate::mcp::McpServerId;

    #[test]
    fn mcp_tool_spec_uses_normalized_core_metadata() {
        let info = McpToolInfo::new(
            McpServerId("Docs Server".into()),
            "Docs Server".into(),
            "search-docs".into(),
            Some("Search the docs.".into()),
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            true,
            false,
        );

        let spec = mcp_tool_spec(&info);

        assert_eq!(
            serde_json::to_value(spec).expect("tool spec should serialize"),
            json!({
                "name": "mcp__docs_server__search_docs",
                "description": "MCP tool from Docs Server. Search the docs.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" }
                    },
                    "required": ["query"]
                },
                "output_mode": "StructuredJson",
                "execution_mode": "ReadOnly",
                "capability_tags": [],
                "supports_parallel": true,
                "preparation_feedback": "None",
                "display_name": "search-docs"
            })
        );
    }

    #[test]
    #[ignore]
    fn bench_mcp_search_text_many_properties() {
        let mut properties = serde_json::Map::new();
        for index in 0..64 {
            properties.insert(format!("field_{index:02}"), json!({ "type": "string" }));
        }
        let mut schema = serde_json::Map::new();
        schema.insert("type".to_string(), json!("object"));
        schema.insert(
            "properties".to_string(),
            serde_json::Value::Object(properties),
        );
        let mut info = McpToolInfo::new(
            McpServerId("Docs Server".into()),
            "Docs Server".into(),
            "search-docs".into(),
            Some("Search workspace documentation and issue history.".into()),
            serde_json::Value::Object(schema),
            true,
            false,
        );
        info.source_description = Some("Local MCP documentation server".into());

        let expected = mcp_search_text(&info);
        assert_eq!(
            expected,
            "mcp__docs_server__search_docs search_docs search-docs Docs Server Docs Server Search workspace documentation and issue history. Local MCP documentation server field_00 field_01 field_02 field_03 field_04 field_05 field_06 field_07 field_08 field_09 field_10 field_11 field_12 field_13 field_14 field_15 field_16 field_17 field_18 field_19 field_20 field_21 field_22 field_23 field_24 field_25 field_26 field_27 field_28 field_29 field_30 field_31 field_32 field_33 field_34 field_35 field_36 field_37 field_38 field_39 field_40 field_41 field_42 field_43 field_44 field_45 field_46 field_47 field_48 field_49 field_50 field_51 field_52 field_53 field_54 field_55 field_56 field_57 field_58 field_59 field_60 field_61 field_62 field_63"
        );

        let iterations = 100_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(mcp_search_text(black_box(&info))).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, iterations * expected.len());
        println!(
            "mcp_search_text_many_properties iterations={iterations} properties=64 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
