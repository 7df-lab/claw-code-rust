use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio::task::JoinError;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use devo_core::PermissionPreset;
use devo_core::ProviderWireApi;
use devo_core::ReasoningEffort;
use devo_core::SessionId;
use devo_core::TurnId;
use devo_protocol::AgentToolPolicy;
use devo_protocol::CommandExecParams;
use devo_protocol::CommandExecProgram;
use devo_protocol::ProviderModelBinding;
use devo_protocol::ProviderVendor;
use devo_protocol::ReferenceSearchId;
use devo_protocol::ReferenceSearchSnapshot;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::SessionPlanStepStatus;
use devo_protocol::SpawnAgentParams;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::native::rpc_session::RollbackMode;
use devo_server::ApprovalResponseParams;
use devo_server::CollaborationMode;
use devo_server::InputItem;
#[cfg(test)]
use devo_server::ItemEventPayload;
use devo_server::SessionHistoryItem;
use devo_server::SessionHistoryItemKind;
use devo_server::SkillSource;
use devo_server::StdioServerClient;
use devo_server::StdioServerClientConfig;

use crate::app_command::GoalObjectiveMode;
use crate::app_command::InputHistoryDirection;
use crate::bottom_pane::SkillInterfaceMetadata;
use crate::bottom_pane::SkillMetadata;
use crate::events::PlanStep;
use crate::events::PlanStepStatus;
use crate::events::SessionListEntry;
use crate::events::SessionPreviewMessage;
use crate::events::SessionPreviewRole;
use crate::events::SubagentMonitorAgent;
use crate::events::SubagentMonitorEvent;
use crate::events::TextItemKind;
use crate::events::TranscriptItem;
use crate::events::TranscriptItemKind;
use crate::events::WorkerEvent;
use crate::transcript::lifecycle::ItemLifecycleEvent;

mod approval_items;
mod compaction_items;
mod goals;
mod history;
mod item_dispatch;
mod native_items;
mod plan_items;
mod session_preview;
mod session_restore;
mod skills;
mod subagent_events;
mod tool_lifecycle;
mod tool_summaries;
mod typed_events;

#[cfg(test)]
pub(crate) use tool_summaries::exploration_actions_from_tool_input;

use session_restore::native_session_id;
use session_restore::restore_session_native;
use session_restore::session_switched_event_from_restore;

const WORKER_SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
const WORKER_ABORT_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

fn active_agent_label_from_session(session: &devo_server::SessionMetadata) -> Option<String> {
    session
        .agent_nickname
        .as_ref()
        .or(session.agent_path.as_ref())
        .map(|label| format!("Agent: {label}"))
}

/// Prefer exact persisted latest-query usage, then the replayed prompt estimate.
/// Aggregate turn usage and the legacy scalar are intentionally excluded because
/// neither identifies the latest model query reliably for historical sessions.
fn last_query_tokens_from_resume(session: &devo_server::SessionMetadata) -> (usize, usize) {
    if let Some(usage) = session.last_query_usage.as_ref() {
        return (usage.display_total_tokens(), usage.input_tokens as usize);
    }
    if session.prompt_token_estimate > 0 {
        return (session.prompt_token_estimate, session.prompt_token_estimate);
    }
    (0, 0)
}

struct EnsureSessionOutcome {
    session_id: SessionId,
    model: Option<String>,
    model_binding_id: Option<String>,
    reasoning_effort_selection: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    created: bool,
}

fn should_apply_terminal_turn_usage_fallback(
    saw_usage_update_for_turn: bool,
    has_authoritative_usage_totals: bool,
) -> bool {
    !saw_usage_update_for_turn && !has_authoritative_usage_totals
}

async fn reconcile_idle_turn(
    client: &mut StdioServerClient,
    session_id: SessionId,
    turn_id: TurnId,
    seen_terminal_item_ids: &mut HashSet<String>,
    seen_terminal_call_ids: &mut HashSet<String>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<devo_protocol::native::turn::Turn> {
    let turn = client.turn_read_native(session_id, turn_id).await?.turn;
    if turn.status == devo_protocol::native::turn::TurnStatus::InProgress {
        anyhow::bail!("server still reports turn {turn_id} in progress");
    }

    let mut cursor = None;
    loop {
        let page = client
            .turn_items_list_native(session_id, turn_id, cursor.clone(), Some(200))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        for envelope in page.data {
            if !matches!(
                envelope.state,
                devo_protocol::native::item::ItemState::Completed
                    | devo_protocol::native::item::ItemState::Failed
                    | devo_protocol::native::item::ItemState::Interrupted
                    | devo_protocol::native::item::ItemState::Lost
            ) {
                continue;
            }
            let call_id = match &envelope.item {
                devo_protocol::native::item::Item::ToolResult { call_id, .. }
                | devo_protocol::native::item::Item::CommandExecution { call_id, .. }
                | devo_protocol::native::item::Item::FileChange { call_id, .. } => {
                    Some(call_id.clone())
                }
                _ => None,
            };
            let Some(call_id) = call_id else {
                continue;
            };
            let item_id = envelope.id.to_string();
            if seen_terminal_item_ids.contains(&item_id)
                || seen_terminal_call_ids.contains(&call_id)
            {
                continue;
            }
            let legacy_item_id = devo_core::ItemId::try_from(item_id.as_str())?;
            for event in native_items::completed_events(&envelope.item, legacy_item_id) {
                let _ = event_tx.send(WorkerEvent::Transcript(event));
            }
            seen_terminal_item_ids.insert(item_id);
            seen_terminal_call_ids.insert(call_id);
        }
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }
    Ok(turn)
}

/// Spawn discovery from a typed `item/completed` ToolResult (L2-DES-APP-009
/// cutover): the typed item carries the same raw output the ACP tool-call
/// path parsed, so discovery no longer depends on the ACP envelope.
async fn maybe_discover_spawned_subagent_from_tool_output(
    raw_output: Option<&serde_json::Value>,
    client: &mut StdioServerClient,
    parent_session_id: SessionId,
    child_agent_sessions: &mut HashSet<SessionId>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    let Some(spawn_result) = subagent_events::spawn_agent_result_from_raw_output(raw_output) else {
        return;
    };
    maybe_discover_spawned_subagent(
        spawn_result,
        None,
        client,
        parent_session_id,
        child_agent_sessions,
        event_tx,
    )
    .await;
}

async fn maybe_discover_spawned_subagent(
    spawn_result: devo_protocol::SpawnAgentResult,
    last_task_message: Option<String>,
    client: &mut StdioServerClient,
    parent_session_id: SessionId,
    child_agent_sessions: &mut HashSet<SessionId>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    let child_session_id = spawn_result.child_session_id;
    let child_session_id_string = child_session_id.to_string();
    if child_agent_sessions.contains(&child_session_id) {
        // Child may already be registered from session_info_update; still hydrate
        // status and last_task_message from agent/list when spawn completes.
    }

    let listed_agent = match client.agent_list_native(parent_session_id).await {
        Ok(result) => result
            .agents
            .iter()
            .find(|item| {
                matches!(
                    &item.item,
                    devo_protocol::native::item::Item::SubAgent {
                        agent_session_id, ..
                    } if agent_session_id.as_str() == child_session_id_string
                )
            })
            .and_then(subagent_events::agent_from_native_subagent),
        Err(error) => {
            tracing::debug!(
                %error,
                %parent_session_id,
                %child_session_id,
                "failed to hydrate spawned subagent from agent/list"
            );
            None
        }
    };

    let agent = listed_agent.unwrap_or(SubagentMonitorAgent {
        session_id: child_session_id,
        parent_session_id,
        agent_path: spawn_result.agent_path,
        nickname: spawn_result.agent_nickname,
        role: "default".to_string(),
        status: spawn_result.status,
        last_task_message,
    });
    child_agent_sessions.insert(agent.session_id);
    let _ = event_tx.send(WorkerEvent::SubagentDiscovered { agent });
}

/// Immutable runtime configuration used to construct the background server client worker.
pub(crate) struct QueryWorkerConfig {
    /// Optional pre-existing session to resume immediately on startup.
    pub(crate) initial_session_id: Option<SessionId>,
    /// Model identifier used for new turns.
    pub(crate) model: String,
    /// Stable provider model binding id used for new turns, when available.
    pub(crate) model_binding_id: Option<String>,
    /// Working directory used for the server session.
    pub(crate) cwd: PathBuf,
    /// Optional log-level override forwarded to the server child process.
    pub(crate) server_log_level: Option<String>,
    /// Initial reasoning effort selection used for new turns.
    pub(crate) reasoning_effort_selection: Option<String>,
    /// Permission preset to apply to the server session when it exists.
    pub(crate) permission_preset: PermissionPreset,
    /// Default collaboration mode for newly prepared sessions.
    pub(crate) default_collaboration_mode: CollaborationMode,
    /// Initial OS sandbox profile from project config (or permission-implied).
    pub(crate) initial_sandbox_profile: Option<String>,
    /// Agent client capabilities to advertise to the server session.
    pub(crate) client_capabilities: devo_protocol::AcpClientCapabilities,
}

/// TODO: Should we extract the OperationCommand to the `protocol` crate? Since it can be shareable.
/// Commands accepted by the background query worker.
enum OperationCommand {
    /// Submit a new user prompt to the session.
    SubmitInput {
        input: Vec<InputItem>,
        approval_policy: Option<String>,
        collaboration_mode: CollaborationMode,
    },
    ExecuteShellCommand {
        command: String,
    },
    SubmitShellInput {
        command: String,
    },
    /// Update the model used for future turns.
    /// TODO: Model should be bind at Session Metadata, not turn, indicate to the model utilized to generate
    /// at next turn. However, we can still bind a model at turn, to indicate what model is utlized generated.
    /// User can change session metadata model to decide what the next turn model is utlized.
    SetModel {
        model: String,
        model_binding_id: Option<String>,
        persist_scope: crate::app_command::PersistScope,
    },
    SetCollaborationMode {
        collaboration_mode: CollaborationMode,
        persist_scope: crate::app_command::PersistScope,
    },
    /// TODO: Same with model, should bind at session metadata.
    /// Update the reasoning effort selection used for future turns.
    SetReasoningEffort(Option<String>),
    /// Replace the provider connection settings and restart the server client.
    ReconfigureProvider {
        /// Provider wire protocol to use for future turns.
        wire_api: ProviderWireApi,
        /// Model identifier to use for future turns.
        model: String,
        /// Optional provider base URL override.
        base_url: Option<String>,
        /// Optional provider API key override.
        api_key: Option<String>,
    },
    /// Validates provider settings with a temporary probe request.
    ValidateProvider {
        provider_vendor: ProviderVendor,
        model_binding: ProviderModelBinding,
        api_key: Option<String>,
    },
    /// Request configured provider vendors from the server.
    ListProviderVendors,
    /// Add or update one provider vendor through the server.
    UpsertProviderVendor {
        provider_vendor: ProviderVendor,
        model_binding: Option<ProviderModelBinding>,
        default_model_binding: Option<String>,
        api_key: Option<String>,
    },
    /// Request a session list from the server.
    ListSessions,
    /// Load a compact conversation preview for one persisted session.
    PreviewSession(SessionId),
    /// Request a skills list from the server.
    ListSkills,
    /// Request MCP server runtime statuses from the server.
    ListMcpServers,
    /// Request tools for one MCP server from the server.
    ListMcpTools {
        name: String,
    },
    /// Request or update a server-backed composer reference search.
    ReferenceSearchRequested {
        query: String,
    },
    /// Cancel the active composer reference search session.
    ReferenceSearchCancelled,
    /// Persistently enable or disable one skill by canonical `SKILL.md` path.
    SetSkillEnabled {
        path: PathBuf,
        enabled: bool,
    },
    /// Persistently enable or disable one MCP server and apply it live.
    SetMcpServerEnabled {
        name: String,
        enabled: bool,
    },
    /// Request proactive compaction for the active session.
    CompactSession,
    /// Show the current goal for the active session.
    ShowGoal,
    /// Open the current goal in the editor.
    EditGoal,
    /// Create or update the current goal objective.
    SetGoalObjective {
        objective: String,
        mode: GoalObjectiveMode,
    },
    /// Pause, resume, or complete the current goal.
    SetGoalStatus {
        status: ThreadGoalStatus,
    },
    /// Clear the current goal.
    ClearGoal,
    /// Clear the active session so the next prompt starts a fresh one lazily.
    StartNewSession,
    /// Switch the active session to a persisted session identifier.
    SwitchSession(SessionId),
    /// Rename the current active session.
    RenameSession(String),
    /// Rename a persisted session without switching to it.
    RenameSessionById {
        session_id: SessionId,
        title: String,
    },
    /// Delete a session. `None` deletes the current active session and starts a
    /// fresh local session; `Some(id)` deletes that session id (and only resets
    /// local state when it is the active one).
    DeleteSession {
        session_id: Option<SessionId>,
    },
    /// Roll back the active session using the server-selected user-turn cut mode.
    RollbackUserTurn {
        user_turn_index: u32,
        mode: RollbackMode,
    },
    /// Fork a new session at a selected user turn.
    ForkAtUserTurn {
        user_turn_index: u32,
        cut: devo_protocol::native::rpc_session::SessionForkCut,
    },
    /// Interrupt the active turn, task, or shell process currently owned by the TUI.
    InterruptActiveWork,
    /// Push input onto the canonical session queue (busy path).
    QueuePush {
        input: Vec<InputItem>,
    },
    /// Promote a queued entry into the active turn as a steer.
    QueueSteer {
        queue_item_id: String,
        expected_turn_id: TurnId,
    },
    /// Remove a queued entry (edit-from-queue).
    QueueRemove {
        queue_item_id: String,
    },
    /// Replace a queued entry's content in place, preserving its position.
    QueueUpdate {
        queue_item_id: String,
        input: Vec<InputItem>,
    },
    /// Ask a side question in a one-turn forked agent.
    RunBtwQuestion {
        question: String,
    },
    ApprovalRespond {
        session_id: SessionId,
        turn_id: TurnId,
        approval_id: String,
        decision: devo_server::ApprovalDecisionValue,
        scope: devo_server::ApprovalScopeValue,
    },
    RequestUserInputRespond {
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        response: devo_protocol::RequestUserInputResponse,
    },
    UpdatePermissions {
        preset: devo_protocol::PermissionPreset,
        persist_scope: crate::app_command::PersistScope,
    },
    UpdateEffectiveContextWindow {
        effective_context_window: u64,
    },
    UpdateSandboxProfile {
        profile: String,
    },
    /// Browse persisted input history via the server/runtime session state.
    BrowseInputHistory(InputHistoryDirection),
    /// Stop the worker loop.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
struct ShellCommandExecStart {
    process_id: String,
    command: String,
    started_event: WorkerEvent,
    params: CommandExecParams,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BtwQuestionState {
    parent_session_id: SessionId,
    question: String,
    latest_answer: Option<String>,
}

fn next_shell_command_exec_start(
    session_id: Option<SessionId>,
    cwd: PathBuf,
    command: String,
    next_shell_process_index: &mut u64,
) -> ShellCommandExecStart {
    let process_id = format!("user-shell-{}", *next_shell_process_index);
    *next_shell_process_index += 1;
    let input = serde_json::json!({
        "cmd": command.clone(),
        "cwd": cwd.clone(),
    });
    ShellCommandExecStart {
        process_id: process_id.clone(),
        command: command.clone(),
        started_event: WorkerEvent::Transcript(tool_lifecycle::tool_opened_from_command_source(
            process_id.clone(),
            command.clone(),
            Some(input),
            devo_protocol::protocol::ExecCommandSource::UserShell,
            Vec::new(),
        )),
        params: CommandExecParams {
            session_id,
            process_id,
            cwd: Some(cwd),
            program: CommandExecProgram::OneShot { command },
            size: None,
        },
    }
}

#[derive(Default)]
struct ProviderValidationCancellation(Mutex<Option<CancellationToken>>);

impl ProviderValidationCancellation {
    fn cancel(&self) {
        if let Some(token) = self
            .0
            .lock()
            .expect("provider validation cancellation mutex should not be poisoned")
            .as_ref()
        {
            token.cancel();
        }
    }

    fn start(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self
            .0
            .lock()
            .expect("provider validation cancellation mutex should not be poisoned") =
            Some(token.clone());
        token
    }
}

/// Handle used by the UI thread to interact with the background query worker.
pub(crate) struct QueryWorkerHandle {
    /// Sender used to submit commands to the worker.
    command_tx: mpsc::UnboundedSender<OperationCommand>,
    provider_validation_cancel: Arc<ProviderValidationCancellation>,
    /// Receiver used by the UI to consume worker events.
    pub(crate) event_rx: mpsc::UnboundedReceiver<WorkerEvent>,
    /// Background task running the worker loop.
    join_handle: JoinHandle<()>,
}

impl QueryWorkerHandle {
    /// Spawns the background worker and returns the UI-facing handle.
    pub(crate) fn spawn(config: QueryWorkerConfig) -> Self {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let provider_validation_cancel = Arc::new(ProviderValidationCancellation::default());
        let join_handle = tokio::spawn(run_worker(
            config,
            command_rx,
            event_tx,
            Arc::clone(&provider_validation_cancel),
        ));
        Self {
            command_tx,
            provider_validation_cancel,
            event_rx,
            join_handle,
        }
    }

    /// Submits one prompt to the worker.
    pub(crate) fn submit_prompt(
        &self,
        prompt: String,
        approval_policy: Option<String>,
    ) -> Result<()> {
        self.submit_input(vec![InputItem::Text { text: prompt }], approval_policy)
    }

    pub(crate) fn submit_input(
        &self,
        input: Vec<InputItem>,
        approval_policy: Option<String>,
    ) -> Result<()> {
        self.submit_input_with_collaboration_mode(input, approval_policy, CollaborationMode::Build)
    }

    pub(crate) fn submit_input_with_collaboration_mode(
        &self,
        input: Vec<InputItem>,
        approval_policy: Option<String>,
        collaboration_mode: CollaborationMode,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SubmitInput {
                input,
                approval_policy,
                collaboration_mode,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn execute_shell_command(&self, command: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ExecuteShellCommand { command })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn submit_shell_input(&self, command: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SubmitShellInput { command })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Updates the active session model for future turns.
    pub(crate) fn set_model(&self, model: String) -> Result<()> {
        self.set_model_selection(model, None)
    }

    pub(crate) fn set_model_selection(
        &self,
        model: String,
        model_binding_id: Option<String>,
    ) -> Result<()> {
        self.set_model_selection_with_scope(
            model,
            model_binding_id,
            crate::app_command::PersistScope::Session,
        )
    }

    pub(crate) fn set_model_selection_with_scope(
        &self,
        model: String,
        model_binding_id: Option<String>,
        persist_scope: crate::app_command::PersistScope,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetModel {
                model,
                model_binding_id,
                persist_scope,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Updates the reasoning effort selection used for future turns.
    pub(crate) fn set_reasoning_effort(
        &self,
        reasoning_effort_selection: Option<String>,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetReasoningEffort(
                reasoning_effort_selection,
            ))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Reconfigures the provider connection used by the background server client.
    pub(crate) fn reconfigure_provider(
        &self,
        wire_api: ProviderWireApi,
        model: String,
        base_url: Option<String>,
        api_key: Option<String>,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ReconfigureProvider {
                wire_api,
                model,
                base_url,
                api_key,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Validates provider settings with a temporary probe request.
    pub(crate) fn validate_provider(
        &self,
        provider_vendor: ProviderVendor,
        model_binding: ProviderModelBinding,
        api_key: Option<String>,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ValidateProvider {
                provider_vendor,
                model_binding,
                api_key,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Cancels the in-flight provider validation probe, if any.
    pub(crate) fn cancel_provider_validation(&self) {
        self.provider_validation_cancel.cancel();
    }

    /// Requests the current configured provider vendors from the background worker.
    pub(crate) fn list_provider_vendors(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ListProviderVendors)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Adds or updates a provider vendor through the background worker.
    pub(crate) fn upsert_provider_vendor(
        &self,
        provider_vendor: ProviderVendor,
        model_binding: Option<ProviderModelBinding>,
        default_model_binding: Option<String>,
        api_key: Option<String>,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::UpsertProviderVendor {
                provider_vendor,
                model_binding,
                default_model_binding,
                api_key,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Requests the current persisted session list from the background worker.
    pub(crate) fn list_sessions(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ListSessions)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn preview_session(&self, session_id: SessionId) -> Result<()> {
        self.command_tx
            .send(OperationCommand::PreviewSession(session_id))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Requests the current skill list from the background worker.
    pub(crate) fn list_skills(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ListSkills)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Requests MCP server runtime statuses from the background worker.
    pub(crate) fn list_mcp_servers(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ListMcpServers)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Requests tools for one MCP server from the background worker.
    pub(crate) fn list_mcp_tools(&self, name: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ListMcpTools { name })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn reference_search_requested(&self, query: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ReferenceSearchRequested { query })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn reference_search_cancelled(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ReferenceSearchCancelled)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn set_skill_enabled(&self, path: PathBuf, enabled: bool) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetSkillEnabled { path, enabled })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn set_mcp_server_enabled(&self, name: String, enabled: bool) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetMcpServerEnabled { name, enabled })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Requests proactive compaction for the current active session.
    pub(crate) fn compact_session(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::CompactSession)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn show_goal(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ShowGoal)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn edit_goal(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::EditGoal)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn set_goal_objective(
        &self,
        objective: String,
        mode: GoalObjectiveMode,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetGoalObjective { objective, mode })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn set_goal_status(&self, status: ThreadGoalStatus) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetGoalStatus { status })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn clear_goal(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ClearGoal)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Clears the active session so the next submitted prompt starts a fresh one lazily.
    pub(crate) fn start_new_session(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::StartNewSession)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Switches the active session to a persisted session identifier.
    pub(crate) fn switch_session(&self, session_id: SessionId) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SwitchSession(session_id))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Renames the current active session.
    pub(crate) fn rename_session(&self, title: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::RenameSession(title))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn rename_session_by_id(&self, session_id: SessionId, title: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::RenameSessionById { session_id, title })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Deletes a session. When `session_id` is `None`, deletes the active session.
    pub(crate) fn delete_session(&self, session_id: Option<SessionId>) -> Result<()> {
        self.command_tx
            .send(OperationCommand::DeleteSession { session_id })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn rollback_to_user_turn(&self, user_turn_index: u32) -> Result<()> {
        self.command_tx
            .send(OperationCommand::RollbackUserTurn {
                user_turn_index,
                mode: RollbackMode::ThroughUserTurn,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn rollback_before_user_turn(&self, user_turn_index: u32) -> Result<()> {
        self.command_tx
            .send(OperationCommand::RollbackUserTurn {
                user_turn_index,
                mode: RollbackMode::BeforeUserTurn,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn fork_at_user_turn(
        &self,
        user_turn_index: u32,
        cut: devo_protocol::native::rpc_session::SessionForkCut,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ForkAtUserTurn {
                user_turn_index,
                cut,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Interrupts the active turn, task, or shell process.
    pub(crate) fn interrupt_active_work(&self) -> Result<()> {
        self.command_tx
            .send(OperationCommand::InterruptActiveWork)
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Push input onto the session queue while a turn is active.
    pub(crate) fn queue_push(&self, input: Vec<InputItem>) -> Result<()> {
        self.command_tx
            .send(OperationCommand::QueuePush { input })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Promote a queued item into the active turn as a steer.
    pub(crate) fn queue_steer(
        &self,
        queue_item_id: String,
        expected_turn_id: TurnId,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::QueueSteer {
                queue_item_id,
                expected_turn_id,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Remove a queued item so it can be edited in the composer.
    pub(crate) fn queue_remove(&self, queue_item_id: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::QueueRemove { queue_item_id })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Replace a queued item's content in place, preserving its position.
    pub(crate) fn queue_update(&self, queue_item_id: String, input: Vec<InputItem>) -> Result<()> {
        self.command_tx
            .send(OperationCommand::QueueUpdate {
                queue_item_id,
                input,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Ask a quick side question without interrupting the active turn.
    pub(crate) fn run_btw_question(&self, question: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::RunBtwQuestion { question })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn approval_respond(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        approval_id: String,
        decision: devo_server::ApprovalDecisionValue,
        scope: devo_server::ApprovalScopeValue,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::ApprovalRespond {
                session_id,
                turn_id,
                approval_id,
                decision,
                scope,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn request_user_input_respond(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        response: devo_protocol::RequestUserInputResponse,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::RequestUserInputRespond {
                session_id,
                turn_id,
                request_id,
                response,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn update_permissions(
        &self,
        preset: devo_protocol::PermissionPreset,
        persist_scope: crate::app_command::PersistScope,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::UpdatePermissions {
                preset,
                persist_scope,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn set_collaboration_mode(
        &self,
        collaboration_mode: CollaborationMode,
        persist_scope: crate::app_command::PersistScope,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::SetCollaborationMode {
                collaboration_mode,
                persist_scope,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn update_effective_context_window(
        &self,
        effective_context_window: u64,
    ) -> Result<()> {
        self.command_tx
            .send(OperationCommand::UpdateEffectiveContextWindow {
                effective_context_window,
            })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn update_sandbox_profile(&self, profile: String) -> Result<()> {
        self.command_tx
            .send(OperationCommand::UpdateSandboxProfile { profile })
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    pub(crate) fn browse_input_history(&self, direction: InputHistoryDirection) -> Result<()> {
        self.command_tx
            .send(OperationCommand::BrowseInputHistory(direction))
            .map_err(|_| anyhow::anyhow!("interactive worker is no longer running"))
    }

    /// Stops the worker task and waits briefly for it to finish.
    pub(crate) async fn shutdown(self) -> Result<()> {
        tracing::info!("query worker shutdown requested");
        let _ = self.command_tx.send(OperationCommand::Shutdown);
        let mut join_handle = self.join_handle;
        tokio::select! {
            result = &mut join_handle => {
                tracing::info!("query worker joined during graceful shutdown");
                map_worker_join_result(result)
            }
            _ = tokio::time::sleep(WORKER_SHUTDOWN_GRACE) => {
                tracing::warn!("query worker did not stop during grace period; aborting task");
                join_handle.abort();
                match tokio::time::timeout(WORKER_ABORT_JOIN_TIMEOUT, &mut join_handle).await {
                    Ok(result) => {
                        tracing::info!("query worker abort join completed");
                        map_worker_join_result(result)
                    }
                    Err(_) => {
                        tracing::warn!("timed out waiting for aborted query worker task");
                        Ok(())
                    }
                }
            }
        }
    }
}

#[cfg(test)]
impl QueryWorkerHandle {
    /// Creates a lightweight stub worker handle for unit tests that exercise UI logic only.
    pub(crate) fn stub() -> Self {
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let (_event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            command_tx,
            provider_validation_cancel: Arc::new(ProviderValidationCancellation::default()),
            event_rx,
            join_handle: tokio::spawn(async move { while command_rx.recv().await.is_some() {} }),
        }
    }
}

async fn run_worker(
    config: QueryWorkerConfig,
    mut command_rx: mpsc::UnboundedReceiver<OperationCommand>,
    event_tx: mpsc::UnboundedSender<WorkerEvent>,
    provider_validation_cancel: Arc<ProviderValidationCancellation>,
) {
    if let Err(error) = run_worker_inner(
        config,
        &mut command_rx,
        &event_tx,
        provider_validation_cancel,
    )
    .await
    {
        let _ = event_tx.send(WorkerEvent::TurnFailed {
            message: error.to_string(),
            hint: None,
            turn_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: 0,
            last_query_input_tokens: 0,
        });
    }
}

async fn run_worker_inner(
    config: QueryWorkerConfig,
    command_rx: &mut mpsc::UnboundedReceiver<OperationCommand>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    provider_validation_cancel: Arc<ProviderValidationCancellation>,
) -> Result<()> {
    // The worker owns the server client and translates UI commands into server
    // calls, then turns server notifications back into lightweight UI events.
    let mut client = spawn_client(&config.cwd, config.server_log_level.clone()).await?;
    let _ = client.initialize(&config.client_capabilities).await?;
    let mut default_model = config.model.clone();
    let mut default_model_binding_id = config.model_binding_id.clone();
    let default_reasoning_effort_selection = config.reasoning_effort_selection.clone();
    let mut default_permission_preset = config.permission_preset;
    let mut default_collaboration_mode = config.default_collaboration_mode;
    let mut session_id: Option<SessionId> = None;
    let mut session_cwd = config.cwd.clone();
    let mut model = default_model.clone();
    let mut model_binding_id = default_model_binding_id.clone();
    let mut reasoning_effort_selection = default_reasoning_effort_selection.clone();
    let mut session_permission_preset: Option<PermissionPreset> = None;
    let initial_sandbox_profile = config.initial_sandbox_profile.clone();
    let mut active_turn_id: Option<TurnId> = None;
    let mut turn_count = 0usize;
    let mut total_input_tokens = 0usize;
    let mut total_output_tokens = 0usize;
    let mut total_tokens = 0usize;
    let mut total_cache_read_tokens = 0usize;
    let mut last_query_total_tokens = 0usize;
    let mut last_query_input_tokens = 0usize;
    let mut saw_usage_update_for_turn = false;
    let mut has_authoritative_usage_totals = false;
    let mut latest_completed_agent_message: Option<String> = None;
    let mut seen_terminal_item_ids: HashSet<String> = HashSet::new();
    let mut seen_terminal_call_ids: HashSet<String> = HashSet::new();
    let mut child_agent_sessions: HashSet<SessionId> = HashSet::new();
    let mut btw_agent_sessions: HashMap<SessionId, BtwQuestionState> = HashMap::new();
    let mut input_history_cursor: Option<usize> = None;
    let mut active_reference_search_id: Option<ReferenceSearchId> = None;
    let mut active_shell_process_ids: HashSet<String> = HashSet::new();
    let mut next_shell_process_index = 1_u64;
    // Session the worker currently holds a canonical event subscription for;
    // `subscription/create` accumulates server-side, so subscribe at most once
    // per activated session.
    let mut subscribed_session_id: Option<SessionId> = None;

    if let Some(initial_session_id) = config.initial_session_id {
        match restore_session_native(&mut client, initial_session_id).await {
            Ok(restore) => {
                active_turn_id = None;
                session_id = Some(initial_session_id);
                session_cwd = restore.session.cwd.clone();
                model = restore.session.model.model.clone();
                model_binding_id = (restore.session.model.provider != "unknown")
                    .then(|| restore.session.model.provider.clone());
                reasoning_effort_selection = restore.session.settings.reasoning_effort.clone();
                session_permission_preset =
                    Some(match restore.session.settings.permission_profile {
                        devo_protocol::native::model::PermissionProfile::Default => {
                            PermissionPreset::Default
                        }
                        devo_protocol::native::model::PermissionProfile::AutoReview => {
                            PermissionPreset::AutoReview
                        }
                        devo_protocol::native::model::PermissionProfile::FullAccess => {
                            PermissionPreset::FullAccess
                        }
                    });
                let event = session_switched_event_from_restore(initial_session_id, &restore);
                let (usage_input, usage_output, usage_total, usage_cache_read) = (
                    restore.session.usage.total.input_tokens as usize,
                    restore.session.usage.total.output_tokens as usize,
                    restore.session.usage.total.total_tokens as usize,
                    restore.session.usage.total.cache_read_input_tokens as usize,
                );
                let _ = event_tx.send(event);
                ensure_session_subscription(
                    &mut client,
                    initial_session_id,
                    &mut subscribed_session_id,
                    event_tx,
                )
                .await;
                total_input_tokens = usage_input;
                total_output_tokens = usage_output;
                total_tokens = usage_total;
                total_cache_read_tokens = usage_cache_read;
                last_query_total_tokens = 0;
                last_query_input_tokens = 0;
                has_authoritative_usage_totals = true;
            }
            Err(error) => {
                let _ = event_tx.send(WorkerEvent::TurnFailed {
                    message: format!("failed to resume session: {error}"),
                    hint: None,
                    turn_count,
                    total_input_tokens,
                    total_output_tokens,
                    total_tokens,
                    total_cache_read_tokens,
                    prompt_token_estimate: total_input_tokens,
                    last_query_input_tokens,
                });
            }
        }
    }
    let _ = emit_skills_list(&mut client, &session_cwd, event_tx, false).await;

    loop {
        tokio::select! {
            maybe_command = command_rx.recv() => {
                match maybe_command {
                    Some(OperationCommand::SubmitInput {
                        input,
                        approval_policy: _,
                        collaboration_mode,
                    }) => {
                        let active_session_id = prepare_session_for_command(
                            &mut client,
                            &config.cwd,
                            &mut model,
                            &mut model_binding_id,
                            &mut reasoning_effort_selection,
                            &mut session_id,
                            &mut subscribed_session_id,
                            session_permission_preset
                                .unwrap_or(default_permission_preset),
                            initial_sandbox_profile.as_deref(),
                            event_tx,
                        )
                        .await?;

                        // Settings patches are persist-first and must not block
                        // `turn/start`. Awaiting this RPC serialized the next
                        // turn behind title-generation's state-change gate and
                        // produced the TUI's 10s `turn/start` timeout.
                        let native_input = devo_client::native_turn_start_input(&input)
                            .expect("all TUI input variants have canonical turn semantics");
                        let _ = client
                            .session_model_update(
                                active_session_id,
                                Some(model.clone()),
                                model_binding_id.clone(),
                                reasoning_effort_selection.clone(),
                                Some(collaboration_mode),
                            )
                            .await;
                        let idempotency_key = devo_protocol::SessionId::new().to_string();
                        let start_result = client
                            .turn_start_native(
                                active_session_id,
                                native_input,
                                idempotency_key,
                            )
                            .await
                            .map(|result| result.turn.id.as_str().to_string())
                            .map_err(|error| error.to_string());
                        match start_result {
                            Ok(turn_id) => {
                                if let Ok(turn_id) = devo_protocol::TurnId::try_from(turn_id.as_str()) {
                                    active_turn_id = Some(turn_id);
                                }
                            }
                            Err(error) if error.contains("turn_already_running") => {
                                // Native turn/start rejects while the previous
                                // turn is still finalizing. Queue instead of
                                // surfacing a timeout/failure to the user.
                                let native_session = native_session_id(active_session_id);
                                match client
                                    .session_queue_push(
                                        devo_protocol::native::rpc_turn::SessionQueuePushParams {
                                            session_id: native_session.clone(),
                                            input: crate::queue_ops::user_input_from_input_items(
                                                &input,
                                            ),
                                            client_user_message_id: None,
                                            idempotency_key: format!(
                                                "tui-queue-{}",
                                                SessionId::new()
                                            ),
                                        },
                                    )
                                    .await
                                {
                                    Ok(
                                        devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued {
                                            entry,
                                        },
                                    ) => {
                                        let _ = emit_queue_snapshot(
                                            &mut client,
                                            &native_session,
                                            devo_protocol::native::queue::QueueChange::Added,
                                            entry.queue_item_id,
                                            /*started_turn_id*/ None,
                                            event_tx,
                                        )
                                        .await;
                                    }
                                    Ok(
                                        devo_protocol::native::rpc_turn::SessionQueuePushResult::Started {
                                            turn,
                                        },
                                    ) => {
                                        let started_turn_id =
                                            TurnId::try_from(turn.id.as_str()).ok();
                                        if let Some(turn_id) = started_turn_id {
                                            active_turn_id = Some(turn_id);
                                        }
                                        let _ = emit_queue_snapshot(
                                            &mut client,
                                            &native_session,
                                            devo_protocol::native::queue::QueueChange::Drained,
                                            devo_protocol::native::ids::QueueItemId::from_string(
                                                String::new(),
                                            ),
                                            started_turn_id,
                                            event_tx,
                                        )
                                        .await;
                                    }
                                    Err(queue_error) => {
                                        let _ = event_tx.send(WorkerEvent::TurnFailed {
                                            message: queue_error.to_string(),
                                            hint: None,
                                            turn_count,
                                            total_input_tokens,
                                            total_output_tokens,
                                            total_tokens,
                                            total_cache_read_tokens,
                                            prompt_token_estimate: total_input_tokens,
                                            last_query_input_tokens,
                                        });
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error,
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(
                        OperationCommand::ExecuteShellCommand { command }
                        | OperationCommand::SubmitShellInput { command },
                    ) => {
                        if active_turn_id.is_some() {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "cannot run shell command while a turn is in progress".to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                            continue;
                        }
                        // Shell commands go through canonical
                        // `task/start` (DD-7 facade) when a session
                        // exists; sessionless exec keeps the legacy
                        // path until the task model defines its scope.
                        if let Some(active_session_id) = session_id {
                            let input = serde_json::json!({
                                "cmd": command.clone(),
                                "cwd": session_cwd.clone(),
                            });
                            let idempotency_key =
                                devo_protocol::SessionId::new().to_string();
                            match client
                                .task_start_process_native(
                                    active_session_id,
                                    command.clone(),
                                    Some(session_cwd.clone()),
                                    idempotency_key,
                                )
                                .await
                            {
                                Ok(result) => {
                                    let process_id = result.item_id.as_str().to_string();
                                    active_shell_process_ids.insert(process_id.clone());
                                    let _ = event_tx.send(WorkerEvent::Transcript(
                                        tool_lifecycle::tool_opened_from_command_source(
                                            process_id,
                                            command.clone(),
                                            Some(input),
                                            devo_protocol::protocol::ExecCommandSource::UserShell,
                                            Vec::new(),
                                        ),
                                    ));
                                }
                                Err(error) => {
                                    let _ = event_tx.send(WorkerEvent::Transcript(
                                        tool_lifecycle::tool_closed_shell(
                                            format!(
                                                "user-shell-failed-{}",
                                                next_shell_process_index
                                            ),
                                            "Shell".to_string(),
                                            Some(error.to_string()),
                                            true,
                                        ),
                                    ));
                                    next_shell_process_index += 1;
                                }
                            }
                            continue;
                        }
                        let shell_start = next_shell_command_exec_start(
                            session_id,
                            session_cwd.clone(),
                            command,
                            &mut next_shell_process_index,
                        );
                        active_shell_process_ids.insert(shell_start.process_id.clone());
                        let _ = event_tx.send(shell_start.started_event);
                        match client.command_exec(shell_start.params).await {
                            Ok(_) => {}
                            Err(error) => {
                                active_shell_process_ids.remove(&shell_start.process_id);
                                let _ = event_tx.send(WorkerEvent::Transcript(
                                    tool_lifecycle::tool_closed_shell(
                                        shell_start.process_id,
                                        shell_start.command,
                                        Some(error.to_string()),
                                        true,
                                    ),
                                ));
                            }
                        }
                    }
                    Some(OperationCommand::SetModel {
                        model: next_model,
                        model_binding_id: next_model_binding_id,
                        persist_scope,
                    }) => {
                        model = next_model;
                        model_binding_id = next_model_binding_id;
                        if persist_scope == crate::app_command::PersistScope::Default {
                            default_model = model.clone();
                            default_model_binding_id = model_binding_id.clone();
                        }
                        input_history_cursor = None;
                        if let Some(active_session_id) = session_id {
                            let _ = client
                                .session_model_update(
                                    active_session_id,
                                    Some(model.clone()),
                                    model_binding_id.clone(),
                                    reasoning_effort_selection.clone(),
                                    None,
                                )
                                .await;
                        }
                    }
                    Some(OperationCommand::SetCollaborationMode {
                        collaboration_mode,
                        persist_scope,
                    }) => {
                        if persist_scope == crate::app_command::PersistScope::Default {
                            default_collaboration_mode = collaboration_mode;
                        }
                        if let Some(active_session_id) = session_id {
                            let _ = client
                                .session_model_update(
                                    active_session_id,
                                    Some(model.clone()),
                                    model_binding_id.clone(),
                                    reasoning_effort_selection.clone(),
                                    Some(collaboration_mode),
                                )
                                .await;
                        }
                    }
                    Some(OperationCommand::SetReasoningEffort(next_reasoning_effort_selection)) => {
                        reasoning_effort_selection = next_reasoning_effort_selection;
                        if let Some(active_session_id) = session_id {
                            let _ = client
                                .session_model_update(
                                    active_session_id,
                                    Some(model.clone()),
                                    model_binding_id.clone(),
                                    reasoning_effort_selection.clone(),
                                    None,
                                )
                                .await;
                        }
                    }
                    Some(OperationCommand::ValidateProvider {
                        provider_vendor,
                        model_binding,
                        api_key,
                    }) => {
                        let cancellation = provider_validation_cancel.start();
                        let validation = client.provider_validate(
                            devo_protocol::native::rpc_admin::ProviderValidateParams {
                                provider_vendor: provider_vendor.into(),
                                model_binding: model_binding.into(),
                                api_key,
                            },
                        );
                        tokio::pin!(validation);
                        let validation_result = tokio::select! {
                            result = &mut validation => Some(result),
                            _ = cancellation.cancelled() => None,
                        };
                        match validation_result {
                            Some(Ok(result)) => {
                                let _ = event_tx.send(WorkerEvent::ProviderValidationSucceeded {
                                    reply_preview: result.reply_preview,
                                });
                            }
                            Some(Err(error)) => {
                                let message = error.to_string();
                                let hint =
                                    devo_provider::recovery_hint_for_message(&message);
                                let _ = event_tx.send(WorkerEvent::ProviderValidationFailed {
                                    message,
                                    hint,
                                });
                            }
                            None => {}
                        }
                    }
                    Some(OperationCommand::ListProviderVendors) => {
                        match tokio::time::timeout(
                            Duration::from_secs(5),
                            client.provider_list(),
                        )
                        .await
                        {
                            Ok(Ok(result)) => {
                                let _ = event_tx.send(WorkerEvent::ProviderVendorsListed {
                                    provider_vendors: result
                                        .providers
                                        .into_iter()
                                        .map(Into::into)
                                        .collect(),
                                });
                            }
                            Ok(Err(error)) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                            Err(_) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: "provider list request timed out".to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::UpsertProviderVendor {
                        provider_vendor,
                        model_binding,
                        default_model_binding,
                        api_key,
                    }) => {
                        match tokio::time::timeout(
                            Duration::from_secs(5),
                            client.provider_upsert(
                                devo_protocol::native::rpc_admin::ProviderUpsertParams {
                                    provider_vendor: provider_vendor.into(),
                                    model_binding: model_binding.map(Into::into),
                                    default_model_binding,
                                    api_key,
                                },
                            ),
                        )
                        .await
                        {
                            Ok(Ok(result)) => {
                                let _ = event_tx.send(WorkerEvent::ProviderVendorUpserted {
                                    provider_vendor: result.provider_vendor.into(),
                                    model_binding: result.model_binding.map(Into::into),
                                });
                            }
                            Ok(Err(error)) => {
                                let _ = event_tx.send(WorkerEvent::ProviderVendorUpsertFailed {
                                    message: error.to_string(),
                                });
                            }
                            Err(_) => {
                                let _ = event_tx.send(WorkerEvent::ProviderVendorUpsertFailed {
                                    message: "provider upsert request timed out".to_string(),
                                });
                            }
                        }
                    }
                Some(OperationCommand::ReconfigureProvider {
                    wire_api: _,
                    model: next_model,
                    base_url: _,
                    api_key: _,
                }) => {
                        // Recreate the client so new provider credentials take effect
                        // without requiring the whole app to restart.
                        model = next_model;
                        model_binding_id = None;
                        client.shutdown().await?;
                        client = spawn_client(
                            &config.cwd,
                            config.server_log_level.clone(),
                        )
                        .await?;
                        client.initialize(&config.client_capabilities).await?;
                        session_id = None;
                        subscribed_session_id = None;
                        child_agent_sessions.clear();
                        btw_agent_sessions.clear();
                        active_turn_id = None;
                        active_reference_search_id = None;
                        last_query_total_tokens = 0;
                    }
                    Some(OperationCommand::ListSessions) => {
                        // Native `session/list` (L2-DES-APP-008):
                        // page through all entries for the picker.
                        let list_result = tokio::time::timeout(Duration::from_secs(5), async {
                            let mut native_sessions = Vec::new();
                            let mut cursor = None;
                            loop {
                                let page = client
                                    .session_list_native(
                                        devo_protocol::native::rpc_session::SessionListParams {
                                            cursor,
                                            ..Default::default()
                                        },
                                    )
                                    .await?;
                                cursor = page.next_cursor;
                                native_sessions.extend(page.data);
                                if cursor.is_none() {
                                    return Ok::<_, anyhow::Error>(native_sessions);
                                }
                            }
                        })
                        .await;
                        match list_result {
                            Ok(Ok(native_sessions)) => {
                                let sessions = native_sessions
                                    .iter()
                                    .filter_map(|session| {
                                        let entry_session_id =
                                            SessionId::try_from(session.id.as_str()).ok()?;
                                        Some(SessionListEntry {
                                            session_id: entry_session_id,
                                            title: session
                                                .title
                                                .clone()
                                                .filter(|title| !title.trim().is_empty())
                                                .unwrap_or_else(|| {
                                                    session
                                                        .preview
                                                        .lines()
                                                        .next()
                                                        .filter(|line| !line.trim().is_empty())
                                                        .unwrap_or("(untitled)")
                                                        .to_string()
                                                }),
                                            preview: session.preview.clone(),
                                            cwd: session.cwd.clone(),
                                            branch: session
                                                .git_info
                                                .as_ref()
                                                .and_then(|git| git.branch.clone()),
                                            last_activity_at: session.last_activity_at,
                                            transcript_size_bytes: session.transcript_size_bytes,
                                            is_active: Some(entry_session_id) == session_id,
                                        })
                                    })
                                    .collect();
                                let _ = event_tx.send(WorkerEvent::SessionsListed { sessions });
                            }
                            Ok(Err(error)) => {
                                let _ = event_tx.send(WorkerEvent::SessionsListFailed {
                                    message: error.to_string(),
                                });
                            }
                            Err(_) => {
                                let _ = event_tx.send(WorkerEvent::SessionsListFailed {
                                    message: "session list request timed out".to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::PreviewSession(preview_session_id)) => {
                        match collect_session_preview(&mut client, preview_session_id).await {
                            Ok(messages) => {
                                let _ = event_tx.send(WorkerEvent::SessionPreviewLoaded {
                                    session_id: preview_session_id,
                                    messages,
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::SessionPreviewFailed {
                                    session_id: preview_session_id,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::ListSkills) => {
                        if let Err(error) =
                            emit_skills_list(&mut client, &session_cwd, event_tx, true).await
                        {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: error.to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                        }
                    }
                    Some(OperationCommand::ListMcpServers) => {
                        if let Err(error) =
                            emit_mcp_servers_list(&mut client, event_tx).await
                        {
                            // Still open the picker from config so a single
                            // broken/runtime-stuck MCP server cannot blank /mcps.
                            tracing::warn!(
                                error = %error,
                                "mcp/list failed; opening /mcps from config only"
                            );
                            let _ = event_tx.send(WorkerEvent::McpServersListed {
                                servers: Vec::new(),
                            });
                        }
                    }
                    Some(OperationCommand::ListMcpTools { name }) => {
                        if let Err(error) =
                            emit_mcp_tools_list(&mut client, name, event_tx).await
                        {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: error.to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                        }
                    }
                    Some(OperationCommand::ReferenceSearchRequested { query }) => {
                        match emit_reference_search_update(
                            &mut client,
                            &session_cwd,
                            &mut active_reference_search_id,
                            query,
                            event_tx,
                        )
                        .await
                        {
                            Ok(()) => {}
                            Err(error) => {
                                tracing::warn!(?error, "reference search request failed");
                            }
                        }
                    }
                    Some(OperationCommand::ReferenceSearchCancelled) => {
                        if let Some(search_id) = active_reference_search_id.take() {
                            let _ = client.search_cancel(search_id).await;
                        }
                    }
                    Some(OperationCommand::CompactSession) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "no active session exists yet; send a prompt or switch to a saved session first".to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                            continue;
                        };
                        if active_turn_id.is_some() {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "cannot compact while a turn is in progress".to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                            continue;
                        }
                        match client
                            .session_compact_start_native(active_session_id)
                            .await
                        {
                            Ok(result) => {
                                if let Ok(turn_id) =
                                    devo_protocol::TurnId::try_from(result.turn.id.as_str())
                                {
                                    active_turn_id = Some(turn_id);
                                }
                                let _ = event_tx.send(WorkerEvent::SessionCompactionStarted);
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::ShowGoal) => {
                        let goal = if let Some(active_session_id) = session_id {
                            match client
                                .session_goal_read_native(active_session_id)
                                .await
                            {
                                Ok(result) => result
                                    .goal
                                    .as_ref()
                                    .map(thread_goal_from_native),
                                Err(error) => {
                                    let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                        message: error.to_string(),
                                    });
                                    continue;
                                }
                            }
                        } else {
                            None
                        };
                        let _ = event_tx.send(WorkerEvent::GoalStatusLoaded { goal });
                    }
                    Some(OperationCommand::EditGoal) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                message: "No goal is currently set.".to_string(),
                            });
                            continue;
                        };
                        match client
                            .session_goal_read_native(active_session_id)
                            .await
                        {
                            Ok(result) => match result.goal.as_ref().map(thread_goal_from_native) {
                                Some(goal) => {
                                    let _ = event_tx.send(WorkerEvent::GoalEditLoaded { goal });
                                }
                                None => {
                                    let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                        message: "No goal is currently set.".to_string(),
                                    });
                                }
                            },
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::SetGoalObjective { objective, mode }) => {
                        let active_session_id = prepare_session_for_command(
                            &mut client,
                            &config.cwd,
                            &mut model,
                            &mut model_binding_id,
                            &mut reasoning_effort_selection,
                            &mut session_id,
                            &mut subscribed_session_id,
                            session_permission_preset
                                .unwrap_or(default_permission_preset),
                            initial_sandbox_profile.as_deref(),
                            event_tx,
                        )
                        .await?;

                        if matches!(mode, GoalObjectiveMode::ConfirmIfExists) {
                            match client
                                .session_goal_read_native(active_session_id)
                                .await
                            {
                                Ok(result) => {
                                    if let Some(current_goal) = result.goal.as_ref().map(thread_goal_from_native) {
                                        let _ = event_tx.send(
                                            WorkerEvent::GoalReplaceConfirmationRequested {
                                                current_goal,
                                                objective,
                                            },
                                        );
                                        continue;
                                    }
                                }
                                Err(error) => {
                                    let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                        message: error.to_string(),
                                    });
                                    continue;
                                }
                            }
                        }

                        // Create/replace modes go through canonical
                        // session/goal/set (ifExists=replace covers both
                        // fresh create and confirmed replacement).
                        // UpdateExisting is an in-place edit, which has
                        // no canonical vocabulary yet and stays legacy.
                        if matches!(
                            mode,
                            GoalObjectiveMode::ConfirmIfExists | GoalObjectiveMode::ReplaceExisting
                        ) {
                            match client
                                .session_goal_set_native(
                                    active_session_id,
                                    objective,
                                    None,
                                    devo_protocol::native::rpc_session::GoalIfExists::Replace,
                                    devo_protocol::SessionId::new().to_string(),
                                )
                                .await
                            {
                                Ok(result) => {
                                    let _ = event_tx.send(WorkerEvent::GoalUpdated {
                                        goal: thread_goal_from_native(&result.goal),
                                    });
                                }
                                Err(error) => {
                                    let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                        message: error.to_string(),
                                    });
                                }
                            }
                            continue;
                        }
                        let GoalObjectiveMode::UpdateExisting {
                            status,
                            token_budget,
                        } = mode
                        else {
                            unreachable!("covered by the canonical branch above");
                        };
                        // Native in-place goal edit (ratified #3):
                        // preserves the goal id, usage stats, and
                        // continuation linkage.
                        let native_status = match status {
                            ThreadGoalStatus::Active => {
                                devo_protocol::native::goal::GoalStatus::Active
                            }
                            ThreadGoalStatus::Paused => {
                                devo_protocol::native::goal::GoalStatus::Paused
                            }
                            ThreadGoalStatus::Complete => {
                                devo_protocol::native::goal::GoalStatus::Completed
                            }
                            ThreadGoalStatus::BudgetLimited => {
                                devo_protocol::native::goal::GoalStatus::BudgetLimited
                            }
                        };
                        let patch = devo_protocol::native::rpc_session::GoalPatch {
                            objective: Some(objective.clone()),
                            status: Some(native_status),
                            token_budget: match token_budget {
                                Some(budget) => {
                                    devo_protocol::native::patch::PatchField::Value(budget)
                                }
                                None => devo_protocol::native::patch::PatchField::Missing,
                            },
                        };
                        match client
                            .session_goal_update_native(
                                active_session_id,
                                patch,
                                devo_protocol::SessionId::new().to_string(),
                            )
                            .await
                        {
                            Ok(result) => {
                                let _ = event_tx.send(WorkerEvent::GoalUpdated {
                                    goal: thread_goal_from_native(&result.goal),
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::SetGoalStatus { status }) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                message: "no active session exists yet; set a goal first".to_string(),
                            });
                            continue;
                        };
                        if status == ThreadGoalStatus::BudgetLimited {
                            let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                message: "budget-limited status is controlled by the system".to_string(),
                            });
                            continue;
                        }
                        // Native goal lifecycle transition with the
                        // expectedGoalId precondition (L2-DES-APP-008).
                        let transition = match status {
                            ThreadGoalStatus::Active => devo_client::GoalLifecycleTransition::Resume,
                            ThreadGoalStatus::Paused => devo_client::GoalLifecycleTransition::Pause,
                            ThreadGoalStatus::Complete => devo_client::GoalLifecycleTransition::Complete,
                            ThreadGoalStatus::BudgetLimited => unreachable!("rejected above"),
                        };
                        let transition_result = match client
                            .session_goal_read_native(active_session_id)
                            .await
                        {
                            Ok(read) => match read.goal {
                                Some(goal) => {
                                    client
                                        .session_goal_transition_native(
                                            active_session_id,
                                            &goal.id,
                                            transition,
                                        )
                                        .await
                                        .map_err(|error| error.to_string())
                                }
                                None => Err("no goal is currently set.".to_string()),
                            },
                            Err(error) => Err(error.to_string()),
                        };
                        match transition_result {
                            Ok(goal) => {
                                if let Some(goal) = goal.as_ref() {
                                    let _ = event_tx.send(WorkerEvent::GoalUpdated {
                                        goal: thread_goal_from_native(goal),
                                    });
                                }
                            }
                            Err(message) => {
                                let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                    message,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::ClearGoal) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::GoalCleared { cleared: false });
                            continue;
                        };
                        let clear_result = match client
                            .session_goal_read_native(active_session_id)
                            .await
                        {
                            Ok(read) => match read.goal {
                                Some(goal) => client
                                    .session_goal_transition_native(
                                        active_session_id,
                                        &goal.id,
                                        devo_client::GoalLifecycleTransition::Clear,
                                    )
                                    .await
                                    .map(|_| true)
                                    .map_err(|error| error.to_string()),
                                None => Ok(false),
                            },
                            Err(error) => Err(error.to_string()),
                        };
                        match clear_result {
                            Ok(cleared) => {
                                let _ = event_tx.send(WorkerEvent::GoalCleared { cleared });
                            }
                            Err(message) => {
                                let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
                                    message,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::SetSkillEnabled { path, enabled }) => {
                        match client
                            .skill_set_enabled_native(path, enabled)
                            .await
                        {
                            Ok(result) => {
                                emit_skills_list_result(
                                    result
                                        .skills
                                        .into_iter()
                                        .map(devo_server::SkillRecord::from)
                                        .collect(),
                                    event_tx,
                                    false,
                                );
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::SetMcpServerEnabled { name, enabled }) => {
                        match client
                            .mcp_set_enabled(
                                devo_protocol::native::rpc_admin::McpSetEnabledParams {
                                    name: name.clone(),
                                    enabled,
                                },
                            )
                            .await
                        {
                            Ok(result) => {
                                let _ = event_tx.send(WorkerEvent::McpServerEnabled {
                                    name,
                                    enabled,
                                    servers: result.servers,
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::McpServerEnableFailed {
                                    name,
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::StartNewSession) => {
                        if let Some(active_session_id) = session_id {
                            match pause_active_goal_before_session_leave(
                                &mut client,
                                active_session_id,
                                active_turn_id,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(error) => {
                                    emit_goal_leave_failure(event_tx, error);
                                    continue;
                                }
                            }
                        }
                        active_turn_id = None;
                        session_id = None;
                        subscribed_session_id = None;
                        active_reference_search_id = None;
                        session_cwd = config.cwd.clone();
                        input_history_cursor = None;
                        turn_count = 0;
                        total_input_tokens = 0;
                        total_output_tokens = 0;
                        total_tokens = 0;
                        total_cache_read_tokens = 0;
                        last_query_total_tokens = 0;
                        last_query_input_tokens = 0;
                        has_authoritative_usage_totals = true;
                        model = default_model.clone();
                        model_binding_id = default_model_binding_id.clone();
                        reasoning_effort_selection =
                            default_reasoning_effort_selection.clone();
                        session_permission_preset = None;
                        let _ = event_tx.send(WorkerEvent::NewSessionPrepared {
                            cwd: session_cwd.clone(),
                            model: model.clone(),
                            model_binding_id: model_binding_id.clone(),
                            reasoning_effort_selection: reasoning_effort_selection.clone(),
                            reasoning_effort: None,
                            permission_preset: default_permission_preset,
                            collaboration_mode: default_collaboration_mode,
                            active_agent_label: None,
                            last_query_total_tokens,
                            last_query_input_tokens,
                            total_cache_read_tokens,
                        });
                        let _ = emit_skills_list(&mut client, &session_cwd, event_tx, false).await;
                    }
                    Some(OperationCommand::SwitchSession(next_session_id)) => {
                        if let Some(active_session_id) =
                            session_id.filter(|session_id| *session_id != next_session_id)
                        {
                            match pause_active_goal_before_session_leave(
                                &mut client,
                                active_session_id,
                                active_turn_id,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(error) => {
                                    emit_goal_leave_failure(event_tx, error);
                                    continue;
                                }
                            }
                        }
                        active_reference_search_id = None;
                        match restore_session_native(&mut client, next_session_id).await {
                            Ok(restore) => {
                                active_turn_id = None;
                                session_id = Some(next_session_id);
                                subscribed_session_id = None;
                                child_agent_sessions.clear();
                                btw_agent_sessions.clear();
                                session_cwd = restore.session.cwd.clone();
                                session_permission_preset = Some(
                                    match restore.session.settings.permission_profile {
                                        devo_protocol::native::model::PermissionProfile::Default => {
                                            PermissionPreset::Default
                                        }
                                        devo_protocol::native::model::PermissionProfile::AutoReview => {
                                            PermissionPreset::AutoReview
                                        }
                                        devo_protocol::native::model::PermissionProfile::FullAccess => {
                                            PermissionPreset::FullAccess
                                        }
                                    },
                                );
                                input_history_cursor = None;
                                let (usage_input, usage_output, usage_total, usage_cache_read) = (
                                    restore.session.usage.total.input_tokens as usize,
                                    restore.session.usage.total.output_tokens as usize,
                                    restore.session.usage.total.total_tokens as usize,
                                    restore.session.usage.total.cache_read_input_tokens as usize,
                                );
                                let event =
                                    session_switched_event_from_restore(next_session_id, &restore);
                                let _ = event_tx.send(event);
                                ensure_session_subscription(
                                    &mut client,
                                    next_session_id,
                                    &mut subscribed_session_id,
                                    event_tx,
                                )
                                .await;
                                model = restore.session.model.model.clone();
                                model_binding_id = (restore.session.model.provider != "unknown")
                                    .then(|| restore.session.model.provider.clone());
                                reasoning_effort_selection = restore
                                    .session
                                    .settings
                                    .reasoning_effort
                                    .map(|effort| effort.to_string());
                                total_input_tokens = usage_input;
                                total_output_tokens = usage_output;
                                total_tokens = usage_total;
                                total_cache_read_tokens = usage_cache_read;
                                let _ =
                                    emit_skills_list(&mut client, &session_cwd, event_tx, false)
                                        .await;
                                last_query_total_tokens = 0;
                                last_query_input_tokens = 0;
                                has_authoritative_usage_totals = true;
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::RenameSession(title)) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::SessionRenameFailed {
                                session_id: None,
                                message: "no active session exists yet; send a prompt or switch to a saved session first".to_string(),
                            });
                            continue;
                        };
                        match rename_persisted_session(&mut client, active_session_id, title).await {
                            Ok(title) => {
                                let _ = event_tx.send(WorkerEvent::SessionRenamed {
                                    session_id: active_session_id.to_string(),
                                    title,
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::SessionRenameFailed {
                                    session_id: Some(active_session_id),
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::RenameSessionById {
                        session_id: target_session_id,
                        title,
                    }) => {
                        match rename_persisted_session(&mut client, target_session_id, title).await {
                            Ok(title) => {
                                let _ = event_tx.send(WorkerEvent::SessionRenamed {
                                    session_id: target_session_id.to_string(),
                                    title,
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::SessionRenameFailed {
                                    session_id: Some(target_session_id),
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::DeleteSession {
                        session_id: requested_session_id,
                    }) => {
                        let target_session_id = requested_session_id.or(session_id);
                        let Some(target_session_id) = target_session_id else {
                            let _ = event_tx.send(WorkerEvent::SessionDeleteFailed {
                                session_id: None,
                                message: "no active session exists yet; send a prompt or switch to a saved session first".to_string(),
                            });
                            continue;
                        };
                        let deleting_active = session_id == Some(target_session_id);
                        if deleting_active {
                            match pause_active_goal_before_session_leave(
                                &mut client,
                                target_session_id,
                                active_turn_id,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(error) => {
                                    emit_goal_leave_failure(event_tx, error);
                                    continue;
                                }
                            }
                        }
                        match client.session_delete_native(target_session_id).await
                        {
                            Ok(_) => {
                                let _ = event_tx.send(WorkerEvent::SessionDeleted {
                                    session_id: target_session_id.to_string(),
                                });
                                if deleting_active {
                                    active_turn_id = None;
                                    session_id = None;
                                    subscribed_session_id = None;
                                    active_reference_search_id = None;
                                    session_cwd = config.cwd.clone();
                                    input_history_cursor = None;
                                    turn_count = 0;
                                    total_input_tokens = 0;
                                    total_output_tokens = 0;
                                    total_tokens = 0;
                                    total_cache_read_tokens = 0;
                                    last_query_total_tokens = 0;
                                    last_query_input_tokens = 0;
                                    has_authoritative_usage_totals = true;
                                    model = default_model.clone();
                                    model_binding_id = default_model_binding_id.clone();
                                    reasoning_effort_selection =
                                        default_reasoning_effort_selection.clone();
                                    session_permission_preset = None;
                                    let _ = event_tx.send(WorkerEvent::NewSessionPrepared {
                                        cwd: session_cwd.clone(),
                                        model: model.clone(),
                                        model_binding_id: model_binding_id.clone(),
                                        reasoning_effort_selection: reasoning_effort_selection.clone(),
                                        reasoning_effort: None,
                                        permission_preset: default_permission_preset,
                                        collaboration_mode: default_collaboration_mode,
                                        active_agent_label: None,
                                        last_query_total_tokens,
                                        last_query_input_tokens,
                                        total_cache_read_tokens,
                                    });
                                    let _ =
                                        emit_skills_list(&mut client, &session_cwd, event_tx, false)
                                            .await;
                                }
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::SessionDeleteFailed {
                                    session_id: Some(target_session_id),
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::RollbackUserTurn {
                        user_turn_index,
                        mode,
                    }) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "no active session exists yet; send a prompt or switch to a saved session first".to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                            continue;
                        };
                        if let Err(error) = pause_active_goal_before_session_leave(
                            &mut client,
                            active_session_id,
                            active_turn_id,
                        )
                        .await
                        {
                            emit_goal_leave_failure(event_tx, error);
                            continue;
                        }
                        // Native rollback (L2-DES-APP-008):
                        // preview → commit → canonical transcript
                        // restore, mirroring the fork arm. When a
                        // git checkpoint exists, commit also
                        // restores the workspace (the canonical
                        // restore-plan semantics); otherwise it is
                        // history-only like the legacy verb.
                        let rollback_restore = async {
                            let plan = client
                                .session_rollback_preview_native(
                                    active_session_id,
                                    user_turn_index,
                                    mode,
                                )
                                .await?;
                            client
                                .session_rollback_commit_native(
                                    plan.restore_plan_id,
                                    plan.workspace_version,
                                )
                                .await?;
                            restore_session_native(&mut client, active_session_id).await
                        }
                        .await;
                        match rollback_restore {
                            Ok(restore) => {
                                active_turn_id = None;
                                session_cwd = restore.session.cwd.clone();
                                input_history_cursor = None;
                                let (usage_input, usage_output, usage_total, usage_cache_read) = (
                                    restore.session.usage.total.input_tokens as usize,
                                    restore.session.usage.total.output_tokens as usize,
                                    restore.session.usage.total.total_tokens as usize,
                                    restore.session.usage.total.cache_read_input_tokens as usize,
                                );
                                let event = session_switched_event_from_restore(
                                    active_session_id,
                                    &restore,
                                );
                                let _ = event_tx.send(event);
                                ensure_session_subscription(
                                    &mut client,
                                    active_session_id,
                                    &mut subscribed_session_id,
                                    event_tx,
                                )
                                .await;
                                model = restore.session.model.model.clone();
                                model_binding_id = (restore.session.model.provider != "unknown")
                                    .then(|| restore.session.model.provider.clone());
                                reasoning_effort_selection = restore
                                    .session
                                    .settings
                                    .reasoning_effort
                                    .map(|effort| effort.to_string());
                                total_input_tokens = usage_input;
                                total_output_tokens = usage_output;
                                total_tokens = usage_total;
                                total_cache_read_tokens = usage_cache_read;
                                last_query_total_tokens = 0;
                                last_query_input_tokens = 0;
                                has_authoritative_usage_totals = true;
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::ForkAtUserTurn {
                        user_turn_index,
                        cut,
                    }) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "no active session exists yet; send a prompt or switch to a saved session first".to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                            continue;
                        };
                        match pause_active_goal_before_session_leave(
                            &mut client,
                            active_session_id,
                            active_turn_id,
                        )
                        .await
                        {
                            Ok(()) => {}
                            Err(error) => {
                                emit_goal_leave_failure(event_tx, error);
                                continue;
                            }
                        }
                        let fork_at = match turn_id_for_user_turn_index(
                            &mut client,
                            active_session_id,
                            user_turn_index,
                        )
                        .await
                        {
                            Ok(turn_id) => Some(turn_id),
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                                continue;
                            }
                        };
                        match client
                            .session_fork_native_with_cut(active_session_id, fork_at, Some(cut))
                            .await
                        {
                            Ok(result) => {
                                let next_session_id = SessionId::try_from(
                                    result.session.id.as_str(),
                                )
                                .map_err(|error| {
                                    anyhow::anyhow!("invalid forked session id: {error}")
                                })?;
                                match restore_session_native(&mut client, next_session_id)
                                    .await
                                {
                                    Ok(restore) => {
                                        active_turn_id = None;
                                        session_id = Some(next_session_id);
                                        subscribed_session_id = None;
                                        child_agent_sessions.clear();
                                        btw_agent_sessions.clear();
                                        session_cwd = restore.session.cwd.clone();
                                        input_history_cursor = None;
                                        let (usage_input, usage_output, usage_total, usage_cache_read) = (
                                            restore.session.usage.total.input_tokens as usize,
                                            restore.session.usage.total.output_tokens as usize,
                                            restore.session.usage.total.total_tokens as usize,
                                            restore.session.usage.total.cache_read_input_tokens as usize,
                                        );
                                        let event = session_switched_event_from_restore(
                                            next_session_id,
                                            &restore,
                                        );
                                        let _ = event_tx.send(event);
                                        ensure_session_subscription(
                                            &mut client,
                                            next_session_id,
                                            &mut subscribed_session_id,
                                            event_tx,
                                        )
                                        .await;
                                        model = restore.session.model.model.clone();
                                        model_binding_id = (restore.session.model.provider != "unknown")
                                            .then(|| restore.session.model.provider.clone());
                                        reasoning_effort_selection = restore
                                            .session
                                            .settings
                                            .reasoning_effort
                                            .map(|effort| effort.to_string());
                                        total_input_tokens = usage_input;
                                        total_output_tokens = usage_output;
                                        total_tokens = usage_total;
                                        total_cache_read_tokens = usage_cache_read;
                                        last_query_total_tokens = 0;
                                        last_query_input_tokens = 0;
                                        has_authoritative_usage_totals = true;
                                    }
                                    Err(error) => {
                                        let _ = event_tx.send(WorkerEvent::TurnFailed {
                                            message: error.to_string(),
                                            hint: None,
                                            turn_count,
                                            total_input_tokens,
                                            total_output_tokens,
                                            total_tokens,
                                            total_cache_read_tokens,
                                            prompt_token_estimate: total_input_tokens,
                                            last_query_input_tokens,
                                        });
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::InterruptActiveWork) => {
                        if let Some(active_session_id) = session_id {
                            if let Err(error) = client
                                .session_interrupt_native(
                                    devo_protocol::native::rpc_session::SessionInterruptScope::Session {
                                        session_id: native_session_id(active_session_id),
                                    },
                                )
                                .await
                            {
                                let _ = event_tx.send(WorkerEvent::InterruptFailed {
                                    message: error.to_string(),
                                });
                            }
                        } else {
                            for process_id in active_shell_process_ids.iter().cloned().collect::<Vec<_>>() {
                                if let Err(error) = client
                                    .session_interrupt_native(
                                        devo_protocol::native::rpc_session::SessionInterruptScope::Command {
                                            process_id,
                                        },
                                    )
                                    .await
                                {
                                    let _ = event_tx.send(WorkerEvent::InterruptFailed {
                                        message: error.to_string(),
                                    });
                                }
                            }
                        }
                    }
                    Some(OperationCommand::RunBtwQuestion { question }) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::BtwFailed {
                                message: "No active session exists yet; send a message first, then try /btw.".to_string(),
                            });
                            continue;
                        };
                        // btw spawns through canonical `task/start`
                        // kind=agent (DD-7): the child session id is
                        // recovered from the returned `item_<uuid>`.
                        match client
                            .task_start_agent_native(
                                devo_protocol::native::rpc_turn::TaskStartParams::Agent {
                                    session_id: devo_protocol::native::ids::SessionId::from_string(
                                        active_session_id.to_string(),
                                    ),
                                    input: vec![devo_protocol::native::item::UserInput::Text {
                                        text: btw_agent_prompt(&question),
                                    }],
                                    fork_turns: Some("all".to_string()),
                                    max_turns: Some(1),
                                    tool_policy: Some(devo_protocol::AgentToolPolicy::DenyAll),
                                    ephemeral: true,
                                    idempotency_key: devo_protocol::SessionId::new().to_string(),
                                },
                            )
                            .await
                        {
                            Ok(result) => {
                                match result.item_id.as_str().strip_prefix("item_")
                                    .and_then(|rest| SessionId::try_from(rest).ok())
                                {
                                    Some(child_session_id) => {
                                        btw_agent_sessions.insert(
                                            child_session_id,
                                            BtwQuestionState {
                                                parent_session_id: active_session_id,
                                                question: question.clone(),
                                                latest_answer: None,
                                            },
                                        );
                                        let _ = event_tx.send(WorkerEvent::BtwStarted { question });
                                    }
                                    None => {
                                        let _ = event_tx.send(WorkerEvent::BtwFailed {
                                            message: format!(
                                                "server returned an invalid agent item id: {}",
                                                result.item_id.as_str()
                                            ),
                                        });
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::BtwFailed {
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::QueuePush { input }) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: "no active session for queue push".to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                            continue;
                        };
                        let native_session =
                            native_session_id(active_session_id);
                        let push_result = client
                            .session_queue_push(
                                devo_protocol::native::rpc_turn::SessionQueuePushParams {
                                    session_id: native_session.clone(),
                                    input: crate::queue_ops::user_input_from_input_items(
                                        &input,
                                    ),
                                    client_user_message_id: None,
                                    idempotency_key: format!(
                                        "tui-queue-{}",
                                        SessionId::new()
                                    ),
                                },
                            )
                            .await;
                        match push_result {
                            Ok(
                                devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued {
                                    entry,
                                },
                            ) => {
                                let _ = emit_queue_snapshot(
                                    &mut client,
                                    &native_session,
                                    devo_protocol::native::queue::QueueChange::Added,
                                    entry.queue_item_id,
                                    /*started_turn_id*/ None,
                                    event_tx,
                                )
                                .await;
                            }
                            Ok(
                                devo_protocol::native::rpc_turn::SessionQueuePushResult::Started {
                                    turn,
                                },
                            ) => {
                                let started_turn_id =
                                    TurnId::try_from(turn.id.as_str()).ok();
                                if let Some(turn_id) = started_turn_id {
                                    active_turn_id = Some(turn_id);
                                }
                                let _ = emit_queue_snapshot(
                                    &mut client,
                                    &native_session,
                                    devo_protocol::native::queue::QueueChange::Drained,
                                    devo_protocol::native::ids::QueueItemId::from_string(
                                        String::new(),
                                    ),
                                    started_turn_id,
                                    event_tx,
                                )
                                .await;
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::QueueSteer {
                        queue_item_id,
                        expected_turn_id,
                    }) => {
                        let Some(active_session_id) = session_id else {
                            continue;
                        };
                        let native_session =
                            native_session_id(active_session_id);
                        match client
                            .session_queue_steer(
                                devo_protocol::native::rpc_turn::SessionQueueSteerParams {
                                    session_id: native_session.clone(),
                                    queue_item_id:
                                        devo_protocol::native::ids::QueueItemId::from_string(
                                            queue_item_id,
                                        ),
                                    expected_turn_id:
                                        devo_protocol::native::ids::TurnId::from_string(
                                            expected_turn_id.to_string(),
                                        ),
                                },
                            )
                            .await
                        {
                            Ok(_) => {
                                let _ = event_tx.send(WorkerEvent::SteerAccepted {
                                    turn_id: expected_turn_id,
                                });
                                let _ = emit_queue_snapshot(
                                    &mut client,
                                    &native_session,
                                    devo_protocol::native::queue::QueueChange::Promoted,
                                    devo_protocol::native::ids::QueueItemId::from_string(
                                        String::new(),
                                    ),
                                    Some(expected_turn_id),
                                    event_tx,
                                )
                                .await;
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::QueueRemove { queue_item_id }) => {
                        let Some(active_session_id) = session_id else {
                            continue;
                        };
                        let native_session =
                            native_session_id(active_session_id);
                        let removed_id =
                            devo_protocol::native::ids::QueueItemId::from_string(
                                queue_item_id.clone(),
                            );
                        match client
                            .session_queue_remove(
                                devo_protocol::native::rpc_turn::SessionQueueRemoveParams {
                                    session_id: native_session.clone(),
                                    queue_item_id: removed_id.clone(),
                                },
                            )
                            .await
                        {
                            Ok(_) => {
                                let _ = emit_queue_snapshot(
                                    &mut client,
                                    &native_session,
                                    devo_protocol::native::queue::QueueChange::Removed,
                                    removed_id,
                                    /*started_turn_id*/ None,
                                    event_tx,
                                )
                                .await;
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::QueueUpdate {
                        queue_item_id,
                        input,
                    }) => {
                        let Some(active_session_id) = session_id else {
                            continue;
                        };
                        let native_session =
                            native_session_id(active_session_id);
                        let updated_id =
                            devo_protocol::native::ids::QueueItemId::from_string(
                                queue_item_id.clone(),
                            );
                        match client
                            .session_queue_update(
                                devo_protocol::native::rpc_turn::SessionQueueUpdateParams {
                                    session_id: native_session.clone(),
                                    queue_item_id: updated_id.clone(),
                                    input: Some(
                                        crate::queue_ops::user_input_from_input_items(
                                            &input,
                                        ),
                                    ),
                                    position: None,
                                },
                            )
                            .await
                        {
                            Ok(_) => {
                                let _ = emit_queue_snapshot(
                                    &mut client,
                                    &native_session,
                                    devo_protocol::native::queue::QueueChange::Updated,
                                    updated_id,
                                    /*started_turn_id*/ None,
                                    event_tx,
                                )
                                .await;
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::TurnFailed {
                                    message: error.to_string(),
                                    hint: None,
                                    turn_count,
                                    total_input_tokens,
                                    total_output_tokens,
                                    total_tokens,
                                    total_cache_read_tokens,
                                    prompt_token_estimate: total_input_tokens,
                                    last_query_input_tokens,
                                });
                            }
                        }
                    }
                    Some(OperationCommand::ApprovalRespond {
                        session_id,
                        turn_id,
                        approval_id,
                        decision,
                        scope,
                    }) => {
                        if let Err(error) = client
                            .approval_respond(ApprovalResponseParams {
                                session_id,
                                turn_id,
                                approval_id: approval_id.into(),
                                decision,
                                scope,
                            })
                            .await
                        {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: error.to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                        }
                    }
                    Some(OperationCommand::RequestUserInputRespond {
                        session_id: _,
                        turn_id: _,
                        request_id,
                        response,
                    }) => {
                        if let Err(error) = client
                            .request_user_input_respond(request_id, response)
                            .await
                        {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: error.to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                        }
                    }
                    Some(OperationCommand::UpdatePermissions { preset, persist_scope }) => {
                        match persist_scope {
                            crate::app_command::PersistScope::Default => {
                                default_permission_preset = preset;
                            }
                            crate::app_command::PersistScope::Session => {
                                session_permission_preset = Some(preset);
                            }
                        }
                        let Some(active_session_id) = session_id else {
                            continue;
                        };
                        if let Err(error) =
                            apply_session_permissions(&mut client, active_session_id, preset).await
                        {
                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                message: error.to_string(),
                                hint: None,
                                turn_count,
                                total_input_tokens,
                                total_output_tokens,
                                total_tokens,
                                total_cache_read_tokens,
                                prompt_token_estimate: total_input_tokens,
                                last_query_input_tokens,
                            });
                        }
                    }
                    Some(OperationCommand::UpdateEffectiveContextWindow {
                        effective_context_window,
                    }) => {
                        let Some(active_session_id) = session_id else {
                            let _ = event_tx.send(
                                WorkerEvent::EffectiveContextWindowUpdated {
                                    effective_context_window,
                                },
                            );
                            continue;
                        };
                        match client
                            .session_settings_update(
                                active_session_id,
                                devo_protocol::native::rpc_session::SessionSettingsPatch {
                                    effective_context_window: Some(effective_context_window),
                                    ..Default::default()
                                },
                            )
                            .await
                        {
                            Ok(result) => {
                                // The server echoes the clamped
                                // per-session value.
                                let applied = result
                                    .session
                                    .settings
                                    .effective_context_window
                                    .unwrap_or(effective_context_window);
                                let _ = event_tx.send(WorkerEvent::EffectiveContextWindowUpdated {
                                    effective_context_window: applied,
                                });
                            }
                            Err(error) => {
                                let _ = event_tx.send(WorkerEvent::InterruptFailed {
                                    message: format!(
                                        "Failed to update compaction threshold: {error}"
                                    ),
                                });
                            }
                        }
                    }
                    Some(OperationCommand::UpdateSandboxProfile { profile }) => {
                        let Some(active_session_id) = session_id else {
                            continue;
                        };
                        if let Err(error) = client
                            .session_settings_update(
                                active_session_id,
                                devo_protocol::native::rpc_session::SessionSettingsPatch {
                                    sandbox_profile: Some(profile),
                                    ..Default::default()
                                },
                            )
                            .await
                        {
                            let _ = event_tx.send(WorkerEvent::InterruptFailed {
                                message: format!("Failed to update sandbox profile: {error}"),
                            });
                        }
                    }
                    Some(OperationCommand::BrowseInputHistory(direction)) => {
                        let text = if let Some(active_session_id) = session_id {
                            match collect_user_input_texts(&mut client, active_session_id).await {
                                Ok(entries) => {
                                    let total = entries.len();
                                    match direction {
                                        InputHistoryDirection::Previous => {
                                            if total == 0 {
                                                None
                                            } else {
                                                let next_index = match input_history_cursor {
                                                    None => total.saturating_sub(1),
                                                    Some(0) => 0,
                                                    Some(index) => index.saturating_sub(1),
                                                };
                                                input_history_cursor = Some(next_index);
                                                entries.get(next_index).cloned()
                                            }
                                        }
                                        InputHistoryDirection::Next => match input_history_cursor {
                                            None => None,
                                            Some(index) if index + 1 >= total => {
                                                input_history_cursor = None;
                                                None
                                            }
                                            Some(index) => {
                                                let next_index = index + 1;
                                                input_history_cursor = Some(next_index);
                                                entries.get(next_index).cloned()
                                            }
                                        },
                                    }
                                }
                                Err(error) => {
                                    let _ = event_tx.send(WorkerEvent::TurnFailed {
                                        message: error.to_string(),
                                        hint: None,
                                        turn_count,
                                        total_input_tokens,
                                        total_output_tokens,
                                        total_tokens,
                                        total_cache_read_tokens,
                                        prompt_token_estimate: total_input_tokens,
                                        last_query_input_tokens,
                                    });
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        let _ = event_tx.send(WorkerEvent::InputHistoryLoaded { direction, text });
                    }
                    Some(OperationCommand::Shutdown) | None => {
                        tracing::info!("query worker received shutdown command");
                        break;
                    }
                }
            }
            notification = client.recv_notification() => {
                match notification {
                    Some(notification) => {
                        let method = notification.method;
                        let params = notification.params;
                        if method == "queue/updated"
                            && let Ok(queue_event) =
                                parse_native_queue_updated(&params)
                        {
                            let _ = event_tx.send(queue_event);
                            continue;
                        }
                        // Native typed events (L2-DES-APP-009): canonical
                        // notification shapes used by the TUI wire client.
                        if params.get("kind").is_none() {
                            match method.as_str() {
                                "turn/started" => {
                                    if let Ok(turn) = serde_json::from_value::<
                                        devo_protocol::native::turn::Turn,
                                    >(params["turn"].clone())
                                        && let Ok(turn_id) = devo_protocol::TurnId::try_from(
                                            turn.id.as_str(),
                                        )
                                    {
                                        let turn_session_id =
                                            SessionId::try_from(turn.session_id.as_str()).ok();
                                        if turn_session_id != session_id {
                                            // Child-session turn (L2-DES-APP-009):
                                            // feed the subagent monitor, never
                                            // the main session's state.
                                            if let Some(child_id) = turn_session_id
                                                && child_agent_sessions.contains(&child_id)
                                            {
                                                let _ = event_tx.send(
                                                    WorkerEvent::SubagentMonitor {
                                                        event: SubagentMonitorEvent::TurnStarted {
                                                            session_id: child_id,
                                                            turn_id,
                                                        },
                                                    },
                                                );
                                            }
                                            continue;
                                        }
                                        active_turn_id = Some(turn_id);
                                        seen_terminal_item_ids.clear();
                                        seen_terminal_call_ids.clear();
                                        saw_usage_update_for_turn = false;
                                        model = turn.model.model.clone();
                                        model_binding_id =
                                            (turn.model.provider != "unknown")
                                                .then(|| turn.model.provider.clone());
                                        reasoning_effort_selection = turn
                                            .model
                                            .reasoning_effort
                                            .map(|effort| effort.to_string());
                                        let _ = event_tx.send(WorkerEvent::TurnStarted {
                                            model: turn.model.model,
                                            model_binding_id: model_binding_id.clone(),
                                            reasoning_effort_selection:
                                                reasoning_effort_selection.clone(),
                                            reasoning_effort: turn.model.reasoning_effort,
                                            turn_id,
                                        });
                                        latest_completed_agent_message = None;
                                    }
                                    continue;
                                }
                                "turn/completed" => {
                                    if let Ok(turn) = serde_json::from_value::<
                                        devo_protocol::native::turn::Turn,
                                    >(params["turn"].clone())
                                    {
                                        let turn_session_id =
                                            SessionId::try_from(turn.session_id.as_str()).ok();
                                        if turn_session_id != session_id {
                                            // Child-session terminal turn: route to
                                            // the subagent monitor only.
                                            if let Some(child_id) = turn_session_id
                                                && child_agent_sessions.contains(&child_id)
                                            {
                                                let monitor_event = match turn.status {
                                                    devo_protocol::native::turn::TurnStatus::Failed => {
                                                        SubagentMonitorEvent::TurnFailed {
                                                            session_id: child_id,
                                                            message: turn
                                                                .error
                                                                .as_ref()
                                                                .map(|error| error.message.clone())
                                                                .unwrap_or_else(|| {
                                                                    "Turn failed".to_string()
                                                                }),
                                                        }
                                                    }
                                                    _ => SubagentMonitorEvent::TurnFinished {
                                                        session_id: child_id,
                                                        status: match turn.status {
                                                            devo_protocol::native::turn::TurnStatus::Completed => "done",
                                                            devo_protocol::native::turn::TurnStatus::Interrupted => "interrupted",
                                                            _ => "working",
                                                        }
                                                        .to_string(),
                                                    },
                                                };
                                                let _ = event_tx.send(
                                                    WorkerEvent::SubagentMonitor {
                                                        event: monitor_event,
                                                    },
                                                );
                                            }
                                            continue;
                                        }
                                        active_turn_id = None;
                                        let completed = matches!(
                                            turn.status,
                                            devo_protocol::native::turn::TurnStatus::Completed
                                                | devo_protocol::native::turn::TurnStatus::Interrupted
                                        );
                                        if completed {
                                            turn_count += 1;
                                        }
                                        if let Some(usage) = &turn.usage {
                                            let input = usage.query.input_tokens as usize;
                                            let total = usage.query.total_tokens as usize;
                                            let cache_read = usage
                                                .query
                                                .cache_read_input_tokens
                                                as usize;
                                            if !saw_usage_update_for_turn {
                                                last_query_input_tokens = input;
                                                last_query_total_tokens = total;
                                            }
                                            if should_apply_terminal_turn_usage_fallback(
                                                saw_usage_update_for_turn,
                                                has_authoritative_usage_totals,
                                            ) {
                                                total_input_tokens += input;
                                                total_output_tokens +=
                                                    usage.query.output_tokens as usize;
                                                total_tokens += total;
                                                total_cache_read_tokens += cache_read;
                                            }
                                        }
                                        let prompt_token_estimate = turn
                                            .usage
                                            .as_ref()
                                            .map(|usage| usage.query.input_tokens as usize)
                                            .unwrap_or(total_input_tokens);
                                        if matches!(
                                            turn.status,
                                            devo_protocol::native::turn::TurnStatus::Failed
                                        ) {
                                            let (message, hint) = match &turn.error {
                                                Some(error) => {
                                                    let hint = error
                                                        .details
                                                        .as_ref()
                                                        .and_then(|details| {
                                                            details
                                                                .get("recoveryHint")
                                                                .and_then(|hint| {
                                                                    hint.as_str()
                                                                        .map(str::to_string)
                                                                })
                                                        })
                                                        .or_else(|| {
                                                            devo_provider::recovery_hint_for_message(
                                                                &error.message,
                                                            )
                                                        });
                                                    (error.message.clone(), hint)
                                                }
                                                None => {
                                                    let message = latest_completed_agent_message
                                                        .take()
                                                        .unwrap_or_else(|| {
                                                            format!(
                                                                "turn failed with status {:?}",
                                                                turn.status
                                                            )
                                                        });
                                                    let hint =
                                                        devo_provider::recovery_hint_for_message(
                                                            &message,
                                                        );
                                                    (message, hint)
                                                }
                                            };
                                            let _ = event_tx.send(WorkerEvent::TurnFailed {
                                                message,
                                                hint,
                                                turn_count,
                                                total_input_tokens,
                                                total_output_tokens,
                                                total_tokens,
                                                total_cache_read_tokens,
                                                prompt_token_estimate,
                                                last_query_input_tokens,
                                            });
                                        } else {
                                            let _ = event_tx.send(WorkerEvent::TurnFinished {
                                                stop_reason: format!("{:?}", turn.status),
                                                turn_count,
                                                total_input_tokens,
                                                total_output_tokens,
                                                total_tokens,
                                                total_cache_read_tokens,
                                                last_query_total_tokens,
                                                last_query_input_tokens,
                                                prompt_token_estimate,
                                            });
                                        }
                                        latest_completed_agent_message = None;
                                    }
                                    continue;
                                }
                                "item/assistantMessage/delta" | "item/reasoning/delta" => {
                                    if let Ok(delta) = serde_json::from_value::<
                                        devo_protocol::native::event::ItemDelta,
                                    >(params)
                                    {
                                        let kind = if method == "item/assistantMessage/delta" {
                                            TextItemKind::Assistant
                                        } else {
                                            TextItemKind::Reasoning
                                        };
                                        let delta_session = SessionId::try_from(
                                            delta.session_id.as_str(),
                                        )
                                        .ok();
                                        if delta_session != session_id {
                                            // Child-session text feeds the
                                            // subagent monitor preview
                                            // (L2-DES-APP-009 cutover).
                                            if let Some(child_id) = delta_session
                                                && child_agent_sessions.contains(&child_id)
                                            {
                                                let _ = event_tx.send(
                                                    WorkerEvent::SubagentMonitor {
                                                        event: SubagentMonitorEvent::TextItemDelta {
                                                            session_id: child_id,
                                                            item_id: devo_protocol::ItemId::try_from(
                                                                delta.item_id.as_str(),
                                                            )
                                                            .ok(),
                                                            kind,
                                                            delta: delta.delta,
                                                        },
                                                    },
                                                );
                                            }
                                            continue;
                                        }
                                        if let Ok(item_id) =
                                            devo_protocol::ItemId::try_from(delta.item_id.as_str())
                                        {
                                            let _ = event_tx.send(WorkerEvent::Transcript(
                                                ItemLifecycleEvent::TextDelta {
                                                    item_id,
                                                    kind,
                                                    delta: delta.delta,
                                                },
                                            ));
                                        }
                                    }
                                    continue;
                                }
                                "item/plan/delta" => {
                                    if let Ok(delta) = serde_json::from_value::<
                                        devo_protocol::native::event::ItemDelta,
                                    >(params.clone())
                                    {
                                        let delta_session =
                                            SessionId::try_from(delta.session_id.as_str()).ok();
                                        if delta_session == session_id
                                            && let Ok(item_id) =
                                                devo_protocol::ItemId::try_from(delta.item_id.as_str())
                                        {
                                            let _ = event_tx.send(WorkerEvent::ProposedPlanDelta {
                                                item_id,
                                                delta: delta.delta,
                                            });
                                        }
                                    }
                                    continue;
                                }
                                "item/commandExecution/outputDelta" => {
                                    if let Ok(delta) = serde_json::from_value::<
                                        devo_protocol::native::event::ItemDelta,
                                    >(params)
                                        && let Ok(value) = serde_json::from_str::<
                                            serde_json::Value,
                                        >(&delta.delta)
                                    {
                                        let tool_use_id = value
                                            .get("tool_use_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        let text = value
                                            .get("text")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        if !tool_use_id.is_empty() {
                                            let _ = event_tx.send(WorkerEvent::Transcript(
                                                tool_lifecycle::transcript_tool_output_chunk(
                                                    tool_use_id.to_string(),
                                                    text.to_string(),
                                                ),
                                            ));
                                        }
                                    }
                                    continue;
                                }
                                "item/toolCall/inputDelta" => {
                                    if let Ok(delta) = serde_json::from_value::<
                                        devo_protocol::native::event::ItemDelta,
                                    >(params)
                                        && let Some(event) =
                                            tool_lifecycle::transcript_tool_input_chunk_from_delta_payload(
                                                &delta.delta,
                                            )
                                    {
                                        let _ = event_tx.send(WorkerEvent::Transcript(event));
                                    }
                                    continue;
                                }
                                "session/created" => {
                                    if let Ok(session) = serde_json::from_value::<
                                        devo_protocol::native::session::Session,
                                    >(params["session"].clone())
                                        && let Some(agent) =
                                            subagent_events::agent_from_native_session(&session)
                                        && Some(agent.parent_session_id) == session_id
                                        && child_agent_sessions.insert(agent.session_id)
                                    {
                                        let _ = event_tx.send(WorkerEvent::SubagentDiscovered {
                                            agent,
                                        });
                                    }
                                    continue;
                                }
                                "session/metadataUpdated" => {
                                    if let Ok(session) = serde_json::from_value::<
                                        devo_protocol::native::session::Session,
                                    >(params["session"].clone())
                                        && SessionId::try_from(session.id.as_str())
                                            .ok()
                                            .is_some_and(|id| Some(id) == session_id)
                                        && let Some(title) = session.title
                                    {
                                        let _ = event_tx.send(WorkerEvent::SessionTitleUpdated {
                                            session_id: session.id.to_string(),
                                            title,
                                        });
                                    }
                                    continue;
                                }
                                "session/statusChanged" => {
                                    let changed_session_id = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok());
                                    let status = params["status"].as_str().unwrap_or("idle");
                                    if let Some(changed_session_id) = changed_session_id
                                        && child_agent_sessions.contains(&changed_session_id)
                                    {
                                        let status = match status {
                                            "active" => devo_protocol::SessionRuntimeStatus::ActiveTurn,
                                            _ => devo_protocol::SessionRuntimeStatus::Idle,
                                        };
                                        let _ = event_tx.send(WorkerEvent::SubagentMonitor {
                                            event: SubagentMonitorEvent::SessionStatusChanged {
                                                session_id: changed_session_id,
                                                status,
                                            },
                                        });
                                    } else if changed_session_id.is_some_and(|id| Some(id) == session_id)
                                        && status != "active"
                                        && let (Some(active_session_id), Some(finished_turn_id)) =
                                            (session_id, active_turn_id)
                                    {
                                        tracing::warn!(
                                            turn_id = %finished_turn_id,
                                            "turn/completed not observed; reconciling authoritative turn state"
                                        );
                                        match reconcile_idle_turn(
                                            &mut client,
                                            active_session_id,
                                            finished_turn_id,
                                            &mut seen_terminal_item_ids,
                                            &mut seen_terminal_call_ids,
                                            event_tx,
                                        )
                                        .await
                                        {
                                            Ok(turn) => {
                                                active_turn_id = None;
                                                if matches!(
                                                    turn.status,
                                                    devo_protocol::native::turn::TurnStatus::Completed
                                                        | devo_protocol::native::turn::TurnStatus::Interrupted
                                                ) {
                                                    turn_count += 1;
                                                }
                                                if let Some(usage) = &turn.usage {
                                                    let input = usage.query.input_tokens as usize;
                                                    let total = usage.query.total_tokens as usize;
                                                    let cache_read = usage.query.cache_read_input_tokens as usize;
                                                    if !saw_usage_update_for_turn {
                                                        last_query_input_tokens = input;
                                                        last_query_total_tokens = total;
                                                    }
                                                    if should_apply_terminal_turn_usage_fallback(
                                                        saw_usage_update_for_turn,
                                                        has_authoritative_usage_totals,
                                                    ) {
                                                        total_input_tokens += input;
                                                        total_output_tokens +=
                                                            usage.query.output_tokens as usize;
                                                        total_tokens += total;
                                                        total_cache_read_tokens += cache_read;
                                                    }
                                                }
                                                if let Some(usage) = &turn.usage {
                                                    let input = usage.query.input_tokens as usize;
                                                    let total = usage.query.total_tokens as usize;
                                                    let cache_read = usage.query.cache_read_input_tokens as usize;
                                                    if !saw_usage_update_for_turn {
                                                        last_query_input_tokens = input;
                                                        last_query_total_tokens = total;
                                                    }
                                                    if should_apply_terminal_turn_usage_fallback(
                                                        saw_usage_update_for_turn,
                                                        has_authoritative_usage_totals,
                                                    ) {
                                                        total_input_tokens += input;
                                                        total_output_tokens +=
                                                            usage.query.output_tokens as usize;
                                                        total_tokens += total;
                                                        total_cache_read_tokens += cache_read;
                                                    }
                                                }
                                                let prompt_token_estimate = turn
                                                    .usage
                                                    .as_ref()
                                                    .map(|usage| usage.query.input_tokens as usize)
                                                    .unwrap_or(total_input_tokens);
                                                if turn.status
                                                    == devo_protocol::native::turn::TurnStatus::Failed
                                                {
                                                    let message = turn
                                                        .error
                                                        .as_ref()
                                                        .map(|error| error.message.clone())
                                                        .unwrap_or_else(|| "Turn failed".to_string());
                                                    let _ = event_tx.send(WorkerEvent::TurnFailed {
                                                        hint: devo_provider::recovery_hint_for_message(&message),
                                                        message,
                                                        turn_count,
                                                        total_input_tokens,
                                                        total_output_tokens,
                                                        total_tokens,
                                                        total_cache_read_tokens,
                                                        prompt_token_estimate,
                                                        last_query_input_tokens,
                                                    });
                                                } else {
                                                    let _ = event_tx.send(WorkerEvent::TurnFinished {
                                                        stop_reason: format!("{:?}", turn.status),
                                                        turn_count,
                                                        total_input_tokens,
                                                        total_output_tokens,
                                                        total_tokens,
                                                        total_cache_read_tokens,
                                                        last_query_total_tokens,
                                                        last_query_input_tokens,
                                                        prompt_token_estimate,
                                                    });
                                                }
                                                latest_completed_agent_message = None;
                                            }
                                            Err(error) => {
                                                tracing::warn!(
                                                    turn_id = %finished_turn_id,
                                                    %error,
                                                    "idle turn reconciliation failed; preserving live tool state"
                                                );
                                                let _ = event_tx.send(WorkerEvent::InterruptFailed {
                                                    message: "Turn ended, but final tool results are unavailable; preserving live state".to_string(),
                                                });
                                            }
                                        }
                                    }
                                    continue;
                                }
                                "session/deleted" => {
                                    let deleted_current_session = params["deletedSessionIds"]
                                        .as_array()
                                        .is_some_and(|ids| {
                                            ids.iter().any(|id| {
                                                id.as_str()
                                                    .and_then(|id| SessionId::try_from(id).ok())
                                                    == session_id
                                            })
                                        });
                                    if deleted_current_session
                                        && let Some(session_id) = session_id
                                    {
                                        let _ = event_tx.send(WorkerEvent::SessionDeleted {
                                            session_id: session_id.to_string(),
                                        });
                                    }
                                    continue;
                                }
                                "session/archived"
                                | "session/closed"
                                | "workspace/changes/updated" => continue,
                                "context/usageUpdated" => {
                                    let event_session_matches = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok())
                                        .is_some_and(|id| Some(id) == session_id);
                                    if event_session_matches
                                        && let Ok(occupancy) = serde_json::from_value::<
                                            devo_protocol::native::item::ContextOccupancy,
                                        >(params["occupancy"].clone())
                                    {
                                        last_query_total_tokens =
                                            occupancy.total_tokens as usize;
                                        let _ = event_tx.send(
                                            WorkerEvent::ContextUsageUpdated { occupancy },
                                        );
                                    }
                                    continue;
                                }
                                "item/started" | "item/completed" => {
                                    match serde_json::from_value::<
                                        devo_protocol::TypedItemEventPayload,
                                    >(params.clone())
                                    {
                                        Ok(payload) => {
                                        // Child-session items belong to
                                        // the subagent monitor, not the
                                        // main transcript (L2-DES-APP-009).
                                        let item_session_id =
                                            SessionId::try_from(payload.item.session_id.as_str())
                                                .ok();
                                        if item_session_id == session_id {
                                            if method == "item/completed" {
                                                let item_id = payload.item.id.to_string();
                                                if !seen_terminal_item_ids.insert(item_id) {
                                                    continue;
                                                }
                                                let call_id = match &payload.item.item {
                                                    devo_protocol::native::item::Item::ToolResult {
                                                        call_id,
                                                        ..
                                                    }
                                                    | devo_protocol::native::item::Item::CommandExecution {
                                                        call_id,
                                                        ..
                                                    }
                                                    | devo_protocol::native::item::Item::FileChange {
                                                        call_id,
                                                        ..
                                                    } => Some(call_id.clone()),
                                                    _ => None,
                                                };
                                                if let Some(call_id) = call_id
                                                    && !seen_terminal_call_ids.insert(call_id)
                                                {
                                                    continue;
                                                }
                                            }
                                            if method == "item/completed"
                                                && let devo_protocol::native::item::Item::AssistantMessage {
                                                    text,
                                                    ..
                                                } = &payload.item.item
                                            {
                                                let text = text.trim();
                                                if !text.is_empty() {
                                                    latest_completed_agent_message =
                                                        Some(text.to_string());
                                                }
                                            }
                                            if method == "item/completed"
                                                && let devo_protocol::native::item::Item::UserInputRequest {
                                                    request_id,
                                                    ..
                                                } = &payload.item.item
                                            {
                                                let _ = event_tx.send(
                                                    WorkerEvent::UserInputResolved {
                                                        request_id: request_id.clone(),
                                                    },
                                                );
                                            }
                                            if method == "item/completed"
                                                && let devo_protocol::native::item::Item::ToolResult {
                                                    output,
                                                    ..
                                                } = &payload.item.item
                                                && let Some(parent_session_id) = session_id
                                            {
                                                maybe_discover_spawned_subagent_from_tool_output(
                                                    Some(output),
                                                    &mut client,
                                                    parent_session_id,
                                                    &mut child_agent_sessions,
                                                    event_tx,
                                                )
                                                .await;
                                            }
                                            match devo_core::ItemId::try_from(
                                                payload.item.id.as_str(),
                                            ) {
                                                Ok(item_id) => {
                                                    item_dispatch::dispatch_typed_item_lifecycle(
                                                        &method, &payload, item_id, event_tx,
                                                    );
                                                }
                                                Err(error) => {
                                                    // A malformed id must not take the
                                                    // whole worker down: the row is
                                                    // skipped and the turn continues.
                                                    tracing::warn!(
                                                        method = %method,
                                                        item_id = %payload.item.id,
                                                        %error,
                                                        "dropping typed item with unparseable id"
                                                    );
                                                }
                                            }
                                        } else if let Some(child_id) = item_session_id
                                            && child_agent_sessions.contains(&child_id)
                                        {
                                            for event in subagent_events::subagent_monitor_events_from_typed_item(
                                                child_id,
                                                &payload.item,
                                            ) {
                                                let _ = event_tx.send(event);
                                            }
                                        }
                                        }
                                        Err(error) => {
                                            // Schema drift or a foreign payload must not
                                            // silently swallow tool completions.
                                            tracing::warn!(
                                                method = %method,
                                                %error,
                                                "failed to decode typed item event"
                                            );
                                        }
                                    }
                                    continue;
                                }
                                "model/queryRetrying" => {
                                    let event_session_matches = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok())
                                        .is_some_and(|id| Some(id) == session_id);
                                    let turn_id = params["turnId"]
                                        .as_str()
                                        .and_then(|id| TurnId::try_from(id).ok());
                                    if event_session_matches && let Some(turn_id) = turn_id {
                                        let phase = match params["phase"].as_str() {
                                            Some("resumed") => {
                                                devo_protocol::ProviderRetryPhase::Resumed
                                            }
                                            _ => {
                                                devo_protocol::ProviderRetryPhase::Scheduled
                                            }
                                        };
                                        let _ = event_tx.send(WorkerEvent::ProviderRetryStatus {
                                            turn_id,
                                            attempt: params["attempt"].as_u64().unwrap_or(0)
                                                as usize,
                                            backoff_ms: params["nextDelayMs"]
                                                .as_u64()
                                                .unwrap_or(0),
                                            provider: params["provider"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string(),
                                            model: params["model"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string(),
                                            phase,
                                            message: params["error"]["message"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string(),
                                        });
                                    }
                                    continue;
                                }
                                "turn/usage/updated" => {
                                    let event_session_matches = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok())
                                        .is_some_and(|id| Some(id) == session_id);
                                    if !event_session_matches {
                                        continue;
                                    }
                                    saw_usage_update_for_turn = true;
                                    if let Some(totals) = params.get("sessionTotals") {
                                        total_input_tokens = totals["inputTokens"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize;
                                        total_output_tokens = totals["outputTokens"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize;
                                        total_tokens = totals["totalTokens"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize;
                                        total_cache_read_tokens =
                                            totals["cacheReadInputTokens"]
                                                .as_u64()
                                                .unwrap_or(0)
                                                as usize;
                                    }
                                    last_query_total_tokens =
                                        params["usage"]["query"]["totalTokens"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize;
                                    last_query_input_tokens =
                                        params["lastQueryInputTokens"]
                                            .as_u64()
                                            .unwrap_or(0)
                                            as usize;
                                    has_authoritative_usage_totals = true;
                                    let _ = event_tx.send(WorkerEvent::UsageUpdated {
                                        total_input_tokens,
                                        total_output_tokens,
                                        total_tokens,
                                        total_cache_read_tokens,
                                        last_query_total_tokens,
                                        last_query_input_tokens,
                                    });
                                    continue;
                                }
                                "item/updated" => {
                                    if let Ok(payload) = serde_json::from_value::<
                                        devo_protocol::TypedItemEventPayload,
                                    >(params.clone())
                                        && SessionId::try_from(payload.item.session_id.as_str())
                                            .ok()
                                            .is_some_and(|id| Some(id) == session_id)
                                        && let devo_protocol::native::item::Item::Plan {
                                            entries,
                                        } = &payload.item.item
                                    {
                                        let steps = entries
                                            .iter()
                                            .map(|entry| PlanStep {
                                                text: entry.step.clone(),
                                                status: match entry.status {
                                                    devo_protocol::native::item::PlanStepStatus::Pending => PlanStepStatus::Pending,
                                                    devo_protocol::native::item::PlanStepStatus::InProgress => PlanStepStatus::InProgress,
                                                    devo_protocol::native::item::PlanStepStatus::Completed => PlanStepStatus::Completed,
                                                },
                                            })
                                            .collect();
                                        let _ = event_tx.send(WorkerEvent::PlanUpdated {
                                            explanation: None,
                                            steps,
                                        });
                                    }
                                    continue;
                                }
                                "context/compactionStarted" => {
                                    let event_session_matches = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok())
                                        .is_some_and(|id| Some(id) == session_id);
                                    if event_session_matches {
                                        let _ = event_tx.send(WorkerEvent::SessionCompactionStarted);
                                    }
                                    continue;
                                }
                                "context/compactionCompleted" => {
                                    let event_session_matches = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok())
                                        .is_some_and(|id| Some(id) == session_id);
                                    if event_session_matches {
                                        // Token totals arrive on the accompanying usage /
                                        // session events; this surfaces busy-state clear.
                                        let _ = event_tx.send(WorkerEvent::SessionCompacted {
                                            total_input_tokens,
                                            total_output_tokens,
                                            total_tokens,
                                            last_query_total_tokens,
                                            last_query_input_tokens,
                                            prompt_token_estimate: total_input_tokens,
                                        });
                                    }
                                    continue;
                                }
                                "context/compactionFailed" => {
                                    let event_session_matches = params["sessionId"]
                                        .as_str()
                                        .and_then(|id| SessionId::try_from(id).ok())
                                        .is_some_and(|id| Some(id) == session_id);
                                    if event_session_matches {
                                        let message = params["message"]
                                            .as_str()
                                            .unwrap_or("Context compaction failed")
                                            .to_string();
                                        let _ = event_tx.send(
                                            WorkerEvent::SessionCompactionFailed { message },
                                        );
                                    }
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        tracing::debug!(method = %method, "ignoring unsupported non-Native notification");
                    }
                    None => break,
                }
            }
        }
    }

    tracing::info!("query worker shutting down stdio client");
    client.shutdown().await?;
    tracing::info!("query worker stdio client shutdown completed");
    Ok(())
}

fn stream_trace_elapsed_ms() -> u128 {
    static STREAM_TRACE_START: OnceLock<Instant> = OnceLock::new();
    STREAM_TRACE_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

fn assistant_token_log_preview(text: &str) -> Option<String> {
    assistant_token_logging_enabled()
        .then(|| format_assistant_token_log_preview(text, assistant_token_log_max_chars()))
}

fn assistant_token_logging_enabled() -> bool {
    static ASSISTANT_TOKEN_LOGGING_ENABLED: OnceLock<bool> = OnceLock::new();
    *ASSISTANT_TOKEN_LOGGING_ENABLED.get_or_init(|| {
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
    let mut preview = String::new();
    let mut chars = text.chars();
    for ch in chars.by_ref().take(max_chars) {
        preview.extend(ch.escape_default());
    }
    if chars.next().is_some() {
        preview.push_str("...");
    }
    preview
}

async fn ensure_session_started(
    client: &mut StdioServerClient,
    cwd: &Path,
    model: &str,
    model_binding_id: &Option<String>,
    session_id: &mut Option<SessionId>,
) -> Result<EnsureSessionOutcome> {
    if let Some(session_id) = session_id {
        return Ok(EnsureSessionOutcome {
            session_id: *session_id,
            model: Some(model.to_string()),
            model_binding_id: model_binding_id.clone(),
            reasoning_effort_selection: None,
            reasoning_effort: None,
            created: false,
        });
    }

    // Native `session/new` (L2-DES-APP-008): the server resolves the
    // model from configuration; the returned canonical session reports it.
    let session = client
        .session_new_native(
            cwd.to_path_buf(),
            devo_protocol::SessionId::new().to_string(),
        )
        .await?;
    let legacy_session_id = SessionId::try_from(session.session.id.as_str())
        .map_err(|error| anyhow::anyhow!("invalid session id from server: {error}"))?;
    *session_id = Some(legacy_session_id);
    Ok(EnsureSessionOutcome {
        session_id: legacy_session_id,
        model: Some(session.session.model.model),
        model_binding_id: model_binding_id.clone(),
        reasoning_effort_selection: session
            .session
            .model
            .reasoning_effort
            .map(|effort| effort.to_string()),
        reasoning_effort: session.session.model.reasoning_effort,
        created: true,
    })
}

/// Prepares the worker session state before turn or goal commands run.
///
/// Commands such as [`OperationCommand::SubmitInput`], [`OperationCommand::SetGoalObjective`],
/// follow-up. When no session is active yet, [`ensure_session_started`] creates one on the
/// server; the returned metadata is merged into the worker's current model, model binding, and
/// reasoning-effort selection. For a newly created session, this also notifies the UI via
/// [`WorkerEvent::SessionActivated`] and applies the configured permission preset. The
/// canonical session event subscription is ensured on every call; it is a no-op once the
/// active session is already subscribed.
#[allow(clippy::too_many_arguments)]
async fn prepare_session_for_command(
    client: &mut StdioServerClient,
    cwd: &Path,
    model: &mut String,
    model_binding_id: &mut Option<String>,
    reasoning_effort_selection: &mut Option<String>,
    session_id: &mut Option<SessionId>,
    subscribed_session_id: &mut Option<SessionId>,
    permission_preset: PermissionPreset,
    initial_sandbox_profile: Option<&str>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<SessionId> {
    let session_start =
        ensure_session_started(client, cwd, model, model_binding_id, session_id).await?;
    if let Some(model_override) = &session_start.model {
        *model = model_override.clone();
    }
    *model_binding_id = session_start
        .model_binding_id
        .clone()
        .or_else(|| model_binding_id.clone());
    *reasoning_effort_selection = session_start
        .reasoning_effort_selection
        .clone()
        .or_else(|| reasoning_effort_selection.clone());
    let active_session_id = session_start.session_id;
    if session_start.created {
        let _ = event_tx.send(WorkerEvent::SessionActivated {
            session_id: active_session_id,
        });
        apply_session_permissions(client, active_session_id, permission_preset).await?;
        if let Some(profile) = initial_sandbox_profile {
            client
                .session_settings_update(
                    active_session_id,
                    devo_protocol::native::rpc_session::SessionSettingsPatch {
                        sandbox_profile: Some(profile.to_string()),
                        ..Default::default()
                    },
                )
                .await?;
        }
    }
    ensure_session_subscription(client, active_session_id, subscribed_session_id, event_tx).await;
    Ok(active_session_id)
}

/// Resolves a user-turn index (counting `Regular` turns in sequence order,
/// matching the fork machinery's user-turn counting) into a turn id for
/// canonical `session/fork` (L2-DES-APP-008 Phase C).
async fn turn_id_for_user_turn_index(
    client: &mut StdioServerClient,
    session_id: SessionId,
    user_turn_index: u32,
) -> Result<TurnId> {
    let mut remaining = user_turn_index as usize;
    let mut cursor = None;
    loop {
        let page = client
            .session_turns_list_native(session_id, cursor.clone(), Some(200))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        for turn in &page.data {
            if matches!(turn.kind, devo_protocol::native::turn::TurnKind::Regular) {
                if remaining == 0 {
                    return TurnId::try_from(turn.id.as_str())
                        .map_err(|error| anyhow::anyhow!("invalid turn id from server: {error}"));
                }
                remaining = remaining.saturating_sub(1);
            }
        }
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => {
                return Err(anyhow::anyhow!(
                    "user turn index {user_turn_index} does not exist in this session"
                ));
            }
        }
    }
}

/// Collects the session's user input texts for input-history browsing via
/// canonical `session/items/list` pages (L2-DES-APP-008 Phase C).
async fn collect_user_input_texts(
    client: &mut StdioServerClient,
    session_id: SessionId,
) -> Result<Vec<String>> {
    let mut texts = Vec::new();
    let mut cursor = None;
    loop {
        let page = client
            .session_items_list_native(session_id, cursor.clone(), Some(500))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        for item in &page.data {
            if let devo_protocol::native::item::Item::UserMessage { content, .. } = &item.item {
                let text = content
                    .iter()
                    .filter_map(|input| match input {
                        devo_protocol::native::item::UserInput::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.trim().is_empty() {
                    texts.push(text);
                }
            }
        }
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }
    Ok(texts)
}

const MAX_PREVIEW_MESSAGES: usize = 4;

fn append_preview_item(
    messages: &mut VecDeque<SessionPreviewMessage>,
    item: devo_protocol::native::item::Item,
) {
    let message = match item {
        devo_protocol::native::item::Item::UserMessage { content, .. } => {
            let text = content
                .into_iter()
                .filter_map(|input| match input {
                    devo_protocol::native::item::UserInput::Text { text } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(SessionPreviewMessage {
                role: SessionPreviewRole::User,
                text,
            })
        }
        devo_protocol::native::item::Item::AssistantMessage { text, .. } => {
            (!text.trim().is_empty()).then_some(SessionPreviewMessage {
                role: SessionPreviewRole::Assistant,
                text,
            })
        }
        devo_protocol::native::item::Item::Reasoning { .. }
        | devo_protocol::native::item::Item::Plan { .. }
        | devo_protocol::native::item::Item::ToolCall { .. }
        | devo_protocol::native::item::Item::ToolResult { .. }
        | devo_protocol::native::item::Item::CommandExecution { .. }
        | devo_protocol::native::item::Item::HostedToolCall { .. }
        | devo_protocol::native::item::Item::FileChange { .. }
        | devo_protocol::native::item::Item::Approval { .. }
        | devo_protocol::native::item::Item::UserInputRequest { .. }
        | devo_protocol::native::item::Item::SubAgent { .. }
        | devo_protocol::native::item::Item::BackgroundTask { .. }
        | devo_protocol::native::item::Item::ContextCompaction { .. }
        | devo_protocol::native::item::Item::GoalProgress { .. }
        | devo_protocol::native::item::Item::Warning { .. } => None,
    };
    if let Some(message) = message {
        if messages.len() == MAX_PREVIEW_MESSAGES {
            messages.pop_front();
        }
        messages.push_back(message);
    }
}

/// Loads only the recent user/assistant dialogue needed by the inline resume picker.
async fn collect_session_preview(
    client: &mut StdioServerClient,
    session_id: SessionId,
) -> Result<Vec<SessionPreviewMessage>> {
    const MAX_PREVIEW_MESSAGES: usize = 4;

    let mut messages = VecDeque::with_capacity(MAX_PREVIEW_MESSAGES);
    let mut cursor = None;
    loop {
        let page = client
            .session_items_list_native(session_id, cursor.clone(), Some(500))
            .await?;
        let page_len = page.data.len();
        let next_cursor = page.next_cursor;
        for item in page.data {
            append_preview_item(&mut messages, item.item);
        }
        match (next_cursor, page_len) {
            (Some(next), len) if len > 0 => cursor = Some(next),
            _ => break,
        }
    }
    Ok(messages.into_iter().collect())
}

async fn rename_persisted_session(
    client: &mut StdioServerClient,
    session_id: SessionId,
    title: String,
) -> Result<String> {
    let result = client
        .session_title_update_native(session_id, title.clone())
        .await?;
    Ok(result.session.title.unwrap_or(title))
}

fn restored_history_items(
    turns: Vec<devo_protocol::native::turn::Turn>,
    items: Vec<devo_protocol::native::item::ItemEnvelope>,
    fallback_mode: devo_protocol::CollaborationMode,
) -> Vec<devo_protocol::SessionHistoryItem> {
    let mut items_by_turn = HashMap::<String, Vec<_>>::new();
    for item in items {
        items_by_turn
            .entry(item.turn_id.as_str().to_string())
            .or_default()
            .push(item);
    }
    let mut history_items = Vec::new();
    for turn in &turns {
        if let Some(turn_items) = items_by_turn.remove(turn.id.as_str()) {
            history_items.extend(
                turn_items
                    .iter()
                    .filter_map(typed_events::history_item_from_native_item),
            );
        }
        if let Some(summary) = typed_events::history_item_from_native_turn(turn, fallback_mode) {
            history_items.push(summary);
        }
    }
    let mut orphan_items = items_by_turn.into_values().flatten().collect::<Vec<_>>();
    orphan_items.sort_by_key(|item| item.seq);
    history_items.extend(
        orphan_items
            .iter()
            .filter_map(typed_events::history_item_from_native_item),
    );
    history_items
}

/// Converts a canonical goal back into the legacy `ThreadGoal` shape the
/// TUI's worker events still carry (L2-DES-APP-008 Phase C transition).
/// `Blocked`/`UsageLimited` map to `Paused` and terminal `Failed`/`Canceled`
/// map to `Complete` because the legacy enum has no finer states.
fn thread_goal_from_native(goal: &devo_protocol::native::goal::Goal) -> devo_protocol::ThreadGoal {
    let status = match goal.status {
        devo_protocol::native::goal::GoalStatus::Active => devo_protocol::ThreadGoalStatus::Active,
        devo_protocol::native::goal::GoalStatus::Paused
        | devo_protocol::native::goal::GoalStatus::Blocked
        | devo_protocol::native::goal::GoalStatus::UsageLimited => {
            devo_protocol::ThreadGoalStatus::Paused
        }
        devo_protocol::native::goal::GoalStatus::BudgetLimited => {
            devo_protocol::ThreadGoalStatus::BudgetLimited
        }
        devo_protocol::native::goal::GoalStatus::Completed
        | devo_protocol::native::goal::GoalStatus::Failed
        | devo_protocol::native::goal::GoalStatus::Canceled => {
            devo_protocol::ThreadGoalStatus::Complete
        }
    };
    let Ok(thread_id) = SessionId::try_from(goal.session_id.as_str()) else {
        unreachable!("canonical goal carries a legacy session id");
    };
    devo_protocol::ThreadGoal {
        thread_id,
        objective: goal.objective.clone(),
        status,
        token_budget: goal
            .token_budget
            .and_then(|budget| i64::try_from(budget).ok()),
        tokens_used: i64::try_from(goal.tokens_used).unwrap_or(i64::MAX),
        time_used_seconds: i64::try_from(goal.time_used_seconds).unwrap_or(i64::MAX),
        created_at: goal.created_at.timestamp(),
        updated_at: goal.updated_at.timestamp(),
    }
}

fn parse_native_queue_updated(params: &serde_json::Value) -> Result<WorkerEvent> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct QueueUpdatedParams {
        change: devo_protocol::native::queue::QueueChange,
        queue_item_id: devo_protocol::native::ids::QueueItemId,
        #[serde(default)]
        started_turn_id: Option<devo_protocol::native::ids::TurnId>,
        queue: Vec<devo_protocol::native::queue::QueueEntry>,
    }
    let parsed: QueueUpdatedParams =
        serde_json::from_value(params.clone()).context("decode queue/updated params")?;
    Ok(WorkerEvent::QueueUpdated {
        change: parsed.change,
        queue_item_id: parsed.queue_item_id,
        started_turn_id: parsed
            .started_turn_id
            .as_ref()
            .and_then(|id| TurnId::try_from(id.as_str()).ok()),
        entries: parsed.queue,
    })
}

async fn emit_queue_snapshot(
    client: &mut StdioServerClient,
    session_id: &devo_protocol::native::ids::SessionId,
    change: devo_protocol::native::queue::QueueChange,
    queue_item_id: devo_protocol::native::ids::QueueItemId,
    started_turn_id: Option<TurnId>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<()> {
    let listed = client
        .session_queue_list(devo_protocol::native::rpc_turn::SessionQueueListParams {
            session_id: session_id.clone(),
        })
        .await
        .context("session/queue/list after queue mutation")?;
    let _ = event_tx.send(WorkerEvent::QueueUpdated {
        change,
        queue_item_id,
        started_turn_id,
        entries: listed.entries,
    });
    Ok(())
}

/// Subscribes to canonical session events at most once per activated session.
///
/// Server-side `subscription/create` accumulates entries, so repeat calls for
/// the same session would leak subscriptions. Callers reset
/// `subscribed_session_id` when the active session changes so the next call
/// re-subscribes. Failures are logged and left non-fatal so prompt submission
/// is never blocked; the tracked id is only recorded on success, so a later
/// call retries.
async fn ensure_session_subscription(
    client: &mut StdioServerClient,
    session_id: SessionId,
    subscribed_session_id: &mut Option<SessionId>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    if *subscribed_session_id == Some(session_id) {
        return;
    }
    match subscribe_session_events(client, session_id, event_tx).await {
        Ok(()) => *subscribed_session_id = Some(session_id),
        Err(error) => {
            tracing::warn!(?session_id, %error, "failed to subscribe to session events");
        }
    }
}

async fn subscribe_session_events(
    client: &mut StdioServerClient,
    session_id: SessionId,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<()> {
    let canonical = native_session_id(session_id);
    let created = client
        .subscription_create(devo_protocol::native::event::SubscriptionCreateParams {
            selectors: vec![devo_protocol::native::event::StreamSelector::Session {
                session_id: canonical.clone(),
            }],
            include_snapshot: true,
            after: Vec::new(),
        })
        .await
        .context("subscription/create for session queue")?;
    for snapshot in created.snapshots {
        if let devo_protocol::native::event::SnapshotData::Session { queue, .. } = snapshot.data {
            let queue_item_id = queue
                .first()
                .map(|entry| entry.queue_item_id.clone())
                .unwrap_or_else(|| {
                    devo_protocol::native::ids::QueueItemId::from_string(String::new())
                });
            let _ = event_tx.send(WorkerEvent::QueueUpdated {
                change: devo_protocol::native::queue::QueueChange::Added,
                queue_item_id,
                started_turn_id: None,
                entries: queue,
            });
        }
    }
    for pending in created.pending_control_requests {
        let pending_session_id = SessionId::try_from(pending.item.session_id.as_str()).ok();
        let pending_turn_id = TurnId::try_from(pending.item.turn_id.as_str()).ok();
        let (Some(pending_session_id), Some(pending_turn_id)) =
            (pending_session_id, pending_turn_id)
        else {
            continue;
        };
        match pending.item.item {
            devo_protocol::native::item::Item::Approval {
                approval_id,
                action_summary,
                justification,
                resource,
                available_scopes,
                command_pattern,
                command_prefix,
                target,
                decision: None,
                ..
            } => {
                let (path, host, target) = match target {
                    Some(devo_protocol::native::item::ApprovalTarget::Path { path }) => {
                        let path = path.display().to_string();
                        (Some(path.clone()), None, Some(path))
                    }
                    Some(devo_protocol::native::item::ApprovalTarget::Host { host }) => {
                        (None, Some(host.clone()), Some(host))
                    }
                    Some(devo_protocol::native::item::ApprovalTarget::Command { command }) => {
                        (None, None, Some(command))
                    }
                    None => (None, None, None),
                };
                let _ = event_tx.send(WorkerEvent::ApprovalRequest {
                    session_id: pending_session_id,
                    turn_id: pending_turn_id,
                    approval_id,
                    action_summary,
                    justification,
                    resource,
                    available_scopes,
                    path,
                    host,
                    target,
                    command_pattern,
                    command_prefix,
                });
            }
            devo_protocol::native::item::Item::UserInputRequest {
                request_id,
                questions,
                answers: None,
                ..
            } => {
                let questions = questions
                    .into_iter()
                    .map(|question| devo_protocol::RequestUserInputQuestion {
                        id: question.id,
                        header: question.header,
                        question: question.question,
                        is_other: question.is_other,
                        is_secret: question.is_secret,
                        options: question.options.map(|options| {
                            options
                                .into_iter()
                                .map(|option| devo_protocol::RequestUserInputOption {
                                    label: option.label,
                                    description: option.description,
                                })
                                .collect()
                        }),
                    })
                    .collect();
                let _ = event_tx.send(WorkerEvent::RequestUserInput {
                    session_id: pending_session_id,
                    turn_id: pending_turn_id,
                    request_id,
                    questions,
                });
            }
            devo_protocol::native::item::Item::Approval {
                decision: Some(_), ..
            }
            | devo_protocol::native::item::Item::UserInputRequest {
                answers: Some(_), ..
            }
            | devo_protocol::native::item::Item::UserMessage { .. }
            | devo_protocol::native::item::Item::AssistantMessage { .. }
            | devo_protocol::native::item::Item::Reasoning { .. }
            | devo_protocol::native::item::Item::Plan { .. }
            | devo_protocol::native::item::Item::ToolCall { .. }
            | devo_protocol::native::item::Item::ToolResult { .. }
            | devo_protocol::native::item::Item::HostedToolCall { .. }
            | devo_protocol::native::item::Item::CommandExecution { .. }
            | devo_protocol::native::item::Item::FileChange { .. }
            | devo_protocol::native::item::Item::SubAgent { .. }
            | devo_protocol::native::item::Item::BackgroundTask { .. }
            | devo_protocol::native::item::Item::ContextCompaction { .. }
            | devo_protocol::native::item::Item::GoalProgress { .. }
            | devo_protocol::native::item::Item::Warning { .. } => {}
        }
    }
    Ok(())
}

async fn pause_active_goal_before_session_leave(
    client: &mut StdioServerClient,
    session_id: SessionId,
    active_turn_id: Option<TurnId>,
) -> Result<()> {
    let goal_status = client
        .session_goal_read_native(session_id)
        .await
        .context("failed to load goal before leaving session")?;
    let goal = goal_status.goal.as_ref().map(thread_goal_from_native);
    if !should_pause_goal_before_session_leave(goal.as_ref()) {
        return Ok(());
    }

    let goal_id = goal_status
        .goal
        .as_ref()
        .map(|goal| goal.id.clone())
        .context("goal disappeared before pause")?;
    client
        .session_goal_transition_native(
            session_id,
            &goal_id,
            devo_client::GoalLifecycleTransition::Pause,
        )
        .await
        .context("failed to pause active goal before leaving session")?;

    if active_turn_id.is_some()
        && let Err(error) = client
            .session_interrupt_native(
                devo_protocol::native::rpc_session::SessionInterruptScope::Session {
                    session_id: native_session_id(session_id),
                },
            )
            .await
    {
        return Err(error).context("failed to interrupt active goal work before leaving session");
    }

    Ok(())
}

fn should_pause_goal_before_session_leave(goal: Option<&devo_protocol::ThreadGoal>) -> bool {
    goal.is_some_and(|goal| {
        matches!(
            goal.status,
            ThreadGoalStatus::Active | ThreadGoalStatus::BudgetLimited
        )
    })
}

fn emit_goal_leave_failure(event_tx: &mpsc::UnboundedSender<WorkerEvent>, error: anyhow::Error) {
    let _ = event_tx.send(WorkerEvent::GoalOperationFailed {
        message: error.to_string(),
    });
}

async fn apply_session_permissions(
    client: &mut StdioServerClient,
    session_id: SessionId,
    preset: PermissionPreset,
) -> Result<()> {
    // Native settings path (L2-DES-APP-008): persist-first on the server,
    // applies to a running turn's next authorization.
    let permission_profile = match preset {
        PermissionPreset::Default => devo_protocol::native::model::PermissionProfile::Default,
        PermissionPreset::AutoReview => devo_protocol::native::model::PermissionProfile::AutoReview,
        PermissionPreset::FullAccess => devo_protocol::native::model::PermissionProfile::FullAccess,
    };
    client
        .session_settings_update(
            session_id,
            devo_protocol::native::rpc_session::SessionSettingsPatch {
                permission_profile: Some(permission_profile),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}

async fn spawn_client(_cwd: &Path, server_log_level: Option<String>) -> Result<StdioServerClient> {
    let program = std::env::current_exe().context("resolve current executable for server child")?;
    StdioServerClient::spawn(StdioServerClientConfig {
        // Re-exec the current binary and enter the hidden server subcommand.
        program,
        args: std::iter::once("server".to_string())
            .chain(["--transport".to_string(), "stdio".to_string()])
            .chain(
                server_log_level
                    .into_iter()
                    .flat_map(|level| ["--log-level".to_string(), level]),
            )
            .collect(),
        // The TUI consumes canonical typed events (worker typed block +
        // typed_events converter).
        typed_items: true,
    })
    .await
}

async fn emit_skills_list(
    client: &mut StdioServerClient,
    cwd: &Path,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    open_picker: bool,
) -> Result<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.skill_list_native(Some(cwd.to_path_buf()), false),
    )
    .await
    .context("skills list request timed out")??;
    // Native skill records convert back to the legacy shape the picker
    // and metadata surfaces consume (ratified #4).
    emit_skills_list_result(
        result
            .skills
            .into_iter()
            .map(devo_server::SkillRecord::from)
            .collect(),
        event_tx,
        open_picker,
    );
    Ok(())
}

async fn emit_reference_search_update(
    client: &mut StdioServerClient,
    cwd: &Path,
    active_search_id: &mut Option<ReferenceSearchId>,
    query: String,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<()> {
    // Native `search/*` (L2-DES-APP-008): the snapshot converts back to
    // the legacy shape the popup renders while server notifications stay
    // legacy-shaped during the event cutover.
    let snapshot: ReferenceSearchSnapshot = if let Some(search_id) = active_search_id.clone() {
        client.search_update(search_id, query).await?.into()
    } else {
        let snapshot: devo_protocol::native::rpc_search::SearchSnapshot =
            client.search_start(Some(cwd.to_path_buf()), query).await?;
        *active_search_id = Some(snapshot.search_id.clone());
        snapshot.into()
    };
    let _ = event_tx.send(WorkerEvent::ReferenceSearchUpdated { snapshot });
    Ok(())
}

fn emit_skills_list_result(
    skills: Vec<devo_server::SkillRecord>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
    open_picker: bool,
) {
    let picker_skills = skills
        .iter()
        .map(crate::skills_picker::skill_picker_entry_from_record)
        .collect();
    let skills = skills
        .iter()
        .filter(|skill| skill.enabled)
        .map(skill_metadata_from_record)
        .collect();
    let _ = event_tx.send(WorkerEvent::SkillsListed {
        skills,
        picker_skills,
        open_picker,
    });
}

async fn emit_mcp_servers_list(
    client: &mut StdioServerClient,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.mcp_list(devo_protocol::native::rpc_admin::McpListParams {}),
    )
    .await
    .context("mcp list request timed out")??;
    let _ = event_tx.send(WorkerEvent::McpServersListed {
        servers: result.servers,
    });
    Ok(())
}

async fn emit_mcp_tools_list(
    client: &mut StdioServerClient,
    name: String,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> Result<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        client.mcp_tools(devo_protocol::native::rpc_admin::McpToolsParams { name: name.clone() }),
    )
    .await
    .context("mcp tools request timed out")??;
    let _ = event_tx.send(WorkerEvent::McpToolsListed {
        name,
        tools: result.tools,
    });
    Ok(())
}

fn render_skill_list_body(skills: &[devo_server::SkillRecord]) -> String {
    if skills.is_empty() {
        return "_No skills found._".to_string();
    }

    skills
        .iter()
        .map(|skill| {
            let enabled = if skill.enabled { "yes" } else { "no" };
            format!(
                "- `{}` - {}\n  enabled: {}\n  source: {}\n  path: `{}`",
                skill.name,
                skill.description,
                enabled,
                render_skill_source(&skill.source),
                skill.path.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn skill_metadata_from_record(skill: &devo_server::SkillRecord) -> SkillMetadata {
    SkillMetadata {
        name: skill.name.clone(),
        description: skill.description.clone(),
        short_description: skill.short_description.clone(),
        interface: skill
            .interface
            .as_ref()
            .map(|interface| SkillInterfaceMetadata {
                display_name: interface.display_name.clone(),
                short_description: interface.short_description.clone(),
            }),
        path_to_skills_md: skill.path.clone(),
    }
}

fn render_skill_source(source: &SkillSource) -> String {
    match source {
        SkillSource::User => "user".to_string(),
        SkillSource::Workspace { cwd } => format!("workspace ({})", cwd.display()),
        SkillSource::Plugin { plugin_id } => format!("plugin ({plugin_id})"),
        SkillSource::System => "system".to_string(),
        SkillSource::Admin => "admin".to_string(),
    }
}

fn btw_agent_prompt(question: &str) -> String {
    format!(
        "You are answering a /btw side question in a lightweight forked agent.\n\
         The inherited conversation is reference context only. Do not continue or modify the \
         main session task. Answer only this side question.\n\
         You cannot use tools in this fork: do not read files, run commands, search, or modify code. \
         Produce one concise answer and stop.\n\n\
         Side question:\n{question}"
    )
}

/// Builds the isolated child-session request for the TUI `/btw` command.
///
/// `/btw` is a side question, not a `turn/steer` input: it must not alter the
/// parent's turn, history, or queues. `ephemeral`, `DenyAll`, and the one-turn
/// limit make that boundary enforceable by the runtime rather than relying
/// only on the model prompt.
fn btw_spawn_params(session_id: SessionId, question: &str) -> SpawnAgentParams {
    SpawnAgentParams {
        session_id,
        message: btw_agent_prompt(question),
        fork_turns: Some("all".to_string()),
        max_turns: Some(1),
        tool_policy: AgentToolPolicy::DenyAll,
        ephemeral: true,
    }
}

async fn close_btw_agent(client: &mut StdioServerClient, child_session_id: SessionId) {
    // Native `agent/cancel` (L2-DES-APP-008 facade): the item id is the
    // child session uuid, `item_`-prefixed.
    let item_id =
        devo_protocol::native::ids::ItemId::from_string(format!("item_{child_session_id}"));
    let _ = client.agent_cancel_native(&item_id).await;
}

fn project_history_items(items: &[SessionHistoryItem]) -> Vec<TranscriptItem> {
    use std::collections::{HashMap, HashSet};

    let mut paired_result_by_call_id = HashMap::new();
    let mut consumed_result_indexes = HashSet::new();

    for (index, item) in items.iter().enumerate() {
        if matches!(
            item.kind,
            SessionHistoryItemKind::ToolResult | SessionHistoryItemKind::Error
        ) && let Some(tool_call_id) = item.tool_call_id.as_deref()
        {
            paired_result_by_call_id
                .entry(tool_call_id.to_string())
                .or_insert(index);
        }
    }

    let metadata_owned_ids = items
        .iter()
        .filter_map(|item| {
            item.tool_call_id
                .clone()
                .filter(|_| item.metadata.is_some())
        })
        .collect::<HashSet<_>>();
    let mut transcript = Vec::new();
    let mut index = 0usize;

    while index < items.len() {
        let item = &items[index];
        if let Some(metadata) = &item.metadata {
            if let Some(tool_call_id) = item.tool_call_id.as_deref()
                && let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied()
                && result_index != index
            {
                consumed_result_indexes.insert(result_index);
            }
            match metadata {
                SessionHistoryMetadata::PlanUpdate { explanation, steps } => {
                    transcript.push(TranscriptItem::new(
                        TranscriptItemKind::System,
                        explanation.clone().unwrap_or_default(),
                        steps
                            .iter()
                            .map(|step| {
                                let status = match step.status {
                                    SessionPlanStepStatus::Pending => "pending",
                                    SessionPlanStepStatus::InProgress => "in_progress",
                                    SessionPlanStepStatus::Completed => "completed",
                                    SessionPlanStepStatus::Cancelled => "cancelled",
                                };
                                format!("{status}: {}", step.text)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ));
                    index += 1;
                    continue;
                }
                SessionHistoryMetadata::ProposedPlan => {
                    transcript.push(TranscriptItem::new(
                        TranscriptItemKind::Assistant,
                        "Proposed Plan".to_string(),
                        item.body.clone(),
                    ));
                    index += 1;
                    continue;
                }
                SessionHistoryMetadata::TurnSummary { .. }
                | SessionHistoryMetadata::Edited { .. } => {}
                SessionHistoryMetadata::Explored { actions } => {
                    let title = item.title.clone();
                    let body = actions
                        .iter()
                        .map(|action| format!("{action:?}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    transcript.push(TranscriptItem::restored_tool_result(title, body));
                    index += 1;
                    continue;
                }
            }
        }
        if item.kind == SessionHistoryItemKind::ToolCall
            && let Some(tool_call_id) = item.tool_call_id.as_deref()
        {
            if metadata_owned_ids.contains(tool_call_id) {
                index += 1;
                continue;
            }
            if let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied() {
                let result_item = &items[result_index];
                consumed_result_indexes.insert(result_index);
                let mut ti = if result_item.kind == SessionHistoryItemKind::Error {
                    TranscriptItem::tool_error(item.title.clone(), result_item.body.clone())
                } else {
                    TranscriptItem::restored_tool_result(
                        item.title.clone(),
                        result_item.body.clone(),
                    )
                };
                if let Some(duration_ms) = result_item.duration_ms {
                    ti = ti.with_duration(duration_ms);
                }
                transcript.push(ti);
                index += 1;
                continue;
            }
        }

        if consumed_result_indexes.contains(&index) {
            index += 1;
            continue;
        }

        let kind = match item.kind {
            SessionHistoryItemKind::User => TranscriptItemKind::User,
            SessionHistoryItemKind::Assistant => TranscriptItemKind::Assistant,
            SessionHistoryItemKind::Reasoning => TranscriptItemKind::Reasoning,
            SessionHistoryItemKind::ToolCall => TranscriptItemKind::ToolCall,
            SessionHistoryItemKind::ToolResult => TranscriptItemKind::ToolResult,
            SessionHistoryItemKind::CommandExecution => TranscriptItemKind::ToolResult,
            SessionHistoryItemKind::Error => TranscriptItemKind::Error,
            SessionHistoryItemKind::TurnSummary => TranscriptItemKind::TurnSummary,
            SessionHistoryItemKind::ContextCompaction => TranscriptItemKind::System,
        };
        let mut transcript_item = match item.kind {
            SessionHistoryItemKind::ToolCall => TranscriptItem::tool_call(item.title.clone()),
            SessionHistoryItemKind::ToolResult => {
                TranscriptItem::restored_tool_result(item.title.clone(), item.body.clone())
            }
            SessionHistoryItemKind::CommandExecution => {
                TranscriptItem::restored_tool_result(item.title.clone(), item.body.clone())
            }
            SessionHistoryItemKind::Error => {
                if item.tool_call_id.is_some() {
                    TranscriptItem::tool_error(item.title.clone(), item.body.clone())
                } else {
                    TranscriptItem::new(kind, String::new(), item.body.clone())
                }
            }
            SessionHistoryItemKind::TurnSummary => {
                // TurnSummary uses title for model name, duration_ms for duration in seconds
                TranscriptItem::new(kind, item.title.clone(), item.body.clone())
            }
            SessionHistoryItemKind::ContextCompaction => {
                let title = if item.title.is_empty() {
                    "Context compacted".to_string()
                } else {
                    item.title.clone()
                };
                TranscriptItem::new(kind, title, String::new())
            }
            SessionHistoryItemKind::User
            | SessionHistoryItemKind::Assistant
            | SessionHistoryItemKind::Reasoning => {
                TranscriptItem::new(kind, item.title.clone(), item.body.clone())
            }
        };
        if let Some(duration_ms) = item.duration_ms {
            transcript_item = transcript_item.with_duration(duration_ms);
        }
        transcript.push(transcript_item);
        index += 1;
    }

    transcript
}

fn map_join_error(error: JoinError) -> anyhow::Error {
    if error.is_cancelled() {
        anyhow::anyhow!("interactive worker task was cancelled")
    } else if error.is_panic() {
        anyhow::anyhow!("interactive worker task panicked")
    } else {
        anyhow::Error::new(error)
    }
}

fn map_worker_join_result(result: std::result::Result<(), JoinError>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.is_cancelled() => Ok(()),
        Err(error) => Err(map_join_error(error)),
    }
}

#[cfg(test)]
pub(crate) fn dispatch_legacy_item_event_for_test(
    method: &str,
    payload: ItemEventPayload,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    use chrono::Utc;
    use devo_protocol::EventContext;
    use devo_protocol::TypedItemEventPayload;
    use devo_protocol::native::ids::{
        ItemId as NativeItemId, SessionId as NativeSessionId, TurnId as NativeTurnId,
    };
    use devo_protocol::native::item::{ItemEnvelope as NativeItemEnvelope, ItemState};
    use devo_protocol::native::wire_projector::project_wire_item;

    let item_id = payload.item.item_id;
    let session_id = payload.context.session_id;
    let turn_id = payload.context.turn_id.unwrap_or_else(TurnId::new);
    let projected_at = Utc::now();
    let native_item =
        project_wire_item(&payload.item.item_kind, &payload.item.payload, projected_at)
            .expect("legacy test payload must project to native item");
    let typed = TypedItemEventPayload {
        context: EventContext {
            session_id,
            turn_id: Some(turn_id),
            item_id: Some(item_id),
            seq: payload.context.seq,
            item_seq: payload.context.item_seq,
        },
        item: NativeItemEnvelope {
            id: NativeItemId::from_legacy_uuid(item_id.into()),
            session_id: NativeSessionId::from_legacy_uuid(session_id.into()),
            turn_id: NativeTurnId::from_legacy_uuid(turn_id.into()),
            seq: payload.context.item_seq.unwrap_or(payload.context.seq),
            revision: 1,
            created_at: projected_at,
            updated_at: projected_at,
            state: if method == "item/completed" {
                ItemState::Completed
            } else {
                ItemState::Running
            },
            item: native_item,
        },
    };
    item_dispatch::dispatch_typed_item_lifecycle(method, &typed, item_id, event_tx);
}

#[cfg(test)]
mod tests {
    use super::ProviderValidationCancellation;
    use chrono::Utc;
    use pretty_assertions::assert_eq;
    use std::future::pending;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use devo_core::SessionId;
    use devo_core::SessionTitleState;
    use devo_server::CommandExecutionPayload;
    use devo_server::SessionMetadata;
    use devo_server::SessionRuntimeStatus;
    use devo_server::SkillRecord;
    use devo_server::SkillScope;
    use devo_server::SkillSource;

    use crate::events::SessionPreviewMessage;
    use crate::events::SessionPreviewRole;

    use super::QueryWorkerHandle;
    use super::ShellCommandExecStart;
    use super::append_preview_item;
    use super::btw_agent_prompt;
    use super::btw_spawn_params;
    use super::dispatch_legacy_item_event_for_test;
    use super::last_query_tokens_from_resume;
    use super::next_shell_command_exec_start;
    use super::project_history_items;
    use super::render_skill_list_body;
    use super::restored_history_items;
    use super::should_apply_terminal_turn_usage_fallback;
    use super::should_pause_goal_before_session_leave;
    use super::tool_lifecycle;
    use super::tool_summaries::normalize_display_output;
    use super::tool_summaries::summarize_tool_call;
    use super::tool_summaries::tool_call_started_actions;
    use super::tool_summaries::tool_call_started_event;
    use super::tool_summaries::truncate_tool_output;
    use crate::events::SessionListEntry;
    use crate::events::SubagentMonitorAgent;
    use crate::events::SubagentMonitorEvent;
    use crate::events::TranscriptItem;
    use crate::events::TranscriptItemKind;
    use crate::events::WorkerEvent;
    use devo_core::ItemId;
    use devo_core::TurnId;
    use devo_protocol::AgentToolPolicy;
    use devo_protocol::PendingServerRequestContext;
    use devo_protocol::ServerRequestKind;
    use devo_protocol::SessionHistoryMetadata;
    use devo_protocol::SessionPlanStepStatus;
    use devo_protocol::SpawnAgentParams;
    use devo_protocol::ThreadGoal;
    use devo_protocol::ThreadGoalStatus;
    use devo_server::ApprovalRequestPayload;
    use devo_server::FileChangePayload;
    use devo_server::ItemEnvelope;
    use devo_server::ItemEventPayload;
    use devo_server::ItemKind;
    use devo_server::SessionHistoryItem;
    use devo_server::SessionHistoryItemKind;
    use devo_server::ToolCallPayload;
    use devo_server::ToolResultPayload;

    #[test]
    fn btw_spawns_an_ephemeral_tool_free_one_turn_side_question() {
        let session_id = SessionId::new();
        let question = "what changed in the parser?";

        assert_eq!(
            btw_spawn_params(session_id, question),
            SpawnAgentParams {
                session_id,
                message: btw_agent_prompt(question),
                fork_turns: Some("all".to_string()),
                max_turns: Some(1),
                tool_policy: AgentToolPolicy::DenyAll,
                ephemeral: true,
            }
        );
    }

    #[tokio::test]
    async fn worker_shutdown_aborts_unresponsive_task() {
        let (command_tx, _command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = QueryWorkerHandle {
            command_tx,
            provider_validation_cancel: Arc::new(ProviderValidationCancellation::default()),
            event_rx,
            join_handle: tokio::spawn(async {
                pending::<()>().await;
            }),
        };

        let completed = tokio::time::timeout(Duration::from_secs(1), worker.shutdown())
            .await
            .map(|result| result.is_ok())
            .unwrap_or(false);

        assert_eq!([completed], [true]);
    }

    #[test]
    fn shell_command_exec_start_uses_distinct_one_shot_processes() {
        let session_id = SessionId::new();
        let mut next_shell_process_index = 1_u64;

        let first = next_shell_command_exec_start(
            Some(session_id),
            PathBuf::from("/tmp/project"),
            "pwd".to_string(),
            &mut next_shell_process_index,
        );
        let second = next_shell_command_exec_start(
            /*session_id*/ None,
            PathBuf::from("/tmp/project"),
            "whoami".to_string(),
            &mut next_shell_process_index,
        );

        assert_eq!(
            vec![first, second],
            vec![
                ShellCommandExecStart {
                    process_id: "user-shell-1".to_string(),
                    command: "pwd".to_string(),
                    started_event:
                        super::super::worker_event_test_helpers::command_execution_started(
                            "user-shell-1".to_string(),
                            "pwd".to_string(),
                            Some(serde_json::json!({
                                "cmd": "pwd",
                                "cwd": PathBuf::from("/tmp/project"),
                            })),
                            devo_protocol::protocol::ExecCommandSource::UserShell,
                            Vec::new(),
                        ),
                    params: devo_protocol::CommandExecParams {
                        session_id: Some(session_id),
                        process_id: "user-shell-1".to_string(),
                        cwd: Some(PathBuf::from("/tmp/project")),
                        program: devo_protocol::CommandExecProgram::OneShot {
                            command: "pwd".to_string(),
                        },
                        size: None,
                    },
                },
                ShellCommandExecStart {
                    process_id: "user-shell-2".to_string(),
                    command: "whoami".to_string(),
                    started_event:
                        super::super::worker_event_test_helpers::command_execution_started(
                            "user-shell-2".to_string(),
                            "whoami".to_string(),
                            Some(serde_json::json!({
                                "cmd": "whoami",
                                "cwd": PathBuf::from("/tmp/project"),
                            })),
                            devo_protocol::protocol::ExecCommandSource::UserShell,
                            Vec::new(),
                        ),
                    params: devo_protocol::CommandExecParams {
                        session_id: None,
                        process_id: "user-shell-2".to_string(),
                        cwd: Some(PathBuf::from("/tmp/project")),
                        program: devo_protocol::CommandExecProgram::OneShot {
                            command: "whoami".to_string(),
                        },
                        size: None,
                    },
                },
            ]
        );
        assert_eq!(next_shell_process_index, 3);
    }

    #[test]
    fn bash_tool_summary_uses_command_text() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            parameters: serde_json::json!({
                "command": "Get-Date -Format \"yyyy-MM-dd\""
            }),
            command_actions: Vec::new(),
        };

        assert_eq!(
            summarize_tool_call(&payload),
            "Shell Get-Date -Format \"yyyy-MM-dd\""
        );
    }

    #[test]
    fn tool_summary_uses_pretty_operation_labels() {
        let cases = [
            (
                "read",
                serde_json::json!({ "path": "/tmp/project/src/lib.rs", "offset": 9, "limit": 4 }),
                "Read /tmp/project/src/lib.rs L:9-12",
            ),
            (
                "write",
                serde_json::json!({ "path": "src/lib.rs" }),
                "Write src/lib.rs",
            ),
            (
                "apply_patch",
                serde_json::json!({ "path": "src/lib.rs" }),
                "Patch src/lib.rs",
            ),
            (
                "glob",
                serde_json::json!({ "pattern": "*.rs", "path": "crates/tui" }),
                "List crates/tui",
            ),
            (
                "grep",
                serde_json::json!({ "pattern": "Usage", "path": "crates/tui" }),
                "Search \"Usage\" in crates/tui",
            ),
            (
                "code_search",
                serde_json::json!({ "query": "usage ledger", "path": "crates/server" }),
                "Code-Search \"usage ledger\" in crates/server",
            ),
            (
                "spawn_agent",
                serde_json::json!({ "agent_nickname": "reviewer", "message": "check usage" }),
                "Spawn-Agent \"reviewer\" \"check usage\"",
            ),
            (
                "await_task",
                serde_json::json!({ "task_id": "task-1", "timeout_secs": 30 }),
                "Await-Task \"task-1\" \"30s\"",
            ),
            (
                "cancel_task",
                serde_json::json!({ "task_id": "task-1" }),
                "Cancel-Task \"task-1\"",
            ),
            ("list_tasks", serde_json::json!({}), "List-Tasks"),
        ];

        for (tool_name, parameters, expected) in cases {
            let payload = ToolCallPayload {
                tool_call_id: "call-1".to_string(),
                tool_name: tool_name.to_string(),
                parameters,
                command_actions: Vec::new(),
            };
            assert_eq!(summarize_tool_call(&payload), expected);
        }
    }

    #[test]
    fn web_search_tool_summary_uses_query_text() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "web_search".to_string(),
            parameters: serde_json::json!({
                "query": "current Rust docs"
            }),
            command_actions: Vec::new(),
        };

        assert_eq!(
            summarize_tool_call(&payload),
            "Web Search(\"current Rust docs\")"
        );
    }

    #[test]
    fn web_fetch_tool_summary_uses_url_text() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "web_fetch".to_string(),
            parameters: serde_json::json!({
                "url": "https://example.test/docs"
            }),
            command_actions: Vec::new(),
        };

        assert_eq!(
            summarize_tool_call(&payload),
            "Web Fetch(\"https://example.test/docs\")"
        );
    }

    #[test]
    fn tool_output_preview_truncates_large_content() {
        let content = (1..=12)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            truncate_tool_output(&content),
            "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\n… "
        );
    }

    #[test]
    fn render_skill_list_body_handles_empty_list() {
        assert_eq!(render_skill_list_body(&[]), "_No skills found._");
    }

    #[test]
    fn render_skill_list_body_uses_markdown_for_names_and_paths() {
        let skill_path = PathBuf::from("skills").join("writer").join("SKILL.md");

        assert_eq!(
            render_skill_list_body(&[SkillRecord {
                id: skill_path.display().to_string(),
                name: "writer".to_string(),
                description: "Draft polished docs".to_string(),
                short_description: None,
                interface: None,
                dependencies: None,
                path: skill_path.clone(),
                enabled: true,
                source: SkillSource::User,
                scope: SkillScope::User,
                plugin_id: None,
            }]),
            format!(
                "- `writer` - Draft polished docs\n  enabled: yes\n  source: user\n  path: `{}`",
                skill_path.display()
            )
        );
    }

    #[cfg(windows)]
    #[test]
    fn render_skill_list_body_preserves_windows_dot_directory_separators() {
        let skill_path =
            PathBuf::from(r"C:\Users\lenovo\.devo\skills\.system\skill-installer\SKILL.md");
        let body = render_skill_list_body(&[SkillRecord {
            id: skill_path.display().to_string(),
            name: "skill-installer".to_string(),
            description: "Install Devo skills".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            path: skill_path,
            enabled: true,
            source: SkillSource::System,
            scope: SkillScope::System,
            plugin_id: None,
        }]);

        let lines = crate::markdown_render::render_markdown_text(&body)
            .lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content)
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "- skill-installer - Install Devo skills".to_string(),
                "  enabled: yes".to_string(),
                "  source: system".to_string(),
                r"  path: C:\Users\lenovo\.devo\skills\.system\skill-installer\SKILL.md"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn completed_tool_result_uses_display_content_preview() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        dispatch_legacy_item_event_for_test(
            "item/completed",
            ItemEventPayload {
                context: devo_server::EventContext {
                    session_id: SessionId::new(),
                    turn_id: None,
                    item_id: None,
                    seq: 1,
                    item_seq: None,
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::ToolResult,
                    payload: serde_json::to_value(ToolResultPayload {
                        tool_call_id: "call-1".to_string(),
                        tool_name: Some("read".to_string()),
                        input: None,
                        content: serde_json::Value::String(
                            "<content>canonical</content>".to_string(),
                        ),
                        display_content: Some("canonical".to_string()),
                        is_error: false,
                        summary: "read output".to_string(),
                    })
                    .expect("serialize tool result payload"),
                },
            },
            &event_tx,
        );

        assert_eq!(
            event_rx.try_recv().expect("worker event"),
            WorkerEvent::Transcript(tool_lifecycle::tool_closed_from_result(
                &ToolResultPayload {
                    tool_call_id: "call-1".to_string(),
                    tool_name: None,
                    input: None,
                    content: serde_json::Value::String("<content>canonical</content>".to_string(),),
                    display_content: Some("canonical".to_string()),
                    is_error: false,
                    summary: String::new(),
                },
            ))
        );
    }

    #[test]
    fn started_approval_request_emits_worker_event() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        dispatch_legacy_item_event_for_test(
            "item/started",
            ItemEventPayload {
                context: devo_server::EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: None,
                    seq: 1,
                    item_seq: None,
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::ApprovalRequest,
                    payload: serde_json::to_value(ApprovalRequestPayload {
                        request: PendingServerRequestContext {
                            request_id: "approval-1".into(),
                            request_kind: ServerRequestKind::ItemFileChangeRequestApproval,
                            session_id,
                            turn_id: Some(turn_id),
                            item_id: None,
                        },
                        approval_id: "approval-1".into(),
                        action_summary: "Write hello.txt".to_string(),
                        justification: "Create a file".to_string(),
                        resource: Some("FileWrite".to_string()),
                        available_scopes: vec!["once".to_string()],
                        path: Some(r"C:\Users\lenovo\Desktop\hello.txt".to_string()),
                        host: None,
                        target: None,
                        command_pattern: None,
                        command_prefix: None,
                    })
                    .expect("serialize approval request payload"),
                },
            },
            &event_tx,
        );

        assert_eq!(
            event_rx.try_recv().expect("approval request event"),
            WorkerEvent::ApprovalRequest {
                session_id,
                turn_id,
                approval_id: "approval-1".to_string(),
                action_summary: "Write hello.txt".to_string(),
                justification: "Create a file".to_string(),
                resource: Some("FileWrite".to_string()),
                available_scopes: vec!["once".to_string()],
                path: Some(r"C:\Users\lenovo\Desktop\hello.txt".to_string()),
                host: None,
                target: None,
                command_pattern: None,
                command_prefix: None,
            }
        );
    }

    #[test]
    fn read_tool_call_start_with_empty_parameters_emits_placeholder_action() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            parameters: serde_json::json!({}),
            command_actions: Vec::new(),
        };

        assert_eq!(
            tool_call_started_actions(&payload),
            vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: String::new(),
                name: String::new(),
                path: PathBuf::new(),
            }]
        );
    }

    #[test]
    fn read_tool_call_start_with_offset_and_limit_emits_line_range() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            parameters: serde_json::json!({
                "filePath": "crates/core/src/query.rs",
                "offset": 10,
                "limit": 5,
            }),
            command_actions: Vec::new(),
        };

        assert_eq!(
            tool_call_started_actions(&payload),
            vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: "Read crates/core/src/query.rs L:10-14".to_string(),
                name: "crates/core/src/query.rs L:10-14".to_string(),
                path: PathBuf::from("crates/core/src/query.rs"),
            }]
        );
    }

    #[test]
    fn code_search_tool_call_start_emits_search_action() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "code_search".to_string(),
            parameters: serde_json::json!({
                "operation": "search",
                "query": "live tool feedback",
                "path": "crates"
            }),
            command_actions: Vec::new(),
        };

        assert_eq!(
            tool_call_started_event(payload.clone()),
            WorkerEvent::Transcript(tool_lifecycle::tool_opened_from_call(&payload)),
        );
    }

    #[test]
    fn code_search_tool_call_start_with_empty_parameters_omits_json_preview() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "code_search".to_string(),
            parameters: serde_json::json!({}),
            command_actions: Vec::new(),
        };

        assert_eq!(
            tool_call_started_event(payload.clone()),
            WorkerEvent::Transcript(tool_lifecycle::tool_opened_from_call(&payload)),
        );
    }

    #[test]
    fn apply_patch_tool_call_start_is_preparing() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "apply_patch".to_string(),
            parameters: serde_json::json!({}),
            command_actions: Vec::new(),
        };

        assert_eq!(
            tool_call_started_event(payload.clone()),
            WorkerEvent::Transcript(tool_lifecycle::tool_opened_from_call(&payload)),
        );
    }

    #[test]
    fn edit_tool_call_start_uses_path_free_live_summary() {
        let payload = ToolCallPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "edit".to_string(),
            parameters: serde_json::json!({"filePath": "test_edit_test.md"}),
            command_actions: Vec::new(),
        };

        assert_eq!(
            tool_call_started_event(payload.clone()),
            WorkerEvent::Transcript(tool_lifecycle::tool_opened_from_call(&payload)),
        );
    }

    #[test]
    fn completed_read_tool_call_emits_update_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        dispatch_legacy_item_event_for_test(
            "item/completed",
            ItemEventPayload {
                context: devo_server::EventContext {
                    session_id: SessionId::new(),
                    turn_id: None,
                    item_id: None,
                    seq: 1,
                    item_seq: None,
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::ToolCall,
                    payload: serde_json::to_value(ToolCallPayload {
                        tool_call_id: "call-1".to_string(),
                        tool_name: "read".to_string(),
                        parameters: serde_json::json!({
                            "filePath": "crates/tui/src/mod.rs"
                        }),
                        command_actions: Vec::new(),
                    })
                    .expect("serialize tool call payload"),
                },
            },
            &event_tx,
        );

        use crate::transcript::lifecycle::ItemLifecycleEvent;

        assert_eq!(
            event_rx.try_recv().expect("worker refresh event"),
            WorkerEvent::Transcript(ItemLifecycleEvent::ToolOpened {
                tool_use_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                input: serde_json::json!({
                    "filePath": "crates/tui/src/mod.rs"
                }),
                command: None,
                command_source: None,
                parsed_commands: vec![devo_protocol::parse_command::ParsedCommand::Read {
                    cmd: "Read crates/tui/src/mod.rs".to_string(),
                    name: "crates/tui/src/mod.rs".to_string(),
                    path: PathBuf::from("crates/tui/src/mod.rs"),
                }],
            })
        );
    }

    #[test]
    fn completed_glob_tool_call_emits_update_with_pattern_and_path() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        dispatch_legacy_item_event_for_test(
            "item/completed",
            ItemEventPayload {
                context: devo_server::EventContext {
                    session_id: SessionId::new(),
                    turn_id: None,
                    item_id: None,
                    seq: 1,
                    item_seq: None,
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::ToolCall,
                    payload: serde_json::to_value(ToolCallPayload {
                        tool_call_id: "call-1".to_string(),
                        tool_name: "glob".to_string(),
                        parameters: serde_json::json!({
                            "pattern": "**/Cargo.toml",
                            "path": "crates"
                        }),
                        command_actions: Vec::new(),
                    })
                    .expect("serialize tool call payload"),
                },
            },
            &event_tx,
        );

        assert_eq!(
            event_rx.try_recv().expect("worker refresh event"),
            WorkerEvent::Transcript(
                crate::transcript::lifecycle::ItemLifecycleEvent::ToolOpened {
                    tool_use_id: "call-1".to_string(),
                    tool_name: "glob".to_string(),
                    input: serde_json::json!({
                        "pattern": "**/Cargo.toml",
                        "path": "crates"
                    }),
                    command: None,
                    command_source: None,
                    parsed_commands: vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
                        cmd: "List crates".to_string(),
                        path: Some("**/Cargo.toml in crates".to_string()),
                    }],
                }
            )
        );
    }

    #[test]
    fn completed_tool_result_falls_back_to_content_preview() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        dispatch_legacy_item_event_for_test(
            "item/completed",
            ItemEventPayload {
                context: devo_server::EventContext {
                    session_id: SessionId::new(),
                    turn_id: None,
                    item_id: None,
                    seq: 1,
                    item_seq: None,
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::ToolResult,
                    payload: serde_json::to_value(ToolResultPayload {
                        tool_call_id: "call-1".to_string(),
                        tool_name: Some("read".to_string()),
                        input: None,
                        content: serde_json::Value::String(
                            "<content>canonical</content>".to_string(),
                        ),
                        display_content: None,
                        is_error: false,
                        summary: "read output".to_string(),
                    })
                    .expect("serialize tool result payload"),
                },
            },
            &event_tx,
        );

        assert_eq!(
            event_rx.try_recv().expect("worker event"),
            WorkerEvent::Transcript(tool_lifecycle::tool_closed_from_result(
                &ToolResultPayload {
                    tool_call_id: "call-1".to_string(),
                    tool_name: None,
                    input: None,
                    content: serde_json::Value::String("<content>canonical</content>".to_string(),),
                    display_content: None,
                    is_error: false,
                    summary: String::new(),
                },
            ))
        );
    }

    #[test]
    fn completed_file_change_item_dispatches_via_legacy_projection() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        dispatch_legacy_item_event_for_test(
            "item/completed",
            ItemEventPayload {
                context: devo_server::EventContext {
                    session_id: SessionId::new(),
                    turn_id: None,
                    item_id: None,
                    seq: 1,
                    item_seq: None,
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::FileChange,
                    payload: serde_json::to_value(FileChangePayload {
                        tool_call_id: "call-1".to_string(),
                        tool_name: Some("apply_patch".to_string()),
                        input: None,
                        changes: vec![(
                            PathBuf::from("foo.txt"),
                            devo_protocol::protocol::FileChange::Update {
                                unified_diff: "diff --git a/foo.txt b/foo.txt\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
                                old_text: None,
                                new_text: None,
                                move_path: None,
                            },
                        )],
                        is_error: false,
                    })
                    .expect("serialize file change payload"),
                },
            },
            &event_tx,
        );

        let WorkerEvent::Transcript(crate::transcript::lifecycle::ItemLifecycleEvent::ToolClosed {
            tool_use_id,
            file_changes: Some(changes),
            ..
        }) = event_rx.try_recv().expect("worker event")
        else {
            panic!("expected file change tool closed event");
        };
        assert_eq!(tool_use_id, "call-1");
        assert!(changes.contains_key(&PathBuf::from("foo.txt")));
    }

    #[test]
    fn terminal_usage_fallback_skips_sessions_with_authoritative_totals() {
        assert!(!super::should_apply_terminal_turn_usage_fallback(
            /*saw_usage_update_for_turn*/ false, /*has_authoritative_usage_totals*/ true,
        ));
        assert!(super::should_apply_terminal_turn_usage_fallback(
            /*saw_usage_update_for_turn*/ false,
            /*has_authoritative_usage_totals*/ false,
        ));
        assert!(!super::should_apply_terminal_turn_usage_fallback(
            /*saw_usage_update_for_turn*/ true, /*has_authoritative_usage_totals*/ false,
        ));
    }

    #[test]
    fn command_execution_started_event_uses_server_command_actions() {
        let payload = CommandExecutionPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: "read".to_string(),
            command: "read crates/tui/src/chatwidget.rs".to_string(),
            source: devo_protocol::protocol::ExecCommandSource::Agent,
            command_actions: vec![devo_protocol::parse_command::ParsedCommand::Read {
                cmd: "read crates/tui/src/chatwidget.rs".to_string(),
                name: "chatwidget.rs".to_string(),
                path: PathBuf::from("crates/tui/src/chatwidget.rs"),
            }],
            input: Some(serde_json::json!({
                "path": "crates/tui/src/chatwidget.rs",
            })),
            output: None,
            is_error: false,
        };

        assert_eq!(
            super::super::worker_event_test_helpers::command_execution_started(
                payload.tool_call_id.clone(),
                payload.command.clone(),
                payload.input.clone(),
                payload.source,
                payload.command_actions.clone(),
            ),
            super::super::worker_event_test_helpers::command_execution_started(
                payload.tool_call_id,
                payload.command,
                payload.input,
                devo_protocol::protocol::ExecCommandSource::Agent,
                payload.command_actions,
            )
        );
    }

    fn test_session_metadata(
        session_id: SessionId,
        parent_session_id: Option<SessionId>,
    ) -> SessionMetadata {
        SessionMetadata {
            session_id,
            cwd: ".".into(),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Saved conversation".to_string()),
            title_state: SessionTitleState::Generating,
            parent_session_id,
            fork_from_id: None,
            fork_at_turn_id: None,
            agent_path: parent_session_id.map(|_| "root/reviewer".to_string()),
            agent_nickname: parent_session_id.map(|_| "reviewer".to_string()),
            agent_role: parent_session_id.map(|_| "default".to_string()),
            ephemeral: false,
            model: Some("test-model".to_string()),
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

    #[test]
    fn last_query_tokens_from_resume_prefers_session_last_query_usage() {
        use devo_protocol::TurnKind;
        use devo_protocol::TurnMetadata;
        use devo_protocol::TurnStatus;
        use devo_protocol::TurnUsage;

        let session_id = SessionId::new();
        let mut session = test_session_metadata(session_id, None);
        session.total_input_tokens = 500;
        session.last_query_total_tokens = 999;
        session.prompt_token_estimate = 55;
        session.last_query_usage = Some(TurnUsage {
            input_tokens: 30,
            output_tokens: 12,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: Some(42),
        });
        let _turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence: 1,
            status: TurnStatus::Completed,
            kind: TurnKind::Regular,
            model: "test-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".to_string(),
            request_thinking: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            usage: Some(TurnUsage {
                input_tokens: 7,
                output_tokens: 2,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(9),
            }),
            stop_reason: None,
            failure_reason: None,
        };

        assert_eq!(last_query_tokens_from_resume(&session), (42, 30));

        session.last_query_usage = None;
        assert_eq!(last_query_tokens_from_resume(&session), (55, 55));

        session.prompt_token_estimate = 0;
        assert_eq!(last_query_tokens_from_resume(&session), (0, 0));
    }

    #[test]
    fn usage_update_state_keeps_latest_total_for_terminal_event_without_usage() {
        use devo_protocol::TurnUsage;

        let mut last_query_total_tokens = 42usize;
        let has_authoritative_usage_totals = false;

        let usage = TurnUsage {
            input_tokens: 35,
            output_tokens: 13,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(5),
            reasoning_output_tokens: None,
            total_tokens: Some(48),
        };
        assert_eq!(last_query_total_tokens, 42);

        let saw_usage_update_for_turn = true;
        let total_input_tokens = 550;
        let total_output_tokens = 110;
        let total_tokens = 660;
        let total_cache_read_tokens = 60;
        last_query_total_tokens = usage.display_total_tokens();
        let last_query_input_tokens = usage.input_tokens as usize;

        if !should_apply_terminal_turn_usage_fallback(
            saw_usage_update_for_turn,
            has_authoritative_usage_totals,
        ) {
            // Simulates a terminal turn/completed event without embedded usage.
        }

        let terminal_event = WorkerEvent::TurnFinished {
            stop_reason: "Completed".to_string(),
            turn_count: 1,
            total_input_tokens,
            total_output_tokens,
            total_tokens,
            total_cache_read_tokens,
            last_query_total_tokens,
            last_query_input_tokens,
            prompt_token_estimate: total_input_tokens,
        };

        assert_eq!(
            terminal_event,
            WorkerEvent::TurnFinished {
                stop_reason: "Completed".to_string(),
                turn_count: 1,
                total_input_tokens: 550,
                total_output_tokens: 110,
                total_tokens: 660,
                total_cache_read_tokens: 60,
                last_query_total_tokens: 48,
                last_query_input_tokens: 35,
                prompt_token_estimate: 550,
            }
        );
    }

    #[test]
    fn session_started_metadata_discovers_child_subagent() {
        let parent = SessionId::new();
        let child = SessionId::new();
        // Post-cutover discovery reads the SessionMetadata carried by the
        // SessionStarted devo event directly (no ACP session-info envelope).
        let metadata = test_session_metadata(child, Some(parent));
        let agent = super::subagent_events::agent_from_session(&metadata).expect("subagent");

        assert_eq!(
            agent,
            SubagentMonitorAgent {
                session_id: child,
                parent_session_id: parent,
                agent_path: "root/reviewer".to_string(),
                nickname: "reviewer".to_string(),
                role: "default".to_string(),
                status: "idle".to_string(),
                last_task_message: None,
            }
        );
    }

    #[test]
    fn child_typed_tool_result_updates_subagent_preview() {
        let child = SessionId::new();
        let item = devo_protocol::native::item::ItemEnvelope {
            id: devo_protocol::native::ids::ItemId::new(),
            session_id: devo_protocol::native::ids::SessionId::from_string(child.to_string()),
            turn_id: devo_protocol::native::ids::TurnId::new(),
            seq: 1,
            revision: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            state: devo_protocol::native::item::ItemState::Completed,
            item: devo_protocol::native::item::Item::ToolResult {
                call_id: "call-1".to_string(),
                output: serde_json::json!("done"),
                display_content: Some("done".to_string()),
                is_error: false,
                truncated: false,
            },
        };

        let events = super::subagent_events::subagent_monitor_events_from_typed_item(child, &item);

        assert_eq!(
            events,
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::ToolResult {
                    session_id: child,
                    tool_use_id: "call-1".to_string(),
                    title: String::new(),
                    preview: "done".to_string(),
                    is_error: false,
                },
            }]
        );
    }

    #[test]
    fn parent_typed_spawn_tool_result_extracts_subagent_discovery_signal() {
        let child = SessionId::new();
        let result =
            super::subagent_events::spawn_agent_result_from_raw_output(Some(&serde_json::json!({
                "task_id": "task-1",
                "child_session_id": child,
                "agent_path": "root/researcher",
                "agent_nickname": "researcher",
                "status": "running"
            })))
            .expect("spawn agent result");

        assert_eq!(result.child_session_id, child);
        assert_eq!(result.agent_path, "root/researcher");
        assert_eq!(result.agent_nickname, "researcher");
        assert_eq!(result.status, "running");
    }

    #[test]
    fn session_leave_pause_decision_only_pauses_active_goals() {
        let session_id = SessionId::new();
        let active_goal = ThreadGoal {
            thread_id: session_id,
            objective: "finish the goal".to_string(),
            status: ThreadGoalStatus::Active,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at: 1,
            updated_at: 1,
        };
        let paused_goal = ThreadGoal {
            status: ThreadGoalStatus::Paused,
            ..active_goal.clone()
        };
        let budget_limited_goal = ThreadGoal {
            status: ThreadGoalStatus::BudgetLimited,
            ..active_goal.clone()
        };

        assert_eq!(
            [
                should_pause_goal_before_session_leave(Some(&active_goal)),
                should_pause_goal_before_session_leave(Some(&budget_limited_goal)),
                should_pause_goal_before_session_leave(Some(&paused_goal)),
                should_pause_goal_before_session_leave(None),
            ],
            [true, true, false, false]
        );
    }

    #[test]
    fn session_list_entries_keep_title_before_identifier() {
        let active_session_id = SessionId::new();
        let summary = SessionMetadata {
            session_id: active_session_id,
            cwd: ".".into(),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Saved conversation".to_string()),
            title_state: SessionTitleState::Generating,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: false,
            model: Some("test-model".to_string()),
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
        };
        let entry = SessionListEntry {
            session_id: summary.session_id,
            title: summary.title.clone().unwrap_or_default(),
            preview: String::new(),
            cwd: summary.cwd.clone(),
            branch: None,
            last_activity_at: summary.last_activity_at,
            transcript_size_bytes: Some(10_300),
            is_active: true,
        };

        assert_eq!(entry.title, "Saved conversation");
        assert_eq!(entry.last_activity_at, summary.last_activity_at);
    }

    #[test]
    fn session_list_entries_mark_inactive_sessions() {
        let summary = SessionMetadata {
            session_id: SessionId::new(),
            cwd: ".".into(),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Saved conversation".to_string()),
            title_state: SessionTitleState::Generating,
            parent_session_id: None,
            fork_from_id: None,
            fork_at_turn_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: false,
            model: Some("test-model".to_string()),
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
        };
        let entry = SessionListEntry {
            session_id: summary.session_id,
            title: summary.title.clone().unwrap_or_default(),
            preview: String::new(),
            cwd: summary.cwd.clone(),
            branch: None,
            last_activity_at: summary.last_activity_at,
            transcript_size_bytes: Some(10_300),
            is_active: false,
        };

        assert!(!entry.is_active);
    }

    #[test]
    fn display_output_normalization_trims_crlf_padding() {
        assert_eq!(
            normalize_display_output("\r\n\r\nhello\r\nworld\r\n\r\n"),
            "hello\nworld"
        );
    }

    #[test]
    fn project_history_merges_tool_call_and_result() {
        let items = vec![
            SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: SessionHistoryItemKind::ToolCall,
                title: "Ran powershell -Command \"Get-Date\"".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: SessionHistoryItemKind::ToolResult,
                title: "Tool output".to_string(),
                body: "2026-04-09".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
        ];

        assert_eq!(
            project_history_items(&items),
            vec![TranscriptItem::restored_tool_result(
                "Ran powershell -Command \"Get-Date\"",
                "2026-04-09"
            )]
        );
    }

    #[test]
    fn project_history_pairs_tool_results_by_call_id_not_time_adjacency() {
        let items = vec![
            SessionHistoryItem {
                tool_call_id: Some("call-a".to_string()),
                kind: SessionHistoryItemKind::ToolCall,
                title: "Ran read a".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            SessionHistoryItem {
                tool_call_id: Some("call-b".to_string()),
                kind: SessionHistoryItemKind::ToolCall,
                title: "Ran read b".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            SessionHistoryItem {
                tool_call_id: Some("call-b".to_string()),
                kind: SessionHistoryItemKind::ToolResult,
                title: "Tool output".to_string(),
                body: "B".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            SessionHistoryItem {
                tool_call_id: Some("call-a".to_string()),
                kind: SessionHistoryItemKind::ToolResult,
                title: "Tool output".to_string(),
                body: "A".to_string(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
        ];

        assert_eq!(
            project_history_items(&items),
            vec![
                TranscriptItem::restored_tool_result("Ran read a", "A"),
                TranscriptItem::restored_tool_result("Ran read b", "B"),
            ]
        );
    }

    #[test]
    fn project_history_understands_plan_metadata() {
        let items = vec![SessionHistoryItem {
            tool_call_id: None,
            kind: SessionHistoryItemKind::Assistant,
            title: String::new(),
            body: r#"{"explanation":"Do work","plan":[{"step":"Inspect","status":"completed"}]}"#
                .to_string(),
            tool_io: None,
            metadata: Some(SessionHistoryMetadata::PlanUpdate {
                explanation: Some("Do work".to_string()),
                steps: vec![devo_protocol::SessionPlanStep {
                    text: "Inspect".to_string(),
                    status: SessionPlanStepStatus::Completed,
                }],
            }),
            duration_ms: None,
        }];

        let projected = project_history_items(&items);
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].kind, TranscriptItemKind::System);
        assert!(projected[0].body.contains("completed: Inspect"));
    }

    #[test]
    fn project_history_prefers_plan_metadata_over_paired_tool_output() {
        let items = vec![
            SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: SessionHistoryItemKind::ToolCall,
                title: "update_plan".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: SessionHistoryItemKind::ToolResult,
                title: "update_plan output".to_string(),
                body:
                    r#"{"explanation":"Do work","plan":[{"step":"Inspect","status":"completed"}]}"#
                        .to_string(),
                tool_io: None,
                metadata: Some(SessionHistoryMetadata::PlanUpdate {
                    explanation: Some("Do work".to_string()),
                    steps: vec![devo_protocol::SessionPlanStep {
                        text: "Inspect".to_string(),
                        status: SessionPlanStepStatus::Completed,
                    }],
                }),
                duration_ms: None,
            },
        ];

        assert_eq!(
            project_history_items(&items),
            vec![TranscriptItem::new(
                TranscriptItemKind::System,
                "Do work",
                "completed: Inspect"
            )]
        );
    }

    #[test]
    fn project_history_keeps_edited_metadata_result_as_fallback_output() {
        let items = vec![
            SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: SessionHistoryItemKind::ToolCall,
                title: "write foo.txt".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: None,
                duration_ms: None,
            },
            SessionHistoryItem {
                tool_call_id: Some("call-1".to_string()),
                kind: SessionHistoryItemKind::ToolResult,
                title: "write output".to_string(),
                body: "patched".to_string(),
                tool_io: None,
                metadata: Some(SessionHistoryMetadata::Edited {
                    changes: std::collections::HashMap::new(),
                }),
                duration_ms: None,
            },
        ];

        assert_eq!(
            project_history_items(&items),
            vec![TranscriptItem::restored_tool_result(
                "write output",
                "patched"
            )]
        );
    }

    #[test]
    fn project_history_restores_command_execution_items() {
        let items = vec![SessionHistoryItem {
            tool_call_id: Some("call-1".to_string()),
            kind: SessionHistoryItemKind::CommandExecution,
            title: "cargo test".to_string(),
            body: "ok".to_string(),
            tool_io: None,
            metadata: None,
            duration_ms: None,
        }];

        assert_eq!(
            project_history_items(&items),
            vec![TranscriptItem::restored_tool_result("cargo test", "ok")]
        );
    }

    #[test]
    fn project_history_preserves_reasoning_items() {
        let items = vec![SessionHistoryItem {
            tool_call_id: None,
            kind: SessionHistoryItemKind::Reasoning,
            title: String::new(),
            body: "thinking aloud".to_string(),
            tool_io: None,
            metadata: None,
            duration_ms: None,
        }];

        assert_eq!(
            project_history_items(&items),
            vec![TranscriptItem::new(
                TranscriptItemKind::Reasoning,
                "",
                "thinking aloud"
            )]
        );
    }
    #[test]
    fn preview_keeps_only_last_four_dialogue_messages() {
        let user = |text: &str| devo_protocol::native::item::Item::UserMessage {
            client_user_message_id: None,
            content: vec![devo_protocol::native::item::UserInput::Text {
                text: text.to_string(),
            }],
            entry: devo_protocol::native::item::UserMessageEntry::default(),
        };
        let assistant = |text: &str| devo_protocol::native::item::Item::AssistantMessage {
            text: text.to_string(),
            phase: None,
        };
        let mut messages = std::collections::VecDeque::new();
        for item in [
            user("one"),
            assistant("two"),
            devo_protocol::native::item::Item::Reasoning {
                text: "hidden".to_string(),
                provider_payload_ref: None,
            },
            user("three"),
            assistant("four"),
            user("five"),
        ] {
            append_preview_item(&mut messages, item);
        }

        assert_eq!(
            messages.into_iter().collect::<Vec<_>>(),
            vec![
                SessionPreviewMessage {
                    role: SessionPreviewRole::Assistant,
                    text: "two".to_string(),
                },
                SessionPreviewMessage {
                    role: SessionPreviewRole::User,
                    text: "three".to_string(),
                },
                SessionPreviewMessage {
                    role: SessionPreviewRole::Assistant,
                    text: "four".to_string(),
                },
                SessionPreviewMessage {
                    role: SessionPreviewRole::User,
                    text: "five".to_string(),
                },
            ]
        );
    }
    #[test]
    fn restored_history_includes_native_turn_summary() {
        let started_at = Utc::now();
        let turn = devo_protocol::native::turn::Turn {
            id: devo_protocol::native::ids::TurnId::from_legacy_uuid(
                devo_protocol::TurnId::new().into(),
            ),
            session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(
                devo_protocol::SessionId::new().into(),
            ),
            sequence: 1,
            kind: devo_protocol::native::turn::TurnKind::Regular,
            status: devo_protocol::native::turn::TurnStatus::Completed,
            model: devo_protocol::native::model::ModelBinding {
                provider: "test".to_string(),
                model: "test-model".to_string(),
                reasoning_effort: None,
            },
            collaboration_mode: Some(devo_protocol::CollaborationMode::Plan),
            started_at,
            completed_at: Some(started_at + chrono::Duration::seconds(3)),
            error: None,
            usage: None,
        };

        assert_eq!(
            restored_history_items(
                vec![turn],
                Vec::new(),
                devo_protocol::CollaborationMode::Build,
            ),
            vec![devo_protocol::SessionHistoryItem {
                tool_call_id: None,
                kind: devo_protocol::SessionHistoryItemKind::TurnSummary,
                title: "test-model".to_string(),
                body: String::new(),
                tool_io: None,
                metadata: Some(devo_protocol::SessionHistoryMetadata::TurnSummary {
                    collaboration_mode: devo_protocol::CollaborationMode::Plan,
                }),
                duration_ms: Some(3),
            }]
        );
    }
}
