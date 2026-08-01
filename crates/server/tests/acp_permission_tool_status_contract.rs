use std::collections::BTreeMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::ProviderVendorCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolCallError;
use devo_core::tools::ToolHandler;
use devo_core::tools::ToolRegistry;
use devo_core::tools::ToolRegistryBuilder;
use devo_core::tools::ToolResult;
use devo_core::tools::ToolResultContent;
use devo_core::tools::json_schema::JsonSchema;
use devo_core::tools::tool_spec::ToolExecutionMode;
use devo_core::tools::tool_spec::ToolOutputMode;
use devo_core::tools::tool_spec::ToolSpec;
use devo_protocol::AcpEmptyResult;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::AcpPromptResult;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::AcpStopReason;
use devo_protocol::AcpToolCallStatus;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::timeout;

#[tokio::test]
async fn acp_permission_flow_uses_request_response_and_tool_status_lifecycle() -> Result<()> {
    let mut prompt = start_permission_prompt(3).await?;
    let permission_request_id = wait_for_pending_tool_call_and_permission_request(
        &mut prompt.notifications_rx,
        &prompt.session_id,
    )
    .await?;
    let permission_response = prompt
        .runtime
        .handle_incoming(
            prompt.connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": permission_request_id,
                "result": {
                    "outcome": {
                        "outcome": "selected",
                        "optionId": "allow_once"
                    }
                }
            }),
        )
        .await;
    assert_eq!(permission_response, None);

    wait_for_in_progress_completed_and_prompt_response(&mut prompt.notifications_rx, 3).await?;
    assert_eq!(prompt.tool_calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn acp_permission_rejection_fails_tool_without_legacy_method() -> Result<()> {
    let mut prompt = start_permission_prompt(3).await?;
    let permission_request_id = wait_for_pending_tool_call_and_permission_request(
        &mut prompt.notifications_rx,
        &prompt.session_id,
    )
    .await?;
    assert_legacy_approval_respond_removed(&prompt.runtime, prompt.connection_id).await?;
    let permission_response = prompt
        .runtime
        .handle_incoming(
            prompt.connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": permission_request_id,
                "result": {
                    "outcome": {
                        "outcome": "selected",
                        "optionId": "reject_once"
                    }
                }
            }),
        )
        .await;
    assert_eq!(permission_response, None);

    wait_for_failed_tool_and_prompt_response(&mut prompt.notifications_rx, 3).await?;
    assert_eq!(prompt.tool_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn acp_permission_cancellation_fails_tool_without_executing() -> Result<()> {
    let mut prompt = start_permission_prompt(3).await?;
    let permission_request_id = wait_for_pending_tool_call_and_permission_request(
        &mut prompt.notifications_rx,
        &prompt.session_id,
    )
    .await?;
    let permission_response = prompt
        .runtime
        .handle_incoming(
            prompt.connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": permission_request_id,
                "result": {
                    "outcome": {
                        "outcome": "cancelled"
                    }
                }
            }),
        )
        .await;
    assert_eq!(permission_response, None);

    wait_for_failed_tool_and_prompt_response(&mut prompt.notifications_rx, 3).await?;
    assert_eq!(prompt.tool_calls.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn acp_session_cancel_returns_empty_result_to_json_rpc_request() -> Result<()> {
    let prompt = start_permission_prompt(3).await?;
    let cancel_response = prompt
        .runtime
        .handle_incoming(
            prompt.connection_id,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "session/cancel",
                "params": {
                    "sessionId": prompt.session_id
                }
            }),
        )
        .await
        .context("session/cancel response")?;
    let cancel_response: AcpSuccessResponse<AcpEmptyResult> =
        serde_json::from_value(cancel_response)?;

    assert_eq!(
        cancel_response,
        AcpSuccessResponse {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(4),
            result: AcpEmptyResult::default(),
        }
    );
    Ok(())
}

struct PermissionPrompt {
    _data_root: TempDir,
    runtime: Arc<ServerRuntime>,
    connection_id: u64,
    notifications_rx: mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
    tool_calls: Arc<AtomicUsize>,
}

async fn start_permission_prompt(prompt_request_id: u64) -> Result<PermissionPrompt> {
    let data_root = TempDir::new()?;
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(PermissionToolProvider {
        stream_calls: AtomicUsize::new(0),
    });
    let registry = Arc::new(approval_tool_registry(Arc::clone(&tool_calls)));
    let runtime = build_runtime(data_root.path(), provider, registry)?;
    let (connection_id, notifications_rx) = initialize_acp_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let session_id = create_acp_session(&runtime, connection_id, &cwd, 2).await?;

    let prompt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": prompt_request_id,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [
                        {
                            "type": "text",
                            "text": "call the approval tool"
                        }
                    ]
                }
            }),
        )
        .await;
    assert_eq!(prompt_response, None);

    Ok(PermissionPrompt {
        _data_root: data_root,
        runtime,
        connection_id,
        notifications_rx,
        session_id,
        tool_calls,
    })
}

struct PermissionToolProvider {
    stream_calls: AtomicUsize,
}

#[async_trait]
impl ModelProviderSDK for PermissionToolProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "title-1".to_string(),
            content: vec![ResponseContent::Text(
                "Generated ACP permission title".to_string(),
            )],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let stream_call = self.stream_calls.fetch_add(1, Ordering::SeqCst);
        if stream_call == 0 {
            let input = serde_json::json!({
                "sandbox_permissions": "require_escalated",
                "justification": "ACP permission contract"
            });
            return Ok(Box::pin(stream::iter(vec![
                Ok(StreamEvent::ToolCallStart {
                    index: 0,
                    id: "tool-1".to_string(),
                    name: "approval_tool".to_string(),
                    input: serde_json::json!({}),
                }),
                Ok(StreamEvent::ToolCallInputDelta {
                    index: 0,
                    partial_json: input.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "tool-call-response".to_string(),
                        content: vec![ResponseContent::ToolUse {
                            id: "tool-1".to_string(),
                            name: "approval_tool".to_string(),
                            input,
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
                id: "final-response".to_string(),
                content: vec![ResponseContent::Text("Done.".to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: ResponseMetadata::default(),
            },
        })])))
    }

    fn name(&self) -> &str {
        "acp-permission-tool-status-provider"
    }
}

struct ApprovalTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolHandler for ApprovalTool {
    fn spec(&self) -> &ToolSpec {
        Box::leak(Box::new(approval_tool_spec()))
    }

    async fn handle(
        &self,
        _ctx: devo_core::tools::ToolContext,
        _input: serde_json::Value,
        _progress: Option<devo_core::tools::ToolProgressSender>,
    ) -> std::result::Result<ToolResult, ToolCallError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::success(
            ToolResultContent::Text("approved".to_string()),
            "approved",
        ))
    }
}

fn approval_tool_registry(calls: Arc<AtomicUsize>) -> ToolRegistry {
    let mut builder = ToolRegistryBuilder::new();
    builder.register_handler("approval_tool", Arc::new(ApprovalTool { calls }));
    builder.push_spec(approval_tool_spec());
    builder.build()
}

fn approval_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "approval_tool".to_string(),
        description: "Requires user permission for ACP contract testing.".to_string(),
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

async fn wait_for_pending_tool_call_and_permission_request(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: &SessionId,
) -> Result<u64> {
    let started = Instant::now();
    let mut saw_pending_tool_call = false;
    let mut permission_request_id = None;
    let mut seen = Vec::new();
    loop {
        let value = match timeout(Duration::from_millis(100), notifications_rx.recv()).await {
            Ok(Some(value)) => value,
            Ok(None) => {
                anyhow::bail!(
                    "notification channel closed before ACP permission request: {seen:#?}"
                )
            }
            Err(_) if started.elapsed() > Duration::from_secs(5) => {
                anyhow::bail!("timed out waiting for ACP permission request: {seen:#?}")
            }
            Err(_) => continue,
        };
        seen.push(value.clone());
        if let Some(params) = acp_session_notification_params(&value) {
            let notification: AcpSessionNotification = serde_json::from_value(params)?;
            if notification.session_id != *session_id {
                continue;
            }
            if matches!(
                notification.update,
                AcpSessionUpdate::ToolCall {
                    ref tool_call_id,
                    status: AcpToolCallStatus::Pending,
                    ..
                } if tool_call_id == "tool-1"
            ) {
                saw_pending_tool_call = true;
            }
        } else if value.get("method") == Some(&serde_json::json!("session/request_permission")) {
            assert_eq!(value["params"]["sessionId"], serde_json::json!(session_id));
            assert_eq!(
                value["params"]["toolCall"]["toolCallId"],
                serde_json::json!("tool-1")
            );
            assert_eq!(
                value["params"]["toolCall"]["status"],
                serde_json::json!("pending")
            );
            assert_eq!(
                value["params"]["options"][0]["optionId"],
                serde_json::json!("allow_once")
            );
            permission_request_id = value.get("id").and_then(serde_json::Value::as_u64);
        }
        if saw_pending_tool_call && let Some(permission_request_id) = permission_request_id {
            return Ok(permission_request_id);
        }
    }
}

async fn wait_for_in_progress_completed_and_prompt_response(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    prompt_request_id: u64,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        let mut saw_in_progress = false;
        let mut saw_completed = false;
        let mut saw_prompt_response = false;
        while let Some(value) = notifications_rx.recv().await {
            if let Some(params) = acp_session_notification_params(&value) {
                let notification: AcpSessionNotification = serde_json::from_value(params)?;
                match notification.update {
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::InProgress),
                        ..
                    } if tool_call_id == "tool-1" => {
                        saw_in_progress = true;
                    }
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::Completed),
                        ..
                    } if tool_call_id == "tool-1" => {
                        saw_completed = true;
                    }
                    _ => {}
                }
            } else if value.get("id") == Some(&serde_json::json!(prompt_request_id)) {
                let response: AcpSuccessResponse<AcpPromptResult> = serde_json::from_value(value)?;
                assert_eq!(response.result.stop_reason, AcpStopReason::EndTurn);
                saw_prompt_response = true;
            }
            if saw_in_progress && saw_completed && saw_prompt_response {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before ACP prompt completed")
    })
    .await
    .context("timed out waiting for ACP prompt completion")?
}

async fn wait_for_failed_tool_and_prompt_response(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    prompt_request_id: u64,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        let mut saw_failed = false;
        let mut saw_prompt_response = false;
        while let Some(value) = notifications_rx.recv().await {
            if let Some(params) = acp_session_notification_params(&value) {
                let notification: AcpSessionNotification = serde_json::from_value(params)?;
                match notification.update {
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::InProgress),
                        ..
                    } if tool_call_id == "tool-1" => {
                        anyhow::bail!("denied permission unexpectedly started tool execution")
                    }
                    AcpSessionUpdate::ToolCallUpdate {
                        tool_call_id,
                        status: Some(AcpToolCallStatus::Failed),
                        ..
                    } if tool_call_id == "tool-1" => {
                        saw_failed = true;
                    }
                    _ => {}
                }
            } else if value.get("id") == Some(&serde_json::json!(prompt_request_id)) {
                let response: AcpSuccessResponse<AcpPromptResult> = serde_json::from_value(value)?;
                assert_eq!(response.result.stop_reason, AcpStopReason::EndTurn);
                saw_prompt_response = true;
            }
            if saw_failed && saw_prompt_response {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before denied ACP prompt completed")
    })
    .await
    .context("timed out waiting for denied ACP prompt completion")?
}

fn acp_session_notification_params(value: &serde_json::Value) -> Option<serde_json::Value> {
    if value.get("method") == Some(&serde_json::json!("session/update")) {
        return value.get("params").cloned();
    }
    value.get("update").is_some().then(|| value.clone())
}

async fn assert_legacy_approval_respond_removed(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 99,
                "method": "approval/respond",
                "params": {}
            }),
        )
        .await
        .context("legacy approval/respond response")?;
    assert_eq!(response["id"], serde_json::json!(99));
    assert_eq!(
        response["error"]["code"],
        serde_json::json!("InvalidParams")
    );
    assert_eq!(
        response["error"]["message"],
        serde_json::json!("unknown method: approval/respond")
    );
    Ok(())
}

fn build_runtime(
    data_root: &Path,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
) -> Result<Arc<ServerRuntime>> {
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("acp_permission_tool_status_contract.db"),
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

async fn initialize_acp_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
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
                        "name": "acp-permission-tool-status-test",
                        "title": "ACP Permission Tool Status Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(initialize_response["jsonrpc"], serde_json::json!("2.0"));
    assert_eq!(initialize_response["id"], serde_json::json!(1));
    Ok((connection_id, notifications_rx))
}

async fn create_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
    request_id: u64,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/new",
                "params": {
                    "cwd": cwd.to_string_lossy().into_owned(),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: AcpSuccessResponse<AcpNewSessionResult> = serde_json::from_value(response)?;
    Ok(response.result.session_id)
}
