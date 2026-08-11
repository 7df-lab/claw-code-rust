use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use bm25::Document;
use bm25::Language;
use bm25::SearchEngine;
use bm25::SearchEngineBuilder;
use devo_protocol::ToolDefinition;

use crate::contracts::ToolAgentScope;
use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::LoadedDeferredTools;
use crate::deferred_loading::PromptLoadingPolicy;
use crate::deferred_loading::execute_tool_search;
use crate::deferred_loading::hide_subagent_agent_coordination_tools;
use crate::deferred_loading::resolve_tool_policy;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;

const TOOL_SEARCH_DEFAULT_LIMIT: usize = 8;
const SELECT_QUERY_PREFIX: &str = "select:";

#[derive(Clone)]
pub struct ToolSearchEntry {
    name: String,
}

pub struct ToolSearchHandler {
    definitions: Vec<ToolDefinition>,
    entries: Vec<ToolSearchEntry>,
    loaded_tools: Arc<Mutex<LoadedDeferredTools>>,
    config: DeferredLoadingConfig,
    search_engine: SearchEngine<usize>,
    spec: ToolSpec,
}

impl ToolSearchHandler {
    pub fn new(
        definitions: Vec<(ToolDefinition, Option<String>)>,
        loaded_tools: Arc<Mutex<LoadedDeferredTools>>,
        config: DeferredLoadingConfig,
    ) -> Self {
        let mut all_definitions = Vec::with_capacity(definitions.len());
        let mut entries = Vec::with_capacity(definitions.len());
        let mut documents = Vec::with_capacity(definitions.len());
        for (definition, search_text) in definitions {
            let is_deferred =
                resolve_tool_policy(&definition.name, &config) == PromptLoadingPolicy::Deferred;
            if is_deferred {
                let search_text = search_text.unwrap_or_else(|| default_search_text(&definition));
                let name = definition.name.clone();
                all_definitions.push(definition);
                let document_id = entries.len();
                entries.push(ToolSearchEntry { name });
                documents.push(Document::new(document_id, search_text));
            } else {
                all_definitions.push(definition);
            }
        }
        let search_engine =
            SearchEngineBuilder::<usize>::with_documents(Language::English, documents).build();

        Self {
            definitions: all_definitions,
            entries,
            loaded_tools,
            config,
            search_engine,
            spec: tool_search_spec(),
        }
    }
}

#[async_trait]
impl ToolHandler for ToolSearchHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let query = input["query"].as_str().ok_or_else(|| {
            ToolCallError::InvalidInput("Expected non-empty string field `query`".into())
        })?;
        let query = query.trim();
        if query.is_empty() {
            return Err(ToolCallError::InvalidInput(
                "Expected non-empty string field `query`".into(),
            ));
        }
        let limit = input["limit"]
            .as_u64()
            .map(|limit| limit as usize)
            .unwrap_or(TOOL_SEARCH_DEFAULT_LIMIT);
        if limit == 0 {
            return Err(ToolCallError::InvalidInput(
                "Expected `limit` to be greater than zero".into(),
            ));
        }

        let mut config = self.config.clone();
        if ctx.agent_scope == ToolAgentScope::Subagent {
            hide_subagent_agent_coordination_tools(&mut config);
        }

        let selection = if is_select_query(query) {
            query.to_string()
        } else {
            let mut selection = String::from(SELECT_QUERY_PREFIX);
            for name in self.search(query, limit) {
                if resolve_tool_policy(name, &config) == PromptLoadingPolicy::Hidden {
                    continue;
                }
                if selection.len() > SELECT_QUERY_PREFIX.len() {
                    selection.push(',');
                }
                selection.push_str(name);
            }
            if selection.len() == SELECT_QUERY_PREFIX.len() {
                return Ok(ToolResult::success(
                    ToolResultContent::Text("No matching deferred tools found.".to_string()),
                    "No tools available",
                ));
            }
            selection
        };

        let mut loaded_tools = self.loaded_tools.lock().map_err(|_| {
            ToolCallError::InternalError("loaded deferred tool state lock poisoned".into())
        })?;
        let result = execute_tool_search(
            &ctx.session_id,
            &selection,
            &self.definitions,
            &mut loaded_tools,
            &config,
        )
        .map_err(ToolCallError::ExecutionFailed)?;

        Ok(ToolResult::success(
            ToolResultContent::Text(result.summary()),
            "Tools available",
        ))
    }
}

impl ToolSearchHandler {
    fn search<'a>(&'a self, query: &str, limit: usize) -> impl Iterator<Item = &'a str> + 'a {
        self.search_engine
            .search(query, limit)
            .into_iter()
            .filter_map(|result| self.entries.get(result.document.id))
            .map(|entry| entry.name.as_str())
    }
}

pub fn tool_search_spec() -> ToolSpec {
    ToolSpec {
        name: "ToolSearch".to_string(),
        description: format!(
            "Fetches full schema definitions for deferred tools so they can be called.\n\nDeferred tools appear by name in system reminder or available-deferred-tools messages. Until fetched, only the name is known; there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete schema definitions. Once a tool's schema appears in that result, it is callable exactly like any initially available tool.\n\nQuery forms:\n- `select:Read,Edit,Grep` - fetch these exact tools by name\n- `notebook jupyter` - keyword search, up to the best matching results\n- `+slack send` - require `slack` in the name and rank by remaining terms\n\nDefaults to {TOOL_SEARCH_DEFAULT_LIMIT} results."
        ),
        input_schema: JsonSchema::object(
            std::collections::BTreeMap::from([
                (
                    "query".to_string(),
                    JsonSchema::string(Some("Search query for available tools.")),
                ),
                (
                    "limit".to_string(),
                    JsonSchema::number(Some("Maximum number of tools to return.")),
                ),
            ]),
            Some(vec!["query".to_string()]),
            Some(false),
        ),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::ReadOnly,
        capability_tags: vec![],
        supports_parallel: true,
        preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
        display_name: Some("ToolSearch".to_string()),
        supports_cancellation: None,
        supports_streaming: None,
    }
}

fn is_select_query(query: &str) -> bool {
    query.starts_with(SELECT_QUERY_PREFIX) || query.starts_with("SELECT:")
}

fn default_search_text(definition: &ToolDefinition) -> String {
    let mut schema_properties = definition
        .input_schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| properties.keys().map(String::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    schema_properties.sort_unstable();

    let part_count = 2 + schema_properties.len();
    let mut text = String::with_capacity(
        definition.name.len()
            + definition.description.len()
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

    push_part(&definition.name);
    push_part(&definition.description);
    for property in schema_properties {
        push_part(property);
    }
    text
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    fn definition(name: &str, description: &str, schema: serde_json::Value) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: schema,
            output_schema: None,
        }
    }

    fn many_tool_search_handler(tool_count: usize) -> ToolSearchHandler {
        let definitions = (0..tool_count)
            .map(|index| {
                (
                    definition(
                        &format!("tool_{index}"),
                        &format!("Search workspace database records for shard {index}"),
                        serde_json::json!({
                            "type": "object",
                            "properties": {
                                "query": { "type": "string" },
                                "limit": { "type": "number" }
                            }
                        }),
                    ),
                    None,
                )
            })
            .collect::<Vec<_>>();
        ToolSearchHandler::new(
            definitions,
            Arc::new(Mutex::new(LoadedDeferredTools::default())),
            DeferredLoadingConfig::default(),
        )
    }

    fn many_tool_definitions_with_properties(
        tool_count: usize,
        property_count: usize,
    ) -> Vec<(ToolDefinition, Option<String>)> {
        (0..tool_count)
            .map(|tool_index| {
                let mut properties = serde_json::Map::new();
                for property_index in 0..property_count {
                    properties.insert(
                        format!("field_{property_index:02}"),
                        serde_json::json!({ "type": "string" }),
                    );
                }
                let mut schema = serde_json::Map::new();
                schema.insert("type".to_string(), serde_json::json!("object"));
                schema.insert(
                    "properties".to_string(),
                    serde_json::Value::Object(properties),
                );

                (
                    definition(
                        &format!("tool_{tool_index}"),
                        &format!("Search workspace database records for shard {tool_index}"),
                        serde_json::Value::Object(schema),
                    ),
                    None,
                )
            })
            .collect()
    }

    #[test]
    #[ignore]
    fn bench_tool_search_handler_new_many_tool_schemas() {
        let definitions = many_tool_definitions_with_properties(256, 32);
        let iterations = 1_000;
        let started = Instant::now();
        let mut total_entries = 0usize;

        for _ in 0..iterations {
            let handler = ToolSearchHandler::new(
                black_box(definitions.clone()),
                Arc::new(Mutex::new(LoadedDeferredTools::default())),
                DeferredLoadingConfig::default(),
            );
            total_entries += black_box(handler.entries.len());
        }

        let elapsed = started.elapsed();
        assert_eq!(total_entries, iterations * 256);
        println!(
            "tool_search_handler_new_many_tool_schemas iterations={iterations} tools=256 properties=32 elapsed_ms={} per_build_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    fn default_search_text_sorts_schema_properties() {
        let text = default_search_text(&definition(
            "tool",
            "Search files",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "zeta": { "type": "string" },
                    "alpha": { "type": "string" }
                }
            }),
        ));

        assert_eq!(text, "tool Search files alpha zeta");
    }

    #[test]
    #[ignore]
    fn bench_default_search_text_many_properties() {
        let mut properties = serde_json::Map::new();
        for index in 0..64 {
            properties.insert(
                format!("field_{index:02}"),
                serde_json::json!({ "type": "string" }),
            );
        }
        let mut schema = serde_json::Map::new();
        schema.insert("type".to_string(), serde_json::json!("object"));
        schema.insert(
            "properties".to_string(),
            serde_json::Value::Object(properties),
        );
        let definition = definition(
            "mcp__docs_server__search_docs",
            "Search workspace documentation and issue history.",
            serde_json::Value::Object(schema),
        );
        let expected = default_search_text(&definition);
        let iterations = 100_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(default_search_text(black_box(&definition))).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, iterations * expected.len());
        println!(
            "default_search_text_many_properties iterations={iterations} properties=64 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_tool_search_collects_result_names() {
        let handler = many_tool_search_handler(256);
        let started = Instant::now();
        let mut total_results = 0;

        for _ in 0..20_000 {
            total_results += black_box(handler.search(black_box("workspace database"), 8)).count();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_results, 160_000);
        println!(
            "tool_search_collects_result_names iterations=20000 tools=256 limit=8 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 20_000.0
        );
    }

    #[tokio::test]
    async fn natural_language_search_returns_matching_available_tool() {
        let loaded_tools = Arc::new(Mutex::new(LoadedDeferredTools::default()));
        let handler = ToolSearchHandler::new(
            vec![
                (
                    definition("read", "Read files", serde_json::json!({"type": "object"})),
                    None,
                ),
                (
                    definition(
                        "mcp__docs__search",
                        "Search docs",
                        serde_json::json!({
                            "type": "object",
                            "properties": { "query": { "type": "string" } }
                        }),
                    ),
                    Some("mcp__docs__search docs knowledge base query".to_string()),
                ),
            ],
            Arc::clone(&loaded_tools),
            DeferredLoadingConfig::default(),
        );

        let result = handler
            .handle(
                ToolContext {
                    tool_call_id: crate::invocation::ToolCallId("call".to_string()),
                    session_id: "session-1".to_string(),
                    turn_id: Some("turn-1".to_string()),
                    workspace_root: std::path::PathBuf::from("."),
                    budgets: crate::contracts::ToolBudgets {
                        output_limit_bytes: 1024,
                        wall_time_limit_ms: None,
                    },
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                    agent_scope: ToolAgentScope::Parent,
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    agent_coordinator: None,
                    client_filesystem: None,
                    file_read_ledger: None,
                    network_proxy: None,
                    network_no_proxy: None,
                    sandbox_profile: None,
                },
                serde_json::json!({ "query": "knowledge base" }),
                None,
            )
            .await
            .expect("tool search should succeed");

        assert_eq!(result.result_summary, "Tools available");
        let loaded_tools = loaded_tools.lock().expect("loaded tools");
        assert!(!loaded_tools.is_loaded("session-1", "mcp__docs__search"));
    }

    #[tokio::test]
    async fn subagent_tool_search_cannot_return_parent_agent_coordination_tools() {
        let loaded_tools = Arc::new(Mutex::new(LoadedDeferredTools::default()));
        let handler = ToolSearchHandler::new(
            vec![
                (
                    definition(
                        "spawn_agent",
                        "Create a child agent",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "send_message",
                        "Send input to a child agent",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "await_task",
                        "Wait for task completion",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "list_tasks",
                        "List child tasks",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "cancel_task",
                        "Cancel a child task",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "wait_agent",
                        "Poll child output",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "list_agents",
                        "List child agents",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
                (
                    definition(
                        "close_agent",
                        "Close a child agent",
                        serde_json::json!({"type": "object"}),
                    ),
                    None,
                ),
            ],
            Arc::clone(&loaded_tools),
            DeferredLoadingConfig::default(),
        );

        for requested in [
            "spawn_agent",
            "spawn-agent",
            "spawnagent",
            "subagent",
            "delegate",
            "send_message",
            "send-message",
            "sendmessage",
            "await_task",
            "await-task",
            "awaittask",
            "list_tasks",
            "list-tasks",
            "listtasks",
            "cancel_task",
            "cancel-task",
            "canceltask",
            "wait_agent",
            "wait-agent",
            "waitagent",
            "subagent_result",
            "subagent-result",
            "list_agents",
            "list-agents",
            "listagents",
            "subagent_status",
            "subagent-status",
            "close_agent",
            "close-agent",
            "closeagent",
        ] {
            let err = handler
                .handle(
                    ToolContext {
                        tool_call_id: crate::invocation::ToolCallId(format!("call-{requested}")),
                        session_id: "session-1".to_string(),
                        turn_id: Some("turn-1".to_string()),
                        workspace_root: std::path::PathBuf::from("."),
                        budgets: crate::contracts::ToolBudgets {
                            output_limit_bytes: 1024,
                            wall_time_limit_ms: None,
                        },
                        cancel_token: tokio_util::sync::CancellationToken::new(),
                        agent_scope: ToolAgentScope::Subagent,
                        collaboration_mode: devo_protocol::CollaborationMode::Build,
                        agent_coordinator: None,
                        client_filesystem: None,
                        file_read_ledger: None,
                        network_proxy: None,
                        network_no_proxy: None,
                        sandbox_profile: None,
                    },
                    serde_json::json!({ "query": format!("select:{requested}") }),
                    None,
                )
                .await
                .expect_err(
                    "subagent ToolSearch should not return parent-agent coordination tools",
                );

            match err {
                ToolCallError::ExecutionFailed(message) => assert!(message.contains("Not found")),
                other => panic!("unexpected error: {other:?}"),
            }
        }

        let loaded_tools = loaded_tools.lock().expect("loaded tools");
        assert!(!loaded_tools.is_loaded("session-1", "spawn_agent"));
        assert!(!loaded_tools.is_loaded("session-1", "send_message"));
        assert!(!loaded_tools.is_loaded("session-1", "wait_agent"));
        assert!(!loaded_tools.is_loaded("session-1", "list_agents"));
        assert!(!loaded_tools.is_loaded("session-1", "close_agent"));
    }
}
