use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::Utc;
use devo_safety::legacy_permissions::{PermissionMode, RuleBasedPolicy};
use pretty_assertions::assert_eq;
use serde_json::json;

use crate::runtime::{
    LegacyRuntimeToolAdapter, RuntimeTool, RuntimeToolExecutor, RuntimeToolRegistry,
    ToolCapabilityTag, ToolContent, ToolDefinitionSpec, ToolExecutionContext, ToolExecutionMode,
    ToolExecutionOutcome, ToolInputError, ToolInvocation, ToolName, ToolPolicySnapshot,
    ToolRuntimeConfigSnapshot,
};
use crate::{Tool, ToolContext, ToolOutput};

struct DummyLegacyTool {
    emit_error_output: bool,
    calls: AtomicUsize,
}

#[async_trait]
impl Tool for DummyLegacyTool {
    fn name(&self) -> &str {
        "dummy"
    }

    fn description(&self) -> &str {
        "dummy tool"
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type":"object"})
    }

    async fn execute(
        &self,
        _ctx: &ToolContext,
        _input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.emit_error_output {
            Ok(ToolOutput::error("legacy failure"))
        } else {
            Ok(ToolOutput {
                content: "legacy ok".into(),
                is_error: false,
                metadata: Some(json!({"ok": true})),
            })
        }
    }
}

struct ValidatingRuntimeTool;

#[async_trait]
impl RuntimeTool for ValidatingRuntimeTool {
    fn definition(&self) -> ToolDefinitionSpec {
        ToolDefinitionSpec {
            name: ToolName("validator".into()),
            description: "validator".into(),
            input_schema: json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::ReadFiles],
        }
    }

    async fn validate(&self, _input: &serde_json::Value) -> Result<(), ToolInputError> {
        Err(ToolInputError::Invalid {
            message: "bad input".into(),
        })
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: ToolExecutionContext,
        _reporter: Arc<dyn super::ToolProgressReporter>,
    ) -> Result<ToolExecutionOutcome, super::ToolExecuteError> {
        panic!("validate should fail before execute")
    }
}

fn make_ctx(enabled_tools: Vec<&str>) -> ToolExecutionContext {
    ToolExecutionContext {
        session_id: "session-1".into(),
        turn_id: "turn-1".into(),
        cwd: PathBuf::from("/tmp"),
        permissions: Arc::new(RuleBasedPolicy::new(PermissionMode::AutoApprove)),
        policy_snapshot: ToolPolicySnapshot {
            mode: "auto_approve".into(),
            summary: None,
        },
        app_config: Arc::new(ToolRuntimeConfigSnapshot {
            enabled_tools: enabled_tools.into_iter().map(str::to_string).collect(),
            max_parallel_read_tools: 1,
        }),
    }
}

#[test]
fn runtime_registry_starts_empty() {
    let registry = RuntimeToolRegistry::new();
    assert!(registry.list().is_empty());
}

#[test]
fn legacy_adapter_definition_preserves_runtime_spec() {
    let adapter = LegacyRuntimeToolAdapter::new(
        Arc::new(DummyLegacyTool {
            emit_error_output: false,
            calls: AtomicUsize::new(0),
        }),
        ToolDefinitionSpec {
            name: ToolName("dummy".into()),
            description: "dummy tool".into(),
            input_schema: json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::ReadFiles],
        },
    );

    let definition = adapter.definition();
    assert_eq!(definition.name, ToolName("dummy".into()));
    assert_eq!(definition.description, "dummy tool");
    assert_eq!(
        definition.capability_tags,
        vec![ToolCapabilityTag::ReadFiles]
    );
}

#[tokio::test]
async fn executor_runs_legacy_adapter_and_returns_mixed_content() {
    let registry = Arc::new(RuntimeToolRegistry::new());
    registry.register(Arc::new(LegacyRuntimeToolAdapter::new(
        Arc::new(DummyLegacyTool {
            emit_error_output: false,
            calls: AtomicUsize::new(0),
        }),
        ToolDefinitionSpec {
            name: ToolName("dummy".into()),
            description: "dummy tool".into(),
            input_schema: json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::ReadFiles],
        },
    )));
    let executor = RuntimeToolExecutor::new(Arc::clone(&registry));
    let invocation = ToolInvocation {
        tool_call_id: super::ToolCallId("call-1".into()),
        session_id: "session-1".into(),
        turn_id: "turn-1".into(),
        tool_name: ToolName("dummy".into()),
        input: json!({}),
        requested_at: Utc::now(),
    };

    let result = executor
        .execute_batch(&[invocation], &make_ctx(vec!["dummy"]))
        .await;
    assert_eq!(result.len(), 1);
    match &result[0].outcome {
        ToolExecutionOutcome::Completed(payload) => {
            assert_eq!(
                payload.content,
                ToolContent::Mixed {
                    text: Some("legacy ok".into()),
                    json: Some(json!({"ok": true})),
                }
            );
        }
        other => panic!("expected completed outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn executor_maps_legacy_error_output_to_failed_outcome() {
    let registry = Arc::new(RuntimeToolRegistry::new());
    registry.register(Arc::new(LegacyRuntimeToolAdapter::new(
        Arc::new(DummyLegacyTool {
            emit_error_output: true,
            calls: AtomicUsize::new(0),
        }),
        ToolDefinitionSpec {
            name: ToolName("dummy".into()),
            description: "dummy tool".into(),
            input_schema: json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: Vec::new(),
        },
    )));
    let executor = RuntimeToolExecutor::new(Arc::clone(&registry));
    let invocation = ToolInvocation {
        tool_call_id: super::ToolCallId("call-1".into()),
        session_id: "session-1".into(),
        turn_id: "turn-1".into(),
        tool_name: ToolName("dummy".into()),
        input: json!({}),
        requested_at: Utc::now(),
    };

    let result = executor
        .execute_batch(&[invocation], &make_ctx(vec!["dummy"]))
        .await;
    match &result[0].outcome {
        ToolExecutionOutcome::Failed(failure) => {
            assert_eq!(failure.code, "tool_error");
            assert_eq!(failure.message, "legacy failure");
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn executor_rejects_disabled_tool() {
    let registry = Arc::new(RuntimeToolRegistry::new());
    registry.register(Arc::new(LegacyRuntimeToolAdapter::new(
        Arc::new(DummyLegacyTool {
            emit_error_output: false,
            calls: AtomicUsize::new(0),
        }),
        ToolDefinitionSpec {
            name: ToolName("dummy".into()),
            description: "dummy tool".into(),
            input_schema: json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: Vec::new(),
        },
    )));
    let executor = RuntimeToolExecutor::new(Arc::clone(&registry));
    let invocation = ToolInvocation {
        tool_call_id: super::ToolCallId("call-1".into()),
        session_id: "session-1".into(),
        turn_id: "turn-1".into(),
        tool_name: ToolName("dummy".into()),
        input: json!({}),
        requested_at: Utc::now(),
    };

    let result = executor
        .execute_batch(&[invocation], &make_ctx(vec!["other"]))
        .await;
    match &result[0].outcome {
        ToolExecutionOutcome::Denied(denied) => {
            assert_eq!(denied.reason, "tool is disabled: dummy");
        }
        other => panic!("expected denied outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn executor_stops_on_validation_failure() {
    let registry = Arc::new(RuntimeToolRegistry::new());
    registry.register(Arc::new(ValidatingRuntimeTool));
    let executor = RuntimeToolExecutor::new(Arc::clone(&registry));
    let invocation = ToolInvocation {
        tool_call_id: super::ToolCallId("call-1".into()),
        session_id: "session-1".into(),
        turn_id: "turn-1".into(),
        tool_name: ToolName("validator".into()),
        input: json!({}),
        requested_at: Utc::now(),
    };

    let result = executor
        .execute_batch(&[invocation], &make_ctx(vec!["validator"]))
        .await;
    match &result[0].outcome {
        ToolExecutionOutcome::Failed(failure) => {
            assert_eq!(failure.code, "invalid_input");
            assert_eq!(failure.message, "invalid tool input: bad input");
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
}

#[test]
fn protocol_tool_definitions_use_enabled_runtime_tools() {
    let registry = RuntimeToolRegistry::new();
    registry.register(Arc::new(ValidatingRuntimeTool));

    let definitions = registry.protocol_tool_definitions(&ToolRuntimeConfigSnapshot {
        enabled_tools: vec!["validator".into()],
        max_parallel_read_tools: 1,
    });

    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "validator");
    assert_eq!(definitions[0].description, "validator");
}

struct DelayedReadOnlyRuntimeTool {
    name: &'static str,
    delay_ms: u64,
}

#[async_trait]
impl RuntimeTool for DelayedReadOnlyRuntimeTool {
    fn definition(&self) -> ToolDefinitionSpec {
        ToolDefinitionSpec {
            name: ToolName(self.name.into()),
            description: self.name.into(),
            input_schema: json!({"type":"object"}),
            output_mode: super::ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![ToolCapabilityTag::ReadFiles],
        }
    }

    async fn validate(&self, _input: &serde_json::Value) -> Result<(), ToolInputError> {
        Ok(())
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: ToolExecutionContext,
        _reporter: Arc<dyn super::ToolProgressReporter>,
    ) -> Result<ToolExecutionOutcome, super::ToolExecuteError> {
        tokio::time::sleep(tokio::time::Duration::from_millis(self.delay_ms)).await;
        Ok(ToolExecutionOutcome::Completed(
            crate::runtime::ToolResultPayload {
                content: ToolContent::Text(self.name.into()),
                metadata: crate::runtime::ToolResultMetadata::default(),
            },
        ))
    }
}

#[tokio::test]
async fn executor_preserves_input_order_for_parallel_read_only_tools() {
    let registry = Arc::new(RuntimeToolRegistry::new());
    registry.register(Arc::new(DelayedReadOnlyRuntimeTool {
        name: "slow",
        delay_ms: 30,
    }));
    registry.register(Arc::new(DelayedReadOnlyRuntimeTool {
        name: "fast",
        delay_ms: 1,
    }));
    let executor = RuntimeToolExecutor::new(Arc::clone(&registry));
    let invocations = vec![
        ToolInvocation {
            tool_call_id: super::ToolCallId("call-1".into()),
            session_id: "session-1".into(),
            turn_id: "turn-1".into(),
            tool_name: ToolName("slow".into()),
            input: json!({}),
            requested_at: Utc::now(),
        },
        ToolInvocation {
            tool_call_id: super::ToolCallId("call-2".into()),
            session_id: "session-1".into(),
            turn_id: "turn-1".into(),
            tool_name: ToolName("fast".into()),
            input: json!({}),
            requested_at: Utc::now(),
        },
    ];

    let results = executor
        .execute_batch(
            &invocations,
            &ToolExecutionContext {
                session_id: "session-1".into(),
                turn_id: "turn-1".into(),
                cwd: PathBuf::from("/tmp"),
                permissions: Arc::new(RuleBasedPolicy::new(PermissionMode::AutoApprove)),
                policy_snapshot: ToolPolicySnapshot {
                    mode: "auto_approve".into(),
                    summary: None,
                },
                app_config: Arc::new(ToolRuntimeConfigSnapshot {
                    enabled_tools: vec!["slow".into(), "fast".into()],
                    max_parallel_read_tools: 2,
                }),
            },
        )
        .await;

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].tool_name, ToolName("slow".into()));
    assert_eq!(results[1].tool_name, ToolName("fast".into()));
}
