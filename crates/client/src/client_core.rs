//! Transport-agnostic Devo server client: JSON-RPC request/response routing,
//! server-initiated ACP client handlers, and notification demultiplexing.
//!
//! Both [`crate::stdio::StdioServerClient`] and [`crate::websocket::WebSocketServerClient`]
//! delegate protocol logic here. Incoming messages are classified as:
//!
//! - **Server → client requests** (`id` + `method`): handled asynchronously; the
//!   response echoes the same JSON-RPC `id` (see `fs/read`, permissions, terminal).
//! - **Server responses** (`id` + `result`/`error`, no `method`): matched against
//!   [`PendingResponses`] via numeric `id` to complete a client-initiated `request`.
//! - **Notifications** (no `id`): forwarded on the notification channel.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use chrono::Utc;
use devo_protocol::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::acp_fs::handle_acp_fs_request;
use crate::acp_permissions::AcpPendingPermissions;
use crate::acp_permissions::handle_acp_request_permission;
use crate::acp_permissions::resolve_acp_permission_response;
use crate::acp_terminal::AcpTerminalManager;
use crate::acp_terminal::handle_acp_terminal_request;

pub const ACP_PROMPT_STARTED_NOTIFICATION_METHOD: &str = "_devo/acp_prompt/started";
pub const ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD: &str = "_devo/acp_prompt/completed";

const SERVER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Synthetic notifications emitted when falling back to detached `session/prompt`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerNotificationMessage {
    pub method: String,
    pub params: serde_json::Value,
}

/// Client-initiated requests awaiting a server response, keyed by JSON-RPC `id`.
pub(crate) type PendingResponses = Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>;

pub(crate) enum ClientWriteMessage {
    Json(serde_json::Value),
    Close,
}

#[derive(Clone)]
pub(crate) struct ClientWriter {
    tx: mpsc::UnboundedSender<ClientWriteMessage>,
}

impl ClientWriter {
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<ClientWriteMessage>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    fn send_value(&self, value: serde_json::Value) -> Result<()> {
        self.tx
            .send(ClientWriteMessage::Json(value))
            .map_err(|_| anyhow!("client writer is closed"))
    }

    fn send_serializable<T: Serialize>(&self, value: &T) -> Result<()> {
        let value = serde_json::to_value(value).context("serialize client payload")?;
        self.send_value(value)
    }

    pub(crate) fn close(&self) {
        let _ = self.tx.send(ClientWriteMessage::Close);
    }
}

#[derive(Clone)]
pub(crate) struct ServerClientReaderState {
    writer: ClientWriter,
    pending: PendingResponses,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

pub(crate) struct ServerClientCore {
    writer: ClientWriter,
    pending: PendingResponses,
    acp_pending_permissions: AcpPendingPermissions,
    acp_terminals: AcpTerminalManager,
    acp_agent_capabilities: Option<AcpAgentCapabilities>,
    client_capabilities: AcpClientCapabilities,
    next_request_id: AtomicU64,
    notifications_rx: mpsc::UnboundedReceiver<ServerNotificationMessage>,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

impl ServerClientCore {
    pub(crate) fn new(writer: ClientWriter, client_capabilities: AcpClientCapabilities) -> Self {
        let (notifications_tx, notifications_rx) = mpsc::unbounded_channel();
        Self {
            writer,
            pending: Arc::new(Mutex::new(HashMap::new())),
            acp_pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            acp_terminals: AcpTerminalManager::new(),
            acp_agent_capabilities: None,
            client_capabilities,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        }
    }

    pub(crate) fn reader_state(&self) -> ServerClientReaderState {
        ServerClientReaderState {
            writer: self.writer.clone(),
            pending: Arc::clone(&self.pending),
            acp_pending_permissions: Arc::clone(&self.acp_pending_permissions),
            acp_terminals: self.acp_terminals.clone(),
            notifications_tx: self.notifications_tx.clone(),
        }
    }

    pub(crate) fn set_client_capabilities(&mut self, client_capabilities: AcpClientCapabilities) {
        self.client_capabilities = client_capabilities;
    }

    #[cfg(test)]
    pub(crate) fn pending_responses(&self) -> PendingResponses {
        Arc::clone(&self.pending)
    }

    #[cfg(test)]
    pub(crate) fn set_agent_capabilities_for_test(&mut self, capabilities: AcpAgentCapabilities) {
        self.acp_agent_capabilities = Some(capabilities);
    }

    pub(crate) async fn initialize(&mut self) -> Result<InitializeResult> {
        let result: AcpInitializeResult = timeout(
            SERVER_RESPONSE_TIMEOUT,
            self.request(
                ACP_INITIALIZE_METHOD,
                AcpInitializeParams {
                    protocol_version: 1,
                    client_capabilities: self.client_capabilities.clone(),
                    client_info: Some(
                        AcpImplementation::new("devo", env!("CARGO_PKG_VERSION"))
                            .with_title("Devo"),
                    ),
                    meta: None,
                },
            ),
        )
        .await
        .context("timed out waiting for initialize response from server")??;
        self.acp_agent_capabilities = Some(result.agent_capabilities.clone());
        let meta = result.meta.as_ref();
        Ok(InitializeResult {
            server_name: result
                .agent_info
                .as_ref()
                .map(|info| info.name.clone())
                .unwrap_or_else(|| "devo-server".to_string()),
            server_version: result
                .agent_info
                .as_ref()
                .map(|info| info.version.clone())
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            platform_family: meta
                .and_then(|meta| meta.get("devo/platformFamily"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(std::env::consts::FAMILY)
                .into(),
            platform_os: meta
                .and_then(|meta| meta.get("devo/platformOs"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(std::env::consts::OS)
                .into(),
            server_home: meta
                .and_then(|meta| meta.get("devo/serverHome"))
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_default(),
        })
    }

    pub(crate) async fn acp_terminal_output_snapshot(
        &self,
        terminal_id: &str,
    ) -> Result<AcpTerminalOutputResult> {
        self.acp_terminals
            .output(terminal_id)
            .await
            .map_err(anyhow::Error::msg)
    }

    pub(crate) async fn session_start(
        &mut self,
        params: SessionStartParams,
    ) -> Result<SessionStartResult> {
        let result: AcpNewSessionResult = self
            .request(
                ACP_SESSION_NEW_METHOD,
                AcpNewSessionParams {
                    cwd: params.cwd.clone(),
                    additional_directories: params.additional_directories.clone(),
                    mcp_servers: Vec::new(),
                    meta: None,
                },
            )
            .await?;
        let session = result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_META))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .context("decode session metadata from ACP session/new response")?
            .unwrap_or_else(|| acp_session_metadata_from_start_params(&params, result.session_id));
        Ok(SessionStartResult { session })
    }

    pub(crate) async fn session_resume(
        &mut self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult> {
        let sessions = self.session_list().await?;
        let session = sessions
            .into_iter()
            .find(|session| session.session_id == params.session_id)
            .with_context(|| {
                format!(
                    "session {} not found for ACP session/resume",
                    params.session_id
                )
            })?;
        let result: AcpResumeSessionResult = self
            .request(
                ACP_SESSION_RESUME_METHOD,
                AcpResumeSessionParams {
                    session_id: params.session_id,
                    cwd: session.cwd.clone(),
                    additional_directories: session.additional_directories.clone(),
                    mcp_servers: Vec::new(),
                    meta: None,
                },
            )
            .await?;
        Ok(result
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEVO_SESSION_RESUME_META))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .context("decode session resume metadata from ACP session/resume response")?
            .unwrap_or_else(|| SessionResumeResult {
                session,
                latest_turn: None,
                loaded_item_count: 0,
                history_items: Vec::new(),
                pending_texts: Vec::new(),
            }))
    }

    pub(crate) async fn session_list(&mut self) -> Result<Vec<SessionMetadata>> {
        let Some(capabilities) = self.acp_agent_capabilities.as_ref() else {
            bail!("ACP initialize must complete before session/list");
        };
        if capabilities.session_capabilities.list.is_none() {
            bail!("ACP agent does not advertise sessionCapabilities.list");
        }

        let mut cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut sessions = Vec::new();
        loop {
            let result: AcpListSessionsResult = self
                .request(
                    ACP_SESSION_LIST_METHOD,
                    AcpListSessionsParams {
                        cwd: None,
                        cursor,
                        meta: None,
                    },
                )
                .await?;
            for session_info in result.sessions {
                let session = session_info
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get(DEVO_SESSION_META))
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .context("decode session metadata from ACP session/list response")?
                    .unwrap_or_else(|| acp_session_metadata_from_session_info(&session_info));
                sessions.push(session);
            }
            let Some(next_cursor) = result.next_cursor else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                bail!("ACP session/list returned a repeated nextCursor");
            }
            cursor = Some(next_cursor);
        }
        Ok(sessions)
    }

    pub(crate) async fn request_devo<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let method = devo_extension_method(method);
        self.request(&method, params).await
    }

    pub(crate) async fn request<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (response_tx, response_rx) = oneshot::channel();
        // The transport reader resolves responses by this id (see
        // `deliver_pending_client_response`).
        self.pending.lock().await.insert(request_id, response_tx);
        let request = AcpClientRequest::new(serde_json::json!(request_id), method, params);
        if let Err(error) = self.writer.send_serializable(&request) {
            self.pending.lock().await.remove(&request_id);
            return Err(error);
        }

        let response = match timeout(SERVER_RESPONSE_TIMEOUT, response_rx).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                self.pending.lock().await.remove(&request_id);
                return Err(error)
                    .with_context(|| format!("server dropped response for request {request_id}"));
            }
            Err(error) => {
                self.pending.lock().await.remove(&request_id);
                return Err(error).with_context(|| {
                    format!("timed out waiting for server response to request {request_id}")
                });
            }
        };
        if response.get("error").is_some() {
            bail_server_error(&response)?;
        }
        let success: AcpSuccessResponse<R> =
            serde_json::from_value(response).context("decode success response from server")?;
        Ok(success.result)
    }

    pub(crate) async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        match self
            .request_devo::<_, TurnStartResult>("turn/start", params.clone())
            .await
        {
            Ok(result) => Ok(result),
            Err(error) if is_method_not_found_error(&error) => {
                self.turn_start_acp_prompt_detached(params).await?;
                Ok(TurnStartResult::Started {
                    turn_id: TurnId::new(),
                    status: TurnStatus::Running,
                    accepted_at: Utc::now(),
                })
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn approval_respond(&mut self, params: ApprovalResponseParams) -> Result<()> {
        if let Some((response, notification)) =
            resolve_acp_permission_response(&self.acp_pending_permissions, &params).await
        {
            self.writer.send_value(response)?;
            let _ = self.notifications_tx.send(notification);
            return Ok(());
        }
        bail!("no pending ACP permission request exists for approval response")
    }

    pub(crate) async fn request_user_input_respond(
        &mut self,
        params: RequestUserInputRespondParams,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .request_devo("request_user_input/respond", params)
            .await?;
        Ok(())
    }

    pub(crate) async fn recv_notification(&mut self) -> Option<ServerNotificationMessage> {
        self.notifications_rx.recv().await
    }

    pub(crate) async fn recv_client_event(&mut self) -> Result<Option<crate::ClientEvent>> {
        let Some(notification) = self.recv_notification().await else {
            return Ok(None);
        };
        crate::events::client_event_from_notification(&notification)
    }

    pub(crate) async fn recv_event(&mut self) -> Result<Option<(String, ServerEvent)>> {
        let Some(notification) = self.recv_notification().await else {
            return Ok(None);
        };
        let ServerNotificationMessage { method, params } = notification;
        let event = serde_json::from_value(params)
            .with_context(|| format!("failed to decode server event for method {method}"))?;
        Ok(Some((method, event)))
    }

    pub(crate) async fn shutdown(&self) {
        self.writer.close();
        self.acp_terminals.release_all().await;
    }

    pub(crate) async fn agent_list(&mut self, params: AgentListParams) -> Result<AgentListResult> {
        self.request_devo("agent/list", params).await
    }

    pub(crate) async fn agent_spawn(
        &mut self,
        params: SpawnAgentParams,
    ) -> Result<SpawnAgentResult> {
        self.request_devo("agent/spawn", params).await
    }

    pub(crate) async fn agent_close(
        &mut self,
        params: CloseAgentParams,
    ) -> Result<CloseAgentResult> {
        self.request_devo("agent/close", params).await
    }

    pub(crate) async fn session_title_update(
        &mut self,
        params: SessionTitleUpdateParams,
    ) -> Result<SessionTitleUpdateResult> {
        self.request_devo("session/title/update", params).await
    }

    pub(crate) async fn session_delete(
        &mut self,
        params: AcpDeleteSessionParams,
    ) -> Result<AcpDeleteSessionResult> {
        self.request(ACP_SESSION_DELETE_METHOD, params).await
    }

    pub(crate) async fn session_metadata_update(
        &mut self,
        params: SessionMetadataUpdateParams,
    ) -> Result<SessionMetadataUpdateResult> {
        self.request_devo("session/metadata/update", params).await
    }

    pub(crate) async fn session_permissions_update(
        &mut self,
        params: SessionPermissionsUpdateParams,
    ) -> Result<SessionPermissionsUpdateResult> {
        self.request_devo("session/permissions/update", params)
            .await
    }

    pub(crate) async fn session_sandbox_profile_update(
        &mut self,
        params: SessionSandboxProfileUpdateParams,
    ) -> Result<SessionSandboxProfileUpdateResult> {
        self.request_devo("session/sandbox_profile/update", params)
            .await
    }

    pub(crate) async fn session_compact(
        &mut self,
        params: SessionCompactParams,
    ) -> Result<SessionCompactResult> {
        self.request_devo("session/compact", params).await
    }

    pub(crate) async fn goal_create(
        &mut self,
        params: GoalCreateParams,
    ) -> Result<GoalCreateResult> {
        self.request_devo("goal/create", params).await
    }

    pub(crate) async fn goal_set(&mut self, params: GoalSetParams) -> Result<GoalSetResult> {
        self.request_devo("goal/set", params).await
    }

    pub(crate) async fn goal_status(
        &mut self,
        params: GoalStatusParams,
    ) -> Result<GoalStatusResult> {
        self.request_devo("goal/status", params).await
    }

    pub(crate) async fn goal_pause(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request_devo("goal/pause", params).await
    }

    pub(crate) async fn goal_resume(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request_devo("goal/resume", params).await
    }

    pub(crate) async fn goal_complete(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.request_devo("goal/complete", params).await
    }

    pub(crate) async fn goal_clear(&mut self, params: GoalClearParams) -> Result<GoalClearResult> {
        self.request_devo("goal/clear", params).await
    }

    pub(crate) async fn session_fork(
        &mut self,
        params: SessionForkParams,
    ) -> Result<SessionForkResult> {
        self.request_devo("session/fork", params).await
    }

    pub(crate) async fn session_rollback(
        &mut self,
        params: SessionRollbackParams,
    ) -> Result<SessionRollbackResult> {
        self.request_devo("session/rollback", params).await
    }

    pub(crate) async fn skills_list(&mut self, params: SkillListParams) -> Result<SkillListResult> {
        self.request_devo("skills/list", params).await
    }

    pub(crate) async fn skills_changed(
        &mut self,
        params: SkillChangedParams,
    ) -> Result<SkillChangedResult> {
        self.request_devo("skills/changed", params).await
    }

    pub(crate) async fn skills_set_enabled(
        &mut self,
        params: SkillSetEnabledParams,
    ) -> Result<SkillSetEnabledResult> {
        self.request_devo("skills/set_enabled", params).await
    }

    pub(crate) async fn model_catalog(
        &mut self,
        params: ModelCatalogParams,
    ) -> Result<ModelCatalogResult> {
        self.request_devo("model/catalog", params).await
    }

    pub(crate) async fn model_saved(
        &mut self,
        params: ModelSavedParams,
    ) -> Result<ModelSavedResult> {
        self.request_devo("model/saved", params).await
    }

    pub(crate) async fn provider_vendor_list(
        &mut self,
        params: ProviderVendorListParams,
    ) -> Result<ProviderVendorListResult> {
        self.request_devo("provider/list", params).await
    }

    pub(crate) async fn provider_vendor_upsert(
        &mut self,
        params: ProviderVendorUpsertParams,
    ) -> Result<ProviderVendorUpsertResult> {
        self.request_devo("provider/upsert", params).await
    }

    pub(crate) async fn provider_validate(
        &mut self,
        params: ProviderValidateParams,
    ) -> Result<ProviderValidateResult> {
        self.request_devo("provider/validate", params).await
    }

    pub(crate) async fn command_exec(
        &mut self,
        params: CommandExecParams,
    ) -> Result<CommandExecResult> {
        self.request_devo("command/exec", params).await
    }

    pub(crate) async fn command_exec_write(
        &mut self,
        params: CommandExecWriteParams,
    ) -> Result<CommandExecWriteResult> {
        self.request_devo("command/exec/write", params).await
    }

    pub(crate) async fn command_exec_resize(
        &mut self,
        params: CommandExecResizeParams,
    ) -> Result<CommandExecResizeResult> {
        self.request_devo("command/exec/resize", params).await
    }

    pub(crate) async fn command_exec_terminate(
        &mut self,
        params: CommandExecTerminateParams,
    ) -> Result<CommandExecTerminateResult> {
        self.request_devo("command/exec/terminate", params).await
    }

    pub(crate) async fn turn_shell_command(
        &mut self,
        params: ShellCommandParams,
    ) -> Result<ShellCommandResult> {
        self.request_devo("turn/shell_command", params).await
    }

    pub(crate) async fn turn_interrupt(
        &mut self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResult> {
        self.request_devo("turn/interrupt", params).await
    }

    pub(crate) async fn turn_steer(&mut self, params: TurnSteerParams) -> Result<TurnSteerResult> {
        self.request_devo("turn/steer", params).await
    }

    pub(crate) async fn reference_search_start(
        &mut self,
        params: ReferenceSearchStartParams,
    ) -> Result<ReferenceSearchStartResult> {
        self.request_devo("search/start", params).await
    }

    pub(crate) async fn reference_search_update(
        &mut self,
        params: ReferenceSearchUpdateParams,
    ) -> Result<ReferenceSearchUpdateResult> {
        self.request_devo("search/update", params).await
    }

    pub(crate) async fn reference_search_cancel(
        &mut self,
        params: ReferenceSearchCancelParams,
    ) -> Result<ReferenceSearchCancelResult> {
        self.request_devo("search/cancel", params).await
    }

    /// Fallback when the server does not implement `_devo/turn/start`.
    ///
    /// Sends blocking ACP `session/prompt` but returns immediately after the
    /// request is written. Completion is delivered later via synthetic
    /// `_devo/acp_prompt/completed` notifications. Multiple detached prompts
    /// may be in flight at once; each uses a distinct JSON-RPC `id` in
    /// [`PendingResponses`], so responses do not collide as long as ids stay unique.
    async fn turn_start_acp_prompt_detached(&mut self, params: TurnStartParams) -> Result<()> {
        let session_id = params.session_id;
        let prompt = params
            .input
            .into_iter()
            .map(acp_content_block_from_input_item)
            .collect();
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let (response_tx, response_rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id, response_tx);
        let request = AcpClientRequest::new(
            serde_json::json!(request_id),
            ACP_SESSION_PROMPT_METHOD,
            AcpPromptParams {
                session_id,
                prompt,
                meta: None,
            },
        );
        if let Err(error) = self.writer.send_serializable(&request) {
            self.pending.lock().await.remove(&request_id);
            return Err(error);
        }

        let _ = self.notifications_tx.send(ServerNotificationMessage {
            method: ACP_PROMPT_STARTED_NOTIFICATION_METHOD.to_string(),
            params: serde_json::json!({ "sessionId": session_id }),
        });
        let notifications_tx = self.notifications_tx.clone();
        tokio::spawn(async move {
            let params = match response_rx.await {
                Ok(response) if response.get("error").is_some() => serde_json::json!({
                    "sessionId": session_id,
                    "error": server_error_text(&response),
                }),
                Ok(response) => {
                    match serde_json::from_value::<AcpSuccessResponse<AcpPromptResult>>(response) {
                        Ok(success) => serde_json::json!({
                            "sessionId": session_id,
                            "stopReason": success.result.stop_reason,
                        }),
                        Err(error) => serde_json::json!({
                            "sessionId": session_id,
                            "error": format!("decode ACP prompt response: {error}"),
                        }),
                    }
                }
                Err(error) => serde_json::json!({
                    "sessionId": session_id,
                    "error": format!("server dropped ACP prompt response: {error}"),
                }),
            };
            let _ = notifications_tx.send(ServerNotificationMessage {
                method: ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD.to_string(),
                params,
            });
        });
        Ok(())
    }
}

impl ServerClientReaderState {
    /// Classifies one inbound server payload and dispatches without blocking the
    /// transport read loop on handler work.
    pub(crate) async fn handle_message(&self, message: serde_json::Value) {
        if let (Some(id), Some(method)) = (
            message.get("id").cloned(),
            message.get("method").and_then(serde_json::Value::as_str),
        ) {
            let params = message
                .get("params")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let state = self.clone();
            let method = method.to_string();
            // Server-initiated ACP client tools may run concurrently; each reply
            // must echo the request `id` assigned by the server.
            tokio::spawn(async move {
                state.handle_client_request(id, &method, params).await;
            });
            return;
        }
        if let Some(id) = message.get("id").and_then(serde_json::Value::as_u64) {
            deliver_pending_client_response(&self.pending, id, message).await;
            return;
        }
        if let Ok(notification) =
            serde_json::from_value::<NotificationEnvelope<serde_json::Value>>(message)
        {
            self.handle_notification(notification);
        }
    }

    pub(crate) async fn finish_reader(&self, transport_name: &'static str) {
        let abandoned_response_count = self.pending.lock().await.drain().count();
        if abandoned_response_count == 0 {
            tracing::warn!(transport_name, "server reader stopped");
        } else {
            tracing::warn!(
                transport_name,
                abandoned_response_count,
                "server reader stopped with pending responses"
            );
        }
        self.acp_terminals.release_all().await;
    }

    fn handle_notification(&self, notification: NotificationEnvelope<serde_json::Value>) {
        if notification.method == ACP_SESSION_UPDATE_METHOD
            && let Ok(acp_notification) =
                serde_json::from_value::<AcpSessionNotification>(notification.params.clone())
            && let Some((method, event)) = original_event_from_acp_notification(&acp_notification)
        {
            let _ = self.notifications_tx.send(ServerNotificationMessage {
                method,
                params: serde_json::to_value(event).expect("serialize original ACP event"),
            });
            return;
        }
        if let Some(method) = devo_extension_inner_method(&notification.method)
            && serde_json::from_value::<ServerEvent>(notification.params.clone()).is_ok()
        {
            let _ = self.notifications_tx.send(ServerNotificationMessage {
                method: method.to_string(),
                params: notification.params,
            });
            return;
        }
        log_notification_received(&notification);
        let _ = self.notifications_tx.send(ServerNotificationMessage {
            method: notification.method,
            params: notification.params,
        });
    }

    async fn handle_client_request(
        self,
        id: serde_json::Value,
        method: &str,
        params: serde_json::Value,
    ) {
        let response = if method == ACP_SESSION_REQUEST_PERMISSION_METHOD {
            match handle_acp_request_permission(
                id.clone(),
                params,
                self.acp_pending_permissions,
                self.notifications_tx,
            )
            .await
            {
                Ok(()) => return,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else if matches!(
            method,
            ACP_FS_READ_TEXT_FILE_METHOD | ACP_FS_WRITE_TEXT_FILE_METHOD
        ) {
            match handle_acp_fs_request(id.clone(), method, params).await {
                Ok(response) => response,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else if matches!(
            method,
            ACP_TERMINAL_CREATE_METHOD
                | ACP_TERMINAL_OUTPUT_METHOD
                | ACP_TERMINAL_WAIT_FOR_EXIT_METHOD
                | ACP_TERMINAL_KILL_METHOD
                | ACP_TERMINAL_RELEASE_METHOD
        ) {
            match handle_acp_terminal_request(
                id.clone(),
                method,
                params,
                self.acp_terminals,
                self.notifications_tx,
            )
            .await
            {
                Ok(response) => response,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else {
            acp_client_error_response(id, -32601, format!("unknown client method {method}"))
        };
        if let Err(error) = self.writer.send_value(response) {
            tracing::warn!(%error, method, "failed to write ACP client response");
        }
    }
}

async fn deliver_pending_client_response(
    pending: &PendingResponses,
    request_id: u64,
    message: serde_json::Value,
) {
    if let Some(tx) = pending.lock().await.remove(&request_id) {
        let _ = tx.send(message);
    } else {
        tracing::warn!(
            request_id,
            "dropping server response for unknown client request id"
        );
    }
}

fn acp_client_error_response(
    id: serde_json::Value,
    code: i64,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn bail_server_error(response: &serde_json::Value) -> Result<()> {
    bail!("{}", server_error_text(response))
}

fn is_method_not_found_error(error: &anyhow::Error) -> bool {
    error.to_string().starts_with("server -32601:")
}

fn server_error_text(response: &serde_json::Value) -> String {
    if let Ok(error) = serde_json::from_value::<ErrorResponse>(response.clone()) {
        let data = if error.error.data.is_null() {
            String::new()
        } else {
            format!(" data={}", error.error.data)
        };
        return format!(
            "server {}: {}{}",
            format_protocol_error_code(&error.error.code),
            error.error.message,
            data
        );
    }
    format!(
        "server {}: {}",
        response
            .get("error")
            .and_then(|error| error.get("code"))
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "unknown".to_string()),
        response
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown server error")
    )
}

fn format_protocol_error_code(code: &ProtocolErrorCode) -> &'static str {
    match code {
        ProtocolErrorCode::NotInitialized => "not_initialized",
        ProtocolErrorCode::InvalidParams => "invalid_params",
        ProtocolErrorCode::SessionNotFound => "session_not_found",
        ProtocolErrorCode::TurnNotFound => "turn_not_found",
        ProtocolErrorCode::TurnAlreadyRunning => "turn_already_running",
        ProtocolErrorCode::ApprovalNotFound => "approval_not_found",
        ProtocolErrorCode::PolicyDenied => "policy_denied",
        ProtocolErrorCode::ContextLimitExceeded => "context_limit_exceeded",
        ProtocolErrorCode::NoActiveTurn => "no_active_turn",
        ProtocolErrorCode::ExpectedTurnMismatch => "expected_turn_mismatch",
        ProtocolErrorCode::ActiveTurnNotSteerable => "active_turn_not_steerable",
        ProtocolErrorCode::EmptyInput => "empty_input",
        ProtocolErrorCode::AlreadyResolved => "already_resolved",
        ProtocolErrorCode::ParentSessionNotFound => "parent_session_not_found",
        ProtocolErrorCode::ForkTurnNotFound => "fork_turn_not_found",
        ProtocolErrorCode::ForkTurnNotStable => "fork_turn_not_stable",
        ProtocolErrorCode::PermissionDenied => "permission_denied",
        ProtocolErrorCode::CursorExpired => "cursor_expired",
        ProtocolErrorCode::QueueItemNotFound => "queue_item_not_found",
        ProtocolErrorCode::WorkspaceUnavailable => "workspace_unavailable",
        ProtocolErrorCode::InheritedSegmentWriteFailed => "inherited_segment_write_failed",
        ProtocolErrorCode::ForkRetentionRequired => "fork_retention_required",
        ProtocolErrorCode::InvalidConfirmToken => "invalid_confirm_token",
        ProtocolErrorCode::UnsupportedDeletePolicy => "unsupported_delete_policy",
        ProtocolErrorCode::InheritedSegmentMaterializationFailed => {
            "inherited_segment_materialization_failed"
        }
        ProtocolErrorCode::ExpectedTargetMessageMismatch => "expected_target_message_mismatch",
        ProtocolErrorCode::OlderMessageRequiresFork => "older_message_requires_fork",
        ProtocolErrorCode::ActiveTurnEditRejected => "active_turn_edit_rejected",
        ProtocolErrorCode::InvalidContentParts => "invalid_content_parts",
        ProtocolErrorCode::InvalidMentions => "invalid_mentions",
        ProtocolErrorCode::WorkspaceRestoreFailedToStart => "workspace_restore_failed_to_start",
        ProtocolErrorCode::RestorePlanNotFound => "restore_plan_not_found",
        ProtocolErrorCode::RestorePlanExpired => "restore_plan_expired",
        ProtocolErrorCode::WorkspaceVersionConflict => "workspace_version_conflict",
        ProtocolErrorCode::InternalError => "internal_error",
    }
}

fn acp_content_block_from_input_item(input: InputItem) -> AcpContentBlock {
    match input {
        InputItem::Text { text } => AcpContentBlock::text(text),
        InputItem::Skill { name, path } => AcpContentBlock::Text {
            annotations: None,
            text: format!("Skill {name}: {}", path.display()),
            meta: None,
        },
        InputItem::LocalImage { path } => AcpContentBlock::Text {
            annotations: None,
            text: format!("Image: {}", path.display()),
            meta: None,
        },
        InputItem::Mention { path, name } => AcpContentBlock::ResourceLink {
            annotations: None,
            uri: file_uri_from_path(&path),
            name: name.unwrap_or_else(|| path.clone()),
            title: None,
            description: None,
            mime_type: None,
            size: None,
            meta: None,
        },
    }
}

fn file_uri_from_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("file://{normalized}")
    } else {
        format!("file:///{normalized}")
    }
}

fn acp_session_metadata_from_start_params(
    params: &SessionStartParams,
    session_id: SessionId,
) -> SessionMetadata {
    let now = Utc::now();
    SessionMetadata {
        session_id,
        cwd: params.cwd.clone(),
        additional_directories: params.additional_directories.clone(),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        title: params.title.clone(),
        title_state: acp_title_state(&params.title),
        parent_session_id: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: params.ephemeral,
        model: params.model.clone(),
        model_binding_id: params.model_binding_id.clone(),
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        status: SessionRuntimeStatus::Idle,
    }
}

fn acp_session_metadata_from_session_info(session_info: &AcpSessionInfo) -> SessionMetadata {
    let updated_at = session_info
        .updated_at
        .as_deref()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    SessionMetadata {
        session_id: session_info.session_id,
        cwd: session_info.cwd.clone(),
        additional_directories: session_info.additional_directories.clone(),
        created_at: updated_at,
        updated_at,
        last_activity_at: updated_at,
        title: session_info.title.clone(),
        title_state: acp_title_state(&session_info.title),
        parent_session_id: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: false,
        model: None,
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        status: SessionRuntimeStatus::Idle,
    }
}

fn acp_title_state(title: &Option<String>) -> SessionTitleState {
    if title.is_some() {
        SessionTitleState::Provisional
    } else {
        SessionTitleState::Unset
    }
}

fn log_notification_received(notification: &NotificationEnvelope<serde_json::Value>) {
    let event_seq = notification
        .params
        .get("context")
        .and_then(|context| context.get("seq"))
        .and_then(serde_json::Value::as_u64);
    let item_id = notification
        .params
        .get("context")
        .and_then(|context| context.get("item_id"))
        .and_then(serde_json::Value::as_str);
    let assistant_delta = (notification.method == "item/agentMessage/delta")
        .then(|| notification.params.get("delta")?.as_str())
        .flatten();
    let delta_len = assistant_delta.map(str::len);
    let assistant_token_text = assistant_delta.and_then(assistant_token_log_preview);
    if let Some(assistant_token_text) = assistant_token_text.as_deref() {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            method = %notification.method,
            event_seq,
            item_id = ?item_id,
            delta_len = ?delta_len,
            assistant_token_text,
            "client received server notification"
        );
    } else {
        tracing::debug!(
            stream_elapsed_ms = stream_trace_elapsed_ms(),
            method = %notification.method,
            event_seq,
            item_id = ?item_id,
            delta_len = ?delta_len,
            "client received server notification"
        );
    }
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled().then(|| {
        let max_chars = assistant_token_log_max_chars();
        format_assistant_token_log_preview(text, max_chars)
    })
}

fn assistant_token_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("DEVO_LOG_ASSISTANT_TOKEN_TEXT")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
    })
}

fn assistant_token_log_max_chars() -> usize {
    static ASSISTANT_TOKEN_LOG_MAX_CHARS: OnceLock<usize> = OnceLock::new();
    *ASSISTANT_TOKEN_LOG_MAX_CHARS.get_or_init(|| {
        std::env::var("DEVO_ASSISTANT_TOKEN_LOG_MAX_CHARS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(512)
    })
}

fn format_assistant_token_log_preview(text: &str, max_chars: usize) -> String {
    let max_chars = max_chars.max(1);
    let mut preview = String::with_capacity(text.len().min(max_chars));
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}
