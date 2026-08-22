use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

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
use devo_protocol::SessionId;
use devo_protocol::SessionMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_provider::ModelProviderSDK;
use devo_provider::SingleProviderRouter;
use devo_server::AcpDeleteSessionResult;
use devo_server::AcpInitializeResult;
use devo_server::AcpListSessionsResult;
use devo_server::AcpNewSessionResult;
use devo_server::AcpSessionDeleteCapabilities;
use devo_server::AcpSuccessResponse;
use devo_server::ClientTransportKind;
use devo_server::DEVO_SESSION_META;
use devo_server::ServerRuntime;
use devo_server::ServerRuntimeDependencies;
use devo_server::acp_session_info_from_metadata;
use futures::Stream;
use futures::stream;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Duration;
use tokio::time::timeout;

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
        Ok(Box::pin(stream::empty()))
    }

    fn name(&self) -> &str {
        "noop-acp-delete-provider"
    }
}

struct BlockingProvider {
    started_tx: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

impl BlockingProvider {
    fn new(started_tx: oneshot::Sender<()>) -> Self {
        Self {
            started_tx: std::sync::Mutex::new(Some(started_tx)),
        }
    }

    fn mark_started(&self) {
        if let Some(started_tx) = self.started_tx.lock().expect("started mutex").take() {
            let _ = started_tx.send(());
        }
    }
}

#[async_trait]
impl ModelProviderSDK for BlockingProvider {
    async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
        self.mark_started();
        std::future::pending::<Result<ModelResponse>>().await
    }

    async fn completion_stream(
        &self,
        _request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        self.mark_started();
        std::future::pending::<Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>>()
            .await
    }

    fn name(&self) -> &str {
        "blocking-acp-delete-provider"
    }
}

#[tokio::test]
async fn acp_session_delete_removes_session_from_history_and_is_idempotent() -> Result<()> {
    let data_root = TempDir::new()?;
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
    let db = Arc::new(devo_server::db::Database::open(
        data_root.path().join("acp_session_delete.db"),
    )?);
    let runtime = ServerRuntime::new(
        data_root.path().to_path_buf(),
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
            Arc::new(ProviderVendorCatalog::default()),
            Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                bundled: Some(BundledSkillsConfig { enabled: false }),
                ..SkillsConfig::default()
            })),
            devo_core::AgentsMdConfig::default(),
            db,
            Arc::new(std::sync::Mutex::new(AppConfigStore::load(
                data_root.path().to_path_buf(),
                None,
            )?)),
        ),
    );
    let (notifications_tx, _notifications_rx) = devo_server::test_outbound_channel(4096);
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
                        "name": "acp-session-delete-test",
                        "title": "ACP Session Delete Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let initialize: AcpSuccessResponse<AcpInitializeResult> =
        serde_json::from_value(initialize_response)?;
    assert_eq!(
        initialize
            .result
            .agent_capabilities
            .session_capabilities
            .delete,
        Some(AcpSessionDeleteCapabilities::default())
    );

    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let new_response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 2,
                "method": "session/new",
                "params": {
                    "cwd": cwd.to_string_lossy().into_owned(),
                    "mcpServers": []
                }
            }),
        )
        .await
        .context("session/new response")?;
    let new_session: AcpSuccessResponse<AcpNewSessionResult> =
        serde_json::from_value(new_response)?;
    let session_id = new_session.result.session_id;
    let session_metadata: SessionMetadata = serde_json::from_value(
        new_session
            .result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_META))
            .cloned()
            .context("missing Devo session metadata")?,
    )?;

    assert_eq!(
        list_acp_sessions(&runtime, connection_id, 3, &cwd).await?,
        AcpListSessionsResult {
            sessions: vec![acp_session_info_from_metadata(&session_metadata)],
            next_cursor: None,
            meta: None,
        }
    );
    assert_eq!(
        delete_acp_session(&runtime, connection_id, 4, &session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(4), AcpDeleteSessionResult::default())
    );
    assert_eq!(
        list_acp_sessions(&runtime, connection_id, 5, &cwd).await?,
        AcpListSessionsResult {
            sessions: Vec::new(),
            next_cursor: None,
            meta: None,
        }
    );
    assert_eq!(
        delete_acp_session(&runtime, connection_id, 6, &session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(6), AcpDeleteSessionResult::default())
    );
    Ok(())
}

#[tokio::test]
async fn acp_session_delete_cancels_running_session_before_removal() -> Result<()> {
    let data_root = TempDir::new()?;
    let (started_tx, started_rx) = oneshot::channel();
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(BlockingProvider::new(started_tx));
    let runtime = build_runtime_with_provider(data_root.path(), provider)?;
    let (acp_connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let (native_connection_id, _native_notifications_rx) =
        initialize_native_connection(&runtime).await?;
    let session = create_acp_session(&runtime, acp_connection_id, 29, data_root.path()).await?;
    start_turn(&runtime, native_connection_id, 30, session.session_id).await?;

    timeout(Duration::from_secs(5), started_rx)
        .await
        .context("timed out waiting for blocking provider to start")?
        .context("blocking provider start signal dropped")?;
    assert!(session_rollout_exists(
        data_root.path(),
        session.session_id
    )?);

    assert_eq!(
        timeout(
            Duration::from_secs(5),
            delete_acp_session(&runtime, acp_connection_id, 31, &session.session_id)
        )
        .await
        .context("session/delete timed out while turn was running")??,
        AcpSuccessResponse::new(serde_json::json!(31), AcpDeleteSessionResult::default())
    );
    wait_for_original_method(&mut notifications_rx, "turn/interrupted")
        .await
        .context("running delete should interrupt the active turn")?;
    assert_eq!(
        list_acp_sessions(&runtime, acp_connection_id, 32, data_root.path()).await?,
        AcpListSessionsResult {
            sessions: Vec::new(),
            next_cursor: None,
            meta: None,
        }
    );
    assert!(!session_rollout_exists(
        data_root.path(),
        session.session_id
    )?);
    Ok(())
}

#[tokio::test]
async fn acp_session_delete_cascades_to_forked_children() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (acp_connection_id, mut notifications_rx) = initialize_acp_connection(&runtime).await?;
    let (native_connection_id, _native_notifications_rx) =
        initialize_native_connection(&runtime).await?;
    let root = create_acp_session(&runtime, acp_connection_id, 7, data_root.path()).await?;
    start_and_complete_turn(
        &runtime,
        native_connection_id,
        &mut notifications_rx,
        root.session_id,
    )
    .await?;
    let child_session_id = fork_session(&runtime, native_connection_id, root.session_id).await?;

    assert_eq!(
        list_acp_sessions(&runtime, acp_connection_id, 11, data_root.path())
            .await?
            .sessions
            .len(),
        2
    );
    assert!(session_rollout_exists(data_root.path(), root.session_id)?);
    assert!(session_rollout_exists(data_root.path(), child_session_id)?);

    delete_acp_session(&runtime, acp_connection_id, 12, &root.session_id).await?;

    assert_eq!(
        list_acp_sessions(&runtime, acp_connection_id, 13, data_root.path()).await?,
        AcpListSessionsResult {
            sessions: Vec::new(),
            next_cursor: None,
            meta: None,
        }
    );
    assert!(!session_rollout_exists(data_root.path(), root.session_id)?);
    assert!(!session_rollout_exists(data_root.path(), child_session_id)?);
    Ok(())
}

#[tokio::test]
async fn acp_session_delete_broadcasts_deleted_session_ids() -> Result<()> {
    let data_root = TempDir::new()?;
    let runtime = build_runtime(data_root.path())?;
    let (owner_connection_id, _owner_notifications_rx) =
        initialize_acp_connection(&runtime).await?;
    let (observer_connection_id, mut observer_notifications_rx) =
        initialize_native_connection(&runtime).await?;
    let cwd = data_root.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let new_session = create_acp_session(&runtime, owner_connection_id, 21, &cwd).await?;
    subscribe_to_session_events(&runtime, observer_connection_id, 20, new_session.session_id)
        .await?;

    assert_eq!(
        delete_acp_session(&runtime, owner_connection_id, 22, &new_session.session_id).await?,
        AcpSuccessResponse::new(serde_json::json!(22), AcpDeleteSessionResult::default())
    );

    let notification = wait_for_original_method(&mut observer_notifications_rx, "session/deleted")
        .await
        .context("observer should receive session/deleted broadcast")?;
    assert_eq!(
        notification["params"]["deletedSessionIds"],
        serde_json::json!([new_session.session_id])
    );
    Ok(())
}

async fn list_acp_sessions(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: &Path,
) -> Result<AcpListSessionsResult> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/list",
                "params": {
                    "cwd": cwd.to_string_lossy().into_owned()
                }
            }),
        )
        .await
        .context("session/list response")?;
    Ok(serde_json::from_value::<AcpSuccessResponse<AcpListSessionsResult>>(response)?.result)
}

async fn delete_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: &SessionId,
) -> Result<AcpSuccessResponse<AcpDeleteSessionResult>> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "session/delete",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/delete response")?;
    serde_json::from_value(response).context("decode session/delete response")
}

fn build_runtime(data_root: &Path) -> Result<Arc<ServerRuntime>> {
    let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
    build_runtime_with_provider(data_root, provider)
}

fn build_runtime_with_provider(
    data_root: &Path,
    provider: Arc<dyn ModelProviderSDK>,
) -> Result<Arc<ServerRuntime>> {
    let db = Arc::new(devo_server::db::Database::open(
        data_root.join("acp_session_delete.db"),
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
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "acp-session-delete-test",
                        "title": "ACP Session Delete Test",
                        "version": "1.0.0"
                    }
                }
            }),
        )
        .await
        .context("initialize response")?;
    let initialize: AcpSuccessResponse<AcpInitializeResult> = serde_json::from_value(response)?;
    assert_eq!(
        initialize
            .result
            .agent_capabilities
            .session_capabilities
            .delete,
        Some(AcpSessionDeleteCapabilities::default())
    );
    Ok((connection_id, notifications_rx))
}

async fn initialize_native_connection(
    runtime: &Arc<ServerRuntime>,
) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
    let (notifications_tx, notifications_rx) = devo_server::test_outbound_channel(4096);
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
                    "_meta": { "devo": { "protocol": "native" } }
                }
            }),
        )
        .await
        .context("Native initialize response")?;
    anyhow::ensure!(
        response.get("result").is_some(),
        "Native initialize failed: {response}"
    );
    Ok((connection_id, notifications_rx))
}

async fn create_acp_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    cwd: &Path,
) -> Result<SessionMetadata> {
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
    serde_json::from_value(
        response
            .result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_META))
            .cloned()
            .context("missing Devo session metadata")?,
    )
    .context("decode Devo session metadata")
}

async fn start_and_complete_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    session_id: SessionId,
) -> Result<()> {
    start_turn(runtime, connection_id, 8, session_id).await?;
    wait_for_original_method(notifications_rx, "turn/completed")
        .await
        .map(|_| ())
}

async fn start_turn(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "turn/start",
                "params": {
                    "sessionId": session_id,
                    "input": [{ "type": "text", "text": "seed fork history" }],
                    "idempotencyKey": format!("turn-{request_id}")
                }
            }),
        )
        .await
        .context("turn/start response")?;
    let _: devo_server::SuccessResponse<devo_protocol::native::rpc_turn::TurnStartResult> =
        serde_json::from_value(response)?;
    Ok(())
}

async fn fork_session(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    session_id: SessionId,
) -> Result<SessionId> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": 9,
                "method": "session/fork",
                "params": {
                    "sessionId": session_id
                }
            }),
        )
        .await
        .context("session/fork response")?;
    let response: devo_server::SuccessResponse<
        devo_protocol::native::rpc_session::SessionForkResult,
    > = serde_json::from_value(response)?;
    SessionId::try_from(response.result.session.id.as_str())
        .context("canonical fork returned a non-addressable session id")
}

async fn subscribe_to_session_events(
    runtime: &Arc<ServerRuntime>,
    connection_id: u64,
    request_id: u64,
    session_id: SessionId,
) -> Result<()> {
    let response = runtime
        .handle_incoming(
            connection_id,
            serde_json::json!({
                "id": request_id,
                "method": "subscription/create",
                "params": {
                    "selectors": [{
                        "kind": "session",
                        "sessionId": session_id
                    }],
                    "includeSnapshot": false
                }
            }),
        )
        .await
        .context("subscription/create response")?;
    anyhow::ensure!(response["result"]["subscriptionId"].is_string());
    Ok(())
}

async fn wait_for_original_method(
    notifications_rx: &mut mpsc::Receiver<serde_json::Value>,
    method: &str,
) -> Result<serde_json::Value> {
    timeout(Duration::from_secs(5), async {
        while let Some(value) = notifications_rx.recv().await {
            if value.get("method") == Some(&serde_json::json!(method)) {
                return Ok(value);
            }
            if value.get("method") == Some(&serde_json::json!("session/update"))
                && value["params"]["_meta"]["devo/originalMethod"].as_str() == Some(method)
            {
                return Ok(value);
            }
        }
        anyhow::bail!("notification channel closed before {method}")
    })
    .await
    .with_context(|| format!("timed out waiting for {method}"))?
}

fn session_rollout_exists(data_root: &Path, session_id: SessionId) -> Result<bool> {
    fn visit(path: &Path, session_id: SessionId) -> Result<bool> {
        if !path.exists() {
            return Ok(false);
        }
        for entry in std::fs::read_dir(path)
            .with_context(|| format!("read rollout directory {}", path.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                if visit(&path, session_id)? {
                    return Ok(true);
                }
                continue;
            }
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(&format!("-{session_id}.jsonl")))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    visit(&data_root.join("sessions"), session_id)
}
