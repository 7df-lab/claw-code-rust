use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use devo_core::tools::create_default_tool_registry;
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
use tokio::sync::Notify;
use tokio::time::timeout;

#[path = "support/goal_continuation.rs"]
mod support;

use support::build_runtime_with_registry;
use support::collect_until_turn_completed;
use support::create_goal;
use support::initialize_connection;
use support::read_goal;
use support::start_session;
use support::wait_for_captured_request_count;

#[derive(Default)]
struct CompletingGoalProvider {
    requests: AtomicUsize,
    captured_requests: Mutex<Vec<ModelRequest>>,
    unexpected_request: Notify,
}

#[async_trait]
impl ModelProviderSDK for CompletingGoalProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(text_response("goal-title", "Completed goal"))
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        self.captured_requests
            .lock()
            .expect("lock captured requests")
            .push(request);

        match request_number {
            1 => {
                let input = serde_json::json!({ "status": "complete" });
                Ok(Box::pin(stream::iter(vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "complete-goal".to_string(),
                        name: "update_goal".to_string(),
                        input: input.clone(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "update-goal-response".to_string(),
                            content: vec![ResponseContent::ToolUse {
                                id: "complete-goal".to_string(),
                                name: "update_goal".to_string(),
                                input,
                            }],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: ResponseMetadata::default(),
                        },
                    }),
                ])))
            }
            2 => Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "The goal is complete.".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: text_response("final-response", "The goal is complete."),
                }),
            ]))),
            _ => {
                self.unexpected_request.notify_one();
                Ok(Box::pin(stream::pending()))
            }
        }
    }

    fn name(&self) -> &str {
        "completing-goal-provider"
    }
}

#[tokio::test]
async fn update_goal_completion_finishes_current_turn_without_another_continuation() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider = Arc::new(CompletingGoalProvider::default());
    let runtime = build_runtime_with_registry(
        data_root.path(),
        provider.clone(),
        Arc::new(create_default_tool_registry()),
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    create_goal(
        &runtime,
        connection_id,
        session_id,
        "complete the goal with update_goal",
        None,
        GoalIfExists::Reject,
        "goal-update-completion",
    )
    .await?;

    collect_until_turn_completed(&mut notifications_rx).await?;
    wait_for_captured_request_count(&provider.captured_requests, /*expected*/ 2).await?;

    let response = read_goal(&runtime, connection_id, session_id).await?;
    assert_eq!(
        response.map(|goal| goal.status),
        Some(devo_protocol::native::goal::GoalStatus::Completed)
    );

    {
        let requests = provider
            .captured_requests
            .lock()
            .expect("lock captured requests");
        assert!(
            requests[1].messages.iter().any(|message| {
                message.content.iter().any(|content| {
                    matches!(
                        content,
                        devo_protocol::RequestContent::ToolResult { content, .. }
                            if content.contains("Goal marked complete")
                    )
                })
            }),
            "the follow-up request should contain the successful update_goal result"
        );
    }

    assert!(
        timeout(
            Duration::from_secs(/*secs*/ 1),
            provider.unexpected_request.notified()
        )
        .await
        .is_err(),
        "a completed goal must not start another provider request"
    );
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    Ok(())
}

fn text_response(id: &str, text: &str) -> ModelResponse {
    ModelResponse {
        id: id.to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}
