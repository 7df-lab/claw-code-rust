use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[path = "support/rollout.rs"]
mod support;

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
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::TurnErrorPayload;
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
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;

const PROVIDER_ERROR_TEXT: &str = "Internal server error";
const FAILING_ATTEMPTS: usize = 6;

#[derive(Default)]
struct ExhaustingRouter {
    attempts: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ExhaustingRouter {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("lock requests").clone()
    }
}

#[async_trait]
impl ProviderRouter for ExhaustingRouter {
    async fn stream(
        &self,
        _route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.requests.lock().expect("lock requests").push(request);
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt < FAILING_ATTEMPTS {
            return Ok(Box::pin(stream::iter(vec![Err(
                ProviderError::ProviderServerError {
                    message: PROVIDER_ERROR_TEXT.to_string(),
                    status_code: Some(500),
                    provider_name: Some("openai".to_string()),
                }
                .into(),
            )])));
        }
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "valid response".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response("valid response"),
            }),
        ])))
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        Ok(model_response("Generated title"))
    }

    fn name(&self) -> &str {
        "exhausting-router"
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
    ) -> Result<std::pin::Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        anyhow::bail!("unused provider should not receive streaming requests")
    }

    fn name(&self) -> &str {
        "unused-provider"
    }
}

#[tokio::test(start_paused = true)]
async fn exhausted_provider_retries_persist_for_history_but_do_not_enter_context() -> Result<()> {
    let data_root = TempDir::new()?;
    write_provider_config(data_root.path())?;
    let router = Arc::new(ExhaustingRouter::default());
    let runtime = build_runtime(data_root.path(), router.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session = start_session(&runtime, connection_id, data_root.path()).await?;

    let failed_turn_id = start_turn(&runtime, connection_id, session.session_id, 3).await?;
    let mut retry_statuses = Vec::new();
    let mut failed_error = None;
    let mut failed_completion_count = 0;
    let mut failed_agent_items = Vec::new();
    timeout(Duration::from_secs(30), async {
        while let Some(value) = notifications_rx.recv().await {
            match value.get("method").and_then(serde_json::Value::as_str) {
                Some("model/queryRetrying") => retry_statuses.push(value["params"].clone()),
                Some("item/started" | "item/completed")
                    if value["params"]["item"]["item"]["type"]
                        == serde_json::json!("assistantMessage") =>
                {
                    failed_agent_items.push(value["params"]["item"].clone());
                }
                Some("turn/completed")
                    if value["params"]["turn"]["status"] == serde_json::json!("failed") =>
                {
                    failed_completion_count += 1;
                    let error = &value["params"]["turn"]["error"];
                    failed_error = Some(TurnErrorPayload {
                        code: error["errorCode"].as_str().unwrap_or_default().to_string(),
                        message: error["message"].as_str().unwrap_or_default().to_string(),
                        recovery_hint: error["details"]["recoveryHint"]
                            .as_str()
                            .map(ToString::to_string),
                    });
                }
                Some("session/statusChanged")
                    if value["params"]["status"] == serde_json::json!("idle") =>
                {
                    break;
                }
                Some(_) | None => {}
            }
        }
    })
    .await
    .context("timed out waiting for failed turn")?;

    assert_eq!(
        retry_statuses,
        expected_retry_statuses(session.session_id, failed_turn_id)
    );
    assert_eq!(
        failed_error,
        Some(TurnErrorPayload {
            code: "PROVIDER_SERVER_ERROR".to_string(),
            message: format!(
                "model provider error: provider server error (Some(500)): {PROVIDER_ERROR_TEXT}"
            ),
            recovery_hint: None,
        })
    );
    assert_eq!(failed_completion_count, 1);
    assert_eq!(failed_agent_items, Vec::<serde_json::Value>::new());

    let rollout = std::fs::read_to_string(rollout_path(data_root.path(), &session))?;
    assert!(rollout.contains(PROVIDER_ERROR_TEXT));
    let persisted_error =
        support::read_rollout_lines_dual(&rollout_path(data_root.path(), &session))?
            .into_iter()
            .find_map(|line| match line {
                devo_core::RolloutLine::Turn(line) if line.turn.id == failed_turn_id => {
                    line.turn.error
                }
                _ => None,
            });
    assert_eq!(
        persisted_error,
        Some(devo_core::TurnError {
            code: "PROVIDER_SERVER_ERROR".to_string(),
            message: format!(
                "model provider error: provider server error (Some(500)): {PROVIDER_ERROR_TEXT}"
            ),
            recovery_hint: None,
        })
    );

    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/resume",
                "params": { "session_id": session.session_id }
            }),
        )
        .await
        .context("session/resume after failed turn")?;
    let resume = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;
    let terminal_history = resume
        .history_items
        .iter()
        .filter(|item| {
            matches!(
                item.kind,
                devo_protocol::SessionHistoryItemKind::Error
                    | devo_protocol::SessionHistoryItemKind::TurnSummary
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let failed_turn = resume.latest_turn.context("latest failed turn")?;
    let duration_secs = failed_turn.completed_at.and_then(|completed| {
        let seconds = (completed - failed_turn.started_at).num_seconds();
        (seconds > 0).then_some(seconds as u64)
    });
    assert_eq!(
        terminal_history,
        vec![
            devo_protocol::SessionHistoryItem::new(
                None,
                devo_protocol::SessionHistoryItemKind::Error,
                "PROVIDER_SERVER_ERROR".to_string(),
                format!(
                    "model provider error: provider server error (Some(500)): {PROVIDER_ERROR_TEXT}"
                ),
            ),
            devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::TurnSummary,
                title: failed_turn.model,
                body: "failed".to_string(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::TurnSummary {
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                }),
                duration_ms: duration_secs,
            },
        ]
    );

    let successful_turn_id = start_turn(&runtime, connection_id, session.session_id, 4).await?;
    wait_for_turn_completed(&mut notifications_rx, successful_turn_id).await?;
    let requests = router.requests();
    let successful_request = requests.last().context("successful provider request")?;
    let request_json = serde_json::to_string(successful_request)?;
    assert!(!request_json.contains(PROVIDER_ERROR_TEXT));
    assert_eq!(router.attempts.load(Ordering::SeqCst), FAILING_ATTEMPTS + 1);
    assert_ne!(successful_turn_id, failed_turn_id);

    Ok(())
}

fn expected_retry_statuses(session_id: SessionId, turn_id: TurnId) -> Vec<serde_json::Value> {
    let mut statuses = Vec::new();
    for attempt in 1..=5 {
        let backoff_ms = 250 * 2_u64.pow((attempt - 1) as u32);
        statuses.push(serde_json::json!({
            "sessionId": session_id,
            "turnId": turn_id,
            "attempt": attempt,
            "maxAttempts": 5,
            "nextDelayMs": backoff_ms,
            "error": {
                "errorCode": "PROVIDER_TEMPORARY_FAILURE",
                "message": format!(
                    "Retrying provider request in {:.1}s",
                    Duration::from_millis(backoff_ms).as_secs_f64()
                ),
                "retryable": true,
                "retryAfterMs": backoff_ms,
                "requiresSnapshot": false
            },
            "provider": "openai",
            "model": "default-model",
            "phase": "scheduled"
        }));
        statuses.push(serde_json::json!({
            "sessionId": session_id,
            "turnId": turn_id,
            "attempt": attempt,
            "maxAttempts": 5,
            "nextDelayMs": 0,
            "error": {
                "errorCode": "PROVIDER_TEMPORARY_FAILURE",
                "message": "Retrying provider request now",
                "retryable": true,
                "retryAfterMs": 0,
                "requiresSnapshot": false
            },
            "provider": "openai",
            "model": "default-model",
            "phase": "resumed"
        }));
    }
    statuses
}

fn write_provider_config(data_root: &std::path::Path) -> Result<()> {
    std::fs::write(
        data_root.join("auth.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "credentials": {
                "test_api_key": { "kind": "api_key", "value": "test-secret" }
            }
        }))?,
    )?;
    std::fs::write(
        data_root.join("config.toml"),
        r#"
[defaults]
model_binding = "main"

[providers.openai]
enabled = true
name = "OpenAI"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "default-model"
provider = "openai"
request_model = "provider-model"
invocation_method = "openai_chat_completions"
"#,
    )?;
    Ok(())
}

fn build_runtime(
    data_root: &std::path::Path,
    router: Arc<ExhaustingRouter>,
) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(UnusedProvider);
    let provider_router: Arc<dyn ProviderRouter> = router;
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("provider_failure_reporting.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            provider,
            provider_router,
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "default-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![Model {
                slug: "default-model".to_string(),
                display_name: "Default Model".to_string(),
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

async fn initialize_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(128);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, notifications_tx)
        .await;
    runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "_meta": { "devo": { "protocol": "native" } },
                    "clientInfo": { "name": "failure-test", "version": "1.0.0" }
                }
            }),
        )
        .await
        .context("initialize response")?;
    Ok((connection_id, notifications_rx))
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<devo_server::SessionMetadata> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/start",
                "params": {
                    "cwd": cwd,
                    "ephemeral": false,
                    "title": null,
                    "model_binding_id": "main"
                }
            }),
        )
        .await
        .context("session/start response")?;
    let response: devo_server::SuccessResponse<devo_server::SessionStartResult> =
        serde_json::from_value(response)?;
    Ok(response.result.session)
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    id: u64,
) -> Result<TurnId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": id,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "try the provider" }],
                    "model_binding_id": "main"
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let response: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)?;
    response.result.turn_id().context("turn should start")
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    turn_id: TurnId,
) -> Result<()> {
    let turn_id = turn_id.to_string();
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method").and_then(serde_json::Value::as_str) == Some("turn/completed")
                && value["params"]["turn"]["id"].as_str() == Some(turn_id.as_str())
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed for {turn_id}")
    })
    .await
    .with_context(|| format!("timed out waiting for turn/completed for {turn_id}"))?
}

fn rollout_path(
    data_root: &std::path::Path,
    session: &devo_server::SessionMetadata,
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
        .join(format!("rollout-{timestamp}-{}.jsonl", session.session_id))
}

fn model_response(text: &str) -> ModelResponse {
    ModelResponse {
        id: "response".to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}
