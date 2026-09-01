use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Datelike;
use chrono::SecondsFormat;
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
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::TurnId;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderRoute;
use devo_provider::ProviderRouter;
use devo_provider::error::ProviderError;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;

struct BlockingRouter {
    stream_calls: mpsc::UnboundedSender<ModelRequest>,
}

#[async_trait]
impl ProviderRouter for BlockingRouter {
    async fn stream(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError> {
        let _ = self.stream_calls.send(request);
        Ok(Box::pin(stream::pending()))
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        Ok(model_response("Generated title"))
    }

    fn name(&self) -> &str {
        "blocking-router"
    }
}

struct UnusedProvider;

#[async_trait]
impl ModelProviderSDK for UnusedProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        anyhow::bail!("unused provider should not receive completion requests")
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("unused provider should not receive streaming requests")
    }

    fn name(&self) -> &str {
        "unused-provider"
    }
}

/// Turn (stream) requests are captured and complete immediately; title
/// (complete) requests park on a gate the test controls, simulating a slow
/// title model.
struct GatedTitleRouter {
    stream_calls: mpsc::UnboundedSender<ModelRequest>,
    title_gate: Arc<Notify>,
    title_entered: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl ProviderRouter for GatedTitleRouter {
    async fn stream(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError> {
        let _ = self.stream_calls.send(request);
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "answer".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response("answer"),
            }),
        ])))
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        let waiting = self.title_gate.notified();
        self.title_entered
            .store(true, std::sync::atomic::Ordering::SeqCst);
        waiting.await;
        Ok(model_response("Gated generated title"))
    }

    fn name(&self) -> &str {
        "gated-title-router"
    }
}

#[tokio::test]
async fn turn_start_append_failure_does_not_launch_model_turn_or_leave_session_active() -> Result<()>
{
    let data_root = TempDir::new()?;
    let (stream_calls_tx, mut stream_calls_rx) = mpsc::unbounded_channel();
    let runtime = build_runtime(data_root.path(), stream_calls_tx)?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let session = start_session(&runtime, connection_id, data_root.path()).await?;
    let rollout_path = rollout_path_for_session(data_root.path(), &session);

    std::fs::remove_file(&rollout_path)?;
    std::fs::create_dir(&rollout_path)?;

    let failed_start = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": turn_start_params(&session.id)
            }),
        )
        .await
        .context("failed turn/start response")?;
    assert_eq!(
        failed_start["error"]["code"],
        serde_json::json!("InternalError")
    );
    assert!(
        failed_start["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("failed to persist turn start")
    );
    assert!(
        timeout(Duration::from_millis(150), stream_calls_rx.recv())
            .await
            .is_err(),
        "failed turn/start unexpectedly invoked provider streaming"
    );

    std::fs::remove_dir(&rollout_path)?;
    let successful_start = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "turn/start",
                "params": turn_start_params(&session.id)
            }),
        )
        .await
        .context("successful turn/start response")?;
    let response: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(successful_start)?;
    assert_eq!(
        response.result.turn.status,
        devo_protocol::native::turn::TurnStatus::InProgress
    );
    stream_calls_rx
        .recv()
        .await
        .context("provider stream call after successful turn/start")?;
    interrupt_session(
        &runtime,
        connection_id,
        &session.id,
        TurnId::try_from(response.result.turn.id.as_str())?,
    )
    .await?;

    Ok(())
}

/// Trace: L2-DES-SERVER-title-generation
/// Verifies: turn/start returns with a heuristic title before LLM polish;
/// polish waits until after the turn merges and may park on a slow provider.
#[tokio::test]
async fn turn_start_answers_before_slow_title_generation_completes() -> Result<()> {
    let data_root = TempDir::new()?;
    let (stream_calls_tx, mut stream_calls_rx) = mpsc::unbounded_channel();
    let title_gate = Arc::new(Notify::new());
    let title_entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = build_runtime_with_router(
        data_root.path(),
        Arc::new(GatedTitleRouter {
            stream_calls: stream_calls_tx,
            title_gate: Arc::clone(&title_gate),
            title_entered: Arc::clone(&title_entered),
        }),
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session = start_session(&runtime, connection_id, data_root.path()).await?;

    let turn_response = timeout(
        Duration::from_secs(2),
        runtime.handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "turn/start",
                "params": turn_start_params(&session.id)
            }),
        ),
    )
    .await
    .context("turn/start stalled on gated title generation")?
    .context("connection closed before turn/start response")?;
    let response: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(turn_response)?;
    assert_eq!(
        response.result.turn.status,
        devo_protocol::native::turn::TurnStatus::InProgress
    );

    wait_for_title_update(&mut notifications_rx, "hello").await?;

    timeout(Duration::from_secs(5), stream_calls_rx.recv())
        .await
        .context("turn stream call after heuristic title")?
        .context("stream call channel closed")?;

    wait_for_notification(&mut notifications_rx, "turn/completed", 5).await?;

    for _ in 0..500 {
        if title_entered.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        title_entered.load(std::sync::atomic::Ordering::SeqCst),
        "title polish should start after the turn merges"
    );

    title_gate.notify_one();
    wait_for_title_update(&mut notifications_rx, "Gated generated title").await?;

    Ok(())
}

#[tokio::test]
async fn message_edit_previous_accepts_skip_restore_and_replaces_prompt_branch() -> Result<()> {
    let data_root = TempDir::new()?;
    let (stream_calls_tx, mut stream_calls_rx) = mpsc::unbounded_channel();
    let runtime = build_runtime(data_root.path(), stream_calls_tx)?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let session = start_session(&runtime, connection_id, data_root.path()).await?;

    let original_start = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "turn/start",
                "params": turn_start_params(&session.id)
            }),
        )
        .await
        .context("original turn/start response")?;
    let original_start: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::TurnStartResult,
    > = serde_json::from_value(original_start)?;
    let original_request = stream_calls_rx
        .recv()
        .await
        .context("original provider request")?;
    assert!(
        request_messages_json(&original_request)?.contains("hello"),
        "original request should contain submitted prompt"
    );
    interrupt_session(
        &runtime,
        connection_id,
        &session.id,
        TurnId::try_from(original_start.result.turn.id.as_str())?,
    )
    .await?;

    let (item_id, expected_revision) =
        previous_user_item(&runtime, connection_id, &session.id).await?;

    let edit_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/message/edit",
                "params": {
                "sessionId": session.id,
                    "itemId": item_id,
                    "expectedRevision": expected_revision,
                    "content": [{ "type": "text", "text": "edited message" }],
                    "workspaceRestore": "skip",
                    "idempotencyKey": "edit-skip-restore"
                }
            }),
        )
        .await
        .context("session/message/edit response")?;
    let edit_response: devo_protocol::native::rpc_session::SessionMessageEditResult =
        serde_json::from_value(edit_response["result"].clone())?;
    let replacement_turn_id = edit_response
        .replacement_turn_id
        .clone()
        .context("replacement turn id")?;
    let replacement_request = stream_calls_rx
        .recv()
        .await
        .context("replacement provider request")?;
    let replacement_messages = request_messages_json(&replacement_request)?;
    assert!(
        replacement_messages.contains("edited message"),
        "replacement request should contain edited prompt: {replacement_messages}"
    );
    assert!(
        !replacement_messages.contains("hello"),
        "replacement request should not include superseded prompt: {replacement_messages}"
    );

    let rollout = std::fs::read_to_string(rollout_path_for_session(data_root.path(), &session))?;
    // v2 write path: edit markers travel as internal lines.
    assert!(rollout.contains(r#""type":"messageEdit""#));
    assert!(rollout.contains(r#""type":"turnSuperseded""#));
    assert!(rollout.contains(&edit_response.item.id.to_string()));
    assert!(rollout.contains(replacement_turn_id.as_str()));

    interrupt_session(
        &runtime,
        connection_id,
        &session.id,
        TurnId::try_from(replacement_turn_id.as_str()).context("legacy replacement turn id")?,
    )
    .await?;

    Ok(())
}

/// Trace: L2-DES-APP-003, L1-REQ-CONV-005
/// Verifies: omitted workspace_restore_policy uses default safe restore and emits restore lifecycle records/events.
#[tokio::test]
async fn message_edit_previous_default_safe_restore_records_and_broadcasts() -> Result<()> {
    let data_root = TempDir::new()?;
    let (stream_calls_tx, mut stream_calls_rx) = mpsc::unbounded_channel();
    let runtime = build_runtime(data_root.path(), stream_calls_tx)?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session = start_session(&runtime, connection_id, data_root.path()).await?;

    let original_start = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "turn/start",
                "params": turn_start_params(&session.id)
            }),
        )
        .await
        .context("original turn/start response")?;
    let original_start: devo_server::SuccessResponse<
        devo_protocol::native::rpc_turn::TurnStartResult,
    > = serde_json::from_value(original_start)?;
    stream_calls_rx
        .recv()
        .await
        .context("original provider request")?;
    interrupt_session(
        &runtime,
        connection_id,
        &session.id,
        TurnId::try_from(original_start.result.turn.id.as_str())?,
    )
    .await?;
    drain_notifications(&mut notifications_rx).await;

    let (item_id, expected_revision) =
        previous_user_item(&runtime, connection_id, &session.id).await?;

    let edit_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/message/edit",
                "params": {
                "sessionId": session.id,
                    "itemId": item_id,
                    "expectedRevision": expected_revision,
                    "content": [{ "type": "text", "text": "edited message" }],
                    "idempotencyKey": "edit-safe-restore"
                }
            }),
        )
        .await
        .context("session/message/edit response")?;
    let edit_response: devo_protocol::native::rpc_session::SessionMessageEditResult =
        serde_json::from_value(edit_response["result"].clone())?;
    let replacement_turn_id = edit_response
        .replacement_turn_id
        .clone()
        .context("replacement turn id")?;
    let replacement_request = stream_calls_rx
        .recv()
        .await
        .context("replacement provider request")?;
    let replacement_messages = request_messages_json(&replacement_request)?;
    assert!(
        replacement_messages.contains("edited message"),
        "replacement request should contain edited prompt: {replacement_messages}"
    );
    assert!(
        !replacement_messages.contains("hello"),
        "replacement request should not include superseded prompt: {replacement_messages}"
    );

    let rollout = std::fs::read_to_string(rollout_path_for_session(data_root.path(), &session))?;
    // v2 write path: workspace restore lines carry camelCase kinds.
    assert!(rollout.contains(r#""kind":"workspaceRestoreStarted""#));
    assert!(rollout.contains(r#""kind":"workspaceRestoreCompleted""#));
    assert!(rollout.contains("\"policy\":\"safe\""));

    let methods = collect_notification_methods(&mut notifications_rx).await;
    assert!(
        methods
            .iter()
            .any(|method| method == "workspace_restore_started"),
        "expected workspace_restore_started notification in {methods:?}"
    );
    assert!(
        methods
            .iter()
            .any(|method| method == "workspace_restore_completed"),
        "expected workspace_restore_completed notification in {methods:?}"
    );

    interrupt_session(
        &runtime,
        connection_id,
        &session.id,
        TurnId::try_from(replacement_turn_id.as_str()).context("legacy replacement turn id")?,
    )
    .await?;

    Ok(())
}

async fn previous_user_item(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
) -> Result<(String, u32)> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 100,
                "method": "session/items/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/items/list response")?;
    let page: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        serde_json::from_value(response["result"].clone())?;
    let item = page
        .data
        .iter()
        .find(|item| {
            matches!(
                &item.item,
                devo_protocol::native::item::Item::UserMessage { .. }
            )
        })
        .context("previous user message item")?;
    Ok((item.id.as_str().to_string(), item.revision))
}

async fn drain_notifications(notifications_rx: &mut mpsc::Receiver<serde_json::Value>) {
    while timeout(Duration::from_millis(10), notifications_rx.recv())
        .await
        .is_ok()
    {}
}

async fn wait_for_notification(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    expected_method: &str,
    timeout_secs: u64,
) -> Result<()> {
    timeout(Duration::from_secs(timeout_secs), async {
        while let Some(value) = notifications_rx.recv().await {
            let method = value
                .get("method")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    value
                        .get("params")
                        .and_then(|params| params.get("_meta"))
                        .and_then(|meta| meta.get("devo/originalMethod"))
                        .and_then(serde_json::Value::as_str)
                });
            if method == Some(expected_method) {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before {expected_method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {expected_method} notification"))??;
    Ok(())
}

async fn wait_for_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    expected_title: &str,
) -> Result<()> {
    let mut seen = Vec::new();
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            let method = value
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<none>");
            seen.push(method.to_string());
            // Native connections project the title event as a metadata
            // update; legacy surfaces keep the dedicated title method.
            let title_landed =
                matches!(method, "session/title/updated" | "session/metadataUpdated")
                    && value["params"]["session"]["title"] == serde_json::json!(expected_title);
            if title_landed {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before title update")
    })
    .await
    .with_context(|| {
        format!("timed out waiting for title update {expected_title}; seen: {seen:?}")
    })??;
    Ok(())
}

async fn collect_notification_methods(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Vec<String> {
    let mut methods = Vec::new();
    while let Ok(Some(notification)) =
        timeout(Duration::from_millis(10), notifications_rx.recv()).await
    {
        if let Some(method) = notification["params"]["_meta"]["devo/originalMethod"]
            .as_str()
            .or_else(|| {
                notification
                    .get("method")
                    .and_then(serde_json::Value::as_str)
            })
        {
            methods.push(method.to_string());
        }
    }
    methods
}

fn model_response(text: &str) -> ModelResponse {
    ModelResponse {
        id: "response-1".to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

fn build_runtime(
    data_root: &Path,
    stream_calls: mpsc::UnboundedSender<ModelRequest>,
) -> Result<Arc<ServerRuntime>> {
    build_runtime_with_router(data_root, Arc::new(BlockingRouter { stream_calls }))
}

fn build_runtime_with_router(
    data_root: &Path,
    router: Arc<dyn ProviderRouter>,
) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(UnusedProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("turn_start_persistence.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            provider,
            router,
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![Model {
                slug: "test-model".to_string(),
                display_name: "Test Model".to_string(),
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
                /*workspace_root*/ None,
            )?)),
        ),
    ))
}

fn request_messages_json(request: &ModelRequest) -> Result<String> {
    serde_json::to_string(&request.messages).context("serialize request messages")
}

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(128);
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
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": {
                        "name": "turn-start-persistence-test",
                        "title": "turn-start-persistence-test",
                        "version": "1.0.0"
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

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
) -> Result<devo_protocol::native::session::Session> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "turn-start-persistence-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(response)?;
    let session_id = response.result.session.id.clone();
    let metadata_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "model": { "provider": "", "model": "test-model" }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let _: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
    > = serde_json::from_value(metadata_response)?;
    Ok(response.result.session)
}

async fn interrupt_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: &devo_protocol::native::ids::SessionId,
    _turn_id: TurnId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
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
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionInterruptResult,
    > = serde_json::from_value(response)?;
    assert!(response.result.interrupted);
    Ok(())
}

fn turn_start_params(session_id: &devo_protocol::native::ids::SessionId) -> serde_json::Value {
    serde_json::json!({
        "sessionId": session_id,
        "input": [{ "type": "text", "text": "hello" }],
        "idempotencyKey": format!("turn-start-persistence-{}", uuid::Uuid::new_v4())
    })
}

fn rollout_path_for_session(
    data_root: &Path,
    session: &devo_protocol::native::session::Session,
) -> std::path::PathBuf {
    let timestamp = session
        .created_at
        .to_rfc3339_opts(SecondsFormat::Secs, true)
        .replace(':', "-");
    data_root
        .join("sessions")
        .join(format!("{:04}", session.created_at.year()))
        .join(format!("{:02}", session.created_at.month()))
        .join(format!("{:02}", session.created_at.day()))
        .join(format!("rollout-{timestamp}-{}.jsonl", session.id))
}
