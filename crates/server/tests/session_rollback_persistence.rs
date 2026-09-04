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

    let preview_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 5,
                "method": "session/rollback/preview",
                "params": {
                    "sessionId": session_id,
                    "userTurnIndex": 1,
                    "mode": "beforeUserTurn"
                }
            }),
        )
        .await
        .context("session/rollback/preview response")?;
    let plan = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::native::rpc_session::RestorePlan>,
    >(preview_response)?
    .result;
    let commit_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 6,
                "method": "session/rollback/commit",
                "params": {
                    "restorePlanId": plan.restore_plan_id,
                    "expectedWorkspaceVersion": plan.workspace_version
                }
            }),
        )
        .await
        .context("session/rollback/commit response")?;
    let commit = serde_json::from_value::<
        devo_server::SuccessResponse<
            devo_protocol::native::rpc_session::SessionRollbackCommitResult,
        >,
    >(commit_response)?
    .result;
    assert_eq!(commit.restored_turn_count, 1);

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
                "method": "session/resume",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/resume response")?;
    let resumed = serde_json::from_value::<
        devo_server::SuccessResponse<devo_protocol::native::rpc_session::SessionResumeResult>,
    >(resume_response)?
    .result;
    let items_response = rebuilt_runtime
        .handle_incoming(
            rebuilt_connection_id,
            serde_json::json!({
                "id": 7,
                "method": "session/items/list",
                "params": { "sessionId": session_id }
            }),
        )
        .await
        .context("session/items/list response")?;
    let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
        serde_json::from_value(items_response["result"].clone())?;
    let visible_bodies = items
        .data
        .iter()
        .filter_map(|item| {
            let value = serde_json::to_value(&item.item).ok()?;
            match value["type"].as_str() {
                Some("userMessage") => value["content"][0]["text"]
                    .as_str()
                    .map(ToString::to_string),
                Some("assistantMessage") => value["text"].as_str().map(ToString::to_string),
                _ => None,
            }
        })
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
    assert_eq!(resumed.session.id.as_str(), session_id.to_string());
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
                "method": "session/new",
                "params": {
                    "cwd": cwd,
                    "idempotencyKey": "rollback-source-session"
                }
            }),
        )
        .await
        .context("session/new response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionNewResult,
    > = serde_json::from_value(response)?;
    Ok(SessionId::try_from(response.result.session.id.as_str())?)
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
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": text }],
                    "idempotencyKey": text
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
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
