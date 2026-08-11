use std::collections::VecDeque;
use std::path::Path;
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
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseMetadata;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionId;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::ClientTransportKind;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;

struct ScriptedReplyProvider {
    replies: Mutex<VecDeque<String>>,
}

impl ScriptedReplyProvider {
    fn new(replies: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().map(str::to_string).collect()),
        }
    }
}

#[async_trait]
impl ModelProviderSDK for ScriptedReplyProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Ok(model_response("Generated title"))
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let text = self
            .replies
            .lock()
            .expect("lock scripted replies")
            .pop_front()
            .context("scripted reply provider exhausted")?;
        Ok(Box::pin(stream::iter(vec![
            Ok(StreamEvent::TextDelta {
                index: 0,
                text: text.clone(),
            }),
            Ok(StreamEvent::MessageDone {
                response: model_response(&text),
            }),
        ])))
    }

    fn name(&self) -> &str {
        "scripted-reply-provider"
    }
}

#[tokio::test]
async fn session_rollback_persists_cut_and_keeps_future_turns_durable() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(
        data_root.path(),
        Arc::new(ScriptedReplyProvider::new([
            "first assistant",
            "second assistant",
            "third assistant",
        ])),
    )?;
    let (connection_id, mut notifications_rx) = initialize_connection(&runtime).await?;
    let session_id = start_session(&runtime, connection_id, data_root.path()).await?;

    start_and_complete_turn(
        &runtime,
        connection_id,
        &mut notifications_rx,
        session_id,
        "first prompt",
    )
    .await?;
    start_and_complete_turn(
        &runtime,
        connection_id,
        &mut notifications_rx,
        session_id,
        "second prompt",
    )
    .await?;

    let rollback_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "_devo/session/rollback",
                "params": {
                    "session_id": session_id,
                    "user_turn_index": 1,
                    "mode": "before_user_turn"
                }
            }),
        )
        .await
        .context("session/rollback response")?;
    let rollback = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionRollbackResult>,
    >(rollback_response)?
    .result;
    assert_eq!(
        rollback.latest_turn.as_ref().map(|turn| turn.sequence),
        Some(1)
    );

    start_and_complete_turn(
        &runtime,
        connection_id,
        &mut notifications_rx,
        session_id,
        "third prompt",
    )
    .await?;

    let rebuilt_runtime = build_runtime(
        data_root.path(),
        Arc::new(ScriptedReplyProvider::new(
            std::iter::empty::<&'static str>(),
        )),
    )?;
    rebuilt_runtime.load_persisted_sessions().await?;
    let (rebuilt_connection_id, _notifications_rx) =
        initialize_connection(&rebuilt_runtime).await?;
    let resume_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 6,
                "method": "_devo/session/resume",
                "params": {
                    "session_id": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resumed = serde_json::from_value::<
        devo_server::SuccessResponse<devo_server::SessionResumeResult>,
    >(resume_response)?
    .result;
    let visible_bodies = resumed
        .history_items
        .iter()
        .filter(|item| {
            matches!(
                item.kind,
                SessionHistoryItemKind::User | SessionHistoryItemKind::Assistant
            )
        })
        .map(|item| item.body.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        visible_bodies,
        vec![
            "first prompt",
            "first assistant",
            "third prompt",
            "third assistant"
        ]
    );
    assert_eq!(
        resumed.latest_turn.as_ref().map(|turn| turn.sequence),
        Some(2)
    );
    Ok(())
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
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("session_rollback_persistence.db"),
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
                    "clientInfo": {
                        "name": "session-rollback-persistence-test",
                        "title": "session-rollback-persistence-test",
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
                    "title": "Rollback source",
                    "model": "test-model",
                    "model_binding_id": null
                }
            }),
        )
        .await
        .context("session/start response")?;
    let response: devo_server::SuccessResponse<devo_server::SessionStartResult> =
        serde_json::from_value(response)?;
    Ok(response.result.session.session_id)
}

async fn start_and_complete_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
    text: &str,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 3,
                "method": "_devo/turn/start",
                "params": {
                    "session_id": session_id,
                    "input": [{ "type": "text", "text": text }],
                    "model": null,
                    "model_binding_id": null,
                    "thinking": null,
                    "sandbox": null,
                    "approval_policy": null,
                    "cwd": null
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_server::TurnStartResult> =
        serde_json::from_value(response)?;
    wait_for_turn_completed(notifications_rx).await
}

async fn wait_for_turn_completed(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!("turn/completed"))
                || has_original_method(&value, "turn/completed")
            {
                return Ok(());
            }
        }
        anyhow::bail!("notification channel closed before turn/completed")
    })
    .await
    .context("timed out waiting for turn/completed")??;
    Ok(())
}

fn has_original_method(value: &serde_json::Value, method: &str) -> bool {
    value.get("method") == Some(&serde_json::json!("session/update"))
        && value["params"]["_meta"]["devo/originalMethod"].as_str() == Some(method)
}
