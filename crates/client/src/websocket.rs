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
        let core = ServerClientCore::new(client_writer, config.client_capabilities);
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

    pub async fn session_resume(
        &mut self,
        params: SessionResumeParams,
    ) -> Result<SessionResumeResult> {
        self.core.session_resume(params).await
    }

    pub async fn session_list(&mut self) -> Result<Vec<SessionMetadata>> {
        self.core.session_list().await
    }

    pub async fn agent_list(&mut self, params: AgentListParams) -> Result<AgentListResult> {
        self.core.agent_list(params).await
    }

    pub async fn agent_spawn(&mut self, params: SpawnAgentParams) -> Result<SpawnAgentResult> {
        self.core.request_devo("agent/spawn", params).await
    }

    pub async fn agent_close(&mut self, params: CloseAgentParams) -> Result<CloseAgentResult> {
        self.core.request_devo("agent/close", params).await
    }

    pub async fn session_title_update(
        &mut self,
        params: SessionTitleUpdateParams,
    ) -> Result<SessionTitleUpdateResult> {
        self.core.request_devo("session/title/update", params).await
    }

    pub async fn session_delete(
        &mut self,
        params: AcpDeleteSessionParams,
    ) -> Result<AcpDeleteSessionResult> {
        self.core
            .request(devo_protocol::ACP_SESSION_DELETE_METHOD, params)
            .await
    }

    pub async fn session_metadata_update(
        &mut self,
        params: SessionMetadataUpdateParams,
    ) -> Result<SessionMetadataUpdateResult> {
        self.core
            .request_devo("session/metadata/update", params)
            .await
    }

    pub async fn session_permissions_update(
        &mut self,
        params: SessionPermissionsUpdateParams,
    ) -> Result<SessionPermissionsUpdateResult> {
        self.core
            .request_devo("session/permissions/update", params)
            .await
    }

    pub async fn session_compaction_update(
        &mut self,
        params: SessionCompactionUpdateParams,
    ) -> Result<SessionCompactionUpdateResult> {
        self.core
            .request_devo("session/compaction/update", params)
            .await
    }

    pub async fn session_sandbox_profile_update(
        &mut self,
        params: SessionSandboxProfileUpdateParams,
    ) -> Result<SessionSandboxProfileUpdateResult> {
        self.core
            .request_devo("session/sandbox_profile/update", params)
            .await
    }

    pub async fn session_compact(
        &mut self,
        params: SessionCompactParams,
    ) -> Result<TurnStartResult> {
        self.core.request_devo("session/compact", params).await
    }

    pub async fn session_cancel(&mut self, params: AcpCancelParams) -> Result<AcpEmptyResult> {
        self.core
            .request(devo_protocol::ACP_SESSION_CANCEL_METHOD, params)
            .await
    }

    pub async fn goal_create(&mut self, params: GoalCreateParams) -> Result<GoalCreateResult> {
        self.core.request_devo("goal/create", params).await
    }

    pub async fn goal_set(&mut self, params: GoalSetParams) -> Result<GoalSetResult> {
        self.core.request_devo("goal/set", params).await
    }

    pub async fn goal_status(&mut self, params: GoalStatusParams) -> Result<GoalStatusResult> {
        self.core.request_devo("goal/status", params).await
    }

    pub async fn goal_pause(&mut self, params: GoalSetStatusParams) -> Result<GoalSetStatusResult> {
        self.core.request_devo("goal/pause", params).await
    }

    pub async fn goal_resume(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.core.request_devo("goal/resume", params).await
    }

    pub async fn goal_complete(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.core.request_devo("goal/complete", params).await
    }

    pub async fn goal_clear(&mut self, params: GoalClearParams) -> Result<GoalClearResult> {
        self.core.request_devo("goal/clear", params).await
    }

    pub async fn session_fork(&mut self, params: SessionForkParams) -> Result<SessionForkResult> {
        self.core.request_devo("session/fork", params).await
    }

    pub async fn session_rollback(
        &mut self,
        params: SessionRollbackParams,
    ) -> Result<SessionRollbackResult> {
        self.core.request_devo("session/rollback", params).await
    }

    pub async fn skills_list(&mut self, params: SkillListParams) -> Result<SkillListResult> {
        self.core.request_devo("skills/list", params).await
    }

    pub async fn skills_changed(
        &mut self,
        params: SkillChangedParams,
    ) -> Result<SkillChangedResult> {
        self.core.request_devo("skills/changed", params).await
    }

    pub async fn skills_set_enabled(
        &mut self,
        params: SkillSetEnabledParams,
    ) -> Result<SkillSetEnabledResult> {
        self.core.request_devo("skills/set_enabled", params).await
    }

    pub async fn mcp_list(
        &mut self,
        params: devo_protocol::canonical::rpc_admin::McpListParams,
    ) -> Result<devo_protocol::canonical::rpc_admin::McpListResult> {
        self.core.request_devo("mcp/list", params).await
    }

    pub async fn mcp_tools(
        &mut self,
        params: devo_protocol::canonical::rpc_admin::McpToolsParams,
    ) -> Result<devo_protocol::canonical::rpc_admin::McpToolsResult> {
        self.core.request_devo("mcp/tools", params).await
    }

    pub async fn mcp_set_enabled(
        &mut self,
        params: devo_protocol::canonical::rpc_admin::McpSetEnabledParams,
    ) -> Result<devo_protocol::canonical::rpc_admin::McpSetEnabledResult> {
        self.core.request_devo("mcp/set_enabled", params).await
    }

    pub async fn model_catalog(
        &mut self,
        params: ModelCatalogParams,
    ) -> Result<ModelCatalogResult> {
        self.core.request_devo("model/catalog", params).await
    }

    pub async fn model_saved(&mut self, params: ModelSavedParams) -> Result<ModelSavedResult> {
        self.core.request_devo("model/saved", params).await
    }

    pub async fn provider_vendor_list(
        &mut self,
        params: ProviderVendorListParams,
    ) -> Result<ProviderVendorListResult> {
        self.core.request_devo("provider/list", params).await
    }

    pub async fn provider_vendor_upsert(
        &mut self,
        params: ProviderVendorUpsertParams,
    ) -> Result<ProviderVendorUpsertResult> {
        self.core.request_devo("provider/upsert", params).await
    }

    pub async fn provider_validate(
        &mut self,
        params: ProviderValidateParams,
    ) -> Result<ProviderValidateResult> {
        self.core.request_devo("provider/validate", params).await
    }

    pub async fn command_exec(&mut self, params: CommandExecParams) -> Result<CommandExecResult> {
        self.core.request_devo("command/exec", params).await
    }

    pub async fn command_exec_write(
        &mut self,
        params: CommandExecWriteParams,
    ) -> Result<CommandExecWriteResult> {
        self.core.request_devo("command/exec/write", params).await
    }

    pub async fn command_exec_resize(
        &mut self,
        params: CommandExecResizeParams,
    ) -> Result<CommandExecResizeResult> {
        self.core.request_devo("command/exec/resize", params).await
    }

    pub async fn command_exec_terminate(
        &mut self,
        params: CommandExecTerminateParams,
    ) -> Result<CommandExecTerminateResult> {
        self.core
            .request_devo("command/exec/terminate", params)
            .await
    }

    pub async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        self.core.turn_start(params).await
    }

    pub async fn turn_shell_command(
        &mut self,
        params: ShellCommandParams,
    ) -> Result<ShellCommandResult> {
        self.core.request_devo("turn/shell_command", params).await
    }

    pub async fn turn_interrupt(
        &mut self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResult> {
        self.core.request_devo("turn/interrupt", params).await
    }

    pub async fn turn_steer(&mut self, params: TurnSteerParams) -> Result<TurnSteerResult> {
        self.core.request_devo("turn/steer", params).await
    }

    pub async fn approval_respond(&mut self, params: ApprovalResponseParams) -> Result<()> {
        self.core.approval_respond(params).await
    }

    pub async fn request_user_input_respond(
        &mut self,
        params: RequestUserInputRespondParams,
    ) -> Result<()> {
        self.core.request_user_input_respond(params).await
    }

    pub async fn reference_search_start(
        &mut self,
        params: ReferenceSearchStartParams,
    ) -> Result<ReferenceSearchStartResult> {
        self.core.request_devo("search/start", params).await
    }

    pub async fn reference_search_update(
        &mut self,
        params: ReferenceSearchUpdateParams,
    ) -> Result<ReferenceSearchUpdateResult> {
        self.core.request_devo("search/update", params).await
    }

    pub async fn reference_search_cancel(
        &mut self,
        params: ReferenceSearchCancelParams,
    ) -> Result<ReferenceSearchCancelResult> {
        self.core.request_devo("search/cancel", params).await
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
