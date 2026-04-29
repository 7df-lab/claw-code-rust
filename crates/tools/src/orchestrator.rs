#![allow(dead_code)]

use std::sync::Arc;

use tracing::{info, warn};

use devo_safety::legacy_permissions::{PermissionDecision, PermissionRequest, ResourceKind};

use crate::invocation::{ToolCallId, ToolInvocation, ToolName};
use crate::{ToolContext, ToolOutput, ToolRegistry};

/// A pending tool call extracted from the model response.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// The result of executing a single tool call.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub tool_use_id: String,
    pub output: ToolOutput,
}

/// Orchestrates the execution of tool calls.
#[allow(dead_code)]
pub struct ToolOrchestrator {
    registry: Arc<ToolRegistry>,
}

#[allow(dead_code)]
impl ToolOrchestrator {
    #[allow(dead_code)]
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a batch of tool calls.
    #[allow(dead_code)]
    pub async fn execute_batch(
        &self,
        calls: &[ToolCall],
        ctx: &ToolContext,
    ) -> Vec<ToolCallResult> {
        let mut results = Vec::with_capacity(calls.len());

        // Partition into concurrent (read-only) and sequential (mutating)
        let (concurrent, sequential): (Vec<_>, Vec<_>) = calls
            .iter()
            .partition(|call| self.registry.supports_parallel(&call.name));

        // Run concurrent tools in parallel
        if !concurrent.is_empty() {
            let futures: Vec<_> = concurrent
                .iter()
                .map(|call| self.execute_single(call, ctx))
                .collect();
            let concurrent_results = futures::future::join_all(futures).await;
            results.extend(concurrent_results);
        }

        // Run sequential tools one by one
        for call in &sequential {
            let result = self.execute_single(call, ctx).await;
            results.push(result);
        }

        results
    }

    pub(crate) async fn execute_single(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
    ) -> ToolCallResult {
        if !self.registry.is_read_only(&call.name) {
            let request = PermissionRequest {
                tool_name: call.name.clone(),
                resource: ResourceKind::Custom(call.name.clone()),
                description: format!("execute tool {}", call.name),
                target: None,
            };

            match ctx.permissions.check(&request).await {
                PermissionDecision::Allow => {}
                PermissionDecision::Deny { reason } => {
                    return ToolCallResult {
                        tool_use_id: call.id.clone(),
                        output: ToolOutput::error(format!("permission denied: {}", reason)),
                    };
                }
                PermissionDecision::Ask { message } => {
                    return ToolCallResult {
                        tool_use_id: call.id.clone(),
                        output: ToolOutput::error(format!(
                            "permission required — run with --permission interactive to approve: {}",
                            message
                        )),
                    };
                }
            }
        }

        info!(tool = %call.name, id = %call.id, "executing tool");

        let handler = match self.registry.get(&call.name) {
            Some(h) => h.clone(),
            None => {
                warn!(tool = %call.name, "tool not found");
                return ToolCallResult {
                    tool_use_id: call.id.clone(),
                    output: ToolOutput::error(format!("unknown tool: {}", call.name)),
                };
            }
        };

        let invocation = ToolInvocation {
            call_id: ToolCallId(call.id.clone()),
            tool_name: ToolName(call.name.clone().into()),
            session_id: ctx.session_id.clone(),
            cwd: ctx.cwd.clone(),
            input: call.input.clone(),
        };

        match handler.handle(invocation, None).await {
            Ok(output) => {
                let is_error = output.is_error();
                let content = output.to_content().into_string();
                ToolCallResult {
                    tool_use_id: call.id.clone(),
                    output: ToolOutput {
                        content,
                        is_error,
                        metadata: None,
                    },
                }
            }
            Err(e) => ToolCallResult {
                tool_use_id: call.id.clone(),
                output: ToolOutput::error(format!("tool execution failed: {}", e)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ToolExecutionError;
    use crate::events::ToolProgressSender;
    use crate::handler_kind::ToolHandlerKind;
    use crate::invocation::{FunctionToolOutput, ToolOutput};
    use crate::json_schema::JsonSchema;
    use crate::registry::ToolRegistryBuilder;
    use crate::tool_handler::ToolHandler;
    use crate::tool_spec::{ToolExecutionMode, ToolOutputMode, ToolSpec};
    use async_trait::async_trait;
    use devo_safety::legacy_permissions::{PermissionMode, RuleBasedPolicy};
    use std::sync::Arc;

    struct ReadOnlyHandler;

    #[async_trait]
    impl ToolHandler for ReadOnlyHandler {
        fn tool_kind(&self) -> ToolHandlerKind {
            ToolHandlerKind::Read
        }
        async fn handle(
            &self,
            _invocation: ToolInvocation,
            _progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            Ok(Box::new(FunctionToolOutput::success("read ok")))
        }
    }

    struct WriteHandler;

    #[async_trait]
    impl ToolHandler for WriteHandler {
        fn tool_kind(&self) -> ToolHandlerKind {
            ToolHandlerKind::Write
        }
        async fn handle(
            &self,
            _invocation: ToolInvocation,
            _progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            Ok(Box::new(FunctionToolOutput::success("write ok")))
        }
    }

    struct FailingHandler;

    #[async_trait]
    impl ToolHandler for FailingHandler {
        fn tool_kind(&self) -> ToolHandlerKind {
            ToolHandlerKind::Invalid
        }
        async fn handle(
            &self,
            _invocation: ToolInvocation,
            _progress: Option<ToolProgressSender>,
        ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
            Err(ToolExecutionError::ExecutionFailed {
                message: "something went wrong".into(),
            })
        }
    }

    fn register_tool(
        builder: &mut ToolRegistryBuilder,
        name: &str,
        handler: Arc<dyn ToolHandler>,
        is_read_only: bool,
    ) {
        let mode = if is_read_only {
            ToolExecutionMode::ReadOnly
        } else {
            ToolExecutionMode::Mutating
        };
        builder.register_handler(name, handler);
        builder.push_spec(ToolSpec {
            name: name.to_string(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: mode,
            capability_tags: vec![],
            supports_parallel: is_read_only,
        });
    }

    fn make_ctx(mode: PermissionMode) -> ToolContext {
        ToolContext {
            cwd: std::path::PathBuf::from("/tmp"),
            permissions: Arc::new(RuleBasedPolicy::new(mode)),
            session_id: "test-session".into(),
        }
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let registry = Arc::new(ToolRegistry::new());
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let call = ToolCall {
            id: "c1".into(),
            name: "nonexistent".into(),
            input: serde_json::json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("unknown tool"));
    }

    #[tokio::test]
    async fn read_only_tool_skips_permission_check() {
        let mut builder = ToolRegistryBuilder::new();
        register_tool(&mut builder, "read_tool", Arc::new(ReadOnlyHandler), true);
        let registry = Arc::new(builder.build());
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::Deny);

        let call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: serde_json::json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(!result.output.is_error);
        assert_eq!(result.output.content, "read ok");
    }

    #[tokio::test]
    async fn mutating_tool_denied_in_deny_mode() {
        let mut builder = ToolRegistryBuilder::new();
        register_tool(&mut builder, "write_tool", Arc::new(WriteHandler), false);
        let registry = Arc::new(builder.build());
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::Deny);

        let call = ToolCall {
            id: "c1".into(),
            name: "write_tool".into(),
            input: serde_json::json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("permission denied"));
    }

    #[tokio::test]
    async fn mutating_tool_allowed_in_auto_approve() {
        let mut builder = ToolRegistryBuilder::new();
        register_tool(&mut builder, "write_tool", Arc::new(WriteHandler), false);
        let registry = Arc::new(builder.build());
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let call = ToolCall {
            id: "c1".into(),
            name: "write_tool".into(),
            input: serde_json::json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(!result.output.is_error);
        assert_eq!(result.output.content, "write ok");
    }

    #[tokio::test]
    async fn failing_tool_wraps_error() {
        let mut builder = ToolRegistryBuilder::new();
        register_tool(&mut builder, "fail_tool", Arc::new(FailingHandler), false);
        let registry = Arc::new(builder.build());
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let call = ToolCall {
            id: "c1".into(),
            name: "fail_tool".into(),
            input: serde_json::json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("tool execution failed"));
    }

    #[tokio::test]
    async fn execute_batch_runs_all_tools() {
        let mut builder = ToolRegistryBuilder::new();
        register_tool(&mut builder, "read_tool", Arc::new(ReadOnlyHandler), true);
        register_tool(&mut builder, "write_tool", Arc::new(WriteHandler), false);
        let registry = Arc::new(builder.build());
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let calls = vec![
            ToolCall {
                id: "c1".into(),
                name: "read_tool".into(),
                input: serde_json::json!({}),
            },
            ToolCall {
                id: "c2".into(),
                name: "write_tool".into(),
                input: serde_json::json!({}),
            },
        ];
        let results = orch.execute_batch(&calls, &ctx).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.output.is_error));
    }
}
