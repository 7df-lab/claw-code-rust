//! Stdio transport for a spawned devo server process.
//!
//! Writes newline-delimited JSON to the child stdin and runs a background stdout
//! reader that delegates framing and JSON-RPC routing to [`ServerClientCore`].

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use devo_protocol::*;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Child;
use tokio::process::ChildStderr;
use tokio::process::ChildStdin;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::client_core::ClientWriteMessage;
use crate::client_core::ClientWriter;
use crate::client_core::ServerClientCore;
use crate::protocol_trace::ProtocolTrace;
use crate::protocol_trace::TraceDirection;

pub use crate::client_core::ServerNotificationMessage;

const SERVER_CHILD_STDIN_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);
const SERVER_CHILD_EXIT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub struct StdioServerClientConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
}

pub struct StdioServerClient {
    child: Child,
    core: ServerClientCore,
    reader_task: JoinHandle<()>,
    writer_task: JoinHandle<Result<()>>,
}

impl StdioServerClient {
    pub async fn spawn(config: StdioServerClientConfig) -> Result<Self> {
        tracing::info!(
            program = %config.program.display(),
            "spawning stdio server client"
        );
        let mut command = Command::new(&config.program);
        for arg in config.args {
            command.arg(arg);
        }
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn {}", config.program.display()))?;
        let stdin = child.stdin.take().context("capture server stdin")?;
        let stdout = child.stdout.take().context("capture server stdout")?;
        let stderr = child.stderr.take().context("capture server stderr")?;

        let (client_writer, write_rx) = ClientWriter::channel();
        let core = ServerClientCore::new(
            client_writer,
            AcpClientCapabilities {
                fs: AcpFileSystemCapabilities {
                    read_text_file: false,
                    write_text_file: false,
                    meta: None,
                },
                terminal: false,
                meta: None,
            },
        );
        let reader_state = core.reader_state();
        let stdin = Arc::new(Mutex::new(stdin));
        let trace = ProtocolTrace::from_env();

        let writer_trace = trace.clone();
        let writer_task =
            tokio::spawn(run_stdin_writer(Arc::clone(&stdin), write_rx, writer_trace));
        let reader_trace = trace;
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(ref t) = reader_trace {
                    t.record(TraceDirection::In, &line);
                }
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(message) => reader_state.handle_message(message).await,
                    Err(_) => {
                        tracing::warn!(line = %line, "failed to parse JSON from server stdout");
                    }
                }
            }
            reader_state.finish_reader("stdio").await;
        });
        tokio::spawn(run_stderr_reader(BufReader::new(stderr).lines()));

        Ok(Self {
            child,
            core,
            reader_task,
            writer_task,
        })
    }

    pub async fn initialize(
        &mut self,
        client_capabilities: &AcpClientCapabilities,
    ) -> Result<InitializeResult> {
        tracing::info!("initializing stdio server client");
        self.core
            .set_client_capabilities(client_capabilities.clone());
        let result = self.core.initialize().await?;
        tracing::info!("stdio server client initialized");
        Ok(result)
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
        self.core.agent_spawn(params).await
    }

    pub async fn agent_close(&mut self, params: CloseAgentParams) -> Result<CloseAgentResult> {
        self.core.agent_close(params).await
    }

    pub async fn session_title_update(
        &mut self,
        params: SessionTitleUpdateParams,
    ) -> Result<SessionTitleUpdateResult> {
        self.core.session_title_update(params).await
    }

    pub async fn session_delete(
        &mut self,
        params: AcpDeleteSessionParams,
    ) -> Result<AcpDeleteSessionResult> {
        self.core.session_delete(params).await
    }

    pub async fn session_metadata_update(
        &mut self,
        params: SessionMetadataUpdateParams,
    ) -> Result<SessionMetadataUpdateResult> {
        self.core.session_metadata_update(params).await
    }

    pub async fn session_permissions_update(
        &mut self,
        params: SessionPermissionsUpdateParams,
    ) -> Result<SessionPermissionsUpdateResult> {
        self.core.session_permissions_update(params).await
    }

    pub async fn session_compaction_update(
        &mut self,
        params: SessionCompactionUpdateParams,
    ) -> Result<SessionCompactionUpdateResult> {
        self.core.session_compaction_update(params).await
    }

    pub async fn session_sandbox_profile_update(
        &mut self,
        params: SessionSandboxProfileUpdateParams,
    ) -> Result<SessionSandboxProfileUpdateResult> {
        self.core.session_sandbox_profile_update(params).await
    }

    pub async fn session_compact(
        &mut self,
        params: SessionCompactParams,
    ) -> Result<TurnStartResult> {
        self.core.session_compact(params).await
    }

    pub async fn session_cancel(&mut self, params: AcpCancelParams) -> Result<AcpEmptyResult> {
        self.core.session_cancel(params).await
    }

    pub async fn goal_create(&mut self, params: GoalCreateParams) -> Result<GoalCreateResult> {
        self.core.goal_create(params).await
    }

    pub async fn goal_set(&mut self, params: GoalSetParams) -> Result<GoalSetResult> {
        self.core.goal_set(params).await
    }

    pub async fn goal_status(&mut self, params: GoalStatusParams) -> Result<GoalStatusResult> {
        self.core.goal_status(params).await
    }

    pub async fn goal_pause(&mut self, params: GoalSetStatusParams) -> Result<GoalSetStatusResult> {
        self.core.goal_pause(params).await
    }

    pub async fn goal_resume(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.core.goal_resume(params).await
    }

    pub async fn goal_complete(
        &mut self,
        params: GoalSetStatusParams,
    ) -> Result<GoalSetStatusResult> {
        self.core.goal_complete(params).await
    }

    pub async fn goal_clear(&mut self, params: GoalClearParams) -> Result<GoalClearResult> {
        self.core.goal_clear(params).await
    }

    pub async fn session_fork(&mut self, params: SessionForkParams) -> Result<SessionForkResult> {
        self.core.session_fork(params).await
    }

    pub async fn session_rollback(
        &mut self,
        params: SessionRollbackParams,
    ) -> Result<SessionRollbackResult> {
        self.core.session_rollback(params).await
    }

    pub async fn skills_list(&mut self, params: SkillListParams) -> Result<SkillListResult> {
        self.core.skills_list(params).await
    }

    pub async fn skills_changed(
        &mut self,
        params: SkillChangedParams,
    ) -> Result<SkillChangedResult> {
        self.core.skills_changed(params).await
    }

    pub async fn skills_set_enabled(
        &mut self,
        params: SkillSetEnabledParams,
    ) -> Result<SkillSetEnabledResult> {
        self.core.skills_set_enabled(params).await
    }

    pub async fn mcp_list(
        &mut self,
        params: devo_protocol::canonical::rpc_admin::McpListParams,
    ) -> Result<devo_protocol::canonical::rpc_admin::McpListResult> {
        self.core.mcp_list(params).await
    }

    pub async fn mcp_tools(
        &mut self,
        params: devo_protocol::canonical::rpc_admin::McpToolsParams,
    ) -> Result<devo_protocol::canonical::rpc_admin::McpToolsResult> {
        self.core.mcp_tools(params).await
    }

    pub async fn mcp_set_enabled(
        &mut self,
        params: devo_protocol::canonical::rpc_admin::McpSetEnabledParams,
    ) -> Result<devo_protocol::canonical::rpc_admin::McpSetEnabledResult> {
        self.core.mcp_set_enabled(params).await
    }

    pub async fn model_catalog(
        &mut self,
        params: ModelCatalogParams,
    ) -> Result<ModelCatalogResult> {
        self.core.model_catalog(params).await
    }

    pub async fn model_saved(&mut self, params: ModelSavedParams) -> Result<ModelSavedResult> {
        self.core.model_saved(params).await
    }

    pub async fn provider_vendor_list(
        &mut self,
        params: ProviderVendorListParams,
    ) -> Result<ProviderVendorListResult> {
        self.core.provider_vendor_list(params).await
    }

    pub async fn provider_vendor_upsert(
        &mut self,
        params: ProviderVendorUpsertParams,
    ) -> Result<ProviderVendorUpsertResult> {
        self.core.provider_vendor_upsert(params).await
    }

    pub async fn provider_validate(
        &mut self,
        params: ProviderValidateParams,
    ) -> Result<ProviderValidateResult> {
        self.core.provider_validate(params).await
    }

    pub async fn command_exec(&mut self, params: CommandExecParams) -> Result<CommandExecResult> {
        self.core.command_exec(params).await
    }

    pub async fn command_exec_write(
        &mut self,
        params: CommandExecWriteParams,
    ) -> Result<CommandExecWriteResult> {
        self.core.command_exec_write(params).await
    }

    pub async fn command_exec_resize(
        &mut self,
        params: CommandExecResizeParams,
    ) -> Result<CommandExecResizeResult> {
        self.core.command_exec_resize(params).await
    }

    pub async fn command_exec_terminate(
        &mut self,
        params: CommandExecTerminateParams,
    ) -> Result<CommandExecTerminateResult> {
        self.core.command_exec_terminate(params).await
    }

    pub async fn turn_start(&mut self, params: TurnStartParams) -> Result<TurnStartResult> {
        self.core.turn_start(params).await
    }

    pub async fn turn_shell_command(
        &mut self,
        params: ShellCommandParams,
    ) -> Result<ShellCommandResult> {
        self.core.turn_shell_command(params).await
    }

    pub async fn turn_interrupt(
        &mut self,
        params: TurnInterruptParams,
    ) -> Result<TurnInterruptResult> {
        self.core.turn_interrupt(params).await
    }

    pub async fn session_queue_push(
        &mut self,
        params: canonical::rpc_turn::SessionQueuePushParams,
    ) -> Result<canonical::rpc_turn::SessionQueuePushResult> {
        self.core.session_queue_push(params).await
    }

    pub async fn session_queue_list(
        &mut self,
        params: canonical::rpc_turn::SessionQueueListParams,
    ) -> Result<canonical::rpc_turn::SessionQueueListResult> {
        self.core.session_queue_list(params).await
    }

    pub async fn session_queue_update(
        &mut self,
        params: canonical::rpc_turn::SessionQueueUpdateParams,
    ) -> Result<canonical::rpc_turn::SessionQueueUpdateResult> {
        self.core.session_queue_update(params).await
    }

    pub async fn session_queue_remove(
        &mut self,
        params: canonical::rpc_turn::SessionQueueRemoveParams,
    ) -> Result<canonical::rpc_turn::SessionQueueRemoveResult> {
        self.core.session_queue_remove(params).await
    }

    pub async fn session_queue_steer(
        &mut self,
        params: canonical::rpc_turn::SessionQueueSteerParams,
    ) -> Result<canonical::rpc_turn::SessionQueueSteerResult> {
        self.core.session_queue_steer(params).await
    }

    pub async fn subscription_create(
        &mut self,
        params: canonical::event::SubscriptionCreateParams,
    ) -> Result<canonical::event::SubscriptionCreateResult> {
        self.core.subscription_create(params).await
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
        self.core.reference_search_start(params).await
    }

    pub async fn reference_search_update(
        &mut self,
        params: ReferenceSearchUpdateParams,
    ) -> Result<ReferenceSearchUpdateResult> {
        self.core.reference_search_update(params).await
    }

    pub async fn reference_search_cancel(
        &mut self,
        params: ReferenceSearchCancelParams,
    ) -> Result<ReferenceSearchCancelResult> {
        self.core.reference_search_cancel(params).await
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
        tracing::info!("stdio server client shutdown requested");
        self.core.shutdown().await;
        match timeout(SERVER_CHILD_STDIN_SHUTDOWN_TIMEOUT, &mut self.writer_task).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                tracing::debug!(%error, "stdio writer stopped with error during shutdown");
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "stdio writer task join failed during shutdown");
            }
            Err(_) => {
                self.writer_task.abort();
            }
        }
        tracing::info!("stdio server stdin shutdown attempted");
        if let Err(error) = self.child.start_kill() {
            tracing::debug!(%error, "failed to start stdio server child kill");
        } else {
            tracing::info!("stdio server child kill requested");
        }
        match timeout(SERVER_CHILD_EXIT_TIMEOUT, self.child.wait()).await {
            Ok(Ok(status)) => {
                tracing::info!(?status, "stdio server child exited during shutdown");
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "failed to wait for stdio server child exit");
            }
            Err(_elapsed) => {
                tracing::debug!("timed out waiting for stdio server child exit");
            }
        }
        self.reader_task.abort();
        let _ = self.reader_task.await;
        Ok(())
    }
}

async fn run_stdin_writer(
    stdin: Arc<Mutex<ChildStdin>>,
    mut write_rx: tokio::sync::mpsc::UnboundedReceiver<ClientWriteMessage>,
    trace: Option<ProtocolTrace>,
) -> Result<()> {
    while let Some(message) = write_rx.recv().await {
        match message {
            ClientWriteMessage::Json(value) => {
                write_ndjson_to_stdin(&stdin, &value, trace.as_ref())
                    .await
                    .context("write client payload")?;
            }
            ClientWriteMessage::Close => {
                let _ = timeout(
                    SERVER_CHILD_STDIN_SHUTDOWN_TIMEOUT,
                    stdin.lock().await.shutdown(),
                )
                .await;
                break;
            }
        }
    }
    Ok(())
}

async fn write_ndjson_to_stdin(
    stdin: &Arc<Mutex<ChildStdin>>,
    value: &serde_json::Value,
    trace: Option<&ProtocolTrace>,
) -> Result<()> {
    let mut line = serde_json::to_vec(value).context("serialize client payload")?;
    if let Some(t) = trace
        && let Ok(s) = std::str::from_utf8(&line)
    {
        t.record(TraceDirection::Out, s);
    }
    line.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(&line)
        .await
        .context("write client payload")?;
    stdin.flush().await.context("flush client payload")?;
    Ok(())
}

async fn run_stderr_reader(mut lines: tokio::io::Lines<BufReader<ChildStderr>>) {
    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            tracing::warn!(server_stderr = %trimmed, "server child stderr");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD;
    use crate::ACP_PROMPT_STARTED_NOTIFICATION_METHOD;
    use crate::client_core::ClientWriteMessage;
    use crate::client_core::ClientWriter;
    use crate::client_core::PendingResponses;
    use crate::client_core::ServerClientCore;
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tokio::io::AsyncBufRead;
    use tokio::io::BufReader;
    use tokio::sync::Mutex;
    use tokio::sync::oneshot;
    use tokio::time::Duration;
    use tokio::time::timeout;

    fn default_test_client_capabilities() -> devo_protocol::AcpClientCapabilities {
        devo_protocol::AcpClientCapabilities {
            fs: devo_protocol::AcpFileSystemCapabilities {
                read_text_file: true,
                write_text_file: true,
                meta: None,
            },
            terminal: false,
            meta: None,
        }
    }

    fn test_agent_capabilities_with_session_list() -> AcpAgentCapabilities {
        AcpAgentCapabilities {
            session_capabilities: devo_protocol::AcpSessionCapabilities {
                list: Some(devo_protocol::AcpSessionListCapabilities::default()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    async fn spawn_test_stdio_client(
        child: Child,
        stdin: ChildStdin,
        client_capabilities: devo_protocol::AcpClientCapabilities,
    ) -> (StdioServerClient, PendingResponses) {
        let stdin = Arc::new(Mutex::new(stdin));
        let (client_writer, mut write_rx) = ClientWriter::channel();
        let core = ServerClientCore::new(client_writer, client_capabilities);
        let pending = core.pending_responses();
        tokio::spawn(async move {
            while let Some(message) = write_rx.recv().await {
                match message {
                    ClientWriteMessage::Json(value) => {
                        if write_ndjson_to_stdin(&stdin, &value, None).await.is_err() {
                            break;
                        }
                    }
                    ClientWriteMessage::Close => {
                        let _ = stdin.lock().await.shutdown().await;
                        break;
                    }
                }
            }
        });
        let client = StdioServerClient {
            child,
            core,
            reader_task: tokio::spawn(async {}),
            writer_task: tokio::spawn(async { Ok(()) }),
        };
        (client, pending)
    }

    #[tokio::test]
    async fn initialize_uses_configured_client_capabilities() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let client_capabilities = devo_protocol::AcpClientCapabilities {
            fs: devo_protocol::AcpFileSystemCapabilities {
                read_text_file: true,
                write_text_file: false,
                meta: None,
            },
            terminal: false,
            meta: None,
        };
        let (mut client, pending) =
            spawn_test_stdio_client(child, stdin, client_capabilities.clone()).await;
        let mut stdout_lines = BufReader::new(stdout).lines();
        let expected_capabilities =
            serde_json::to_value(&client_capabilities).expect("serialize client capabilities");

        let initialize = tokio::spawn(async move {
            let result = client.initialize(&client_capabilities).await;
            (result, client)
        });

        let request = read_request_line(&mut stdout_lines).await;
        assert_eq!(request["method"], ACP_INITIALIZE_METHOD);
        assert_eq!(request["params"]["protocolVersion"], serde_json::json!(1));
        assert_eq!(
            request["params"]["clientCapabilities"],
            expected_capabilities
        );
        pending
            .lock()
            .await
            .remove(&1)
            .expect("initialize has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "protocolVersion": 1
                }
            }))
            .expect("send initialize response");

        let (result, mut client) = initialize.await.expect("initialize task joins");
        result.expect("initialize response is accepted");
        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn session_start_accepts_standard_acp_response_without_devo_metadata() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let (mut client, pending) =
            spawn_test_stdio_client(child, stdin, default_test_client_capabilities()).await;
        let cwd = std::env::current_dir().expect("current dir");
        let additional_directory = cwd.join("shared");
        let session_id = devo_protocol::SessionId::new();
        let params = SessionStartParams {
            cwd: cwd.clone(),
            additional_directories: vec![additional_directory.clone()],
            ephemeral: true,
            title: Some("ACP session".to_string()),
            model: Some("test-model".to_string()),
            model_binding_id: Some("binding".to_string()),
        };
        let mut stdout_lines = BufReader::new(stdout).lines();

        let session_start = tokio::spawn(async move {
            let result = client.session_start(params).await;
            (result, client)
        });

        let request = read_request_line(&mut stdout_lines).await;
        assert_eq!(request["method"], ACP_SESSION_NEW_METHOD);
        pending
            .lock()
            .await
            .remove(&1)
            .expect("session/new has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessionId": session_id
                }
            }))
            .expect("send session/new response");

        let (result, mut client) = session_start.await.expect("session_start task joins");
        let session = result
            .expect("standard ACP session/new response is accepted")
            .session;
        let expected = SessionMetadata {
            session_id,
            cwd,
            additional_directories: vec![additional_directory],
            created_at: session.created_at,
            updated_at: session.updated_at,
            last_activity_at: session.updated_at,
            title: Some("ACP session".to_string()),
            title_state: SessionTitleState::Provisional,
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: true,
            model: Some("test-model".to_string()),
            model_binding_id: Some("binding".to_string()),
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
        };
        assert_eq!(session, expected);

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn session_list_accepts_standard_acp_sessions_without_devo_metadata() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let (mut client, pending) =
            spawn_test_stdio_client(child, stdin, default_test_client_capabilities()).await;
        client
            .core
            .set_agent_capabilities_for_test(test_agent_capabilities_with_session_list());
        let cwd = std::env::current_dir().expect("current dir");
        let additional_directory = cwd.join("shared");
        let session_id = devo_protocol::SessionId::new();
        let updated_at = "2026-06-20T00:00:00Z";
        let expected_timestamp = chrono::DateTime::parse_from_rfc3339(updated_at)
            .expect("parse updatedAt")
            .with_timezone(&Utc);
        let mut stdout_lines = BufReader::new(stdout).lines();

        let session_list = tokio::spawn(async move {
            let result = client.session_list().await;
            (result, client)
        });

        let request = read_request_line(&mut stdout_lines).await;
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "session/list",
                "params": {}
            })
        );
        pending
            .lock()
            .await
            .remove(&1)
            .expect("session/list has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessions": [
                        {
                            "sessionId": session_id,
                            "cwd": cwd,
                            "title": "External ACP",
                            "updatedAt": updated_at,
                            "additionalDirectories": [additional_directory]
                        }
                    ]
                }
            }))
            .expect("send session/list response");

        let (result, mut client) = session_list.await.expect("session_list task joins");
        assert_eq!(
            result.expect("standard ACP session/list response is accepted"),
            vec![SessionMetadata {
                session_id,
                cwd,
                additional_directories: vec![additional_directory],
                created_at: expected_timestamp,
                updated_at: expected_timestamp,
                last_activity_at: expected_timestamp,
                title: Some("External ACP".to_string()),
                title_state: SessionTitleState::Provisional,
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
                last_context_occupancy: None,
                status: SessionRuntimeStatus::Idle,
                collaboration_mode: Default::default(),
                effective_context_window: None,
            }]
        );

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn session_resume_accepts_standard_acp_response_without_devo_metadata() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let (mut client, pending) =
            spawn_test_stdio_client(child, stdin, default_test_client_capabilities()).await;
        client
            .core
            .set_agent_capabilities_for_test(test_agent_capabilities_with_session_list());
        let cwd = std::env::current_dir().expect("current dir");
        let additional_directory = cwd.join("shared");
        let session_id = devo_protocol::SessionId::new();
        let updated_at = "2026-06-20T00:00:00Z";
        let expected_timestamp = chrono::DateTime::parse_from_rfc3339(updated_at)
            .expect("parse updatedAt")
            .with_timezone(&Utc);
        let expected_session = SessionMetadata {
            session_id,
            cwd: cwd.clone(),
            additional_directories: vec![additional_directory.clone()],
            created_at: expected_timestamp,
            updated_at: expected_timestamp,
            last_activity_at: expected_timestamp,
            title: Some("External ACP".to_string()),
            title_state: SessionTitleState::Provisional,
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
            last_context_occupancy: None,
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: Default::default(),
            effective_context_window: None,
        };
        let mut stdout_lines = BufReader::new(stdout).lines();

        let session_resume = tokio::spawn(async move {
            let result = client
                .session_resume(SessionResumeParams { session_id })
                .await;
            (result, client)
        });

        let list_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(list_request["method"], ACP_SESSION_LIST_METHOD);
        pending
            .lock()
            .await
            .remove(&1)
            .expect("session/list has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "sessions": [
                        {
                            "sessionId": session_id,
                            "cwd": cwd,
                            "title": "External ACP",
                            "updatedAt": updated_at,
                            "additionalDirectories": [additional_directory]
                        }
                    ]
                }
            }))
            .expect("send session/list response");

        let resume_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(
            resume_request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/resume",
                "params": {
                    "sessionId": session_id,
                    "cwd": cwd,
                    "additionalDirectories": [additional_directory],
                    "mcpServers": []
                }
            })
        );
        pending
            .lock()
            .await
            .remove(&2)
            .expect("session/resume has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {}
            }))
            .expect("send session/resume response");

        let (result, mut client) = session_resume.await.expect("session_resume task joins");
        assert_eq!(
            result.expect("standard ACP session/resume response is accepted"),
            SessionResumeResult {
                session: expected_session,
                latest_turn: None,
                loaded_item_count: 0,
                history_items: Vec::new(),
                pending_texts: Vec::new(),
            }
        );

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn turn_start_sends_devo_extension_with_full_params() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let (mut client, pending) =
            spawn_test_stdio_client(child, stdin, default_test_client_capabilities()).await;
        let params = TurnStartParams {
            session_id: devo_protocol::SessionId::new(),
            input: vec![devo_protocol::InputItem::Text {
                text: "research this".to_string(),
            }],
            model: Some("test-model".to_string()),
            model_binding_id: Some("test-binding".to_string()),
            reasoning_effort_selection: Some("high".to_string()),
            sandbox: Some("workspace-write".to_string()),
            approval_policy: Some("on-request".to_string()),
            cwd: Some(PathBuf::from("workspace")),
            collaboration_mode: devo_protocol::CollaborationMode::Plan,
            execution_mode: devo_protocol::TurnExecutionMode::Regular,
        };
        let expected_params = serde_json::to_value(&params).expect("serialize turn params");
        let mut stdout_lines = BufReader::new(stdout).lines();

        let turn_start = tokio::spawn(async move {
            let result = client.turn_start(params).await;
            (result, client)
        });
        let request_line = timeout(Duration::from_secs(5), stdout_lines.next_line())
            .await
            .expect("read request line before timeout")
            .expect("read request line")
            .expect("request line is present");
        let request =
            serde_json::from_str::<serde_json::Value>(&request_line).expect("request line is JSON");

        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "_devo/turn/start",
                "params": expected_params,
            })
        );

        let response_tx = pending
            .lock()
            .await
            .remove(&1)
            .expect("turn_start has pending response");
        response_tx
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "disposition": "started",
                    "turn_id": devo_protocol::TurnId::new(),
                    "status": "Running",
                    "accepted_at": "2026-06-20T00:00:00Z"
                }
            }))
            .expect("send turn_start response");
        let (result, mut client) = turn_start.await.expect("turn_start task joins");
        result.expect("turn_start response is accepted");

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn turn_start_falls_back_to_acp_prompt_with_lifecycle_notifications() {
        let (child, stdin, stdout) = request_capture_child_for_turn_start_test().await;
        let (mut client, pending) =
            spawn_test_stdio_client(child, stdin, default_test_client_capabilities()).await;
        let session_id = devo_protocol::SessionId::new();
        let params = TurnStartParams {
            session_id,
            input: vec![devo_protocol::InputItem::Text {
                text: "native prompt".to_string(),
            }],
            model: Some("ignored-by-acp".to_string()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: None,
            cwd: None,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            execution_mode: devo_protocol::TurnExecutionMode::Regular,
        };
        let mut stdout_lines = BufReader::new(stdout).lines();

        let turn_start = tokio::spawn(async move {
            let result = client.turn_start(params).await;
            (result, client)
        });

        let first_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(first_request["method"], "_devo/turn/start");
        pending
            .lock()
            .await
            .remove(&1)
            .expect("devo turn/start has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {
                    "code": -32601,
                    "message": "method not found"
                }
            }))
            .expect("send method-not-found response");

        let second_request = read_request_line(&mut stdout_lines).await;
        assert_eq!(
            second_request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [
                        {
                            "type": "text",
                            "text": "native prompt"
                        }
                    ]
                }
            })
        );

        let (result, mut client) = turn_start.await.expect("turn_start task joins");
        result.expect("ACP prompt fallback starts");
        let started = client
            .recv_notification()
            .await
            .expect("ACP prompt started notification");
        assert_eq!(started.method, ACP_PROMPT_STARTED_NOTIFICATION_METHOD);
        assert_eq!(
            started.params,
            serde_json::json!({ "sessionId": session_id })
        );

        pending
            .lock()
            .await
            .remove(&2)
            .expect("ACP prompt has pending response")
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "stopReason": "end_turn"
                }
            }))
            .expect("send ACP prompt response");
        let completed = timeout(Duration::from_secs(5), client.recv_notification())
            .await
            .expect("completed notification before timeout")
            .expect("ACP prompt completed notification");
        assert_eq!(completed.method, ACP_PROMPT_COMPLETED_NOTIFICATION_METHOD);
        assert_eq!(
            completed.params,
            serde_json::json!({
                "sessionId": session_id,
                "stopReason": "end_turn"
            })
        );

        let _ = client.child.start_kill();
        let _ = client.child.wait().await;
    }

    #[tokio::test]
    async fn stdout_reader_drops_pending_responses_when_stdout_closes() {
        let (response_tx, response_rx) = oneshot::channel();
        let request_id = 7;
        let (client_writer, _) = ClientWriter::channel();
        let core = ServerClientCore::new(client_writer, default_test_client_capabilities());
        let pending = core.pending_responses();
        pending.lock().await.insert(request_id, response_tx);
        let (mut child, _stdin) = child_stdin_for_stdout_reader_test().await;

        core.reader_state().finish_reader("stdio-test").await;

        assert!(response_rx.await.is_err());
        assert_eq!(pending.lock().await.len(), 0);

        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    #[cfg(windows)]
    async fn request_capture_child_for_turn_start_test()
    -> (Child, ChildStdin, tokio::process::ChildStdout) {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-Command",
            "for ($i = 0; $i -lt 2; $i++) { $line = [Console]::In.ReadLine(); if ($null -eq $line) { break }; [Console]::Out.WriteLine($line) }; Start-Sleep -Seconds 30",
        ]);
        request_capture_child_for_turn_start_command(command).await
    }

    #[cfg(unix)]
    async fn request_capture_child_for_turn_start_test()
    -> (Child, ChildStdin, tokio::process::ChildStdout) {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 2 ] && IFS= read -r line; do printf '%s\n' \"$line\"; i=$((i + 1)); done; sleep 30",
        ]);
        request_capture_child_for_turn_start_command(command).await
    }

    async fn request_capture_child_for_turn_start_command(
        mut command: Command,
    ) -> (Child, ChildStdin, tokio::process::ChildStdout) {
        command.kill_on_drop(true);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn request capture child");
        let stdin = child.stdin.take().expect("capture child stdin");
        let stdout = child.stdout.take().expect("capture child stdout");
        (child, stdin, stdout)
    }

    async fn read_request_line<R>(stdout_lines: &mut tokio::io::Lines<R>) -> serde_json::Value
    where
        R: AsyncBufRead + Unpin,
    {
        let request_line = timeout(Duration::from_secs(5), stdout_lines.next_line())
            .await
            .expect("read request line before timeout")
            .expect("read request line")
            .expect("request line is present");
        serde_json::from_str::<serde_json::Value>(&request_line).expect("request line is JSON")
    }

    #[cfg(windows)]
    async fn child_stdin_for_stdout_reader_test() -> (Child, Arc<Mutex<ChildStdin>>) {
        let mut command = Command::new("cmd");
        command.args(["/C", "more >NUL"]);
        child_stdin_for_stdout_reader_command(command).await
    }

    #[cfg(unix)]
    async fn child_stdin_for_stdout_reader_test() -> (Child, Arc<Mutex<ChildStdin>>) {
        let mut command = Command::new("sh");
        command.args(["-c", "cat >/dev/null"]);
        child_stdin_for_stdout_reader_command(command).await
    }

    async fn child_stdin_for_stdout_reader_command(
        mut command: Command,
    ) -> (Child, Arc<Mutex<ChildStdin>>) {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn child for stdin");
        let stdin = child.stdin.take().expect("capture child stdin");
        (child, Arc::new(Mutex::new(stdin)))
    }
}
