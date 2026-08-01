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
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::ProviderRouter;
use devo_provider::SingleProviderRouter;
use devo_provider::error::ProviderError;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

#[derive(Default)]
struct TestProvider;

#[async_trait]
impl ModelProviderSDK for TestProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(model_response("Raw provider title"))
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: "turn complete".to_string(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response("turn complete"),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "test-provider"
    }
}

#[derive(Default)]
struct RecordingRouter {
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingRouter {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .expect("router requests mutex should not be poisoned")
            .clone()
    }
}

#[async_trait]
impl ProviderRouter for RecordingRouter {
    async fn stream(
        &self,
        _route: devo_provider::ProviderRoute,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![Ok(StreamEvent::MessageDone {
            response: model_response("router stream complete"),
        })])))
    }

    async fn complete(
        &self,
        _route: devo_provider::ProviderRoute,
        request: ModelRequest,
    ) -> Result<ModelResponse, ProviderError> {
        self.requests
            .lock()
            .expect("router requests mutex should not be poisoned")
            .push(request);
        Ok(model_response("Generated title from router"))
    }

    fn name(&self) -> &str {
        "recording-router"
    }
}

#[tokio::test]
async fn restore_seeds_sqlite_metadata_before_initial_stats() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(TestProvider);
    let source_runtime = build_runtime(
        data_root.path(),
        "source.db",
        Arc::clone(&provider),
        Arc::new(SingleProviderRouter::new(Arc::clone(&provider))),
    )?
    .0;
    let (connection_id, _notifications_rx) = initialize_connection(&source_runtime).await?;

    let start_response = source_runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "additionalDirectories": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::AcpNewSessionResult>,
    >(start_response.clone())
    .with_context(|| format!("session/new returned {start_response}"))?
    .result
    .session_id;

    let (restored_runtime, restored_db) = build_runtime(
        data_root.path(),
        "restored.db",
        Arc::clone(&provider),
        Arc::new(SingleProviderRouter::new(Arc::clone(&provider))),
    )?;
    restored_runtime.load_persisted_sessions().await?;

    let restored_session = restored_db
        .get_session(&session_id)?
        .context("restored session metadata should be seeded")?;
    assert_eq!(restored_session.session_id, session_id);

    let restored_stats = restored_db
        .get_stats(&session_id)?
        .context("restored session stats should be seeded")?;
    assert_eq!(restored_stats.total_input_tokens, 0);
    assert_eq!(restored_stats.total_output_tokens, 0);

    Ok(())
}

#[tokio::test]
async fn title_generation_uses_resolved_provider_request_model() -> Result<()> {
    let data_root = TempDir::new()?;
    std::fs::create_dir_all(data_root.path().join(".devo"))?;
    std::fs::write(
        data_root.path().join(".devo").join("config.toml"),
        r#"
[model.kimi-k3]
display_name = "Catalog Title Model"
provider = "openai_chat_completions"
reasoning_capability = "toggle"
reasoning_implementation = { model_variant = { variants = [
  { selection_value = "disabled", model_slug = "kimi-k3", label = "Off", description = "Disable reasoning effort" },
  { selection_value = "enabled", model_slug = "vendor/title-model", reasoning_effort = "medium", label = "On", description = "Enable reasoning effort" },
] } }
base_instructions = "Test title model"
"#,
    )?;

    let provider: Arc<dyn ModelProviderSDK> = Arc::new(TestProvider);
    let recording_router = Arc::new(RecordingRouter::default());
    let (runtime, _db) = build_runtime(
        data_root.path(),
        "title.db",
        provider,
        Arc::clone(&recording_router) as Arc<dyn ProviderRouter>,
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;

    let start_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": data_root.path(),
                    "additionalDirectories": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let session_id = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::AcpNewSessionResult>,
    >(start_response.clone())
    .with_context(|| format!("session/new returned {start_response}"))?
    .result
    .session_id;

    let prompt_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": "fix title generation routing for CLI logs" }]
                }
            }),
        )
        .await;
    assert_eq!(prompt_response, None);

    wait_for_title_update(&mut notifications_rx, "Generated title from router").await?;

    let requests = recording_router.requests();
    assert_eq!(requests.len(), 1);
    let title_request = requests.into_iter().next().expect("one title request");
    assert_eq!(title_request.model, "vendor/title-model");
    assert_eq!(title_request.request_thinking.as_deref(), Some("disabled"));
    assert!(title_request.tools.is_none());
    assert_eq!(title_request.reasoning_effort, None);
    assert_eq!(title_request.max_tokens, 1024);

    Ok(())
}

fn build_runtime(
    data_root: &std::path::Path,
    db_name: &str,
    provider: Arc<dyn ModelProviderSDK>,
    provider_router: Arc<dyn ProviderRouter>,
) -> Result<(
    Arc<devo_server::ServerRuntime>,
    Arc<devo_server::db::Database>,
)> {
    let db = Arc::new(devo_server::db::Database::open(data_root.join(db_name))?);
    let runtime = devo_server::ServerRuntime::new(
        data_root.to_path_buf(),
        devo_server::ServerRuntimeDependencies::new(
            provider,
            provider_router,
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::default()),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            Arc::clone(&db),
            Arc::new(Mutex::new(AppConfigStore::load(
                data_root.to_path_buf(),
                None,
            )?)),
        ),
    );
    Ok((runtime, db))
}

async fn initialize_connection(
    runtime: &Arc<devo_server::ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(devo_server::ClientTransportKind::Stdio, notifications_tx)
        .await;
    let initialize_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 10,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "test",
                        "title": "test",
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

async fn wait_for_title_update(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    expected_title: &str,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("session/title/updated"))
                && value["params"]["session"]["title"] == serde_json::json!(expected_title)
            {
                return Ok(());
            }
            if value.get("method") == Some(&serde_json::json!("session/update"))
                && value["params"]["update"]["sessionUpdate"]
                    == serde_json::json!("session_info_update")
                && value["params"]["update"]["title"] == serde_json::json!(expected_title)
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before expected title update")
    })
    .await
    .context("timed out waiting for title update")??;
    Ok(())
}

fn model_response(text: &str) -> ModelResponse {
    ModelResponse {
        id: "test-response".to_string(),
        content: vec![ResponseContent::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        usage: Usage::default(),
        metadata: ResponseMetadata::default(),
    }
}
