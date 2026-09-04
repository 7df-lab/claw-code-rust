use std::path::PathBuf;

use devo_protocol::ApprovalDecisionValue;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::CollaborationMode;
use devo_protocol::InputItem;
use devo_protocol::RequestUserInputResponse;
use devo_protocol::SessionId;
use devo_protocol::ThreadGoalStatus;
use devo_protocol::TurnId;
use devo_protocol::TurnStartParams;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub(crate) enum PersistScope {
    /// Apply to the active session only; do not write user/project defaults.
    #[default]
    Session,
    /// Write user/project defaults and hot-apply to the active session when one exists.
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum InputHistoryDirection {
    Previous,
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum GoalObjectiveMode {
    ConfirmIfExists,
    ReplaceExisting,
    UpdateExisting {
        status: ThreadGoalStatus,
        token_budget: Option<i64>,
    },
}

/// Command requests emitted by v2 UI components.
///
/// Thin wrapper around protocol-wide operations. Claw's
/// protocol is RPC-shaped instead, so the TUI owns a small command enum and the
/// host/worker adapter converts the relevant variants into protocol params.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) enum AppCommand {
    RunUserShellCommand {
        command: String,
    },
    /// Validate a provider Connection and model through the canonical Native RPC.
    ProviderValidate {
        params: devo_protocol::native::rpc_admin::ProviderValidateParams,
    },
    /// Load provider Connections and directory templates through the canonical Native RPC.
    ProviderList,
    /// Persist a provider Connection and its model directory through the canonical Native RPC.
    ProviderUpsert {
        params: devo_protocol::native::rpc_admin::ProviderUpsertParams,
    },
    /// Disconnect a configured provider Connection.
    DisconnectProvider {
        provider_id: String,
    },
    /// Remove one model from a configured provider Connection.
    RemoveProviderModel {
        provider_id: String,
        model_id: String,
    },
    SubmitShellInput {
        command: String,
    },
    ExecuteShellCommand {
        command: String,
    },
    Compact,
    ShowGoal,
    EditGoal,
    SetGoalObjective {
        objective: String,
        mode: GoalObjectiveMode,
    },
    SetGoalStatus {
        status: ThreadGoalStatus,
    },
    ClearGoal,
    UserTurn {
        input: Vec<InputItem>,
        cwd: Option<PathBuf>,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        sandbox: Option<String>,
        approval_policy: Option<String>,
        collaboration_mode: CollaborationMode,
    },
    OverrideTurnContext {
        cwd: Option<PathBuf>,
        model: Option<String>,
        reasoning_effort_selection: Option<Option<String>>,
        sandbox: Option<Option<String>>,
        approval_policy: Option<Option<String>>,
        persist_scope: PersistScope,
    },
    SetCollaborationMode {
        collaboration_mode: CollaborationMode,
        persist_scope: PersistScope,
    },
    /// Enqueue input on the active session while a turn is busy.
    QueuePush {
        input: Vec<InputItem>,
    },
    /// Promote a queued item into the active turn as a steer.
    QueueSteer {
        queue_item_id: String,
        expected_turn_id: TurnId,
    },
    /// Remove a queued item (e.g. before editing in the composer).
    QueueRemove {
        queue_item_id: String,
    },
    /// Replace a queued item's content in place, preserving its position.
    QueueUpdate {
        queue_item_id: String,
        input: Vec<InputItem>,
    },
    RunBtwQuestion {
        question: String,
    },
    ApprovalRespond {
        session_id: SessionId,
        turn_id: TurnId,
        approval_id: String,
        decision: ApprovalDecisionValue,
        scope: ApprovalScopeValue,
    },
    RequestUserInputRespond {
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        response: RequestUserInputResponse,
    },
    UpdatePermissions {
        preset: devo_protocol::PermissionPreset,
        persist_scope: PersistScope,
    },
    UpdateEffectiveContextWindow {
        effective_context_window: u64,
    },
    UpdateSandboxProfile {
        profile: String,
    },
    BrowseInputHistory {
        direction: InputHistoryDirection,
    },
    SwitchSession {
        session_id: SessionId,
    },
    ListSessions,
    PreviewSession {
        session_id: SessionId,
    },
    RenameSession {
        title: String,
    },
    RenameSessionById {
        session_id: SessionId,
        title: String,
    },
    /// Delete a session. `None` deletes the current active session.
    DeleteSession {
        session_id: Option<SessionId>,
    },
    RollbackToUserTurn {
        user_turn_index: u32,
    },
    ForkAtUserTurn {
        user_turn_index: u32,
        /// `Through` continues from the selected turn; `Before` drops it (edit-earlier).
        cut: devo_protocol::native::rpc_session::SessionForkCut,
    },
    /// Request MCP server runtime statuses (`mcp/list`).
    ListMcpServers,
    /// Request tools for one MCP server (`mcp/tools`).
    ListMcpTools {
        name: String,
    },
    /// Persist enable/disable for one MCP server in user config.
    SetMcpServerEnabled {
        name: String,
        enabled: bool,
    },
    /// Persistently enable or disable one skill by `SKILL.md` path.
    SetSkillEnabled {
        path: PathBuf,
        enabled: bool,
        name: String,
    },
}

#[allow(dead_code)]
pub(crate) enum AppCommandView<'a> {
    Interrupt {
        reason: &'a Option<String>,
    },
    CleanBackgroundTerminals,
    RunUserShellCommand {
        command: &'a str,
    },
    SubmitShellInput {
        command: &'a str,
    },
    ExecuteShellCommand {
        command: &'a str,
    },
    Compact,
    ShowGoal,
    EditGoal,
    SetGoalObjective {
        objective: &'a str,
        mode: GoalObjectiveMode,
    },
    SetGoalStatus {
        status: ThreadGoalStatus,
    },
    ClearGoal,
    UserTurn {
        input: &'a [InputItem],
        cwd: &'a Option<PathBuf>,
        model: &'a Option<String>,
        model_binding_id: &'a Option<String>,
        reasoning_effort_selection: &'a Option<String>,
        sandbox: &'a Option<String>,
        approval_policy: &'a Option<String>,
        collaboration_mode: CollaborationMode,
    },
    RunBtwQuestion {
        question: &'a str,
    },
    ApprovalRespond {
        approval_id: &'a str,
        decision: &'a ApprovalDecisionValue,
        scope: &'a ApprovalScopeValue,
    },
    RequestUserInputRespond {
        request_id: &'a str,
        response: &'a RequestUserInputResponse,
    },
    UpdatePermissions {
        preset: devo_protocol::PermissionPreset,
        persist_scope: PersistScope,
    },
    UpdateEffectiveContextWindow {
        effective_context_window: u64,
    },
    UpdateSandboxProfile {
        profile: &'a str,
    },
    OverrideTurnContext {
        cwd: &'a Option<PathBuf>,
        model: &'a Option<String>,
        reasoning_effort_selection: &'a Option<Option<String>>,
        sandbox: &'a Option<Option<String>>,
        approval_policy: &'a Option<Option<String>>,
    },
    ReloadUserConfig,
    ListSkills {
        cwds: &'a [PathBuf],
        force_reload: bool,
    },
    SetThreadName {
        name: &'a str,
    },
    Shutdown,
    ThreadRollback {
        num_turns: u32,
    },
    Review {
        request: &'a str,
    },
    BrowseInputHistory {
        direction: InputHistoryDirection,
    },
    SwitchSession {
        session_id: SessionId,
    },
    RenameSession {
        title: &'a str,
    },
    DeleteSession,
    RollbackToUserTurn {
        user_turn_index: u32,
    },
    ForkAtUserTurn {
        user_turn_index: u32,
        cut: devo_protocol::native::rpc_session::SessionForkCut,
    },
}

impl AppCommand {
    #[allow(dead_code)]
    pub(crate) fn run_user_shell_command(command: String) -> Self {
        Self::RunUserShellCommand { command }
    }

    pub(crate) fn user_turn(
        input: Vec<InputItem>,
        cwd: Option<PathBuf>,
        model: Option<String>,
        reasoning_effort_selection: Option<String>,
        sandbox: Option<String>,
        approval_policy: Option<String>,
    ) -> Self {
        Self::user_turn_with_collaboration_mode(
            input,
            cwd,
            model,
            /*model_binding_id*/ None,
            reasoning_effort_selection,
            sandbox,
            approval_policy,
            CollaborationMode::Build,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn user_turn_with_collaboration_mode(
        input: Vec<InputItem>,
        cwd: Option<PathBuf>,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        sandbox: Option<String>,
        approval_policy: Option<String>,
        collaboration_mode: CollaborationMode,
    ) -> Self {
        Self::UserTurn {
            input,
            cwd,
            model,
            model_binding_id,
            reasoning_effort_selection,
            sandbox,
            approval_policy,
            collaboration_mode,
        }
    }

    pub(crate) fn execute_shell_command(command: String) -> Self {
        Self::ExecuteShellCommand { command }
    }

    pub(crate) fn submit_shell_input(command: String) -> Self {
        Self::SubmitShellInput { command }
    }

    #[allow(dead_code)]
    pub(crate) fn text_turn(text: String, cwd: Option<PathBuf>, model: Option<String>) -> Self {
        Self::user_turn(
            vec![InputItem::Text { text }],
            cwd,
            model,
            /*reasoning_effort_selection*/ None,
            /*sandbox*/ None,
            /*approval_policy*/ None,
        )
    }

    pub(crate) fn override_turn_context(
        cwd: Option<PathBuf>,
        model: Option<String>,
        reasoning_effort_selection: Option<Option<String>>,
        sandbox: Option<Option<String>>,
        approval_policy: Option<Option<String>>,
    ) -> Self {
        Self::override_turn_context_with_scope(
            cwd,
            model,
            reasoning_effort_selection,
            sandbox,
            approval_policy,
            PersistScope::Session,
        )
    }

    pub(crate) fn override_turn_context_with_scope(
        cwd: Option<PathBuf>,
        model: Option<String>,
        reasoning_effort_selection: Option<Option<String>>,
        sandbox: Option<Option<String>>,
        approval_policy: Option<Option<String>>,
        persist_scope: PersistScope,
    ) -> Self {
        Self::OverrideTurnContext {
            cwd,
            model,
            reasoning_effort_selection,
            sandbox,
            approval_policy,
            persist_scope,
        }
    }

    pub(crate) fn set_collaboration_mode(
        collaboration_mode: CollaborationMode,
        persist_scope: PersistScope,
    ) -> Self {
        Self::SetCollaborationMode {
            collaboration_mode,
            persist_scope,
        }
    }

    pub(crate) fn update_permissions(
        preset: devo_protocol::PermissionPreset,
        persist_scope: PersistScope,
    ) -> Self {
        Self::UpdatePermissions {
            preset,
            persist_scope,
        }
    }

    pub(crate) fn browse_input_history(direction: InputHistoryDirection) -> Self {
        Self::BrowseInputHistory { direction }
    }

    pub(crate) fn compact() -> Self {
        Self::Compact
    }

    pub(crate) fn show_goal() -> Self {
        Self::ShowGoal
    }

    pub(crate) fn edit_goal() -> Self {
        Self::EditGoal
    }

    pub(crate) fn set_goal_objective(objective: String, mode: GoalObjectiveMode) -> Self {
        Self::SetGoalObjective { objective, mode }
    }

    pub(crate) fn set_goal_status(status: ThreadGoalStatus) -> Self {
        Self::SetGoalStatus { status }
    }

    pub(crate) fn clear_goal() -> Self {
        Self::ClearGoal
    }

    pub(crate) fn switch_session(session_id: SessionId) -> Self {
        Self::SwitchSession { session_id }
    }

    pub(crate) fn list_sessions() -> Self {
        Self::ListSessions
    }

    pub(crate) fn preview_session(session_id: SessionId) -> Self {
        Self::PreviewSession { session_id }
    }

    pub(crate) fn rename_session(title: String) -> Self {
        Self::RenameSession { title }
    }

    pub(crate) fn rename_session_by_id(session_id: SessionId, title: String) -> Self {
        Self::RenameSessionById { session_id, title }
    }

    pub(crate) fn delete_session() -> Self {
        Self::DeleteSession { session_id: None }
    }

    pub(crate) fn delete_session_by_id(session_id: SessionId) -> Self {
        Self::DeleteSession {
            session_id: Some(session_id),
        }
    }

    pub(crate) fn rollback_to_user_turn(user_turn_index: u32) -> Self {
        Self::RollbackToUserTurn { user_turn_index }
    }

    pub(crate) fn fork_at_user_turn(
        user_turn_index: u32,
        cut: devo_protocol::native::rpc_session::SessionForkCut,
    ) -> Self {
        Self::ForkAtUserTurn {
            user_turn_index,
            cut,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::RunUserShellCommand { .. } => "run_user_shell_command",
            Self::ProviderValidate { .. } => "provider_validate",
            Self::ProviderList => "provider_list",
            Self::ProviderUpsert { .. } => "provider_upsert",
            Self::DisconnectProvider { .. } => "disconnect_provider",
            Self::RemoveProviderModel { .. } => "remove_provider_model",
            Self::SubmitShellInput { .. } => "submit_shell_input",
            Self::ExecuteShellCommand { .. } => "execute_shell_command",
            Self::Compact => "compact",
            Self::ShowGoal => "show_goal",
            Self::EditGoal => "edit_goal",
            Self::SetGoalObjective { .. } => "set_goal_objective",
            Self::SetGoalStatus { .. } => "set_goal_status",
            Self::ClearGoal => "clear_goal",
            Self::UserTurn { .. } => "user_turn",
            Self::OverrideTurnContext { .. } => "override_turn_context",
            Self::SetCollaborationMode { .. } => "set_collaboration_mode",
            Self::QueuePush { .. } => "queue_push",
            Self::QueueSteer { .. } => "queue_steer",
            Self::QueueRemove { .. } => "queue_remove",
            Self::QueueUpdate { .. } => "queue_update",
            Self::RunBtwQuestion { .. } => "run_btw_question",
            Self::ApprovalRespond { .. } => "approval_respond",
            Self::RequestUserInputRespond { .. } => "request_user_input_respond",
            Self::UpdatePermissions { .. } => "update_permissions",
            Self::UpdateEffectiveContextWindow { .. } => "update_effective_context_window",
            Self::UpdateSandboxProfile { .. } => "update_sandbox_profile",
            Self::BrowseInputHistory { .. } => "browse_input_history",
            Self::SwitchSession { .. } => "switch_session",
            Self::ListSessions => "list_sessions",
            Self::PreviewSession { .. } => "preview_session",
            Self::RenameSession { .. } => "rename_session",
            Self::RenameSessionById { .. } => "rename_session_by_id",
            Self::DeleteSession { .. } => "delete_session",
            Self::RollbackToUserTurn { .. } => "rollback_to_user_turn",
            Self::ForkAtUserTurn { .. } => "fork_at_user_turn",
            Self::ListMcpServers => "list_mcp_servers",
            Self::ListMcpTools { .. } => "list_mcp_tools",
            Self::SetMcpServerEnabled { .. } => "set_mcp_server_enabled",
            Self::SetSkillEnabled { .. } => "set_skill_enabled",
        }
    }

    #[allow(dead_code)]
    pub(crate) fn view(&self) -> AppCommandView<'_> {
        match self {
            Self::RunUserShellCommand { command } => {
                AppCommandView::RunUserShellCommand { command }
            }
            Self::ProviderValidate { .. } | Self::ProviderList | Self::ProviderUpsert { .. } => {
                AppCommandView::ReloadUserConfig
            }
            Self::DisconnectProvider { .. } => AppCommandView::ReloadUserConfig,
            Self::RemoveProviderModel { .. } => AppCommandView::ReloadUserConfig,
            Self::SubmitShellInput { command } => AppCommandView::SubmitShellInput { command },
            Self::ExecuteShellCommand { command } => {
                AppCommandView::ExecuteShellCommand { command }
            }
            Self::Compact => AppCommandView::Compact,
            Self::ShowGoal => AppCommandView::ShowGoal,
            Self::EditGoal => AppCommandView::EditGoal,
            Self::SetGoalObjective { objective, mode } => AppCommandView::SetGoalObjective {
                objective,
                mode: *mode,
            },
            Self::SetGoalStatus { status } => AppCommandView::SetGoalStatus { status: *status },
            Self::ClearGoal => AppCommandView::ClearGoal,
            Self::UserTurn {
                input,
                cwd,
                model,
                model_binding_id,
                reasoning_effort_selection,
                sandbox,
                approval_policy,
                collaboration_mode,
            } => AppCommandView::UserTurn {
                input,
                cwd,
                model,
                model_binding_id,
                reasoning_effort_selection,
                sandbox,
                approval_policy,
                collaboration_mode: *collaboration_mode,
            },
            Self::OverrideTurnContext {
                cwd,
                model,
                reasoning_effort_selection,
                sandbox,
                approval_policy,
                ..
            } => AppCommandView::OverrideTurnContext {
                cwd,
                model,
                reasoning_effort_selection,
                sandbox,
                approval_policy,
            },
            Self::SetCollaborationMode { .. } => AppCommandView::ReloadUserConfig,
            Self::QueuePush { .. }
            | Self::QueueSteer { .. }
            | Self::QueueRemove { .. }
            | Self::QueueUpdate { .. } => AppCommandView::ReloadUserConfig,
            Self::RunBtwQuestion { question } => AppCommandView::RunBtwQuestion { question },
            Self::ApprovalRespond {
                approval_id,
                decision,
                scope,
                ..
            } => AppCommandView::ApprovalRespond {
                approval_id,
                decision,
                scope,
            },
            Self::RequestUserInputRespond {
                request_id,
                response,
                ..
            } => AppCommandView::RequestUserInputRespond {
                request_id,
                response,
            },
            Self::UpdatePermissions {
                preset,
                persist_scope,
            } => AppCommandView::UpdatePermissions {
                preset: *preset,
                persist_scope: *persist_scope,
            },
            Self::UpdateEffectiveContextWindow {
                effective_context_window,
            } => AppCommandView::UpdateEffectiveContextWindow {
                effective_context_window: *effective_context_window,
            },
            Self::UpdateSandboxProfile { profile } => {
                AppCommandView::UpdateSandboxProfile { profile }
            }
            Self::BrowseInputHistory { direction } => AppCommandView::BrowseInputHistory {
                direction: *direction,
            },
            Self::SwitchSession { session_id } => AppCommandView::SwitchSession {
                session_id: *session_id,
            },
            Self::RenameSession { title } => AppCommandView::RenameSession { title },
            Self::ListSessions | Self::PreviewSession { .. } | Self::RenameSessionById { .. } => {
                AppCommandView::ReloadUserConfig
            }
            Self::DeleteSession { .. } => AppCommandView::DeleteSession,
            Self::RollbackToUserTurn { user_turn_index } => AppCommandView::ThreadRollback {
                num_turns: *user_turn_index,
            },
            Self::ForkAtUserTurn {
                user_turn_index, ..
            } => AppCommandView::ThreadRollback {
                num_turns: *user_turn_index,
            },
            Self::ListMcpServers
            | Self::ListMcpTools { .. }
            | Self::SetMcpServerEnabled { .. }
            | Self::SetSkillEnabled { .. } => AppCommandView::ReloadUserConfig,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn to_turn_start_params(&self, session_id: SessionId) -> Option<TurnStartParams> {
        let Self::UserTurn {
            input,
            cwd,
            model,
            model_binding_id,
            reasoning_effort_selection,
            sandbox,
            approval_policy,
            collaboration_mode,
        } = self
        else {
            return None;
        };

        Some(TurnStartParams {
            session_id,
            input: input.clone(),
            model: model.clone(),
            model_binding_id: model_binding_id.clone(),
            reasoning_effort_selection: reasoning_effort_selection.clone(),
            sandbox: sandbox.clone(),
            approval_policy: approval_policy.clone(),
            cwd: cwd.clone(),
            collaboration_mode: *collaboration_mode,
            execution_mode: devo_server::TurnExecutionMode::Regular,
        })
    }
}
