use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::CollaborationMode;
use devo_protocol::PendingInputItem;
use devo_protocol::SessionId;
use devo_protocol::ThreadGoal;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use devo_safety::PermissionMode;

use super::commands::SessionCommand;
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
use crate::session::SessionMetadata;
use crate::turn::TurnMetadata;
use devo_core::SessionRecord;
use devo_core::SessionTitleState;
use devo_core::TurnConfig;
use devo_core::TurnId;

const SESSION_MAILBOX_CAPACITY: usize = 64;

#[derive(Clone)]
pub(crate) struct SessionHandle {
    session_id: SessionId,
    tx: mpsc::Sender<SessionCommand>,
    max_turns: Option<u32>,
    state_change_gate: Arc<tokio::sync::Mutex<()>>,
}

impl SessionHandle {
    pub(crate) fn id(&self) -> SessionId {
        self.session_id
    }

    pub(crate) fn max_turns(&self) -> Option<u32> {
        self.max_turns
    }

    pub(crate) async fn enqueue_pending_turn_input(&self, item: PendingInputItem) {
        let _ = self
            .send(SessionCommand::EnqueuePendingTurnInput { item })
            .await;
    }

    pub(crate) fn spawn(
        session_id: SessionId,
        state: SessionActorState,
        runtime: Arc<crate::runtime::ServerRuntime>,
    ) -> Self {
        let max_turns = state.max_turns;
        let (tx, rx) = mpsc::channel(SESSION_MAILBOX_CAPACITY);
        let handle = Self {
            session_id,
            tx,
            max_turns,
            state_change_gate: Arc::new(tokio::sync::Mutex::new(())),
        };
        tokio::spawn(super::actor_loop::run_session_actor(state, rx, runtime));
        handle
    }

    async fn send(&self, command: SessionCommand) -> bool {
        self.tx.send(command).await.is_ok()
    }

    /// Serializes idle-session state changes that must not overlap turn
    /// admission, such as two-phase rollback commit and message edit.
    pub(crate) async fn lock_state_change(&self) -> tokio::sync::OwnedMutexGuard<()> {
        Arc::clone(&self.state_change_gate).lock_owned().await
    }

    /// Non-blocking enqueue. Used by turn event streams so they never park on a
    /// session actor that is itself waiting for that stream to finish.
    fn try_send(&self, command: SessionCommand) -> bool {
        self.tx.try_send(command).is_ok()
    }

    pub(crate) async fn execute_turn(
        &self,
        runtime: Arc<crate::runtime::ServerRuntime>,
        request: ExecuteTurnRequest,
    ) {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::ExecuteTurn {
                runtime,
                request,
                reply: reply_tx,
            })
            .await
        {
            return;
        }
        let _ = reply_rx.await;
    }

    pub(crate) async fn summary(&self) -> Option<SessionMetadata> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetSummary { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn spawn_snapshot(&self) -> Option<SpawnSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetSpawnSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn approval_cache_snapshot(&self) -> Option<ApprovalCacheSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetApprovalCacheSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn collaboration_mode(&self) -> Option<CollaborationMode> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetCollaborationMode { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn set_active_goal(&self, goal: Option<ThreadGoal>) {
        let _ = self.send(SessionCommand::SetActiveGoal { goal }).await;
    }

    pub(crate) fn try_set_active_goal(&self, goal: Option<ThreadGoal>) -> bool {
        self.try_send(SessionCommand::SetActiveGoal { goal })
    }

    pub(crate) async fn parent_session_id(&self) -> Option<Option<SessionId>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetParentSessionId { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn turn_reservation_snapshot(&self) -> Option<TurnReservationSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetTurnReservationSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn hook_context_snapshot(&self) -> Option<HookContextSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetHookContextSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn turn_persistence_snapshot(&self) -> Option<TurnPersistenceSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetTurnPersistenceSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn persist_turn_line(
        &self,
        runtime: Arc<crate::runtime::ServerRuntime>,
        turn: crate::TurnMetadata,
    ) -> anyhow::Result<()> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::PersistTurnLine {
                runtime,
                turn,
                reply: reply_tx,
            })
            .await
        {
            anyhow::bail!("session actor shut down");
        }
        reply_rx.await.context("persist turn reply dropped")?
    }

    pub(crate) async fn shell_exec_context(
        &self,
        cwd: std::path::PathBuf,
    ) -> Option<ShellExecContextSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetShellExecContext {
                cwd,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn title_generation_context(&self) -> Option<TitleGenerationContext> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetTitleGenerationContext { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn pending_queue_snapshot(&self) -> Option<PendingQueueSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetPendingQueueSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn pop_queued_turn_input(
        &self,
        require_idle_session: bool,
    ) -> Option<Option<QueuedTurnInputData>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::PopQueuedTurnInput {
                require_idle_session,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn active_turn_id(&self) -> Option<Option<TurnId>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetActiveTurnId { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn record(&self) -> Option<Option<SessionRecord>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetRecord { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn prepare_persist_item(&self, turn_id: TurnId) -> Option<PersistItemPrep> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::PreparePersistItem {
                turn_id,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn take_shutdown_deferred_snapshot(&self) -> Option<ShutdownDeferredSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::TakeShutdownDeferredSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn allocate_item_seq(&self) -> Option<u64> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::AllocateItemSeq { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn append_persisted_item(&self, item: PersistedTurnItem) {
        let _ = self
            .send(SessionCommand::AppendPersistedItem { item })
            .await;
    }

    pub(crate) async fn append_history_item(&self, item: crate::session::SessionHistoryItem) {
        let _ = self.send(SessionCommand::AppendHistoryItem { item }).await;
    }

    pub(crate) async fn take_deferred_items(&self) -> DeferredItems {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::TakeDeferredItems { reply: reply_tx })
            .await
        {
            return DeferredItems::default();
        }
        reply_rx.await.unwrap_or_default()
    }

    #[allow(dead_code)]
    pub(crate) async fn touch_last_activity(&self) {
        let _ = self.send(SessionCommand::TouchLastActivity).await;
    }

    pub(crate) async fn apply_approval_scope(
        &self,
        scope: ApprovalScopeValue,
        pending: PendingApproval,
    ) {
        let _ = self
            .send(SessionCommand::ApplyApprovalScope { scope, pending })
            .await;
    }

    pub(crate) async fn replace_state(&self, state: SessionActorState) {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .send(SessionCommand::ReplaceState {
                state: Box::new(state),
                reply: reply_tx,
            })
            .await
        {
            let _ = reply_rx.await;
        }
    }

    pub(crate) async fn update_summary(&self, summary: SessionMetadata) {
        let _ = self.send(SessionCommand::UpdateSummary { summary }).await;
    }

    pub(crate) async fn set_first_user_input_if_unset(
        &self,
        text: String,
    ) -> Option<Option<String>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::SetFirstUserInputIfUnset {
                text,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn update_title(
        &self,
        title: String,
        title_state: SessionTitleState,
    ) -> Option<Option<SessionMetadata>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::UpdateTitle {
                title,
                title_state,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn begin_active_turn(&self, turn: TurnMetadata, turn_config: TurnConfig) {
        let _ = self
            .send(SessionCommand::BeginActiveTurn { turn, turn_config })
            .await;
    }

    pub(crate) async fn clear_active_turn_if_matches(&self, turn_id: TurnId) -> Option<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::ClearActiveTurnIfMatches {
                turn_id,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn set_session_idle(&self, latest_turn: Option<TurnMetadata>) {
        let _ = self
            .send(SessionCommand::SetSessionIdle { latest_turn })
            .await;
    }

    pub(crate) async fn activate_queued_turn(&self, turn: TurnMetadata, turn_config: TurnConfig) {
        let _ = self
            .send(SessionCommand::ActivateQueuedTurn { turn, turn_config })
            .await;
    }

    pub(crate) async fn update_core_permission_mode(&self, permission_mode: PermissionMode) {
        let _ = self
            .send(SessionCommand::UpdateCorePermissionMode { permission_mode })
            .await;
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn update_record_rollout_path(&self, rollout_path: PathBuf) {
        let _ = self
            .send(SessionCommand::UpdateRecordRolloutPath { rollout_path })
            .await;
    }

    #[allow(dead_code)]
    pub(crate) async fn apply_parent_usage_snapshot(&self, snapshot: ParentUsageSnapshot) {
        let _ = self
            .send(SessionCommand::ApplyParentUsageSnapshot { snapshot })
            .await;
    }

    /// Best-effort usage apply for callers that must not block on the mailbox
    /// (child/parent turn event streams).
    pub(crate) fn try_apply_parent_usage_snapshot(&self, snapshot: ParentUsageSnapshot) -> bool {
        self.try_send(SessionCommand::ApplyParentUsageSnapshot { snapshot })
    }

    pub(crate) fn try_touch_last_activity(&self) -> bool {
        self.try_send(SessionCommand::TouchLastActivity)
    }

    pub(crate) async fn interrupt_active_turn(&self) -> Option<Option<TurnMetadata>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::InterruptActiveTurn { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn export_runtime_session(&self) -> Option<crate::execution::RuntimeSession> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::ExportRuntimeSession { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn update_session_workspace(
        &self,
        cwd: PathBuf,
        runtime_context: Arc<crate::session_context::SessionRuntimeContext>,
    ) {
        let _ = self
            .send(SessionCommand::UpdateSessionWorkspace {
                cwd,
                runtime_context,
            })
            .await;
    }

    pub(crate) async fn update_session_metadata(
        &self,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        collaboration_mode: Option<devo_protocol::CollaborationMode>,
    ) -> Option<SessionMetadata> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::UpdateSessionMetadata {
                model,
                model_binding_id,
                reasoning_effort_selection,
                collaboration_mode,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn apply_permission_profile(
        &self,
        profile: devo_safety::RuntimePermissionProfile,
    ) -> bool {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::ApplyPermissionProfile {
                profile,
                reply: reply_tx,
            })
            .await
        {
            return false;
        }
        reply_rx.await.is_ok()
    }

    /// Best-effort permission-profile notification for the persist-first
    /// settings write path (L2-DES-CONV-002 Phase 2): the change is already
    /// durable, so the actor must not be waited on (it may be running a turn).
    /// Mailbox FIFO still guarantees the actor applies it before the next
    /// `ExecuteTurn`, so the next turn always sees the new profile.
    pub(crate) fn notify_permission_profile(&self, profile: devo_safety::RuntimePermissionProfile) {
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = self.try_send(SessionCommand::ApplyPermissionProfile {
            profile,
            reply: reply_tx,
        });
    }

    /// Best-effort sandbox-profile notification; same ordering argument as
    /// [`Self::notify_permission_profile`].
    pub(crate) fn notify_sandbox_profile(&self, profile: String) {
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = self.try_send(SessionCommand::ApplySandboxProfile {
            profile,
            reply: reply_tx,
        });
    }

    /// Best-effort effective-context-window notification; same ordering
    /// argument as [`Self::notify_permission_profile`].
    pub(crate) fn notify_effective_context_window(&self, limit: usize) {
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = self.try_send(SessionCommand::ApplyEffectiveContextWindow {
            limit,
            reply: reply_tx,
        });
    }

    /// Best-effort metadata notification (model/effort/collaboration mode);
    /// same ordering argument as [`Self::notify_permission_profile`].
    pub(crate) fn notify_session_metadata(
        &self,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        collaboration_mode: Option<devo_protocol::CollaborationMode>,
    ) {
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = self.try_send(SessionCommand::UpdateSessionMetadata {
            model,
            model_binding_id,
            reasoning_effort_selection,
            collaboration_mode,
            reply: reply_tx,
        });
    }

    /// Applies a new sandbox profile to the session. Returns `None` when the
    /// session actor is gone, `Some(Err(..))` when the profile name is invalid
    /// (state is left unchanged), and `Some(Ok(name))` with the canonical
    /// profile name on success.
    pub(crate) async fn apply_sandbox_profile(
        &self,
        profile: String,
    ) -> Option<Result<String, String>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::ApplySandboxProfile {
                profile,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn set_session_title_user_rename(
        &self,
        title: String,
    ) -> Option<SessionMetadata> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::SetSessionTitleUserRename {
                title,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn set_tool_registry(
        &self,
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
    ) -> bool {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::SetToolRegistry {
                tool_registry,
                reply: reply_tx,
            })
            .await
        {
            return false;
        }
        reply_rx.await.is_ok()
    }

    pub(crate) async fn runtime_context(
        &self,
    ) -> Option<Arc<crate::session_context::SessionRuntimeContext>> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetRuntimeContext { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn resume_snapshot(&self) -> Option<super::snapshots::SessionResumeSnapshot> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::GetResumeSnapshot { reply: reply_tx })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn try_begin_active_turn(
        &self,
        turn: TurnMetadata,
        turn_config: TurnConfig,
    ) -> Option<bool> {
        let (reply_tx, reply_rx) = oneshot::channel();
        if !self
            .send(SessionCommand::TryBeginActiveTurn {
                turn,
                turn_config,
                reply: reply_tx,
            })
            .await
        {
            return None;
        }
        reply_rx.await.ok()
    }

    pub(crate) async fn shutdown(&self) {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .send(SessionCommand::Shutdown { reply: reply_tx })
            .await
        {
            let _ = reply_rx.await;
        }
    }
}
