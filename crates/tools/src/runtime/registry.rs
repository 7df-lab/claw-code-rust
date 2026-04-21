use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use devo_protocol::ToolDefinition as ProtocolToolDefinition;

use crate::runtime::{RuntimeTool, ToolDefinitionSpec, ToolName, ToolRuntimeConfigSnapshot};

/// In-memory runtime registry for improved tools.
pub struct RuntimeToolRegistry {
    tools: RwLock<HashMap<ToolName, Arc<dyn RuntimeTool>>>,
}

impl RuntimeToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, tool: Arc<dyn RuntimeTool>) {
        let definition = tool.definition();
        self.tools
            .write()
            .expect("runtime tool registry poisoned")
            .insert(definition.name, tool);
    }

    pub fn get(&self, name: &ToolName) -> Option<Arc<dyn RuntimeTool>> {
        self.tools
            .read()
            .expect("runtime tool registry poisoned")
            .get(name)
            .cloned()
    }

    pub fn list(&self) -> Vec<ToolDefinitionSpec> {
        self.tools
            .read()
            .expect("runtime tool registry poisoned")
            .values()
            .map(|tool| tool.definition())
            .collect()
    }

    pub fn is_enabled(&self, app_config: &ToolRuntimeConfigSnapshot, name: &ToolName) -> bool {
        app_config.enabled_tools.is_empty()
            || app_config
                .enabled_tools
                .iter()
                .any(|enabled| enabled == name.0.as_str())
    }

    pub fn list_enabled(&self, app_config: &ToolRuntimeConfigSnapshot) -> Vec<ToolDefinitionSpec> {
        self.list()
            .into_iter()
            .filter(|definition| self.is_enabled(app_config, &definition.name))
            .collect()
    }

    /// Builds provider-facing tool definitions while the protocol remains on the older shape.
    pub fn protocol_tool_definitions(
        &self,
        app_config: &ToolRuntimeConfigSnapshot,
    ) -> Vec<ProtocolToolDefinition> {
        self.list_enabled(app_config)
            .into_iter()
            .map(|definition| ProtocolToolDefinition {
                name: definition.name.0.to_string(),
                description: definition.description,
                input_schema: definition.input_schema,
            })
            .collect()
    }
}

impl Default for RuntimeToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
