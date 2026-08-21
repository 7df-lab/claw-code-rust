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
use devo_protocol::ReasoningCapability;
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
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedRequest {
    route: ProviderRoute,
    model_slug: ModelProfileKey,
    request_model: String,
    request_thinking: Option<String>,
}

#[derive(Default)]
struct RecordingRouter {
    stream_requests: Mutex<Vec<RecordedRequest>>,
    complete_requests: Mutex<Vec<RecordedRequest>>,
}

impl RecordingRouter {
    fn stream_requests(&self) -> Vec<RecordedRequest> {
        self.stream_requests
            .lock()
            .expect("stream requests mutex should not be poisoned")
            .clone()
    }

    fn complete_requests(&self) -> Vec<RecordedRequest> {
        self.complete_requests
            .lock()
            .expect("complete requests mutex should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl ProviderRouter for RecordingRouter {
    async fn stream(
        &self,
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError>
    {
        self.stream_requests
            .lock()
            .expect("stream requests mutex should not be poisoned")
            .push(RecordedRequest {
                route,
                model_slug: request.model_slug,
                request_model: request.model,
                request_thinking: request.request_thinking,
            });
        Ok(Box::pin(stream::iter(vec![
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
        route: ProviderRoute,
        request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        self.complete_requests
            .lock()
            .expect("complete requests mutex should not be poisoned")
            .push(RecordedRequest {
                route,
                model_slug: request.model_slug,
                request_model: request.model,
                request_thinking: request.request_thinking,
            });
        Ok(model_response("Generated routed title"))
    }

    fn name(&self) -> &str {
        "recording-router"
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

#[tokio::test]
async fn duplicate_slug_session_binding_routes_turn_and_title_to_selected_binding() -> Result<()> {
    let data_root = TempDir::new()?;
    write_duplicate_slug_provider_config(data_root.path())?;
    let router = Arc::new(RecordingRouter::default());
    let runtime = build_runtime_with_models(
        data_root.path(),
        router.clone(),
        "deepseek-v4-flash",
        vec![Model {
            slug: "deepseek-v4-flash".to_string(),
            display_name: "DeepSeek V4 Flash".to_string(),
            ..Model::default()
        }],
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session_id = start_session_with_binding(
        &runtime,
        connection_id,
        data_root.path(),
        "deepseek-v4-flash",
        Some("deepseek-v4-flash-deepseek-ac"),
    )
    .await?;
    start_turn(&runtime, connection_id, session_id).await?;

    wait_for_notification_value(&mut notifications_rx, "turn/completed").await?;
    wait_for_complete_request(&router).await?;

    let expected_route = ProviderRoute::binding("deepseek-ac", ProviderWireApi::AnthropicMessages);
    assert_eq!(
        router.stream_requests(),
        vec![RecordedRequest {
            route: expected_route.clone(),
            model_slug: ModelProfileKey::CatalogSlug("deepseek-v4-flash".to_string()),
            request_model: "deepseek-v4-flash".to_string(),
            request_thinking: Some("enabled".to_string()),
        }]
    );
    assert_eq!(
        router.complete_requests(),
        vec![RecordedRequest {
            route: expected_route,
            model_slug: ModelProfileKey::CatalogSlug("deepseek-v4-flash".to_string()),
            request_model: "deepseek-v4-flash".to_string(),
            request_thinking: Some("disabled".to_string()),
        }]
    );

    Ok(())
}

#[tokio::test]
async fn session_model_switch_routes_turn_and_title_to_selected_provider_binding() -> Result<()> {
    let data_root = TempDir::new()?;
    write_provider_config(data_root.path())?;
    let router = Arc::new(RecordingRouter::default());
    let runtime = build_runtime(data_root.path(), router.clone())?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;
    update_session_model(&runtime, connection_id, session_id, "alt-model").await?;
    start_turn(&runtime, connection_id, session_id).await?;

    wait_for_notification_value(&mut notifications_rx, "turn/completed").await?;
    wait_for_complete_request(&router).await?;

    let expected_route =
        ProviderRoute::binding("alternate", ProviderWireApi::OpenAIChatCompletions);
    assert_eq!(
        router.stream_requests(),
        vec![RecordedRequest {
            route: expected_route.clone(),
            model_slug: ModelProfileKey::CatalogSlug("alt-model".to_string()),
            request_model: "vendor/alt-model".to_string(),
            request_thinking: None,
        }]
    );
    assert_eq!(
        router.complete_requests(),
        vec![RecordedRequest {
            route: expected_route,
            model_slug: ModelProfileKey::CatalogSlug("alt-model".to_string()),
            request_model: "vendor/alt-model".to_string(),
            request_thinking: Some("disabled".to_string()),
        }]
    );

    Ok(())
}

#[tokio::test]
async fn explicit_binding_controls_route_request_model_and_catalog_profile() -> Result<()> {
    let data_root = TempDir::new()?;
    write_glm_provider_config(data_root.path())?;
    let router = Arc::new(RecordingRouter::default());
    let runtime = build_runtime_with_models(
        data_root.path(),
        router.clone(),
        "glm-5.2",
        vec![Model {
            slug: "glm-5.2".to_string(),
            display_name: "GLM 5.2".to_string(),
            reasoning_capability: ReasoningCapability::Toggle,
            ..Model::default()
        }],
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session_with_binding(
        &runtime,
        connection_id,
        data_root.path(),
        "glm-5.2",
        Some("glm-zai"),
    )
    .await?;

    let response = send_turn_start(
        &runtime,
        connection_id,
        session_id,
        4,
        Some("legacy-wrong-model"),
        Some("glm-zai"),
        Some("enabled"),
    )
    .await?
    .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)?;

    wait_for_notification_value(&mut notifications_rx, "turn/completed").await?;
    wait_for_complete_request(&router).await?;

    let expected_route = ProviderRoute::binding("zai", ProviderWireApi::OpenAIChatCompletions);
    assert_eq!(
        router.stream_requests(),
        vec![RecordedRequest {
            route: expected_route.clone(),
            model_slug: ModelProfileKey::CatalogSlug("glm-5.2".to_string()),
            request_model: "renamed-provider-model".to_string(),
            request_thinking: Some("enabled".to_string()),
        }]
    );
    assert_eq!(
        router.complete_requests(),
        vec![RecordedRequest {
            route: expected_route,
            model_slug: ModelProfileKey::CatalogSlug("glm-5.2".to_string()),
            request_model: "renamed-provider-model".to_string(),
            request_thinking: Some("disabled".to_string()),
        }]
    );

    Ok(())
}

#[tokio::test]
async fn turn_start_rejects_invalid_explicit_bindings() -> Result<()> {
    let data_root = TempDir::new()?;
    write_glm_provider_config(data_root.path())?;
    let router = Arc::new(RecordingRouter::default());
    let runtime = build_runtime_with_models(
        data_root.path(),
        router,
        "glm-5.2",
        vec![Model {
            slug: "glm-5.2".to_string(),
            display_name: "GLM 5.2".to_string(),
            ..Model::default()
        }],
    )?;
    let (connection_id, _notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session_with_binding(
        &runtime,
        connection_id,
        data_root.path(),
        "glm-5.2",
        Some("glm-zai"),
    )
    .await?;

    let response = send_turn_start(
        &runtime,
        connection_id,
        session_id,
        4,
        Some("glm-5.2"),
        Some("missing-binding"),
        /*reasoning_effort_selection*/ None,
    )
    .await?
    .context("turn/start response")?;
    let error: devo_server::ErrorResponse = serde_json::from_value(response)?;

    assert_eq!(
        error.error,
        devo_server::ProtocolError {
            code: devo_server::ProtocolErrorCode::InvalidParams,
            message: "model binding `missing-binding` does not exist".to_string(),
            data: serde_json::json!({}),
        }
    );

    let response = send_turn_start(
        &runtime,
        connection_id,
        session_id,
        5,
        /*model*/ None,
        Some("glm-disabled"),
        /*reasoning_effort_selection*/ None,
    )
    .await?
    .context("turn/start response")?;
    let error: devo_server::ErrorResponse = serde_json::from_value(response)?;
    assert_eq!(
        error.error,
        devo_server::ProtocolError {
            code: devo_server::ProtocolErrorCode::InvalidParams,
            message: "model binding `glm-disabled` is disabled".to_string(),
            data: serde_json::json!({}),
        }
    );

    let response = send_turn_start(
        &runtime,
        connection_id,
        session_id,
        6,
        /*model*/ None,
        Some("glm-disabled-provider"),
        /*reasoning_effort_selection*/ None,
    )
    .await?
    .context("turn/start response")?;
    let error: devo_server::ErrorResponse = serde_json::from_value(response)?;
    assert_eq!(
        error.error,
        devo_server::ProtocolError {
            code: devo_server::ProtocolErrorCode::InvalidParams,
            message:
                "model binding `glm-disabled-provider` references disabled provider `disabled-zai`"
                    .to_string(),
            data: serde_json::json!({}),
        }
    );

    Ok(())
}

fn write_duplicate_slug_provider_config(data_root: &std::path::Path) -> Result<()> {
    write_test_auth_config(data_root)?;
    std::fs::write(
        data_root.join("config.toml"),
        r#"
[defaults]
model_binding = "deepseek-v4-flash-deepseek-ac"

[providers.deepseek]
enabled = true
name = "DeepSeek"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[providers.deepseek-ac]
enabled = true
name = "DeepSeek Anthropic"
credential = "test_api_key"
wire_apis = ["anthropic_messages"]

[model_bindings.deepseek-v4-flash-deepseek]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "deepseek"
request_model = "deepseek-v4-flash"
invocation_method = "openai_chat_completions"

[model_bindings.deepseek-v4-flash-deepseek-ac]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "deepseek-ac"
request_model = "deepseek-v4-flash"
invocation_method = "anthropic_messages"
"#,
    )?;
    Ok(())
}

fn write_provider_config(data_root: &std::path::Path) -> Result<()> {
    write_test_auth_config(data_root)?;
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

fn write_glm_provider_config(data_root: &std::path::Path) -> Result<()> {
    write_test_auth_config(data_root)?;
    std::fs::write(
        data_root.join("config.toml"),
        r#"
[defaults]
model_binding = "glm-zai"

[providers.zai]
enabled = true
name = "ZAI"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[providers.disabled-zai]
enabled = false
name = "Disabled ZAI"
credential = "test_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.glm-zai]
enabled = true
model_slug = "glm-5.2"
provider = "zai"
request_model = "renamed-provider-model"
invocation_method = "openai_chat_completions"

[model_bindings.glm-disabled]
enabled = false
model_slug = "glm-5.2"
provider = "zai"
request_model = "disabled-provider-model"
invocation_method = "openai_chat_completions"

[model_bindings.glm-disabled-provider]
enabled = true
model_slug = "glm-5.2"
provider = "disabled-zai"
request_model = "disabled-provider-model"
invocation_method = "openai_chat_completions"
"#,
    )?;
    Ok(())
}

fn write_test_auth_config(data_root: &std::path::Path) -> Result<()> {
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
    Ok(())
}

fn build_runtime(
    data_root: &std::path::Path,
    router: Arc<RecordingRouter>,
) -> Result<Arc<ServerRuntime>> {
    build_runtime_with_models(
        data_root,
        router,
        "default-model",
        vec![
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
        ],
    )
}

fn build_runtime_with_models(
    data_root: &std::path::Path,
    router: Arc<RecordingRouter>,
    default_model: &str,
    models: Vec<Model>,
) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(UnusedProvider);
    let provider_router: Arc<dyn ProviderRouter> = router;
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("provider_routing.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            provider,
            provider_router,
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            default_model.to_string(),
            Arc::new(PresetModelCatalog::new(models)),
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
                        "name": "provider-routing-test",
                        "title": "provider-routing-test",
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
    cwd: &std::path::Path,
) -> Result<SessionId> {
    start_session_with_binding(
        runtime,
        connection_id,
        cwd,
        "default-model",
        /*model_binding_id*/ None,
    )
    .await
}

async fn start_session_with_binding(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &std::path::Path,
    model: &str,
    model_binding_id: Option<&str>,
) -> Result<SessionId> {
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
                    "model": model,
                    "model_binding_id": model_binding_id
                }
            }),
        )
        .await
        .context("session/start response")?;
    let response_value = response.clone();
    let response: devo_server::SuccessResponse<devo_server::SessionStartResult> =
        serde_json::from_value(response)
            .with_context(|| format!("decode session/start response: {response_value}"))?;
    Ok(response.result.session.session_id)
}

async fn update_session_model(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    model: &str,
) -> Result<()> {
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
    let _: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionMetadataUpdateResult,
    > = serde_json::from_value(response)
        .with_context(|| format!("decode session/metadata/update response: {response_value}"))?;
    Ok(())
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = send_turn_start(
        runtime,
        connection_id,
        session_id,
        4,
        /*model*/ None,
        /*model_binding_id*/ None,
        /*reasoning_effort_selection*/ None,
    )
    .await?
    .context("turn/start response")?;
    let response_value = response.clone();
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)
            .with_context(|| format!("decode turn/start response: {response_value}"))?;
    Ok(())
}

async fn send_turn_start(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    id: u64,
    model: Option<&str>,
    model_binding_id: Option<&str>,
    reasoning_effort_selection: Option<&str>,
) -> Result<Option<serde_json::Value>> {
    Ok(runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": id,
                "method": "turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": "use the selected provider" }],
                    "model": model,
                    "model_binding_id": model_binding_id,
                    "reasoning_effort_selection": reasoning_effort_selection,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await)
}

async fn wait_for_notification_value(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<serde_json::Value> {
    let wanted = serde_json::json!(method);
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&wanted) || has_original_method(&value, method) {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before {method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {method}"))?
}

async fn wait_for_complete_request(router: &RecordingRouter) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        loop {
            if !router.complete_requests().is_empty() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("timed out waiting for title completion request")?
}

fn has_original_method(value: &serde_json::Value, method: &str) -> bool {
    value.get("method") == Some(&serde_json::json!("session/update"))
        && value["params"]["_meta"]["devo/originalMethod"].as_str() == Some(method)
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
