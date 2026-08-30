use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

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
use devo_protocol::ModelProfileKey;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstStreamBehavior {
    CompleteImmediately,
    BlockUntilReleased,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedRequest {
    route: ProviderRoute,
    model_slug: ModelProfileKey,
    request_model: String,
}

struct RecordingRouter {
    stream_requests: Mutex<Vec<RecordedRequest>>,
    first_stream_behavior: FirstStreamBehavior,
    first_stream_started: Notify,
    release_first_stream: Notify,
}

impl RecordingRouter {
    fn new(first_stream_behavior: FirstStreamBehavior) -> Self {
        Self {
            stream_requests: Mutex::new(Vec::new()),
            first_stream_behavior,
            first_stream_started: Notify::new(),
            release_first_stream: Notify::new(),
        }
    }

    fn stream_requests(&self) -> Vec<RecordedRequest> {
        self.stream_requests
            .lock()
            .expect("stream requests mutex should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl ProviderRouter for RecordingRouter {
    async fn stream(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError> {
        let request_number = {
            let mut requests = self
                .stream_requests
                .lock()
                .expect("stream requests mutex should not be poisoned");
            requests.push(RecordedRequest {
                route,
                model_slug: request.model_slug,
                request_model: request.model,
            });
            requests.len()
        };
        if request_number == 1
            && self.first_stream_behavior == FirstStreamBehavior::BlockUntilReleased
        {
            self.first_stream_started.notify_one();
            self.release_first_stream.notified().await;
        }
        Ok(Box::pin(stream::iter([
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "routed reply".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response("routed reply"),
            }),
        ])))
    }

    async fn complete(
        &self,
        _route: ProviderRoute,
        _request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        Ok(model_response("Generated routed title"))
    }

    fn name(&self) -> &str {
        "model-selection-recording-router"
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

fn model_response(text: &str) -> ModelResponse {
    ModelResponse {
        id: "response".to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}

fn write_provider_config(data_root: &std::path::Path) -> Result<()> {
    std::fs::write(
        data_root.join("auth.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 1,
            "credentials": {
                "test_api_key": {
                    "kind": "api_key",
                    "value": "test-secret"
                }
            }
        }))?,
    )?;
    std::fs::write(
        data_root.join("config.toml"),
        r#"
[defaults]
model_binding = "main"

[providers.default]
enabled = true
name = "Default"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[providers.alternate]
enabled = true
name = "Alternate"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.main]
enabled = true
model_slug = "default-model"
provider = "default"
request_model = "vendor/default-model"
invocation_method = "openai_chat_completions"

[model_bindings.alt]
enabled = true
model_slug = "alt-model"
provider = "alternate"
request_model = "vendor/alt-model"
invocation_method = "openai_chat_completions"
"#,
    )?;
    Ok(())
}

fn build_runtime(
    data_root: &std::path::Path,
    router: Arc<RecordingRouter>,
) -> Result<Arc<ServerRuntime>> {
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("model-selection-regression.db"),
    )?);
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(UnusedProvider);
    let provider_router: Arc<dyn ProviderRouter> = router;
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            provider,
            provider_router,
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "default-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![
                Model {
                    slug: "default-model".to_string(),
                    display_name: "Default Model".to_string(),
                    ..Model::default()
                },
                Model {
                    slug: "alt-model".to_string(),
                    display_name: "Alt Model".to_string(),
                    ..Model::default()
                },
            ])),
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
    let response = runtime
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
                        "name": "model-selection-regression",
                        "title": "model-selection-regression",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    anyhow::ensure!(
        response.get("error").is_none(),
        "initialize failed: {response}"
    );
    Ok((connection_id, notifications_rx))
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "model-selection-regression-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response_value = response.clone();
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(response)
        .with_context(|| format!("decode session/new response: {response_value}"))?;
    Ok(SessionId::try_from(response.result.session.id.as_str())?)
}

async fn update_model(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    model: &str,
) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "model": { "provider": "", "model": model }
                }
            }),
        )
        .await
        .context("session/metadata/update response")?;
    let response_value = response.clone();
    Ok(serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
        >,
    >(response)
    .with_context(|| format!("decode session/metadata/update response: {response_value}"))?
    .result)
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    idempotency_key: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 4,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "use the selected model" }],
                    "idempotencyKey": idempotency_key
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let response_value = response.clone();
    let _: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response)
            .with_context(|| format!("decode turn/start response: {response_value}"))?;
    Ok(())
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    timeout(Duration::from_secs(10), async {
        while let Some(notification) = notifications_rx.recv().await {
            if notification["method"] == "turn/completed" {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for turn/completed")?
}

/// Trace: L2-DES-CONV-002
/// Verifies: a slug-only model update during a running turn clears the old
/// binding through MergeTurn so the following turn uses the new model.
#[tokio::test]
async fn model_update_during_turn_applies_to_next_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    write_provider_config(data_root.path())?;
    let router = Arc::new(RecordingRouter::new(
        FirstStreamBehavior::BlockUntilReleased,
    ));
    let runtime = build_runtime(data_root.path(), Arc::clone(&router))?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_turn(
        &runtime,
        connection_id,
        session_id,
        "model-selection-regression-turn-1",
    )
    .await?;
    timeout(
        Duration::from_secs(10),
        router.first_stream_started.notified(),
    )
    .await
    .context("first turn should reach the provider")?;

    let update = update_model(&runtime, connection_id, session_id, "alt-model").await?;
    assert_eq!(update.session.model.model, "alt-model");
    assert!(update.applied_to_active_turn);

    router.release_first_stream.notify_one();
    wait_for_turn_completed(&mut notifications_rx).await?;

    start_turn(
        &runtime,
        connection_id,
        session_id,
        "model-selection-regression-turn-2",
    )
    .await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(
        router.stream_requests(),
        vec![
            RecordedRequest {
                route: ProviderRoute::binding("default", ProviderWireApi::OpenAIChatCompletions),
                model_slug: ModelProfileKey::CatalogSlug("default-model".to_string()),
                request_model: "vendor/default-model".to_string(),
            },
            RecordedRequest {
                route: ProviderRoute::binding("alternate", ProviderWireApi::OpenAIChatCompletions,),
                model_slug: ModelProfileKey::CatalogSlug("alt-model".to_string()),
                request_model: "vendor/alt-model".to_string(),
            },
        ]
    );
    Ok(())
}

/// Trace: L2-DES-CONV-002
/// Verifies: a cold session persists a slug-only model update, then resume
/// and the next turn resolve the new model instead of the stale binding.
#[tokio::test]
async fn cold_session_model_update_survives_resume_and_turn() -> Result<()> {
    let data_root = TempDir::new()?;
    write_provider_config(data_root.path())?;

    let initial_runtime = build_runtime(
        data_root.path(),
        Arc::new(RecordingRouter::new(
            FirstStreamBehavior::CompleteImmediately,
        )),
    )?;
    let (initial_connection_id, mut initial_notifications) =
        initialize_connection(&initial_runtime).await?;
    let session_id =
        start_session(&initial_runtime, initial_connection_id, data_root.path()).await?;
    start_turn(
        &initial_runtime,
        initial_connection_id,
        session_id,
        "model-selection-regression-old-turn",
    )
    .await?;
    wait_for_turn_completed(&mut initial_notifications).await?;
    drop(initial_runtime);

    // Do not call load_persisted_sessions: the restarted runtime has only the
    // SQLite index, so this update exercises the cold-session write path.
    let router = Arc::new(RecordingRouter::new(
        FirstStreamBehavior::CompleteImmediately,
    ));
    let runtime = build_runtime(data_root.path(), Arc::clone(&router))?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let update = update_model(&runtime, connection_id, session_id, "alt-model").await?;
    assert_eq!(update.session.model.model, "alt-model");

    let resume_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/resume",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/resume response")?;
    let resume_value = resume_response.clone();
    let resume: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionResumeResult,
    > = serde_json::from_value(resume_response)
        .with_context(|| format!("decode session/resume response: {resume_value}"))?;
    assert_eq!(resume.result.session.model.model, "alt-model");

    start_turn(
        &runtime,
        connection_id,
        session_id,
        "model-selection-regression-cold-turn",
    )
    .await?;
    wait_for_turn_completed(&mut notifications_rx).await?;

    assert_eq!(
        router.stream_requests(),
        vec![RecordedRequest {
            route: ProviderRoute::binding("alternate", ProviderWireApi::OpenAIChatCompletions),
            model_slug: ModelProfileKey::CatalogSlug("alt-model".to_string()),
            request_model: "vendor/alt-model".to_string(),
        }]
    );
    Ok(())
}

/// Trace: L2-DES-CONV-002 DD-10
/// Verifies: the settings snapshot round-trips raw reasoning-effort
/// selections — including toggle keywords the `ReasoningEffort` enum cannot
/// express — through metadata/update responses, resume, read, and the cold
/// list snapshot. Parsing the selection through the enum on any read path
/// silently dropped the value and clients restored the default effort.
async fn effort_selection_round_trips_through_all_reads(effort: &str) -> Result<()> {
    let data_root = TempDir::new()?;
    write_provider_config(data_root.path())?;

    let initial_runtime = build_runtime(
        data_root.path(),
        Arc::new(RecordingRouter::new(
            FirstStreamBehavior::CompleteImmediately,
        )),
    )?;
    let (initial_connection_id, mut initial_notifications) =
        initialize_connection(&initial_runtime).await?;
    let session_id =
        start_session(&initial_runtime, initial_connection_id, data_root.path()).await?;
    start_turn(
        &initial_runtime,
        initial_connection_id,
        session_id,
        "effort-roundtrip-turn",
    )
    .await?;
    wait_for_turn_completed(&mut initial_notifications).await?;

    let update = initial_runtime
        .handle_incoming(
            initial_connection_id,
            serde_json::json!({
                "id": 21,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "reasoningEffort": effort }
                }
            }),
        )
        .await
        .context("metadata/update response")?;
    let update_value = update.clone();
    let update: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
    > = serde_json::from_value(update_value)
        .with_context(|| format!("decode metadata/update response: {update}"))?;
    assert_eq!(
        update.result.session.settings.reasoning_effort.as_deref(),
        Some(effort),
        "metadata/update response must echo the raw selection"
    );
    drop(initial_runtime);

    // Restart: every read path below must observe the persisted selection.
    let runtime = build_runtime(
        data_root.path(),
        Arc::new(RecordingRouter::new(
            FirstStreamBehavior::CompleteImmediately,
        )),
    )?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;

    let read_json = |method: &str, id: u64| {
        runtime.handle_incoming(
            connection_id,
            serde_json::json!({
                "id": id,
                "method": method,
                "params": { "sessionId": session_id }
            }),
        )
    };
    let resume: serde_json::Value = read_json("session/resume", 22)
        .await
        .context("session/resume response")?;
    assert_eq!(
        resume["result"]["session"]["settings"]["reasoningEffort"].as_str(),
        Some(effort),
        "session/resume must return the persisted selection"
    );
    let read: serde_json::Value = read_json("session/read", 23)
        .await
        .context("session/read response")?;
    assert_eq!(
        read["result"]["session"]["settings"]["reasoningEffort"].as_str(),
        Some(effort),
        "session/read must return the persisted selection"
    );
    let list: serde_json::Value = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 24,
                "method": "session/list",
                "params": { "cwds": [data_root.path()] }
            }),
        )
        .await
        .context("session/list response")?;
    let listed = list["result"]["data"]
        .as_array()
        .context("session/list data array")?
        .iter()
        .find(|entry| entry["id"].as_str() == Some(session_id.to_string().as_str()))
        .context("session present in list")?
        .clone();
    assert_eq!(
        listed["settings"]["reasoningEffort"].as_str(),
        Some(effort),
        "cold session/list snapshot must return the persisted selection"
    );
    Ok(())
}

/// Trace: L2-DES-CONV-002 DD-10
/// Verifies: toggle-keyword selections (variant/toggle-style models) survive
/// every read path after a restart.
#[tokio::test]
async fn metadata_update_toggle_selection_round_trips_through_all_reads() -> Result<()> {
    effort_selection_round_trips_through_all_reads("enabled").await
}

/// Trace: L2-DES-CONV-002 DD-10
/// Verifies: typed-level selections survive the same read paths.
#[tokio::test]
async fn metadata_update_levels_selection_round_trips_through_all_reads() -> Result<()> {
    effort_selection_round_trips_through_all_reads("xhigh").await
}

/// Trace: L2-DES-CONV-002 DD-4
/// Verifies: patching the same effort twice is a no-op the second time — no
/// new settings field line, no version bump. The old comparison ran the
/// persisted value through the `ReasoningEffort` enum, so a stored toggle
/// keyword compared unequal to itself on every patch.
#[tokio::test]
async fn repeated_identical_effort_patch_does_not_append_field_line() -> Result<()> {
    let data_root = TempDir::new()?;
    write_provider_config(data_root.path())?;
    let runtime = build_runtime(
        data_root.path(),
        Arc::new(RecordingRouter::new(
            FirstStreamBehavior::CompleteImmediately,
        )),
    )?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    let send_patch = |id: u64| {
        runtime.handle_incoming(
            connection_id,
            serde_json::json!({
                "id": id,
                "method": "session/metadata/update",
                "params": {
                    "sessionId": session_id,
                    "expectedVersion": 0,
                    "settings": { "reasoningEffort": "enabled" }
                }
            }),
        )
    };
    let first: serde_json::Value = send_patch(31).await.context("first patch")?;
    let second: serde_json::Value = send_patch(32).await.context("second patch")?;
    assert_eq!(
        first["result"]["session"]["settings"]["reasoningEffort"].as_str(),
        Some("enabled")
    );
    let first_version = first["result"]["session"]["version"].as_u64();
    let second_version = second["result"]["session"]["version"].as_u64();
    assert_eq!(
        first_version, second_version,
        "identical patch must not append a field line or bump the version"
    );
    Ok(())
}
