use std::sync::Arc;

use devo_core::SessionTitleState;
use devo_core::TurnId;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::CollaborationMode;
use devo_protocol::PendingInputItem;
use devo_protocol::ThreadGoal;
use tokio::sync::oneshot;

use super::snapshots::{
    HookContextSnapshot, PendingQueueSnapshot, PersistItemPrep, QueuedTurnInputData,
    ShellExecContextSnapshot, ShutdownDeferredSnapshot, TitleGenerationContext,
    TurnPersistenceSnapshot, TurnReservationSnapshot,
};
use super::state::{ApprovalCacheSnapshot, DeferredItems, SessionActorState, SpawnSnapshot};
use crate::execution::PendingApproval;
use crate::execution::PersistedTurnItem;
use crate::runtime::subagent_usage::ParentUsageSnapshot;
use crate::runtime::turn_exec::ExecuteTurnRequest;
use crate::session::SessionHistoryItem;
use crate::session::SessionMetadata;
use crate::turn::TurnMetadata;
use devo_core::TurnConfig;

pub(crate) enum SessionCommand {
    ExecuteTurn {
        runtime: Arc<crate::runtime::ServerRuntime>,
        request: ExecuteTurnRequest,
        reply: oneshot::Sender<()>,
    },
    GetSummary {
        reply: oneshot::Sender<SessionMetadata>,
    },
    GetSpawnSnapshot {
        reply: oneshot::Sender<SpawnSnapshot>,
    },
    GetApprovalCacheSnapshot {
        reply: oneshot::Sender<ApprovalCacheSnapshot>,
    },
    GetCollaborationMode {
        reply: oneshot::Sender<CollaborationMode>,
    },
    GetParentSessionId {
        reply: oneshot::Sender<Option<devo_protocol::SessionId>>,
    },
    GetTurnReservationSnapshot {
        reply: oneshot::Sender<TurnReservationSnapshot>,
    },
    GetHookContextSnapshot {
        reply: oneshot::Sender<HookContextSnapshot>,
    },
    GetTurnPersistenceSnapshot {
        reply: oneshot::Sender<TurnPersistenceSnapshot>,
    },
    GetShellExecContext {
        cwd: std::path::PathBuf,
        reply: oneshot::Sender<ShellExecContextSnapshot>,
    },
    GetTitleGenerationContext {
        reply: oneshot::Sender<TitleGenerationContext>,
    },
    GetPendingQueueSnapshot {
        reply: oneshot::Sender<PendingQueueSnapshot>,
    },
    PopQueuedTurnInput {
        require_idle_session: bool,
        reply: oneshot::Sender<Option<QueuedTurnInputData>>,
    },
    EnqueuePendingTurnInput {
        item: PendingInputItem,
    },
    GetActiveTurnId {
        reply: oneshot::Sender<Option<TurnId>>,
    },
    GetRecord {
        reply: oneshot::Sender<Option<devo_core::SessionRecord>>,
    },
    PreparePersistItem {
        turn_id: TurnId,
        reply: oneshot::Sender<PersistItemPrep>,
    },
    TakeShutdownDeferredSnapshot {
        reply: oneshot::Sender<ShutdownDeferredSnapshot>,
    },
    AllocateItemSeq {
        reply: oneshot::Sender<u64>,
    },
    AppendPersistedItem {
        item: PersistedTurnItem,
    },
    AppendHistoryItem {
        item: SessionHistoryItem,
    },
    TakeDeferredItems {
        reply: oneshot::Sender<DeferredItems>,
    },
    TouchLastActivity,
    ApplyApprovalScope {
        scope: ApprovalScopeValue,
        pending: PendingApproval,
    },
    UpdateSummary {
        summary: SessionMetadata,
    },
    SetFirstUserInputIfUnset {
        text: String,
        reply: oneshot::Sender<Option<String>>,
    },
    UpdateTitle {
        title: String,
        title_state: SessionTitleState,
        reply: oneshot::Sender<Option<SessionMetadata>>,
    },
    BeginActiveTurn {
        turn: TurnMetadata,
        turn_config: TurnConfig,
    },
    ClearActiveTurnIfMatches {
        turn_id: TurnId,
        reply: oneshot::Sender<bool>,
    },
    SetSessionIdle {
        latest_turn: Option<TurnMetadata>,
    },
    ActivateQueuedTurn {
        turn: TurnMetadata,
        turn_config: TurnConfig,
    },
    UpdateCorePermissionMode {
        permission_mode: devo_safety::PermissionMode,
    },
    SetActiveGoal {
        goal: Option<ThreadGoal>,
    },
    #[cfg_attr(not(test), allow(dead_code))]
    UpdateRecordRolloutPath {
        rollout_path: std::path::PathBuf,
    },
    ApplyParentUsageSnapshot {
        snapshot: ParentUsageSnapshot,
    },
    InterruptActiveTurn {
        reply: oneshot::Sender<Option<TurnMetadata>>,
    },
    ExportRuntimeSession {
        reply: oneshot::Sender<crate::execution::RuntimeSession>,
    },
    UpdateSessionWorkspace {
        cwd: std::path::PathBuf,
        runtime_context: Arc<crate::session_context::SessionRuntimeContext>,
    },
    UpdateSessionMetadata {
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        collaboration_mode: Option<CollaborationMode>,
        reply: oneshot::Sender<SessionMetadata>,
    },
    ApplyPermissionProfile {
        profile: devo_safety::RuntimePermissionProfile,
        reply: oneshot::Sender<()>,
    },
    ApplyEffectiveContextWindow {
        limit: usize,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ApplySandboxProfile {
        profile: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    SetSessionTitleUserRename {
        title: String,
        reply: oneshot::Sender<SessionMetadata>,
    },
    SetToolRegistry {
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
        reply: oneshot::Sender<()>,
    },
    GetRuntimeContext {
        reply: oneshot::Sender<Arc<crate::session_context::SessionRuntimeContext>>,
    },
    GetResumeSnapshot {
        reply: oneshot::Sender<super::snapshots::SessionResumeSnapshot>,
    },
    TryBeginActiveTurn {
        turn: TurnMetadata,
        turn_config: TurnConfig,
        reply: oneshot::Sender<bool>,
    },
    ReplaceState {
        state: Box<SessionActorState>,
        reply: oneshot::Sender<()>,
    },
    PersistTurnLine {
        runtime: Arc<crate::runtime::ServerRuntime>,
        turn: TurnMetadata,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}
