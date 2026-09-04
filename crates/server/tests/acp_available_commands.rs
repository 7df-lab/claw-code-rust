use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_core::AppConfigStore;
use devo_core::BundledSkillsConfig;
use devo_core::FileSystemSkillCatalog;
use devo_core::PresetModelCatalog;
use devo_core::SkillsConfig;
use devo_core::tools::ToolRegistry;
use devo_protocol::AcpNewSessionResult;
use devo_protocol::Model;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::OutboundFrame;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

struct NoopProvider;

#[async_trait]
impl ModelProviderSDK for NoopProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(ModelResponse {
            id: "noop".to_string(),
            content: Vec::new(),
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage::default(),
            metadata: ResponseMetadata::default(),
        })
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>> {
        Ok(Box::pin(stream::empty()))
    }

    fn name(&self) -> &str {
        "noop-provider"
    }
}

#[tokio::test]
async fn acp_available_commands_are_session_update_after_session_response() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (outgoing_tx, mut outgoing_rx) = devo_server::test_outbound_channel(4096);
    let connection_id = runtime
        .register_connection(ClientTransportKind::Stdio, outgoing_tx.clone())
        .await;
    initialize_connection(&runtime, connection_id).await?;

    let cwd = data_root.path().join("workspace");
    std::fs::create_dir_all(&cwd)?;
    let incoming_response = runtime
        .handle_incoming_with_actions(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": path_value(&cwd),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let (response_value, post_response_actions) = incoming_response.into_parts();
    let response: AcpSuccessResponse<AcpNewSessionResult> =
        serde_json::from_value(response_value.clone())?;
    let session_id = response.result.session_id;
    outgoing_tx
        .send(OutboundFrame::json_rpc_response(
            connection_id,
            response_value,
        ))
        .await
        .context("enqueue simulated transport response")?;
    runtime
        .run_post_response_actions(post_response_actions)
        .await;

    let messages_before_response = recv_until_response(&mut outgoing_rx, 2, session_id).await?;
    assert!(
        !messages_before_response
            .iter()
            .any(|message| is_available_commands_update(message, session_id)),
        "available_commands_update arrived before response: {messages_before_response:?}"
    );

    let available_commands = recv_available_commands_update(&mut outgoing_rx, session_id).await?;
    assert_available_command_names(&available_commands)?;
    Ok(())
}

async fn initialize_connection(runtime: &Arc<ServerRuntime>, connection_id: u64) -> Result<()> {
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
                        "name": "acp-available-commands-order-test",
                        "title": "ACP Available Commands Order Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    assert_eq!(initialize_response["id"], serde_json::json!(1));
    Ok(())
}

async fn recv_until_response(
    outgoing_rx: &mut mpsc::Receiver<serde_json::Value>,
    request_id: u64,
    session_id: SessionId,
) -> Result<Vec<serde_json::Value>> {
    timeout(Duration::from_secs(5), async {
        let mut messages = Vec::new();
        loop {
            let message = outgoing_rx
                .recv()
                .await
                .context("outgoing channel closed before response")?;
            if message.get("id") == Some(&serde_json::json!(request_id)) {
                return Ok(messages);
            }
            if message["params"]["sessionId"].as_str() == Some(&session_id.to_string()) {
                messages.push(message);
            }
        }
    })
    .await
    .context("timed out waiting for response")?
}

async fn recv_available_commands_update(
    outgoing_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(5), async {
        loop {
            let message = outgoing_rx
                .recv()
                .await
                .context("outgoing channel closed before available commands update")?;
            if is_available_commands_update(&message, session_id) {
                return Ok(message);
            }
        }
    })
    .await
    .context("timed out waiting for available commands update")?
}

fn is_available_commands_update(message: &serde_json::Value, session_id: SessionId) -> bool {
    message["method"] == serde_json::json!("session/update")
        && message["params"]["sessionId"].as_str() == Some(&session_id.to_string())
        && message["params"]["update"]["sessionUpdate"].as_str()
            == Some("available_commands_update")
}

fn assert_available_command_names(message: &serde_json::Value) -> Result<()> {
    let commands = message["params"]["update"]["availableCommands"]
        .as_array()
        .context("availableCommands must be an array")?;
    let names = commands
        .iter()
        .filter_map(|command| command["name"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["compact", "goal"]);
    Ok(())
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("acp_available_commands.db"),
    )?);
    Ok(ServerRuntime::new(
        data_root.to_path_buf(),
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(SingleProviderRouter::new(provider)),
            Arc::new(ToolRegistry::new()),
            devo_server::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(PresetModelCatalog::new(vec![Model {
                slug: "test-model".to_string(),
                display_name: "test-model".to_string(),
                ..Model::default()
            }])),
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

fn path_value(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
