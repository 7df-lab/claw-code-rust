use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use devo_protocol::ToolDefinition;

use crate::contracts::ToolContext;
use crate::contracts::ToolResult;
use crate::deferred_loading::DeferredLoadingConfig;
use crate::deferred_loading::DeferredToolPrompt;
use crate::deferred_loading::LoadedDeferredTools;
use crate::deferred_loading::assemble_deferred_tool_prompt;
use crate::deferred_loading::execute_tool_search;
use crate::errors::ToolDispatchError;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolPreparationFeedback;
use crate::tool_spec::ToolSpec;
use crate::unified_exec::store::ProcessStore;

#[derive(Clone)]
pub struct ToolRegistry {
    pub(crate) handlers: HashMap<String, Arc<dyn ToolHandler>>,
    pub(crate) specs: Vec<ToolSpec>,
    pub(crate) spec_index: HashMap<String, usize>,
    pub(crate) spec_exposure: HashMap<String, ToolExposure>,
    pub(crate) spec_search_text: HashMap<String, String>,
    pub(crate) unified_exec_store: Option<Arc<ProcessStore>>,
    pub(crate) loaded_deferred_tools: Arc<Mutex<LoadedDeferredTools>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExposure {
    Direct,
    Deferred,
    Hidden,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry {
            handlers: HashMap::new(),
            specs: Vec::new(),
            spec_index: HashMap::new(),
            spec_exposure: HashMap::new(),
            spec_search_text: HashMap::new(),
            unified_exec_store: None,
            loaded_deferred_tools: Arc::new(Mutex::new(LoadedDeferredTools::default())),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ToolHandler>> {
        self.handlers.get(name)
    }

    /// Returns the shared unified-exec process store when this registry owns one.
    pub fn unified_exec_store(&self) -> Option<Arc<ProcessStore>> {
        self.unified_exec_store.clone()
    }

    pub fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.spec_index.get(name).map(|&idx| &self.specs[idx])
    }

    pub fn is_read_only(&self, name: &str) -> bool {
        self.spec(name)
            .is_some_and(|s| s.execution_mode == ToolExecutionMode::ReadOnly)
    }

    pub fn supports_parallel(&self, name: &str) -> bool {
        self.spec(name).is_some_and(|s| s.supports_parallel)
    }

    pub fn preparation_feedback(&self, name: &str) -> ToolPreparationFeedback {
        self.spec(name)
            .map(|spec| spec.preparation_feedback)
            .unwrap_or(ToolPreparationFeedback::None)
    }

    pub async fn dispatch(
        &self,
        name: &str,
        ctx: ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolDispatchError> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| ToolDispatchError::UnknownTool {
                name: name.to_string(),
            })?;
        handler
            .handle(ctx, input, None)
            .await
            .map_err(ToolDispatchError::from)
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.specs
            .iter()
            .map(|spec| ToolDefinition {
                name: spec.name.clone(),
                description: spec.description.clone(),
                input_schema: with_sandbox_permission_fields(
                    &spec.name,
                    spec.input_schema.to_json_value(),
                ),
                output_schema: unified_exec_output_schema(&spec.name),
            })
            .collect()
    }

    pub fn restricted_to_specs(&self, names: &[&str]) -> Self {
        let mut registry = ToolRegistry::new();
        registry.unified_exec_store = self.unified_exec_store.clone();
        registry.loaded_deferred_tools = Arc::clone(&self.loaded_deferred_tools);

        for spec in &self.specs {
            if !names.iter().any(|name| *name == spec.name) {
                continue;
            }
            registry
                .spec_index
                .insert(spec.name.clone(), registry.specs.len());
            if let Some(exposure) = self.spec_exposure.get(&spec.name).copied() {
                registry.spec_exposure.insert(spec.name.clone(), exposure);
            }
            if let Some(search_text) = self.spec_search_text.get(&spec.name) {
                registry
                    .spec_search_text
                    .insert(spec.name.clone(), search_text.clone());
            }
            registry.specs.push(spec.clone());
        }

        for name in names {
            if let Some(handler) = self.handlers.get(*name) {
                registry
                    .handlers
                    .insert((*name).to_string(), Arc::clone(handler));
            }
        }

        registry
    }

    pub fn deferred_tool_prompt(
        &self,
        session_id: &str,
        loaded_tools: &LoadedDeferredTools,
        config: &DeferredLoadingConfig,
    ) -> DeferredToolPrompt {
        assemble_deferred_tool_prompt(
            &self.tool_definitions(),
            &loaded_tools.list_loaded(session_id),
            &self.effective_deferred_loading_config(config),
        )
    }

    pub fn load_deferred_tools(
        &self,
        session_id: &str,
        config: &DeferredLoadingConfig,
        query: &str,
    ) -> Result<String, String> {
        let mut loaded_tools = self
            .loaded_deferred_tools
            .lock()
            .map_err(|_| "loaded deferred tool state lock poisoned".to_string())?;
        execute_tool_search(
            session_id,
            query,
            &self.tool_definitions(),
            &mut loaded_tools,
            config,
        )
        .map(|result| result.summary())
    }

    pub fn loaded_deferred_tools(&self) -> Arc<Mutex<LoadedDeferredTools>> {
        Arc::clone(&self.loaded_deferred_tools)
    }

    pub fn effective_deferred_loading_config(
        &self,
        base: &DeferredLoadingConfig,
    ) -> DeferredLoadingConfig {
        apply_exposure_overrides(base, &self.spec_exposure)
    }

    pub fn all_handlers(&self) -> impl Iterator<Item = (&String, &Arc<dyn ToolHandler>)> {
        self.handlers.iter()
    }

    pub fn search_text_for(&self, name: &str) -> Option<&str> {
        self.spec_search_text.get(name).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    pub async fn terminate_unified_exec_processes(&self) {
        if let Some(store) = &self.unified_exec_store {
            store.terminate_all().await;
        }
    }
}

fn with_sandbox_permission_fields(
    tool_name: &str,
    mut schema: serde_json::Value,
) -> serde_json::Value {
    if !super::tool_accepts_sandbox_escalation_fields(tool_name) {
        return schema;
    }

    let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return schema;
    };

    properties.insert(
        "sandbox_permissions".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Sandbox permissions for the command. Use \"with_additional_permissions\" to request additional sandboxed filesystem or network permissions (preferred), or \"require_escalated\" to request running without sandbox restrictions; defaults to \"use_default\".",
            "enum": ["use_default", "require_escalated", "with_additional_permissions"]
        }),
    );
    properties.insert(
        "additional_permissions".to_string(),
        additional_permissions_schema(),
    );
    properties.insert(
        "justification".to_string(),
        serde_json::json!({
            "type": "string",
            "description": "Only set if sandbox_permissions is \"require_escalated\".\nRequest approval from the user to run this command outside the sandbox.\nPhrased as a simple question that summarizes the purpose of the\ncommand as it relates to the task at hand - e.g. 'Do you want to\nfetch and pull the latest version of this git branch?'"
        }),
    );
    properties.insert(
        "prefix_rule".to_string(),
        serde_json::json!({
            "type": "array",
            "description": "Only specify when sandbox_permissions is `require_escalated`.\nSuggest a prefix command pattern that will allow you to fulfill similar requests from the user in the future.\nShould be a short but reasonable prefix, e.g. [\"git\", \"pull\"] or [\"uv\", \"run\"] or [\"pytest\"].",
            "items": { "type": "string" }
        }),
    );

    schema
}

fn additional_permissions_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "network": {
                "type": "object",
                "properties": {
                    "enabled": {
                        "type": "boolean",
                        "description": "Set to true to request network access."
                    }
                },
                "additionalProperties": false
            },
            "file_system": {
                "type": "object",
                "properties": {
                    "read": {
                        "type": "array",
                        "description": "Absolute paths to grant read access to.",
                        "items": { "type": "string" }
                    },
                    "write": {
                        "type": "array",
                        "description": "Absolute paths to grant write access to.",
                        "items": { "type": "string" }
                    }
                },
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

fn unified_exec_output_schema(tool_name: &str) -> Option<serde_json::Value> {
    if tool_name != "exec_command" && tool_name != "write_stdin" {
        return None;
    }

    Some(serde_json::json!({
        "type": "object",
        "properties": {
            "exit_code": {
                "type": "number",
                "description": "Process exit code when the command finished during this call."
            },
            "process_id": {
                "type": "number",
                "description": "Process identifier to pass to write_stdin when the process is still running."
            },
            "output": {
                "type": "string",
                "description": "Command output text, possibly truncated."
            }
        },
        "required": ["output"],
        "additionalProperties": false
    }))
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ToolRegistryBuilder {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    specs: Vec<ToolSpec>,
    spec_index: HashMap<String, usize>,
    spec_exposure: HashMap<String, ToolExposure>,
    spec_search_text: HashMap<String, String>,
    unified_exec_store: Option<Arc<ProcessStore>>,
    loaded_deferred_tools: Arc<Mutex<LoadedDeferredTools>>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        ToolRegistryBuilder {
            handlers: HashMap::new(),
            specs: Vec::new(),
            spec_index: HashMap::new(),
            spec_exposure: HashMap::new(),
            spec_search_text: HashMap::new(),
            unified_exec_store: None,
            loaded_deferred_tools: Arc::new(Mutex::new(LoadedDeferredTools::default())),
        }
    }

    pub fn push_spec(&mut self, spec: ToolSpec) {
        let name = spec.name.clone();
        self.spec_index.insert(name, self.specs.len());
        self.specs.push(spec);
    }

    pub fn push_spec_with_exposure(&mut self, spec: ToolSpec, exposure: ToolExposure) {
        let name = spec.name.clone();
        self.spec_index.insert(name, self.specs.len());
        self.spec_exposure.insert(spec.name.clone(), exposure);
        self.specs.push(spec);
    }

    pub fn set_search_text(&mut self, name: &str, search_text: String) {
        self.spec_search_text.insert(name.to_string(), search_text);
    }

    pub fn register_handler(&mut self, name: &str, handler: Arc<dyn ToolHandler>) {
        self.handlers.insert(name.to_string(), handler);
    }

    pub fn set_unified_exec_store(&mut self, store: Arc<ProcessStore>) {
        self.unified_exec_store = Some(store);
    }

    pub fn set_loaded_deferred_tools(&mut self, loaded_tools: Arc<Mutex<LoadedDeferredTools>>) {
        self.loaded_deferred_tools = loaded_tools;
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.specs
            .iter()
            .map(|spec| ToolDefinition {
                name: spec.name.clone(),
                description: spec.description.clone(),
                input_schema: with_sandbox_permission_fields(
                    &spec.name,
                    spec.input_schema.to_json_value(),
                ),
                output_schema: unified_exec_output_schema(&spec.name),
            })
            .collect()
    }

    pub fn tool_search_entries(&self) -> Vec<(ToolDefinition, Option<String>)> {
        self.tool_definitions()
            .into_iter()
            .map(|definition| {
                let search_text = self.spec_search_text.get(&definition.name).cloned();
                (definition, search_text)
            })
            .collect()
    }

    pub fn effective_deferred_loading_config(
        &self,
        base: &DeferredLoadingConfig,
    ) -> DeferredLoadingConfig {
        apply_exposure_overrides(base, &self.spec_exposure)
    }

    pub fn build(self) -> ToolRegistry {
        ToolRegistry {
            handlers: self.handlers,
            specs: self.specs,
            spec_index: self.spec_index,
            spec_exposure: self.spec_exposure,
            spec_search_text: self.spec_search_text,
            unified_exec_store: self.unified_exec_store,
            loaded_deferred_tools: self.loaded_deferred_tools,
        }
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_exposure_overrides(
    base: &DeferredLoadingConfig,
    exposure: &HashMap<String, ToolExposure>,
) -> DeferredLoadingConfig {
    let mut config = base.clone();
    for (name, exposure) in exposure {
        let hidden_by_caller = config.hidden.contains(name);
        config.preloaded.remove(name);
        config.deferred.remove(name);
        if hidden_by_caller {
            continue;
        }
        config.hidden.remove(name);
        match exposure {
            ToolExposure::Direct => {
                config.preloaded.insert(name.clone());
            }
            ToolExposure::Deferred => {
                config.deferred.insert(name.clone());
            }
            ToolExposure::Hidden => {
                config.hidden.insert(name.clone());
            }
        }
    }
    config
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::contracts::ToolCallError;
    use crate::contracts::ToolContext;
    use crate::contracts::ToolProgressSender;
    use crate::contracts::ToolResult;
    use crate::contracts::ToolResultContent;
    use crate::json_schema::JsonSchema;
    use crate::tool_spec::ToolExecutionMode;
    use crate::tool_spec::ToolOutputMode;
    use crate::tool_spec::ToolSpec;
    use async_trait::async_trait;
    use devo_tools::contracts::ToolBudgets;
    use pretty_assertions::assert_eq;
    use tokio_util::sync::CancellationToken;

    struct EchoHandler {
        spec: ToolSpec,
    }

    impl EchoHandler {
        fn new() -> Self {
            Self {
                spec: ToolSpec::new(
                    "echo",
                    "echo tool",
                    JsonSchema::object(Default::default(), None, None),
                ),
            }
        }
    }

    fn test_ctx() -> ToolContext {
        ToolContext {
            output_store: None,
            tool_call_id: devo_tools::ToolCallId("test-id".to_string()),
            session_id: "test-session".to_string(),
            turn_id: Some("test-turn".to_string()),
            workspace_root: PathBuf::from("~/user/devo"),
            budgets: ToolBudgets {
                wall_time_limit_ms: Some(6_000),
                output_limit_bytes: 32 * 1024,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_permission_overlay: None,
            sandbox_profile: None,
        }
    }

    #[async_trait]
    impl ToolHandler for EchoHandler {
        fn spec(&self) -> &ToolSpec {
            &self.spec
        }

        async fn handle(
            &self,
            _ctx: ToolContext,
            _input: serde_json::Value,
            _progress: Option<ToolProgressSender>,
        ) -> Result<ToolResult, ToolCallError> {
            Ok(ToolResult::success(
                ToolResultContent::Text("echo".into()),
                "echo",
            ))
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("echo", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "echo".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = builder.build();
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn registry_tool_definitions() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("echo", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "echo".into(),
            description: "test".into(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = builder.build();
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].description, "test");
    }

    #[test]
    fn registry_restricted_to_specs_keeps_only_named_tools() {
        let mut builder = ToolRegistryBuilder::new();
        for name in ["webfetch", "write"] {
            builder.register_handler(name, Arc::new(EchoHandler::new()));
            builder.push_spec(ToolSpec {
                name: name.into(),
                description: format!("{name} description"),
                input_schema: JsonSchema::string(None),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            });
        }
        let registry = builder.build();

        let restricted = registry.restricted_to_specs(&["webfetch"]);

        assert_eq!(
            restricted
                .tool_definitions()
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>(),
            vec!["webfetch"]
        );
        assert!(restricted.get("webfetch").is_some());
        assert!(restricted.get("write").is_none());
        assert!(restricted.spec("write").is_none());
    }

    #[test]
    fn registry_builds_deferred_tool_prompt() {
        let mut builder = ToolRegistryBuilder::new();
        for name in ["read", "ToolSearch", "web_search"] {
            builder.push_spec(ToolSpec {
                name: name.into(),
                description: format!("{name} description"),
                input_schema: JsonSchema::object(Default::default(), None, None),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            });
        }
        let registry = builder.build();
        let loaded_tools = LoadedDeferredTools::default();

        let prompt = registry.deferred_tool_prompt(
            "session-1",
            &loaded_tools,
            &DeferredLoadingConfig::default(),
        );

        assert_eq!(
            prompt
                .exposed
                .iter()
                .map(|tool| &tool.name)
                .collect::<Vec<_>>(),
            vec!["read", "ToolSearch", "web_search"]
        );
        assert_eq!(
            prompt
                .deferred
                .iter()
                .map(|tool| &tool.name)
                .collect::<Vec<_>>(),
            Vec::<&String>::new()
        );
    }

    #[test]
    fn registry_hidden_config_overrides_deferred_exposure() {
        let mut builder = ToolRegistryBuilder::new();
        builder.push_spec_with_exposure(
            ToolSpec {
                name: "spawn_agent".into(),
                description: "spawn description".into(),
                input_schema: JsonSchema::object(Default::default(), None, None),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
            ToolExposure::Deferred,
        );
        let registry = builder.build();
        let mut config = DeferredLoadingConfig::default();
        config.hidden.insert("spawn_agent".to_string());

        let effective = registry.effective_deferred_loading_config(&config);

        assert_eq!(
            effective.hidden,
            std::collections::BTreeSet::from(["spawn_agent".to_string()])
        );
        assert!(!effective.deferred.contains("spawn_agent"));
    }

    #[test]
    fn registry_reports_deferred_tools_available_for_session() {
        let mut builder = ToolRegistryBuilder::new();
        for name in ["read", "ToolSearch", "web_search"] {
            builder.push_spec(ToolSpec {
                name: name.into(),
                description: format!("{name} description"),
                input_schema: JsonSchema::object(Default::default(), None, None),
                output_mode: ToolOutputMode::Text,
                execution_mode: ToolExecutionMode::ReadOnly,
                capability_tags: vec![],
                supports_parallel: true,
                preparation_feedback: ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            });
        }
        let registry = builder.build();

        let summary = registry
            .load_deferred_tools(
                "session-1",
                &DeferredLoadingConfig::default(),
                "select:WebSearch",
            )
            .expect("preloaded tool should be available");

        assert_eq!(
            summary,
            "Already available 1 tool(s): web_search. Call these tools directly without loading."
        );
        let loaded_tools = registry.loaded_deferred_tools();
        let loaded_tools = loaded_tools.lock().expect("loaded tool state");
        assert!(!loaded_tools.is_loaded("session-1", "web_search"));
    }

    #[test]
    fn registry_adds_output_schema_for_unified_exec_tools() {
        let mut builder = ToolRegistryBuilder::new();
        builder.push_spec(ToolSpec {
            name: "exec_command".into(),
            description: "exec".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });

        let registry = builder.build();
        let defs = registry.tool_definitions();

        assert_eq!(
            defs[0].output_schema,
            Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "exit_code": {
                        "type": "number",
                        "description": "Process exit code when the command finished during this call."
                    },
                    "process_id": {
                        "type": "number",
                        "description": "Process identifier to pass to write_stdin when the process is still running."
                    },
                    "output": {
                        "type": "string",
                        "description": "Command output text, possibly truncated."
                    }
                },
                "required": ["output"],
                "additionalProperties": false
            }))
        );
    }

    fn permission_field_presence(tool_name: &str) -> (bool, bool, bool, bool) {
        let mut builder = ToolRegistryBuilder::new();
        builder.push_spec(ToolSpec {
            name: tool_name.into(),
            description: "command".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });

        let registry = builder.build();
        let defs = registry.tool_definitions();
        let properties = defs[0]
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("object schema should have properties");

        (
            properties.contains_key("sandbox_permissions"),
            properties.contains_key("additional_permissions"),
            properties.contains_key("justification"),
            properties.contains_key("prefix_rule"),
        )
    }

    /// Trace: L2-DES-SAFETY-002
    /// Verifies: sandbox escalation fields are advertised on exec_command, shell_command, and bash.
    #[test]
    fn registry_adds_permission_fields_for_shell_family_command_tools() {
        for tool_name in ["exec_command", "shell_command", "bash"] {
            assert_eq!(
                permission_field_presence(tool_name),
                (true, true, true, true),
                "{tool_name} should advertise sandbox escalation fields"
            );
        }
    }

    /// Trace: L2-DES-SAFETY-002
    /// Verifies: write_stdin and non-command tools do not advertise sandbox escalation fields.
    #[test]
    fn registry_omits_permission_fields_for_non_spawn_tools() {
        for tool_name in ["write_stdin", "read"] {
            assert_eq!(
                permission_field_presence(tool_name),
                (false, false, false, false),
                "{tool_name} should not advertise sandbox escalation fields"
            );
        }
    }

    #[tokio::test]
    async fn registry_dispatch_unknown_tool() {
        let builder = ToolRegistryBuilder::new();
        let registry = builder.build();
        let result = registry
            .dispatch("nonexistent", test_ctx(), serde_json::json!({}))
            .await;
        match result {
            Err(ToolDispatchError::UnknownTool { name }) => assert_eq!(name, "nonexistent"),
            Err(other) => panic!("expected UnknownTool error, got: {other}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn registry_supports_parallel() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("read", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "read".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = builder.build();
        assert!(registry.supports_parallel("read"));
    }

    #[test]
    fn registry_builder_default() {
        let builder = ToolRegistryBuilder::default();
        let registry = builder.build();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registry_is_read_only() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("read", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "read".into(),
            description: String::new(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        builder.register_handler("write", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "write".into(),
            description: String::new(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = builder.build();
        assert!(registry.is_read_only("read"));
        assert!(!registry.is_read_only("write"));
        assert!(!registry.is_read_only("nonexistent"));
    }

    #[test]
    fn registry_spec_lookup() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("tool", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "tool".into(),
            description: "desc".into(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = builder.build();
        let spec = registry.spec("tool");
        assert!(spec.is_some());
        assert_eq!(spec.unwrap().description, "desc");
        assert!(registry.spec("missing").is_none());
    }

    #[test]
    fn registry_supports_parallel_for_missing_returns_false() {
        let registry = ToolRegistryBuilder::new().build();
        assert!(!registry.supports_parallel("nonexistent"));
    }

    #[tokio::test]
    async fn registry_dispatch_success() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("echo", Arc::new(EchoHandler::new()));
        builder.push_spec(ToolSpec {
            name: "echo".into(),
            description: String::new(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = builder.build();
        let result = registry
            .dispatch("echo", test_ctx(), serde_json::json!({}))
            .await;
        assert!(result.is_ok());
    }
}
