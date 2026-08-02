use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use devo_core::SessionConfig;
use devo_core::SessionRecord;
use devo_core::TurnKind;
use devo_protocol::PendingInputItem;
use devo_protocol::SessionId;
use devo_safety::PermissionMode;
use devo_safety::RuntimePermissionProfile;

use crate::session::SessionMetadata;
use crate::session_context::SessionRuntimeContext;
use crate::turn::TurnMetadata;
use devo_core::tools::ToolRegistry;

/// Snapshot used when reserving or queueing a turn on a session actor.
#[derive(Clone)]
pub(crate) struct TurnReservationSnapshot {
    pub(crate) max_turns: Option<u32>,
    pub(crate) active_turn: Option<TurnMetadata>,
    pub(crate) latest_turn: Option<TurnMetadata>,
    pub(crate) ephemeral: bool,
    pub(crate) parent_session_id: Option<SessionId>,
    pub(crate) summary: SessionMetadata,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
    pub(crate) steer_input_queue: Arc<StdMutex<VecDeque<PendingInputItem>>>,
}

/// Hook runner inputs derived from session actor state.
#[derive(Clone)]
pub(crate) struct HookContextSnapshot {
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) record: Option<SessionRecord>,
    pub(crate) summary: SessionMetadata,
    pub(crate) config: SessionConfig,
}

/// Fields needed to persist a turn line to rollout storage.
#[derive(Clone)]
pub(crate) struct TurnPersistenceSnapshot {
    pub(crate) record: Option<SessionRecord>,
}

/// Permission and tool context for shell-command turns.
#[derive(Clone)]
pub(crate) struct ShellExecContextSnapshot {
    pub(crate) permission_mode: PermissionMode,
    pub(crate) permission_profile: RuntimePermissionProfile,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) file_read_ledger: Arc<devo_core::tools::FileReadLedger>,
    pub(crate) sandbox_profile: Option<String>,
}

/// Context for async title generation.
#[derive(Clone)]
pub(crate) struct TitleGenerationContext {
    pub(crate) model_selection: Option<String>,
    pub(crate) reasoning_effort_selection: Option<String>,
    pub(crate) title_state: devo_core::SessionTitleState,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
}

/// Pending turn queue broadcast snapshot.
#[derive(Clone, Default)]
pub(crate) struct PendingQueueSnapshot {
    pub(crate) pending_count: usize,
}

/// Fields returned by session/resume without locking the actor mailbox.
#[derive(Clone)]
pub(crate) struct SessionResumeSnapshot {
    pub(crate) summary: SessionMetadata,
    pub(crate) latest_turn: Option<TurnMetadata>,
    pub(crate) loaded_item_count: u64,
    pub(crate) history_items: Vec<crate::session::SessionHistoryItem>,
    pub(crate) pending_texts: Vec<String>,
}

/// Popped queued turn input for follow-up execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedTurnInputData {
    pub(crate) queued_input_id: devo_core::PendingInputId,
    pub(crate) display_input: String,
    pub(crate) input_text: String,
    pub(crate) input_messages: Vec<String>,
    pub(crate) collaboration_mode: devo_protocol::CollaborationMode,
    pub(crate) model_selection: Option<String>,
    pub(crate) subagent_usage_owner: Option<(SessionId, Option<devo_core::TurnId>)>,
}

/// Turn kind and durable record before persisting an item.
#[derive(Clone)]
pub(crate) struct PersistItemPrep {
    pub(crate) turn_kind: TurnKind,
    pub(crate) record: Option<SessionRecord>,
}

/// Deferred streaming items captured during graceful shutdown.
#[derive(Clone, Default)]
pub(crate) struct ShutdownDeferredSnapshot {
    pub(crate) deferred_assistant: Option<(devo_core::ItemId, u64, String)>,
    pub(crate) deferred_reasoning: Option<(devo_core::ItemId, u64, String)>,
    pub(crate) active_turn_id: Option<devo_core::TurnId>,
    pub(crate) record: Option<SessionRecord>,
}
