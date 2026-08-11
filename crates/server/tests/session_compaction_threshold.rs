//! Global compaction threshold via `session/compaction/update` and config.toml.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AgentsMdConfig;
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
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::SuccessResponse;
use futures::Stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

struct NoopProvider;

#[async_trait]
impl ModelProviderSDK for NoopProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "noop-response".to_string(),
            content: vec![ResponseContent::Text("noop".to_string())],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(futures::stream::empty()))
    }

    fn name(&self) -> &str {
        "noop-compaction-threshold-provider"
    }
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("compaction_threshold.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::load()?),
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.to_path_buf(),
                /*workspace_root*/ None,
            )?)),
        ),
    ))
}

async fn initialize_connection(runtime: &Arc<ServerRuntime>) -> Result<u64> {
    let (notifications_tx, _notifications_rx) = devo_server::test_outbound_channel(128);
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
                        "name": "compaction-threshold-test",
                        "title": "Compaction Threshold Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(
        initialize_response["result"]["agentInfo"]["name"],
        serde_json::json!("devo-server")
    );
    Ok(connection_id)
}

async fn start_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    cwd: &Path,
) -> Result<devo_server::SessionStartResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/start",
                "params": {
                    "cwd": cwd,
                    "ephemeral": true,
                    "title": null
                }
            }),
        )
        .await
        .context("session/start response")?;
    let response: SuccessResponse<devo_server::SessionStartResult> =
        serde_json::from_value(response)?;
    Ok(response.result)
}

async fn compaction_update(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
    effective_context_window: u64,
) -> Result<devo_server::SessionCompactionUpdateResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "session/compaction/update",
                "params": {
                    "sessionId": session_id,
                    "effectiveContextWindow": effective_context_window
                }
            }),
        )
        .await
        .context("session/compaction/update response")?;
    let response: SuccessResponse<devo_server::SessionCompactionUpdateResult> =
        serde_json::from_value(response)?;
    Ok(response.result)
}

#[tokio::test]
async fn compaction_update_writes_global_config_and_applies_to_session() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    let runtime = build_runtime(data_root.path())?;
    let connection_id = initialize_connection(&runtime).await?;
    let started = start_session(&runtime, connection_id, &cwd).await?;
    assert!(
        started.session.effective_context_window.is_some(),
        "new session should expose an applied effective window"
    );
    let model_default = started.session.effective_context_window;

    let updated = compaction_update(
        &runtime,
        connection_id,
        started.session.session_id,
        /*effective_context_window*/ 250_000,
    )
    .await?;
    assert_eq!(updated.effective_context_window, 250_000);
    assert!(updated.context_window_tokens >= 250_000);

    let config_text = std::fs::read_to_string(data_root.path().join("config.toml"))?;
    let document: toml::Value = toml::from_str(&config_text)?;
    assert_eq!(
        document["compaction_token_limit"].as_integer(),
        Some(250_000)
    );

    let second = start_session(&runtime, connection_id, &cwd).await?;
    assert_eq!(
        second.session.effective_context_window,
        Some(250_000),
        "new sessions inherit the global compaction preference"
    );
    assert_ne!(second.session.effective_context_window, model_default);
    Ok(())
}

#[tokio::test]
async fn new_session_reads_existing_global_compaction_limit() -> Result<()> {
    let data_root = TempDir::new()?;
    let cwd = data_root.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    std::fs::write(
        data_root.path().join("config.toml"),
        "compaction_token_limit = 100000\n",
    )?;

    let runtime = build_runtime(data_root.path())?;
    let connection_id = initialize_connection(&runtime).await?;
    let started = start_session(&runtime, connection_id, &cwd).await?;
    assert_eq!(started.session.effective_context_window, Some(100_000));
    Ok(())
}
