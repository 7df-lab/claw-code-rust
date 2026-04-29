use std::collections::HashMap;
use std::sync::Arc;

use devo_protocol::ToolDefinition;

use crate::errors::ToolDispatchError;
use crate::invocation::{ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;
use crate::tool_spec::{ToolExecutionMode, ToolSpec};

#[derive(Clone)]
pub struct ToolRegistry {
    pub(crate) handlers: HashMap<String, Arc<dyn ToolHandler>>,
    pub(crate) specs: Vec<ToolSpec>,
    pub(crate) spec_index: HashMap<String, usize>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        ToolRegistry {
            handlers: HashMap::new(),
            specs: Vec::new(),
            spec_index: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ToolHandler>> {
        self.handlers.get(name)
    }

    pub fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.spec_index.get(name).map(|&idx| &self.specs[idx])
    }

    pub fn is_read_only(&self, name: &str) -> bool {
        self.spec(name)
            .map_or(false, |s| s.execution_mode == ToolExecutionMode::ReadOnly)
    }

    pub fn supports_parallel(&self, name: &str) -> bool {
        self.spec(name).map_or(false, |s| s.supports_parallel)
    }

    pub async fn dispatch(
        &self,
        name: &str,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn ToolOutput>, ToolDispatchError> {
        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| ToolDispatchError::UnknownTool {
                name: name.to_string(),
            })?;
        handler
            .handle(invocation, None)
            .await
            .map_err(ToolDispatchError::from)
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.specs
            .iter()
            .map(|spec| ToolDefinition {
                name: spec.name.clone(),
                description: spec.description.clone(),
                input_schema: spec.input_schema.to_json_value(),
            })
            .collect()
    }

    pub fn all_handlers(&self) -> impl Iterator<Item = (&String, &Arc<dyn ToolHandler>)> {
        self.handlers.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }
}

pub struct ToolRegistryBuilder {
    handlers: HashMap<String, Arc<dyn ToolHandler>>,
    specs: Vec<ToolSpec>,
    spec_index: HashMap<String, usize>,
}

impl ToolRegistryBuilder {
    pub fn new() -> Self {
        ToolRegistryBuilder {
            handlers: HashMap::new(),
            specs: Vec::new(),
            spec_index: HashMap::new(),
        }
    }

    pub fn push_spec(&mut self, spec: ToolSpec) {
        let name = spec.name.clone();
        self.spec_index.insert(name, self.specs.len());
        self.specs.push(spec);
    }

    pub fn register_handler(&mut self, name: &str, handler: Arc<dyn ToolHandler>) {
        self.handlers.insert(name.to_string(), handler);
    }

    pub fn build(self) -> ToolRegistry {
        ToolRegistry {
            handlers: self.handlers,
            specs: self.specs,
            spec_index: self.spec_index,
        }
    }
}

impl Default for ToolRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ToolExecutionError;
    use crate::events::ToolProgressSender;
    use crate::invocation::{FunctionToolOutput, ToolCallId, ToolName, ToolOutput};
    use crate::json_schema::JsonSchema;
    use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};
    use async_trait::async_trait;
    use std::path::PathBuf;

    struct EchoHandler;

    #[async_trait]
    impl ToolHandler for EchoHandler {
        fn tool_kind(&self) -> crate::handler_kind::ToolHandlerKind {
            crate::handler_kind::ToolHandlerKind::Read
        }

        async fn handle(
            &self,
            _invocation: ToolInvocation,
            _progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            Ok(Box::new(FunctionToolOutput::success("echo")))
        }
    }

    #[test]
    fn registry_register_and_get() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("echo", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "echo".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
        });
        let registry = builder.build();
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn registry_tool_definitions() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("echo", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "echo".into(),
            description: "test".into(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
        });
        let registry = builder.build();
        let defs = registry.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
        assert_eq!(defs[0].description, "test");
    }

    #[tokio::test]
    async fn registry_dispatch_unknown_tool() {
        let builder = ToolRegistryBuilder::new();
        let registry = builder.build();
        let invocation = ToolInvocation {
            call_id: ToolCallId("c1".into()),
            tool_name: ToolName("nonexistent".into()),
            session_id: "s1".into(),
            cwd: PathBuf::from("/tmp"),
            input: serde_json::json!({}),
        };
        let result = registry.dispatch("nonexistent", invocation).await;
        match result {
            Err(ToolDispatchError::UnknownTool { name }) => assert_eq!(name, "nonexistent"),
            Err(other) => panic!("expected UnknownTool error, got: {other}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn registry_supports_parallel() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("read", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "read".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
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
        builder.register_handler("read", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "read".into(),
            description: String::new(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
        });
        builder.register_handler("write", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "write".into(),
            description: String::new(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
        });
        let registry = builder.build();
        assert!(registry.is_read_only("read"));
        assert!(!registry.is_read_only("write"));
        assert!(!registry.is_read_only("nonexistent"));
    }

    #[test]
    fn registry_spec_lookup() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("tool", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "tool".into(),
            description: "desc".into(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
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
        builder.register_handler("echo", Arc::new(EchoHandler));
        builder.push_spec(ToolSpec {
            name: "echo".into(),
            description: String::new(),
            input_schema: JsonSchema::string(None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
        });
        let registry = builder.build();
        let invocation = ToolInvocation {
            call_id: ToolCallId("c1".into()),
            tool_name: ToolName("echo".into()),
            session_id: "s1".into(),
            cwd: PathBuf::from("/tmp"),
            input: serde_json::json!({}),
        };
        let result = registry.dispatch("echo", invocation).await;
        assert!(result.is_ok());
    }
}
