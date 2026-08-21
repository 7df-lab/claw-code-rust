use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::tools::ToolCallError;
use devo_core::tools::ToolHandler;
use devo_core::tools::ToolRegistry;
use devo_core::tools::ToolRegistryBuilder;
use devo_core::tools::ToolResult;
use devo_core::tools::ToolResultContent;
use devo_core::tools::create_default_tool_registry;
use devo_core::tools::json_schema::JsonSchema;
use devo_core::tools::tool_spec::ToolExecutionMode;
use devo_core::tools::tool_spec::ToolOutputMode;
use devo_core::tools::tool_spec::ToolSpec;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::native::rpc_session::GoalIfExists;
use devo_provider::ModelProviderSDK;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[path = "support/goal_continuation.rs"]
mod support;

use support::CapturingProvider;
use support::build_runtime;
use support::build_runtime_with_registry;
use support::collect_until_turn_completed;
use support::create_goal;
use support::initialize_connection;
use support::pause_goal_and_interrupt_session;
use support::start_session;
use support::wait_for_approval_request;
use support::wait_for_notification;

#[tokio::test]
async fn goal_set_does_not_start_continuation_in_plan_mode() -> Result<()> {
    // Trace: L2-DES-GOAL-001
    let data_root = TempDir::new()?;
    let provider = Arc::new(CapturingProvider::default());
    let runtime = build_runtime(data_root.path(), provider.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 40,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "plan first" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "plan"
                }
            }),
        )
        .await
        .context("plan turn/start response")?;
    collect_until_turn_completed(&mut notifications_rx).await?;
    assert_eq!(provider.requests.lock().expect("lock requests").len(), 1);

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "do not continue in plan mode",
        None,
        GoalIfExists::Reject,
        "goal-plan-mode",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.lock().expect("lock requests").len(), 1);
    Ok(())
}

struct ToolCallProvider {
    requests: AtomicUsize,
    tool_name: &'static str,
    tool_input: serde_json::Value,
}

struct EscalatingTool;

#[async_trait]
impl ModelProviderSDK for ToolCallProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("goal continuation test does not use non-streaming completion")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst);
        if request_number == 0 {
            return Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".to_string(),
                    name: self.tool_name.to_string(),
                    input: serde_json::json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: self.tool_input.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "tool-call-response".to_string(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".to_string(),
                            name: self.tool_name.to_string(),
                            input: self.tool_input.clone(),
                        }],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ])));
        }

        Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "tool-followup-response".to_string(),
                content: vec![ResponseContent::Text("Tool follow-up done.".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        })])))
    }

    fn name(&self) -> &str {
        "tool-call-goal-provider"
    }
}

#[async_trait]
impl ToolHandler for EscalatingTool {
    fn spec(&self) -> &ToolSpec {
        Box::leak(Box::new(escalating_tool_spec()))
    }

    async fn handle(
        &self,
        _ctx: devo_core::tools::ToolContext,
        _input: serde_json::Value,
        _progress: Option<devo_core::tools::ToolProgressSender>,
    ) -> std::result::Result<ToolResult, ToolCallError> {
        Ok(ToolResult::success(
            ToolResultContent::Text("approved".to_string()),
            "approved",
        ))
    }
}

fn escalating_tool_registry() -> ToolRegistry {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("escalating_tool", Arc::new(EscalatingTool));
    builder.push_spec(escalating_tool_spec());
    builder.build()
}

fn escalating_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "escalating_tool".to_string(),
        description: "Escalates so the test can observe pending approval.".to_string(),
        input_schema: JsonSchema::object(BTreeMap::new(), None, None),
        output_mode: ToolOutputMode::Text,
        execution_mode: ToolExecutionMode::Mutating,
        capability_tags: vec![],
        supports_parallel: false,
        preparation_feedback: devo_core::tools::ToolPreparationFeedback::None,
        display_name: None,
        supports_cancellation: None,
        supports_streaming: None,
    }
}

#[tokio::test]
async fn goal_set_does_not_start_continuation_while_approval_is_pending() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ToolCallProvider {
        requests: AtomicUsize::new(0),
        tool_name: "escalating_tool",
        tool_input: serde_json::json!({
            "sandbox_permissions": "require_escalated",
            "justification": "approval test"
        }),
    });
    let registry = Arc::new(escalating_tool_registry());
    let runtime = build_runtime_with_registry(data_root.path(), provider.clone(), registry)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 50,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "ask for approval" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": "on-request",
                    "cwd": null
                }
            }),
        )
        .await
        .context("approval turn/start response")?;
    let start_result: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(start_response)?;
    wait_for_approval_request(&mut notifications_rx).await?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "wait for approval first",
        None,
        GoalIfExists::Reject,
        "goal-approval-pending",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    pause_goal_and_interrupt_session(
        &runtime,
        connection_id,
        session_id,
        start_result
            .result
            .turn_id()
            .expect("turn/start should start approval turn"),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn goal_set_does_not_start_continuation_while_user_input_is_pending() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(ToolCallProvider {
        requests: AtomicUsize::new(0),
        tool_name: "request_user_input",
        tool_input: serde_json::json!({
            "question": "Which path should the goal use?"
        }),
    });
    let runtime = build_runtime_with_registry(
        data_root.path(),
        provider.clone(),
        Arc::new(create_default_tool_registry()),
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 60,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "ask the user" }],
                    "model": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null,
                    "collaboration_mode": "plan"
                }
            }),
        )
        .await
        .context("request_user_input turn/start response")?;
    let start_result: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(start_response)?;
    wait_for_notification(&mut notifications_rx, "item/tool/requestUserInput").await?;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "wait for user input first",
        None,
        GoalIfExists::Reject,
        "goal-user-input-pending",
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(/*millis*/ 50)).await;
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);

    pause_goal_and_interrupt_session(
        &runtime,
        connection_id,
        session_id,
        start_result
            .result
            .turn_id()
            .expect("turn/start should start request-user-input turn"),
    )
    .await?;
    Ok(())
}
