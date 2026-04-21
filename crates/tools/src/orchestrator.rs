use std::sync::Arc;

use tracing::{info, warn};

use devo_safety::legacy_permissions::{PermissionDecision, PermissionRequest, ResourceKind};

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
///
/// Corresponds to Claude Code's `toolOrchestration.ts` and
/// `toolExecution.ts`. Handles:
/// - Looking up tools in the registry
/// - Permission checks before execution
/// - Serial vs concurrent dispatch
/// - Error wrapping
pub struct ToolOrchestrator {
    registry: Arc<ToolRegistry>,
}

impl ToolOrchestrator {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }

    /// Execute a batch of tool calls.
    ///
    /// Read-only tools that support concurrency are executed in parallel.
    /// Mutating tools are executed sequentially to avoid conflicts.
    pub async fn execute_batch(
        &self,
        calls: &[ToolCall],
        ctx: &ToolContext,
    ) -> Vec<ToolCallResult> {
        let mut results = Vec::with_capacity(calls.len());

        // Partition into concurrent (read-only) and sequential (mutating)
        let (concurrent, sequential): (Vec<_>, Vec<_>) = calls.iter().partition(|call| {
            self.registry
                .get(&call.name)
                .map(|t| t.supports_concurrency())
                .unwrap_or(false)
        });

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
        let Some(tool) = self.registry.get(&call.name) else {
            warn!(tool = %call.name, "tool not found");
            return ToolCallResult {
                tool_use_id: call.id.clone(),
                output: ToolOutput::error(format!("unknown tool: {}", call.name)),
            };
        };

        // Permission check for mutating tools
        if !tool.is_read_only() {
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
                    // Interactive approval is not yet wired to a UI prompt.
                    // Surface as a tool error so the model can report it to the user
                    // rather than silently failing. The CLI can later intercept this
                    // by providing a PermissionPolicy that blocks and asks the user.
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

        match tool.execute(ctx, call.input.clone()).await {
            Ok(output) => ToolCallResult {
                tool_use_id: call.id.clone(),
                output,
            },
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
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use devo_safety::legacy_permissions::{PermissionMode, RuleBasedPolicy};

    use crate::{Tool, ToolContext, ToolOutput};

    struct ReadOnlyTool;

    #[async_trait]
    impl Tool for ReadOnlyTool {
        fn name(&self) -> &str {
            "read_tool"
        }
        fn description(&self) -> &str {
            "reads stuff"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput::success("read ok"))
        }
        fn is_read_only(&self) -> bool {
            true
        }
    }

    struct WriteTool {
        call_count: AtomicUsize,
    }

    #[async_trait]
    impl Tool for WriteTool {
        fn name(&self) -> &str {
            "write_tool"
        }
        fn description(&self) -> &str {
            "writes stuff"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolOutput> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("write ok"))
        }
        fn is_read_only(&self) -> bool {
            false
        }
    }

    struct FailingTool;

    #[async_trait]
    impl Tool for FailingTool {
        fn name(&self) -> &str {
            "failing_tool"
        }
        fn description(&self) -> &str {
            "always fails"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        async fn execute(
            &self,
            _ctx: &ToolContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolOutput> {
            anyhow::bail!("something went wrong")
        }
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
            input: json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("unknown tool"));
    }

    #[tokio::test]
    async fn read_only_tool_skips_permission_check() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ReadOnlyTool));
        let registry = Arc::new(reg);
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::Deny);

        let call = ToolCall {
            id: "c1".into(),
            name: "read_tool".into(),
            input: json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(!result.output.is_error);
        assert_eq!(result.output.content, "read ok");
    }

    #[tokio::test]
    async fn mutating_tool_denied_in_deny_mode() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(WriteTool {
            call_count: AtomicUsize::new(0),
        }));
        let registry = Arc::new(reg);
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::Deny);

        let call = ToolCall {
            id: "c1".into(),
            name: "write_tool".into(),
            input: json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("permission denied"));
    }

    #[tokio::test]
    async fn mutating_tool_allowed_in_auto_approve() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(WriteTool {
            call_count: AtomicUsize::new(0),
        }));
        let registry = Arc::new(reg);
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let call = ToolCall {
            id: "c1".into(),
            name: "write_tool".into(),
            input: json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(!result.output.is_error);
        assert_eq!(result.output.content, "write ok");
    }

    #[tokio::test]
    async fn interactive_mode_returns_ask() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(WriteTool {
            call_count: AtomicUsize::new(0),
        }));
        let registry = Arc::new(reg);
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::Interactive);

        let call = ToolCall {
            id: "c1".into(),
            name: "write_tool".into(),
            input: json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("permission required"));
    }

    #[tokio::test]
    async fn failing_tool_wraps_error() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(FailingTool));
        let registry = Arc::new(reg);
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let call = ToolCall {
            id: "c1".into(),
            name: "failing_tool".into(),
            input: json!({}),
        };
        let result = orch.execute_single(&call, &ctx).await;
        assert!(result.output.is_error);
        assert!(result.output.content.contains("tool execution failed"));
    }

    #[tokio::test]
    async fn execute_batch_runs_all_tools() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(ReadOnlyTool));
        reg.register(Arc::new(WriteTool {
            call_count: AtomicUsize::new(0),
        }));
        let registry = Arc::new(reg);
        let orch = ToolOrchestrator::new(registry);
        let ctx = make_ctx(PermissionMode::AutoApprove);

        let calls = vec![
            ToolCall {
                id: "c1".into(),
                name: "read_tool".into(),
                input: json!({}),
            },
            ToolCall {
                id: "c2".into(),
                name: "write_tool".into(),
                input: json!({}),
            },
        ];
        let results = orch.execute_batch(&calls, &ctx).await;
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.output.is_error));
    }
}
