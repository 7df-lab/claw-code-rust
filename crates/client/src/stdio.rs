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
    /// Opt into native typed item events (L2-DES-APP-009); only consumers
    /// that handle typed shapes may set this.
    pub typed_items: bool,
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
        let mut core = ServerClientCore::new(
            client_writer,
            AcpClientCapabilities {
                fs: AcpFileSystemCapabilities {
                    read_text_file: false,
                    write_text_file: false,
                    meta: None,
                },
                terminal: false,
                session: None,
                meta: None,
            },
        );
        core.set_typed_items_opt_in(config.typed_items);
        // The stdio client (TUI/CLI) speaks the Native session surface;
        // declare it so colliding method names route to Native handlers
        // (L2-DES-APP-009 DD-6).
        core.set_native_protocol_opt_in(true);
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

    /// Native settings patch; see `client_core::session_settings_update`.
    pub async fn session_settings_update(
        &mut self,
        session_id: SessionId,
        patch: devo_protocol::native::rpc_session::SessionSettingsPatch,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        self.core.session_settings_update(session_id, patch).await
    }

    /// Native model/metadata update; see `client_core::session_model_update`.
    pub async fn session_model_update(
        &mut self,
        session_id: SessionId,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        collaboration_mode: Option<devo_protocol::CollaborationMode>,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        self.core
            .session_model_update(
                session_id,
                model,
                model_binding_id,
                reasoning_effort_selection,
                collaboration_mode,
            )
            .await
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
        self.core.command_exec(params).await
    }

    /// Native `turn/start`; see `client_core::turn_start_native`.
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

    pub async fn turn_read_native(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Result<devo_protocol::native::rpc_turn::TurnReadResult> {
        self.core.turn_read_native(session_id, turn_id).await
    }

    pub async fn turn_items_list_native(
        &mut self,
        session_id: SessionId,
        turn_id: TurnId,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope>> {
        self.core
            .turn_items_list_native(session_id, turn_id, cursor, limit)
            .await
    }

    /// Native `session/compact/start`; see
    /// `client_core::session_compact_start_native`.
    pub async fn session_compact_start_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_turn::TurnStartResult> {
        self.core.session_compact_start_native(session_id).await
    }

    /// Native `session/interrupt`; see `client_core::session_interrupt_native`.
    pub async fn session_interrupt_native(
        &mut self,
        scope: devo_protocol::native::rpc_session::SessionInterruptScope,
    ) -> Result<devo_protocol::native::rpc_session::SessionInterruptResult> {
        self.core.session_interrupt_native(scope).await
    }

    /// Native `task/start` kind=process; see
    /// `client_core::task_start_process_native`.
    pub async fn task_start_process_native(
        &mut self,
        session_id: SessionId,
        command: String,
        cwd: Option<PathBuf>,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_turn::TaskStartResult> {
        self.core
            .task_start_process_native(session_id, command, cwd, idempotency_key)
            .await
    }

    /// Native `agent/list`; see `client_core::agent_list_native`.
    pub async fn agent_list_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_turn::AgentListResult> {
        self.core.agent_list_native(session_id).await
    }

    /// Native `agent/cancel`; see `client_core::agent_cancel_native`.
    pub async fn agent_cancel_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
    ) -> Result<()> {
        self.core.agent_cancel_native(item_id).await
    }

    /// Native `agent/message`; see `client_core::agent_message_native`.
    pub async fn agent_message_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
        input: Vec<devo_protocol::native::item::UserInput>,
    ) -> Result<()> {
        self.core.agent_message_native(item_id, input).await
    }

    /// Native `agent/read`; see `client_core::agent_read_native`.
    pub async fn agent_read_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
    ) -> Result<devo_protocol::native::rpc_turn::AgentReadResult> {
        self.core.agent_read_native(item_id).await
    }

    /// Native `task/interrupt`; see `client_core::task_interrupt_native`.
    pub async fn task_interrupt_native(
        &mut self,
        item_id: &devo_protocol::native::ids::ItemId,
    ) -> Result<()> {
        self.core.task_interrupt_native(item_id).await
    }

    /// Native `task/start` kind=agent; see
    /// `client_core::task_start_agent_native`.
    pub async fn task_start_agent_native(
        &mut self,
        params: devo_protocol::native::rpc_turn::TaskStartParams,
    ) -> Result<devo_protocol::native::rpc_turn::TaskStartResult> {
        self.core.task_start_agent_native(params).await
    }

    /// Native `session/new`; see `client_core::session_new_native`.
    pub async fn session_new_native(
        &mut self,
        cwd: PathBuf,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionNewResult> {
        self.core.session_new_native(cwd, idempotency_key).await
    }

    /// Native `session/rollback/preview`; see
    /// `client_core::session_rollback_preview_native`.
    pub async fn session_rollback_preview_native(
        &mut self,
        session_id: SessionId,
        user_turn_index: u32,
        mode: devo_protocol::native::rpc_session::RollbackMode,
    ) -> Result<devo_protocol::native::rpc_session::RestorePlan> {
        self.core
            .session_rollback_preview_native(session_id, user_turn_index, mode)
            .await
    }

    /// Native `session/rollback/commit`; see
    /// `client_core::session_rollback_commit_native`.
    pub async fn session_rollback_commit_native(
        &mut self,
        restore_plan_id: devo_protocol::native::ids::RestorePlanId,
        expected_workspace_version: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionRollbackCommitResult> {
        self.core
            .session_rollback_commit_native(restore_plan_id, expected_workspace_version)
            .await
    }

    /// Native `model/list`; see `client_core::model_list_native`.
    pub async fn model_list_native(
        &mut self,
    ) -> Result<devo_protocol::native::rpc_admin::ModelListResult> {
        self.core.model_list_native().await
    }

    /// Native `skill/list` (ratified #4): workspace-scoped listing.
    pub async fn skill_list_native(
        &mut self,
        cwd: Option<PathBuf>,
        force_reload: bool,
    ) -> Result<devo_protocol::native::rpc_admin::SkillListResult> {
        self.core
            .request(
                "skill/list",
                devo_protocol::native::rpc_admin::SkillListParams { cwd, force_reload },
            )
            .await
    }

    /// Native `skill/set_enabled` (ratified #4): keyed by path.
    pub async fn skill_set_enabled_native(
        &mut self,
        path: PathBuf,
        enabled: bool,
    ) -> Result<devo_protocol::native::rpc_admin::SkillSetEnabledResult> {
        self.core
            .request(
                "skill/set_enabled",
                devo_protocol::native::rpc_admin::SkillSetEnabledParams {
                    path,
                    enabled,
                    cwd: None,
                },
            )
            .await
    }

    /// Native `session/list`; see `client_core::session_list_native`.
    pub async fn session_list_native(
        &mut self,
        params: devo_protocol::native::rpc_session::SessionListParams,
    ) -> Result<devo_protocol::native::rpc_session::SessionListResult> {
        self.core.session_list_native(params).await
    }

    /// Native `session/delete`; see `client_core::session_delete_native`.
    pub async fn session_delete_native(&mut self, session_id: SessionId) -> Result<()> {
        self.core.session_delete_native(session_id).await
    }

    /// Native `session/resume`; see `client_core::session_resume_native`.
    pub async fn session_resume_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_session::SessionResumeResult> {
        self.core.session_resume_native(session_id).await
    }

    /// Native `session/goal/set`; see `client_core::session_goal_set_native`.
    pub async fn session_goal_set_native(
        &mut self,
        session_id: SessionId,
        objective: String,
        token_budget: Option<u64>,
        if_exists: devo_protocol::native::rpc_session::GoalIfExists,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionGoalSetResult> {
        self.core
            .session_goal_set_native(
                session_id,
                objective,
                token_budget,
                if_exists,
                idempotency_key,
            )
            .await
    }

    /// Native `session/goal/update`; see `client_core::session_goal_update_native`.
    pub async fn session_goal_update_native(
        &mut self,
        session_id: SessionId,
        patch: devo_protocol::native::rpc_session::GoalPatch,
        idempotency_key: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionGoalUpdateResult> {
        self.core
            .session_goal_update_native(session_id, patch, idempotency_key)
            .await
    }

    /// Native `session/goal/read`; see `client_core::session_goal_read_native`.
    pub async fn session_goal_read_native(
        &mut self,
        session_id: SessionId,
    ) -> Result<devo_protocol::native::rpc_session::SessionGoalReadResult> {
        self.core.session_goal_read_native(session_id).await
    }

    /// Native `session/items/list`; see
    /// `client_core::session_items_list_native`.
    pub async fn session_items_list_native(
        &mut self,
        session_id: SessionId,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope>> {
        self.core
            .session_items_list_native(session_id, cursor, limit)
            .await
    }

    /// Native `session/turns/list`; see
    /// `client_core::session_turns_list_native`.
    pub async fn session_turns_list_native(
        &mut self,
        session_id: SessionId,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<devo_protocol::native::page::Page<devo_protocol::native::turn::Turn>> {
        self.core
            .session_turns_list_native(session_id, cursor, limit)
            .await
    }

    /// Native `session/fork`; see `client_core::session_fork_native`.
    pub async fn session_fork_native(
        &mut self,
        session_id: SessionId,
        at_turn_id: Option<TurnId>,
    ) -> Result<devo_protocol::native::rpc_session::SessionForkResult> {
        self.core.session_fork_native(session_id, at_turn_id).await
    }

    /// Native `session/fork` with cut mode; see
    /// `client_core::session_fork_native_with_cut`.
    pub async fn session_fork_native_with_cut(
        &mut self,
        session_id: SessionId,
        at_turn_id: Option<TurnId>,
        cut: Option<devo_protocol::native::rpc_session::SessionForkCut>,
    ) -> Result<devo_protocol::native::rpc_session::SessionForkResult> {
        self.core
            .session_fork_native_with_cut(session_id, at_turn_id, cut)
            .await
    }

    /// Native session title rename; see
    /// `client_core::session_title_update_native`.
    pub async fn session_title_update_native(
        &mut self,
        session_id: SessionId,
        title: String,
    ) -> Result<devo_protocol::native::rpc_session::SessionMetadataUpdateResult> {
        self.core
            .session_title_update_native(session_id, title)
            .await
    }

    /// Native goal lifecycle transition; see
    /// `client_core::session_goal_transition_native`.
    pub async fn session_goal_transition_native(
        &mut self,
        session_id: SessionId,
        expected_goal_id: &devo_protocol::native::ids::GoalId,
        transition: crate::GoalLifecycleTransition,
    ) -> Result<Option<devo_protocol::native::goal::Goal>> {
        self.core
            .session_goal_transition_native(session_id, expected_goal_id, transition)
            .await
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

    /// Native `search/start`.
    pub async fn search_start(
        &mut self,
        cwd: Option<PathBuf>,
        query: String,
    ) -> Result<devo_protocol::native::rpc_search::SearchSnapshot> {
        self.core.search_start(cwd, query).await
    }

    /// Native `search/update`.
    pub async fn search_update(
        &mut self,
        search_id: ReferenceSearchId,
        query: String,
    ) -> Result<devo_protocol::native::rpc_search::SearchSnapshot> {
        self.core.search_update(search_id, query).await
    }

    /// Native `search/cancel`.
    pub async fn search_cancel(&mut self, search_id: ReferenceSearchId) -> Result<()> {
        self.core.search_cancel(search_id).await
    }

    pub async fn recv_notification(&mut self) -> Option<ServerNotificationMessage> {
        self.core.recv_notification().await
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
    use crate::client_core::ClientWriteMessage;
    use crate::client_core::ClientWriter;
    use crate::client_core::PendingResponses;
    use crate::client_core::ServerClientCore;
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
                read_text_file: false,
                write_text_file: false,
                meta: None,
            },
            terminal: false,
            session: None,
            meta: None,
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
            session: None,
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
