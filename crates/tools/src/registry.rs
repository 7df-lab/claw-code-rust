use std::collections::HashMap;
use std::sync::Arc;

use devo_protocol::ToolDefinition;

use crate::Tool;

/// Central registry of available tools.
///
/// The registry owns all tool instances and provides lookup by name.
/// Tools are registered once at startup and remain immutable for the
/// lifetime of the session.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Return all tools for inclusion in the model request.
    pub fn all(&self) -> Vec<&Arc<dyn Tool>> {
        self.tools.values().collect()
    }

    /// Build tool definitions suitable for the model API.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    use crate::{ToolContext, ToolOutput};

    struct DummyTool {
        tool_name: &'static str,
        read_only: bool,
    }

    #[async_trait]
    impl crate::Tool for DummyTool {
        fn name(&self) -> &str {
            self.tool_name
        }
        fn description(&self) -> &str {
            "dummy"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::success("ok"))
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            tool_name: "test_tool",
            read_only: true,
        }));
        assert!(reg.get("test_tool").is_some());
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn all_returns_registered_tools() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            tool_name: "a",
            read_only: true,
        }));
        reg.register(Arc::new(DummyTool {
            tool_name: "b",
            read_only: false,
        }));
        assert_eq!(reg.all().len(), 2);
    }

    #[test]
    fn tool_definitions_maps_correctly() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            tool_name: "my_tool",
            read_only: true,
        }));
        let defs = reg.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "my_tool");
        assert_eq!(defs[0].description, "dummy");
    }

    #[test]
    fn register_overwrites_duplicate_name() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(DummyTool {
            tool_name: "same",
            read_only: true,
        }));
        reg.register(Arc::new(DummyTool {
            tool_name: "same",
            read_only: false,
        }));
        let tool = reg.get("same").unwrap();
        assert!(!tool.is_read_only());
    }

    #[test]
    fn default_creates_empty_registry() {
        let reg = ToolRegistry::default();
        assert!(reg.all().is_empty());
    }
}
