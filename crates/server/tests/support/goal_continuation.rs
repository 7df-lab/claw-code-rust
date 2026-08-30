#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::RequestContent;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::Stream;
use futures::StreamExt;
use futures::stream;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[derive(Default)]
pub struct CapturingProvider {
    pub requests: Mutex<Vec<ModelRequest>>,
}

#[derive(Default)]
pub struct PendingProvider {
    pub requests: AtomicUsize,
}

pub struct QueuedPriorityProvider {
    pub requests: Mutex<Vec<ModelRequest>>,
    pub release_first: Arc<Notify>,
}

pub struct UsageProvider {
    pub requests: AtomicUsize,
    pub captured_requests: Mutex<Vec<ModelRequest>>,
    pub usage: Usage,
}

pub struct BudgetWrapupPendingProvider {
    pub requests: AtomicUsize,
    pub captured_requests: Mutex<Vec<ModelRequest>>,
    pub usage: Usage,
}

pub struct FailingProvider {
    pub requests: AtomicUsize,
    pub message: String,
}

#[async_trait]
impl ModelProviderSDK for CapturingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.lock().expect("lock requests").push(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Working on the goal.".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: text_response(
                    "goal-response",
                    "Working on the goal.",
                    StopReason::EndTurn,
                ),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "capturing-goal-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for QueuedPriorityProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = {
            let mut requests = self.requests.lock().expect("lock requests");
            requests.push(request);
            requests.len()
        };
        if request_number == 1 {
            let release_first = Arc::clone(&self.release_first);
            return Ok(Box::pin(stream::once(async move {
                release_first.notified().await;
                Ok(StreamEvent::MessageDone {
                    response: text_response(
                        "queued-first-response",
                        "First turn done.",
                        StopReason::EndTurn,
                    ),
                })
            })));
        }

        Ok(Box::pin(stream::pending()))
    }

    fn name(&self) -> &str {
        "queued-priority-goal-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for UsageProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.captured_requests
            .lock()
            .expect("lock requests")
            .push(request);
        let usage = self.usage.clone();
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Budget usage done.".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: ModelResponse {
                    id: "usage-response".to_string(),
                    content: vec![ResponseContent::Text("Budget usage done.".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage,
                    metadata: ResponseMetadata::default(),
                },
            }),
        ])))
    }

    fn name(&self) -> &str {
        "usage-goal-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for BudgetWrapupPendingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let request_number = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        self.captured_requests
            .lock()
            .expect("lock requests")
            .push(request);
        if request_number == 1 {
            let usage = self.usage.clone();
            return Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: "Budget usage done.".to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "budget-usage-response".to_string(),
                        content: vec![ResponseContent::Text("Budget usage done.".to_string())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage,
                        metadata: ResponseMetadata::default(),
                    },
                }),
            ])));
        }

        Ok(Box::pin(
            stream::iter(vec![Ok(StreamEvent::TextDelta {
                index: 0,
                text: "Budget wrap-up started.".to_string(),
            })])
            .chain(stream::pending()),
        ))
    }

    fn name(&self) -> &str {
        "budget-wrapup-pending-goal-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for FailingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!(self.message.clone()))
    }

    fn name(&self) -> &str {
        "failing-goal-provider"
    }
}

#[async_trait]
impl ModelProviderSDK for PendingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(title_response())
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::pending()))
    }

    fn name(&self) -> &str {
        "pending-goal-provider"
    }
}

pub fn build_runtime(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    build_runtime_with_registry(data_root, provider, Arc::new(ToolRegistry::new()))
}

pub fn build_runtime_with_registry(
    data_root: &std::path::Path,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
) -> Result<Arc<ServerRuntime>> {
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("goal_continuation.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            registry,
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![Model {
                slug: "test-model".to_string(),
                display_name: "test-model".to_string(),
                ..Model::default()
            }])),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.to_path_buf(),
                None,
            )?)),
        ),
    ))
}

pub async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(1024);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "goal-test",
                        "title": "goal-test",
                        "version": "1.0.0"
                    },
                    "_meta": {
                        "devo": {
                            "protocol": "native"
                        }
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let response: serde_json::Value = initialize_response;
    assert_eq!(
        response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok((connection_id, notifications_rx))
}

pub async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<devo_protocol::SessionId> {
    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 10,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "goal-continuation-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(start_response)?;
    Ok(devo_protocol::SessionId::try_from(
        response.result.session.id.as_str(),
    )?)
}

pub async fn set_session_mode(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    mode: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "mode": mode }
                }
            }),
        )
        .await
        .context("session/metadata/update mode response")?;
    if response.get("error").is_some() {
        anyhow::bail!("session/metadata/update failed: {response}");
    }
    tokio::time::sleep(Duration::from_millis(/*millis*/ 25)).await;
    Ok(())
}

pub async fn create_goal(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    objective: &str,
    token_budget: Option<u64>,
    if_exists: devo_protocol::native::rpc_session::GoalIfExists,
    idempotency_key: &str,
) -> Result<devo_protocol::native::goal::Goal> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": idempotency_key,
                "method": "session/goal/set",
                "params": {
                    "sessionId": session_id,
                    "objective": objective,
                    "tokenBudget": token_budget,
                    "ifExists": if_exists,
                    "idempotencyKey": idempotency_key
                }
            }),
        )
        .await
        .context("session/goal/set response")?;
    let response_value = response.clone();
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionGoalSetResult,
    > = serde_json::from_value(response)
        .with_context(|| format!("decode session/goal/set response: {response_value}"))?;
    Ok(response.result.goal)
}

pub async fn read_goal(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
) -> Result<Option<devo_protocol::native::goal::Goal>> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": "goal-read",
                "method": "session/goal/read",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/goal/read response")?;
    let response_value = response.clone();
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionGoalReadResult,
    > = serde_json::from_value(response)
        .with_context(|| format!("decode session/goal/read response: {response_value}"))?;
    Ok(response.result.goal)
}

pub async fn transition_goal(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
    method: &str,
    goal_id: &devo_protocol::native::ids::GoalId,
) -> Result<devo_protocol::native::goal::Goal> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": format!("goal-transition-{method}"),
                "method": method,
                "params": {
                    "sessionId": session_id,
                    "expectedGoalId": goal_id
                }
            }),
        )
        .await
        .with_context(|| format!("{method} response"))?;
    let response_value = response.clone();
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionGoalTransitionResult,
    > = serde_json::from_value(response)
        .with_context(|| format!("decode {method} response: {response_value}"))?;
    Ok(response.result.goal)
}

pub async fn wait_for_notification(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<serde_json::Value> {
    let expected = serde_json::json!(method);
    timeout(Duration::from_secs(/*secs*/ 5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&expected) {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before {method}")
    })
    .await
    .context("timed out waiting for notification")?
}

pub async fn wait_for_approval_request(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(/*secs*/ 5), async {
        while let Some(value) = notifications_rx.recv().await {
            if matches!(
                value.get("method").and_then(serde_json::Value::as_str),
                Some(
                    devo_protocol::ACP_SESSION_REQUEST_PERMISSION_METHOD
                        | "approval/command/request"
                        | "approval/fileChange/request"
                        | "approval/permission/request"
                )
            ) {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before approval request")
    })
    .await
    .context("timed out waiting for approval request")?
}

pub async fn wait_for_request_count(requests: &AtomicUsize, expected: usize) -> Result<()> {
    timeout(Duration::from_secs(/*secs*/ 5), async {
        loop {
            if requests.load(Ordering::SeqCst) == expected {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .context("timed out waiting for provider request")?
}

pub async fn wait_for_captured_request_count(
    requests: &Mutex<Vec<ModelRequest>>,
    expected: usize,
) -> Result<()> {
    timeout(Duration::from_secs(/*secs*/ 5), async {
        loop {
            if requests.lock().expect("lock requests").len() == expected {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(/*millis*/ 10)).await;
        }
    })
    .await
    .context("timed out waiting for captured provider request")?
}

pub async fn collect_until_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<Vec<serde_json::Value>> {
    timeout(Duration::from_secs(/*secs*/ 5), async {
        let mut values = Vec::new();
        while let Some(value) = notifications_rx.recv().await {
            let completed = value.get("method") == Some(&serde_json::json!("turn/completed"));
            values.push(value);
            if completed {
                return Ok(values);
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for turn/completed")?
}

pub async fn pause_goal_and_interrupt_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: devo_protocol::SessionId,
) -> Result<()> {
    let goal = read_goal(runtime, connection_id, session_id)
        .await?
        .context("goal to pause")?;
    transition_goal(
        runtime,
        connection_id,
        session_id,
        "session/goal/pause",
        &goal.id,
    )
    .await?;
    let _ = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 91,
                "method": "session/interrupt",
                "params": {
                    "scope": {
                        "scope": "session",
                        "sessionId": session_id
                    }
                }
            }),
        )
        .await
        .context("session/interrupt response")?;
    Ok(())
}

pub fn is_user_message_item(value: &serde_json::Value) -> bool {
    matches!(
        value.get("method").and_then(serde_json::Value::as_str),
        Some("item/started" | "item/completed")
    ) && value["params"]["item"]["item"]["type"] == serde_json::json!("userMessage")
}

pub fn request_contains_text(request: &ModelRequest, needle: &str) -> bool {
    request.messages.iter().any(|message| {
        message.content.iter().any(|content| match content {
            RequestContent::Text { text } | RequestContent::Reasoning { text } => {
                text.contains(needle)
            }
            RequestContent::ProviderReasoning { .. }
            | RequestContent::ToolUse { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolResult { .. } => false,
        })
    })
}

pub fn request_last_message_contains_text(request: &ModelRequest, needle: &str) -> bool {
    request.messages.last().is_some_and(|message| {
        message.content.iter().any(|content| match content {
            RequestContent::Text { text } | RequestContent::Reasoning { text } => {
                text.contains(needle)
            }
            RequestContent::ProviderReasoning { .. }
            | RequestContent::ToolUse { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolResult { .. } => false,
        })
    })
}

fn text_response(id: &str, text: &str, stop_reason: StopReason) -> ModelResponse {
    ModelResponse {
        id: id.to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(stop_reason),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

pub fn title_response() -> ModelResponse {
    text_response(
        "goal-title-response",
        "Goal test title",
        StopReason::EndTurn,
    )
}
