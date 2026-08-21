//! WebSocket transport for an already-running devo server process.
//!
//! The client sends one JSON-RPC message per WebSocket text frame and reads
//! responses, notifications, and server-initiated ACP client requests from the
//! same connection.

use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::*;
use futures::SinkExt;
use futures::StreamExt;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::client_core::ClientWriteMessage;
use crate::client_core::ClientWriter;
use crate::client_core::ServerClientCore;
use crate::client_core::ServerNotificationMessage;

const WEBSOCKET_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct WebSocketServerClientConfig {
    pub endpoint: String,
    pub client_capabilities: AcpClientCapabilities,
}

pub struct WebSocketServerClient {
    core: ServerClientCore,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<Result<()>>,
}

impl WebSocketServerClient {
    pub async fn connect(config: WebSocketServerClientConfig) -> Result<Self> {
        tracing::info!(endpoint = %config.endpoint, "connecting websocket server client");
        let (socket, _) = connect_async(&config.endpoint)
            .await
            .with_context(|| format!("connect websocket server {}", config.endpoint))?;
        let (mut writer, mut reader) = socket.split();
        let (client_writer, mut write_rx) = ClientWriter::channel();
        let mut core = ServerClientCore::new(client_writer, config.client_capabilities);
        // This first-party client speaks the Native API. ACP clients connect
        // directly to the server adapter and omit the Native wire marker.
        core.set_native_protocol_opt_in(true);
        let reader_state = core.reader_state();

        let writer_task = tokio::spawn(async move {
            while let Some(message) = write_rx.recv().await {
                match message {
                    ClientWriteMessage::Json(value) => {
                        writer
                            .send(Message::Text(
                                serde_json::to_string(&value)
                                    .context("serialize websocket client payload")?
                                    .into(),
                            ))
                            .await
                            .context("write websocket client payload")?;
                    }
                    ClientWriteMessage::Close => {
                        let _ = writer.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
            Ok(())
        });

        let reader_task = tokio::spawn(async move {
            while let Some(frame) = reader.next().await {
                match frame {
                    Ok(Message::Text(text)) => match serde_json::from_str(text.as_str()) {
                        Ok(message) => reader_state.handle_message(message).await,
                        Err(error) => {
                            tracing::warn!(%error, "failed to parse JSON from websocket server")
                        }
                    },
                    Ok(Message::Close(_)) => break,
                    Ok(Message::Binary(_)) => {
                        tracing::debug!("ignoring binary websocket server frame");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "websocket server reader stopped with error");
                        break;
                    }
                }
            }
            reader_state.finish_reader("websocket").await;
        });

        Ok(Self {
            core,
            reader_task,
            writer_task,
        })
    }

    pub async fn initialize(&mut self) -> Result<InitializeResult> {
        self.core.initialize().await
    }

    pub async fn session_start(
        &mut self,
        params: SessionStartParams,
    ) -> Result<SessionStartResult> {
        self.core.session_start(params).await
    }

    /// Creates a session through the Native `session/new` application path.
    pub async fn session_new_native(
        &mut self,
        cwd: std::path::PathBuf,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionNewResult> {
        self.core.session_new_native(cwd, idempotency_key).await
    }

    pub async fn session_resume(
        &mut self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult> {
        self.core.session_resume(params).await
    }

    pub async fn session_list(&mut self) -> Result<Vec<SessionMetadata>> {
        self.core.session_list().await
    }

    pub async fn session_delete(
        &mut self,
        params: AcpDeleteSessionParams,
    ) -> Result<AcpDeleteSessionResult> {
        self.core
            .request(devo_protocol::ACP_SESSION_DELETE_METHOD, params)
            .await
    }

    /// Native settings patch; see `client_core::session_settings_update`.
    pub async fn session_settings_update(
        &mut self,
        session_id: devo_protocol::SessionId,
        patch: devo_protocol::native::rpc_session::SessionSettingsPatch,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        self.core.session_settings_update(session_id, patch).await
    }

    pub async fn session_cancel(&mut self, params: AcpCancelParams) -> Result<()> {
        self.core.session_cancel(params).await
    }

    pub async fn mcp_list(
        &mut self,
        params: devo_protocol::native::rpc_admin::McpListParams,
    ) -> Result<devo_protocol::native::rpc_admin::McpListResult> {
        self.core.mcp_list(params).await
    }

    pub async fn mcp_tools(
        &mut self,
        params: devo_protocol::native::rpc_admin::McpToolsParams,
    ) -> Result<devo_protocol::native::rpc_admin::McpToolsResult> {
        self.core.mcp_tools(params).await
    }

    pub async fn mcp_set_enabled(
        &mut self,
        params: devo_protocol::native::rpc_admin::McpSetEnabledParams,
    ) -> Result<devo_protocol::native::rpc_admin::McpSetEnabledResult> {
        self.core.mcp_set_enabled(params).await
    }

    pub async fn provider_list(
        &mut self,
    ) -> Result<devo_protocol::native::rpc_admin::ProviderListResult> {
        self.core.provider_list().await
    }

    pub async fn provider_upsert(
        &mut self,
        params: devo_protocol::native::rpc_admin::ProviderUpsertParams,
    ) -> Result<devo_protocol::native::rpc_admin::ProviderUpsertResult> {
        self.core.provider_upsert(params).await
    }

    pub async fn provider_validate(
        &mut self,
        params: devo_protocol::native::rpc_admin::ProviderValidateParams,
    ) -> Result<devo_protocol::native::rpc_admin::ProviderValidateResult> {
        self.core.provider_validate(params).await
    }

    pub async fn command_exec(&mut self, params: CommandExecParams) -> Result<CommandExecResult> {
        self.core.request("command/exec", params).await
    }

    pub async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        self.core.turn_start(params).await
    }

    /// Starts a turn through the Native `turn/start` application path.
    pub async fn turn_start_native(
        &mut self,
        session_id: SessionId,
        input: Vec<devo_protocol::native::item::UserInput>,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_turn::TurnStartResult> {
        self.core
            .turn_start_native(session_id, input, idempotency_key)
            .await
    }

    /// Native `session/interrupt`; see `client_core::session_interrupt_native`.
    pub async fn session_interrupt_native(
        &mut self,
        scope: devo_protocol::native::rpc_session::SessionInterruptScope,
    ) -> Result<devo_protocol::native::rpc_session::SessionInterruptResult> {
        self.core.session_interrupt_native(scope).await
    }

    pub async fn session_queue_push(
        &mut self,
        params: native::rpc_turn::SessionQueuePushParams,
    ) -> Result<native::rpc_turn::SessionQueuePushResult> {
        self.core.session_queue_push(params).await
    }

    pub async fn session_queue_list(
        &mut self,
        params: native::rpc_turn::SessionQueueListParams,
    ) -> Result<native::rpc_turn::SessionQueueListResult> {
        self.core.session_queue_list(params).await
    }

    pub async fn session_queue_update(
        &mut self,
        params: native::rpc_turn::SessionQueueUpdateParams,
    ) -> Result<native::rpc_turn::SessionQueueUpdateResult> {
        self.core.session_queue_update(params).await
    }

    pub async fn session_queue_remove(
        &mut self,
        params: native::rpc_turn::SessionQueueRemoveParams,
    ) -> Result<native::rpc_turn::SessionQueueRemoveResult> {
        self.core.session_queue_remove(params).await
    }

    pub async fn session_queue_steer(
        &mut self,
        params: native::rpc_turn::SessionQueueSteerParams,
    ) -> Result<native::rpc_turn::SessionQueueSteerResult> {
        self.core.session_queue_steer(params).await
    }

    pub async fn subscription_create(
        &mut self,
        params: native::event::SubscriptionCreateParams,
    ) -> Result<native::event::SubscriptionCreateResult> {
        self.core.subscription_create(params).await
    }

    pub async fn approval_respond(&mut self, params: ApprovalResponseParams) -> Result<()> {
        self.core.approval_respond(params).await
    }

    pub async fn request_user_input_respond(
        &mut self,
        request_id: String,
        response: RequestUserInputResponse,
    ) -> Result<()> {
        self.core
            .request_user_input_respond(request_id, response)
            .await
    }

    pub async fn search_start(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        query: String,
    ) -> Result<devo_protocol::native::rpc_search::SearchSnapshot> {
        self.core.search_start(cwd, query).await
    }

    pub async fn search_update(
        &mut self,
        search_id: ReferenceSearchId,
        query: String,
    ) -> Result<devo_protocol::native::rpc_search::SearchSnapshot> {
        self.core.search_update(search_id, query).await
    }

    pub async fn search_cancel(&mut self, search_id: ReferenceSearchId) -> Result<()> {
        self.core.search_cancel(search_id).await
    }

    pub async fn recv_notification(&mut self) -> Option<ServerNotificationMessage> {
        self.core.recv_notification().await
    }

    pub async fn recv_client_event(&mut self) -> Result<Option<crate::ClientEvent>> {
        self.core.recv_client_event().await
    }

    pub async fn recv_event(&mut self) -> Result<Option<(String, ServerEvent)>> {
        self.core.recv_event().await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.core.shutdown().await;
        match timeout(WEBSOCKET_SHUTDOWN_TIMEOUT, &mut self.writer_task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                tracing::debug!(%error, "websocket writer stopped with error during shutdown");
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "websocket writer task join failed during shutdown");
            }
            Err(_) => {
                self.writer_task.abort();
            }
        }
        self.reader_task.abort();
        let _ = self.reader_task.await;
        Ok(())
    }
}
