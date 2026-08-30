//! Transport-agnostic Devo server client: JSON-RPC request/response routing,
//! Native reverse-request handling, and notification demultiplexing.
//!
//! Both [`crate::stdio::StdioServerClient`] and [`crate::websocket::WebSocketServerClient`]
//! delegate protocol logic here. Incoming messages are classified as:
//!
//! - **Server → client requests** (`id` + `method`): handled asynchronously; the
//!   response echoes the same JSON-RPC `id`.
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

use crate::native_approval::PendingApprovals;
use crate::native_approval::discard_approval_request;
use crate::native_approval::handle_approval_request;
use crate::native_approval::resolve_approval_response;

const SERVER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
enum ServerResponseWait {
    Standard,
    Unbounded,
}

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
    pending_approvals: PendingApprovals,
    native_pending_user_inputs: NativePendingUserInputs,
    notifications_tx: mpsc::UnboundedSender<ServerNotificationMessage>,
}

/// Pending canonical `userInput/request` reverse requests: canonical
/// `requestId` → JSON-RPC request id of the inbound server request
/// (L2-DES-APP-008 DD-8).
pub(crate) type NativePendingUserInputs = Arc<Mutex<HashMap<String, serde_json::Value>>>;

pub(crate) struct ServerClientCore {
    writer: ClientWriter,
    pending: PendingResponses,
    pending_approvals: PendingApprovals,
    native_pending_user_inputs: NativePendingUserInputs,
    acp_agent_capabilities: Option<AcpAgentCapabilities>,
    client_capabilities: AcpClientCapabilities,
    /// Opt into native typed item events on initialize (L2-DES-APP-009).
    /// Per-consumer until every first-party client handles typed shapes.
    typed_items_opt_in: bool,
    /// Declare the Native protocol surface on initialize.
    /// (L2-DES-APP-009 DD-6). Per-consumer until every first-party client
    /// stops relying on ACP-routed methods (`session/new`, `session/list`,
    /// …); the websocket client still uses them.
    native_protocol_opt_in: bool,
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
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
            native_pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            acp_agent_capabilities: None,
            client_capabilities,
            typed_items_opt_in: false,
            native_protocol_opt_in: false,
            next_request_id: AtomicU64::new(1),
            notifications_rx,
            notifications_tx,
        }
    }

    pub(crate) fn set_typed_items_opt_in(&mut self, opted_in: bool) {
        self.typed_items_opt_in = opted_in;
    }

    pub(crate) fn set_native_protocol_opt_in(&mut self, opted_in: bool) {
        self.native_protocol_opt_in = opted_in;
    }

    pub(crate) fn reader_state(&self) -> ServerClientReaderState {
        ServerClientReaderState {
            writer: self.writer.clone(),
            pending: Arc::clone(&self.pending),
            pending_approvals: Arc::clone(&self.pending_approvals),
            native_pending_user_inputs: Arc::clone(&self.native_pending_user_inputs),
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
                    // Opt-ins ride the `_meta.devo` extension object: the
                    // Native protocol surface (L2-DES-APP-009 DD-6) routes
                    // colliding method names to Native handlers; typed
                    // items select native typed notifications. Both are
                    // per-consumer until every first-party client migrates.
                    meta: {
                        let mut devo_ext = serde_json::Map::new();
                        if self.native_protocol_opt_in {
                            devo_ext.insert(
                                devo_protocol::DEVO_PROTOCOL_META.to_string(),
                                serde_json::Value::String(
                                    devo_protocol::DEVO_PROTOCOL_NATIVE.to_string(),
                                ),
                            );
                        }
                        if self.typed_items_opt_in {
                            devo_ext.insert(
                                devo_protocol::DEVO_TYPED_ITEMS_META.to_string(),
                                serde_json::Value::Bool(true),
                            );
                        }
                        (!devo_ext.is_empty()).then(|| {
                            devo_protocol::AcpMeta::from_iter([(
                                devo_protocol::DEVO_EXTENSION_META.to_string(),
                                serde_json::Value::Object(devo_ext),
                            )])
                        })
                    },
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
                    title: params.title.clone(),
                    model: params.model.clone(),
                    model_binding_id: params.model_binding_id.clone(),
                    ephemeral: params.ephemeral,
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

    pub(crate) async fn request<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.request_with_wait(method, params, ServerResponseWait::Standard)
            .await
    }

    async fn request_without_timeout<P, R>(&mut self, method: &str, params: P) -> Result<R>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.request_with_wait(method, params, ServerResponseWait::Unbounded)
            .await
    }

    async fn request_with_wait<P, R>(
        &mut self,
        method: &str,
        params: P,
        wait: ServerResponseWait,
    ) -> Result<R>
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

        let response = match wait {
            ServerResponseWait::Standard => {
                match timeout(SERVER_RESPONSE_TIMEOUT, response_rx).await {
                    Ok(Ok(response)) => response,
                    Ok(Err(error)) => {
                        self.pending.lock().await.remove(&request_id);
                        return Err(error).with_context(|| {
                            format!("server dropped response for request {request_id}")
                        });
                    }
                    Err(error) => {
                        self.pending.lock().await.remove(&request_id);
                        return Err(error)
                            .with_context(|| format!("{method} request {request_id} timed out"));
                    }
                }
            }
            ServerResponseWait::Unbounded => match response_rx.await {
                Ok(response) => response,
                Err(error) => {
                    self.pending.lock().await.remove(&request_id);
                    return Err(error).with_context(|| {
                        format!("server dropped response for request {request_id}")
                    });
                }
            },
        };
        if response.get("error").is_some() {
            bail_server_error(&response)?;
        }
        let success: AcpSuccessResponse<R> =
            serde_json::from_value(response).context("decode success response from server")?;
        Ok(success.result)
    }

    pub(crate) async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        self.request("turn/start", params).await
    }

    pub(crate) async fn turn_start_native(
        &mut self,
        session_id: SessionId,
        input: Vec<devo_protocol::native::item::UserInput>,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_turn::TurnStartResult> {
        self.request(
            "turn/start",
            devo_protocol::native::rpc_turn::TurnStartParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                input,
                client_user_message_id: None,
                idempotency_key,
            },
        )
        .await
    }

    /// Native `session/compact/start` (L2-DES-APP-008 Phase B).
    pub(crate) async fn session_compact_start_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_turn::TurnStartResult> {
        self.request(
            "session/compact/start",
            devo_protocol::native::rpc_session::SessionCompactStartParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
            },
        )
        .await
    }

    /// Native `session/rollback/preview` (L2-DES-APP-008): computes the
    /// history/workspace impact of a rollback without changing state.
    pub(crate) async fn session_rollback_preview_native(
        &mut self,
        session_id: SessionId,
        user_turn_index: u32,
        mode: devo_protocol::native::rpc_session::RollbackMode,
    ) -> Result<devo_protocol::native::rpc_session::RestorePlan> {
        self.request(
            "session/rollback/preview",
            devo_protocol::native::rpc_session::SessionRollbackPreviewParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                user_turn_index,
                mode,
            },
        )
        .await
    }

    /// Native `session/rollback/commit` (L2-DES-APP-008): commits a
    /// previously previewed restore plan.
    pub(crate) async fn session_rollback_commit_native(
        &mut self,
        restore_plan_id: devo_protocol::native::ids::RestorePlanId,
        expected_workspace_version: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionRollbackCommitResult> {
        self.request(
            "session/rollback/commit",
            devo_protocol::native::rpc_session::SessionRollbackCommitParams {
                restore_plan_id,
                expected_workspace_version,
            },
        )
        .await
    }

    /// Native `session/goal/update` (ratified #3): in-place edit patch.
    pub(crate) async fn session_goal_update_native(
        &mut self,
        session_id: SessionId,
        patch: devo_protocol::native::rpc_session::GoalPatch,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionGoalUpdateResult> {
        self.request(
            "session/goal/update",
            devo_protocol::native::rpc_session::SessionGoalUpdateParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                expected_goal_id: None,
                patch,
                idempotency_key,
            },
        )
        .await
    }

    /// Native `session/new` (L2-DES-APP-008 Phase B): create a durable
    /// session with idempotency-key replay safety.
    pub(crate) async fn session_new_native(
        &mut self,
        cwd: std::path::PathBuf,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionNewResult> {
        self.request(
            "session/new",
            devo_protocol::native::rpc_session::SessionNewParams {
                cwd,
                idempotency_key,
            },
        )
        .await
    }

    /// Native `session/list` (L2-DES-APP-008): one page of canonical
    /// session snapshots for the session picker.
    pub(crate) async fn session_list_native(
        &mut self,
        params: devo_protocol::native::rpc_session::SessionListParams,
    ) -> Result<devo_protocol::native::rpc_session::SessionListResult> {
        self.request("session/list", params).await
    }

    /// Native `session/delete` (L2-DES-APP-008): deletes the session tree.
    pub(crate) async fn session_delete_native(&mut self, session_id: SessionId) -> Result<()> {
        self.request(
            "session/delete",
            devo_protocol::native::rpc_session::SessionDeleteParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
            },
        )
        .await
        .map(|_: devo_protocol::native::rpc_session::SessionDeleteResult| ())
    }

    /// Native `session/resume` (L2-DES-APP-008 Phase B): hydrates the
    /// session and returns the canonical snapshot. Transcript restore is via
    /// `session/items/list` / `subscription/*`, not this result.
    pub(crate) async fn session_resume_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_session::SessionResumeResult> {
        self.request(
            "session/resume",
            devo_protocol::native::rpc_session::SessionResumeParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
            },
        )
        .await
    }

    /// Native `session/items/list` (L2-DES-APP-008 Phase C): one page of
    /// history items for transcript restore.
    pub(crate) async fn session_items_list_native(
        &mut self,
        session_id: SessionId,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope>> {
        self.request(
            "session/items/list",
            devo_protocol::native::rpc_session::SessionItemsListParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                turn_id: None,
                page: devo_protocol::native::page::PageParams { cursor, limit },
            },
        )
        .await
    }

    /// Native `session/turns/list`: one page of turns (used to resolve a
    /// user-turn index into a turn id for `session/fork`).
    pub(crate) async fn session_turns_list_native(
        &mut self,
        session_id: SessionId,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<devo_protocol::native::page::Page<devo_protocol::native::turn::Turn>> {
        self.request(
            "session/turns/list",
            devo_protocol::native::rpc_session::SessionTurnsListParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                page: devo_protocol::native::page::PageParams { cursor, limit },
            },
        )
        .await
    }

    /// Native `session/fork` (L2-DES-APP-008 Phase B).
    pub(crate) async fn session_fork_native(
        &mut self,
        session_id: SessionId,
        at_turn_id: Option<TurnId>,
    ) -> Result<devo_protocol::native::rpc_session::SessionForkResult> {
        self.session_fork_native_with_cut(session_id, at_turn_id, None)
            .await
    }

    /// Native `session/fork` with an explicit cut mode.
    pub(crate) async fn session_fork_native_with_cut(
        &mut self,
        session_id: SessionId,
        at_turn_id: Option<TurnId>,
        cut: Option<devo_protocol::native::rpc_session::SessionForkCut>,
    ) -> Result<devo_protocol::native::rpc_session::SessionForkResult> {
        self.request(
            "session/fork",
            devo_protocol::native::rpc_session::SessionForkParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                at_turn_id: at_turn_id.map(|turn_id| {
                    devo_protocol::native::ids::TurnId::from_string(turn_id.to_string())
                }),
                cut,
            },
        )
        .await
    }

    /// Native session title rename via the `title` patch of
    /// `session/metadata/update` (L2-DES-APP-008 Phase B).
    pub(crate) async fn session_title_update_native(
        &mut self,
        session_id: SessionId,
        title: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        self.request(
            "session/metadata/update",
            devo_protocol::native::rpc_session::SessionMetadataUpdateParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                expected_version: 0,
                title: devo_protocol::native::patch::PatchField::Value(title),
                model: None,
                model_binding_id: None,
                settings: None,
            },
        )
        .await
    }

    /// Native `session/goal/set` (L2-DES-APP-008 Phase B).
    pub(crate) async fn session_goal_set_native(
        &mut self,
        session_id: SessionId,
        objective: String,
        token_budget: Option<u64>,
        if_exists: devo_protocol::native::rpc_session::GoalIfExists,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionGoalSetResult> {
        self.request(
            "session/goal/set",
            devo_protocol::native::rpc_session::SessionGoalSetParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                objective,
                token_budget,
                if_exists,
                idempotency_key,
            },
        )
        .await
    }

    /// Native `session/goal/read` (L2-DES-APP-008 Phase B).
    pub(crate) async fn session_goal_read_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_session::SessionGoalReadResult> {
        self.request(
            "session/goal/read",
            devo_protocol::native::rpc_session::SessionGoalReadParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
            },
        )
        .await
    }

    /// Native goal lifecycle transition with the `expectedGoalId`
    /// precondition; `Clear` returns no goal payload.
    pub(crate) async fn session_goal_transition_native(
        &mut self,
        session_id: SessionId,
        expected_goal_id: &devo_protocol::native::ids::GoalId,
        transition: GoalLifecycleTransition,
    ) -> Result<Option<devo_protocol::native::goal::Goal>> {
        let method = match transition {
            GoalLifecycleTransition::Pause => "session/goal/pause",
            GoalLifecycleTransition::Resume => "session/goal/resume",
            GoalLifecycleTransition::Complete => "session/goal/complete",
            GoalLifecycleTransition::Cancel => "session/goal/cancel",
            GoalLifecycleTransition::Clear => "session/goal/clear",
        };
        let params = devo_protocol::native::rpc_session::SessionGoalTransitionParams {
            session_id: devo_protocol::native::ids::SessionId::from_string(session_id.to_string()),
            expected_goal_id: expected_goal_id.clone(),
        };
        if matches!(transition, GoalLifecycleTransition::Clear) {
            let _: serde_json::Value = self.request(method, params).await?;
            return Ok(None);
        }
        let result: devo_protocol::native::rpc_session::SessionGoalTransitionResult =
            self.request(method, params).await?;
        Ok(Some(result.goal))
    }

    /// Native `session/interrupt`: stop the selected active work scope.
    pub(crate) async fn session_interrupt_native(
        &mut self,
        scope: devo_protocol::native::rpc_session::SessionInterruptScope,
    ) -> Result<devo_protocol::native::rpc_session::SessionInterruptResult> {
        self.request(
            "session/interrupt",
            devo_protocol::native::rpc_session::SessionInterruptParams { scope },
        )
        .await
    }

    /// Native `agent/list` (L2-DES-APP-008 Phase B facade): child agents
    /// as `SubAgent` item envelopes.
    pub(crate) async fn agent_list_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_turn::AgentListResult> {
        self.request(
            "agent/list",
            devo_protocol::native::rpc_turn::AgentListParams {
                session_id: Some(devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                )),
            },
        )
        .await
    }

    /// Native `agent/cancel` (Phase B facade).
    pub(crate) async fn agent_cancel_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .request(
                "agent/cancel",
                devo_protocol::native::rpc_turn::AgentCancelParams {
                    item_id: item_id.clone(),
                },
            )
            .await?;
        Ok(())
    }

    /// Native `agent/message` (Phase B facade).
    pub(crate) async fn agent_message_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
        input: Vec<devo_protocol::native::item::UserInput>,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .request(
                "agent/message",
                devo_protocol::native::rpc_turn::AgentMessageParams {
                    item_id: item_id.clone(),
                    input,
                },
            )
            .await?;
        Ok(())
    }

    /// Native `agent/read` (Phase B facade).
    pub(crate) async fn agent_read_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
    ) -> Result<devo_protocol::native::rpc_turn::AgentReadResult> {
        self.request(
            "agent/read",
            devo_protocol::native::rpc_turn::AgentReadParams {
                item_id: item_id.clone(),
            },
        )
        .await
    }

    /// Native `task/start` with `kind: "process"` (L2-DES-APP-008 DD-7
    /// facade).
    pub(crate) async fn task_start_process_native(
        &mut self,
        session_id: SessionId,
        command: String,
        cwd: Option<std::path::PathBuf>,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_turn::TaskStartResult> {
        self.request(
            "task/start",
            devo_protocol::native::rpc_turn::TaskStartParams::Process {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                command,
                cwd,
                idempotency_key,
            },
        )
        .await
    }

    /// Native `task/start` with `kind: "agent"` (L2-DES-APP-008 DD-7):
    /// spawns a child-session agent; the result item id is the child
    /// session's `item_<uuid>`.
    pub(crate) async fn task_start_agent_native(
        &mut self,
        params: devo_protocol::native::rpc_turn::TaskStartParams,
    ) -> Result<devo_protocol::native::rpc_turn::TaskStartResult> {
        self.request("task/start", params).await
    }

    /// Native `task/interrupt` (DD-7 facade).
    pub(crate) async fn task_interrupt_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
    ) -> Result<()> {
        let _: serde_json::Value = self
            .request(
                "task/interrupt",
                devo_protocol::native::rpc_turn::TaskInterruptParams {
                    item_id: item_id.clone(),
                },
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn approval_respond(&mut self, params: ApprovalResponseParams) -> Result<()> {
        if let Some(response) = resolve_approval_response(&self.pending_approvals, &params).await {
            self.writer.send_value(response)?;
            return Ok(());
        }
        bail!("no pending Native approval request exists for approval response")
    }

    pub(crate) async fn request_user_input_respond(
        &mut self,
        request_id: String,
        response: RequestUserInputResponse,
    ) -> Result<()> {
        let pending_request_id = self
            .native_pending_user_inputs
            .lock()
            .await
            .remove(request_id.as_str())
            .ok_or_else(|| anyhow!("no pending canonical user-input request exists"))?;
        let answer = devo_protocol::native::methods::UserInputRespondParams {
            request_id,
            answers: serde_json::to_value(&response.answers).expect("serialize user input answers"),
        };
        let response = devo_protocol::acp_success_response(
            pending_request_id,
            serde_json::to_value(answer).expect("serialize canonical user input answer"),
        );
        self.writer.send_value(response)?;
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
    }

    pub(crate) async fn session_delete(
        &mut self,
        params: AcpDeleteSessionParams,
    ) -> Result<AcpDeleteSessionResult> {
        self.request(ACP_SESSION_DELETE_METHOD, params).await
    }

    /// Native settings patch (L2-DES-APP-008): sent on the canonical
    /// method string with `expectedVersion: 0`
    /// (no precondition — first-party clients do not track session versions
    /// yet; the server treats 0 as "skip the check").
    pub(crate) async fn session_settings_update(
        &mut self,
        session_id: SessionId,
        patch: devo_protocol::native::rpc_session::SessionSettingsPatch,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        self.request(
            "session/metadata/update",
            devo_protocol::native::rpc_session::SessionMetadataUpdateParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                expected_version: 0,
                title: devo_protocol::native::patch::PatchField::Missing,
                model: None,
                model_binding_id: None,
                settings: Some(patch),
            },
        )
        .await
    }

    /// Native model/metadata update: model slug, provider binding,
    /// reasoning effort, and collaboration mode in one patch.
    pub(crate) async fn session_model_update(
        &mut self,
        session_id: SessionId,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        collaboration_mode: Option<devo_protocol::CollaborationMode>,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        let settings = if reasoning_effort_selection.is_some() || collaboration_mode.is_some() {
            Some(devo_protocol::native::rpc_session::SessionSettingsPatch {
                reasoning_effort: reasoning_effort_selection,
                mode: collaboration_mode.and_then(|mode| {
                    serde_json::to_value(mode)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                }),
                ..Default::default()
            })
        } else {
            None
        };
        self.request(
            "session/metadata/update",
            devo_protocol::native::rpc_session::SessionMetadataUpdateParams {
                session_id: devo_protocol::native::ids::SessionId::from_string(
                    session_id.to_string(),
                ),
                expected_version: 0,
                title: devo_protocol::native::patch::PatchField::Missing,
                model: model.map(|slug| devo_protocol::native::model::ModelBinding {
                    // The server resolves routing from the slug/binding; the
                    // provider label is response-side information.
                    provider: String::new(),
                    model: slug,
                    reasoning_effort: None,
                }),
                model_binding_id,
                settings,
            },
        )
        .await
    }

    pub(crate) async fn session_cancel(&mut self, params: AcpCancelParams) -> Result<()> {
        let notification = AcpClientNotification::new(ACP_SESSION_CANCEL_METHOD, params);
        self.writer.send_serializable(&notification)
    }

    pub(crate) async fn mcp_list(
        &mut self,
        params: devo_protocol::native::rpc_admin::McpListParams,
    ) -> Result<devo_protocol::native::rpc_admin::McpListResult> {
        self.request("mcp/list", params).await
    }

    pub(crate) async fn mcp_tools(
        &mut self,
        params: devo_protocol::native::rpc_admin::McpToolsParams,
    ) -> Result<devo_protocol::native::rpc_admin::McpToolsResult> {
        self.request("mcp/tools", params).await
    }

    pub(crate) async fn mcp_set_enabled(
        &mut self,
        params: devo_protocol::native::rpc_admin::McpSetEnabledParams,
    ) -> Result<devo_protocol::native::rpc_admin::McpSetEnabledResult> {
        self.request("mcp/set_enabled", params).await
    }

    /// Native `model/list` (L2-DES-APP-008): the model catalog in the
    /// parity canonical `ModelInfo` shape (ratified #7).
    pub(crate) async fn model_list_native(
        &mut self,
    ) -> Result<devo_protocol::native::rpc_admin::ModelListResult> {
        self.request(
            "model/list",
            devo_protocol::native::rpc_admin::ModelListParams {},
        )
        .await
    }

    pub(crate) async fn provider_list(
        &mut self,
    ) -> Result<devo_protocol::native::rpc_admin::ProviderListResult> {
        self.request(
            "provider/list",
            devo_protocol::native::rpc_admin::ProviderListParams {},
        )
        .await
    }

    pub(crate) async fn provider_upsert(
        &mut self,
        params: devo_protocol::native::rpc_admin::ProviderUpsertParams,
    ) -> Result<devo_protocol::native::rpc_admin::ProviderUpsertResult> {
        self.request("provider/upsert", params).await
    }

    pub(crate) async fn provider_validate(
        &mut self,
        params: devo_protocol::native::rpc_admin::ProviderValidateParams,
    ) -> Result<devo_protocol::native::rpc_admin::ProviderValidateResult> {
        self.request_without_timeout("provider/validate", params)
            .await
    }

    pub(crate) async fn command_exec(
        &mut self,
        params: CommandExecParams,
    ) -> Result<CommandExecResult> {
        self.request("command/exec", params).await
    }

    pub(crate) async fn session_queue_push(
        &mut self,
        params: native::rpc_turn::SessionQueuePushParams,
    ) -> Result<native::rpc_turn::SessionQueuePushResult> {
        self.request("session/queue/push", params).await
    }

    pub(crate) async fn session_queue_list(
        &mut self,
        params: native::rpc_turn::SessionQueueListParams,
    ) -> Result<native::rpc_turn::SessionQueueListResult> {
        self.request("session/queue/list", params).await
    }

    pub(crate) async fn session_queue_update(
        &mut self,
        params: native::rpc_turn::SessionQueueUpdateParams,
    ) -> Result<native::rpc_turn::SessionQueueUpdateResult> {
        self.request("session/queue/update", params).await
    }

    pub(crate) async fn session_queue_remove(
        &mut self,
        params: native::rpc_turn::SessionQueueRemoveParams,
    ) -> Result<native::rpc_turn::SessionQueueRemoveResult> {
        self.request("session/queue/remove", params).await
    }

    pub(crate) async fn session_queue_steer(
        &mut self,
        params: native::rpc_turn::SessionQueueSteerParams,
    ) -> Result<native::rpc_turn::SessionQueueSteerResult> {
        self.request("session/queue/steer", params).await
    }

    pub(crate) async fn subscription_create(
        &mut self,
        params: native::event::SubscriptionCreateParams,
    ) -> Result<native::event::SubscriptionCreateResult> {
        self.request("subscription/create", params).await
    }

    /// Native `search/start` (L2-DES-APP-008): connection-local composer
    /// reference search with the canonical camelCase snapshot.
    pub(crate) async fn search_start(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        query: String,
    ) -> Result<devo_protocol::native::rpc_search::SearchSnapshot> {
        let result: devo_protocol::native::rpc_search::SearchStartResult = self
            .request(
                "search/start",
                devo_protocol::native::rpc_search::SearchStartParams { cwd, query },
            )
            .await?;
        Ok(result.snapshot)
    }

    /// Native `search/update` (L2-DES-APP-008).
    pub(crate) async fn search_update(
        &mut self,
        search_id: devo_protocol::ReferenceSearchId,
        query: String,
    ) -> Result<devo_protocol::native::rpc_search::SearchSnapshot> {
        let result: devo_protocol::native::rpc_search::SearchUpdateResult = self
            .request(
                "search/update",
                devo_protocol::native::rpc_search::SearchUpdateParams { search_id, query },
            )
            .await?;
        Ok(result.snapshot)
    }

    /// Native `search/cancel` (L2-DES-APP-008).
    pub(crate) async fn search_cancel(
        &mut self,
        search_id: devo_protocol::ReferenceSearchId,
    ) -> Result<()> {
        self.request(
            "search/cancel",
            devo_protocol::native::rpc_search::SearchCancelParams { search_id },
        )
        .await
        .map(|_: devo_protocol::native::rpc_search::SearchCancelResult| ())
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
            if matches!(
                method.as_str(),
                "approval/command/request"
                    | "approval/fileChange/request"
                    | "approval/permission/request"
                    | "session/goal/completionApproval/request"
                    | "userInput/request"
            ) {
                state.handle_client_request(id, &method, params).await;
                return;
            }
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
        self.pending_approvals.lock().await.clear();
        self.native_pending_user_inputs.lock().await.clear();
        if abandoned_response_count == 0 {
            tracing::warn!(transport_name, "server reader stopped");
        } else {
            tracing::warn!(
                transport_name,
                abandoned_response_count,
                "server reader stopped with pending responses"
            );
        }
    }

    fn handle_notification(&self, notification: NotificationEnvelope<serde_json::Value>) {
        if notification.method == "item/completed"
            && notification.params.pointer("/item/item/decision").is_some()
            && let Some(approval_id) = notification
                .params
                .pointer("/item/item/approvalId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        {
            let pending_approvals = Arc::clone(&self.pending_approvals);
            tokio::spawn(async move {
                discard_approval_request(&pending_approvals, &approval_id).await;
            });
        }
        if notification.method == "item/completed"
            && let Some(request_id) = notification
                .params
                .pointer("/item/item/requestId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        {
            let pending_user_inputs = Arc::clone(&self.native_pending_user_inputs);
            tokio::spawn(async move {
                pending_user_inputs.lock().await.remove(&request_id);
            });
        }
        if notification.method == "serverRequest/resolved"
            && let Some(request_id) = notification
                .params
                .get("requestId")
                .or_else(|| notification.params.get("request_id"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        {
            let pending_user_inputs = Arc::clone(&self.native_pending_user_inputs);
            tokio::spawn(async move {
                pending_user_inputs.lock().await.remove(&request_id);
            });
        }
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
        let response = if matches!(
            method,
            "approval/command/request"
                | "approval/fileChange/request"
                | "approval/permission/request"
                | "session/goal/completionApproval/request"
        ) {
            // Native reverse approval request (L2-DES-APP-008 DD-8): the
            // server's own item events drive the approval UI; the pending
            // entry only needs to resolve the JSON-RPC response later.
            match handle_approval_request(id.clone(), params, self.pending_approvals).await {
                Ok(()) => return,
                Err(message) => acp_client_error_response(id, -32603, message),
            }
        } else if method == "userInput/request" {
            // Native reverse question request (DD-8): the broadcast event
            // drives the question UI; register the request so
            // `request_user_input_respond` answers it on this channel.
            let Some(native_request_id) = params
                .get("requestId")
                .or_else(|| params.get("request_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|request_id| !request_id.is_empty())
                .map(str::to_string)
            else {
                let response = acp_client_error_response(
                    id,
                    -32603,
                    "userInput/request params.requestId is required",
                );
                if let Err(error) = self.writer.send_value(response) {
                    tracing::warn!(%error, method, "failed to write ACP client response");
                }
                return;
            };
            self.native_pending_user_inputs
                .lock()
                .await
                .insert(native_request_id, id);
            return;
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
        ProtocolErrorCode::GoalNotFound => "goal_not_found",
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
        fork_from_id: None,
        fork_at_turn_id: None,
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
        last_context_occupancy: None,
        status: SessionRuntimeStatus::Idle,
        collaboration_mode: Default::default(),
        effective_context_window: None,
        permission_preset: None,
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
        fork_from_id: None,
        fork_at_turn_id: None,
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
        last_context_occupancy: None,
        status: SessionRuntimeStatus::Idle,
        collaboration_mode: Default::default(),
        effective_context_window: None,
        permission_preset: None,
    }
}

fn acp_title_state(title: &Option<String>) -> SessionTitleState {
    if title.is_some() {
        SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate)
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

/// Converts TUI turn input into canonical `UserInput`.
pub fn native_turn_start_input(
    input: &[InputItem],
) -> Option<Vec<devo_protocol::native::item::UserInput>> {
    input
        .iter()
        .map(|item| match item {
            InputItem::Text { text } => {
                Some(devo_protocol::native::item::UserInput::Text { text: text.clone() })
            }
            InputItem::LocalImage { path } => {
                Some(devo_protocol::native::item::UserInput::LocalImage {
                    path: path.clone(),
                    detail: None,
                })
            }
            InputItem::Mention { path, .. } => {
                Some(devo_protocol::native::item::UserInput::Mention { uri: path.clone() })
            }
            InputItem::Skill { name, .. } => {
                Some(devo_protocol::native::item::UserInput::Skill { name: name.clone() })
            }
        })
        .collect()
}

/// Goal lifecycle transitions for `session_goal_transition_native`
/// (L2-DES-APP-008 Phase B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalLifecycleTransition {
    Pause,
    Resume,
    Complete,
    Cancel,
    Clear,
}
