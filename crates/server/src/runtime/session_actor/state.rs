use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use devo_core::SessionConfig;
use devo_core::SessionRecord;
use devo_core::SessionState;
use devo_core::TurnId;
use devo_core::TurnRecord;
use devo_core::tools::ToolRegistry;
use devo_protocol::SessionId;

use crate::execution::PersistedTurnItem;
use crate::runtime::RuntimeSession;
use crate::session::SessionHistoryItem;
use crate::session::SessionMetadata;
use crate::session_context::SessionRuntimeContext;
use crate::turn::TurnMetadata;

use super::turn_inline::TurnInlineState;

/// Immutable parent snapshot used by `spawn_agent` during an active parent turn.
#[derive(Clone)]
pub(crate) struct SpawnSnapshot {
    pub(crate) parent_summary: SessionMetadata,
    pub(crate) parent_config: SessionConfig,
    pub(crate) stable_items: Vec<PersistedTurnItem>,
    pub(crate) parent_latest_turn: Option<TurnMetadata>,
    pub(crate) parent_active_turn_id: Option<TurnId>,
    pub(crate) parent_tool_registry: Option<Arc<ToolRegistry>>,
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
    pub(crate) steer_input_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
}

/// Approval caches cloned at turn start for permission checks while the actor
/// is busy executing a turn.
#[derive(Clone, Default)]
pub(crate) struct ApprovalCacheSnapshot {
    pub(crate) session_approval_cache: crate::execution::ApprovalGrantCache,
    pub(crate) turn_approval_cache: crate::execution::ApprovalGrantCache,
}

#[derive(Clone, Default)]
pub(crate) struct DeferredItems {
    pub(crate) assistant: Option<(devo_core::ItemId, u64, String)>,
    pub(crate) reasoning: Option<(devo_core::ItemId, u64, String)>,
}

use tokio::sync::Mutex as TokioMutex;

/// Streaming-era mutable fields touched by the turn event bridge.
#[derive(Default)]
pub(crate) struct SessionStreamState {
    pub(crate) deferred_assistant: Option<(devo_core::ItemId, u64, String)>,
    pub(crate) deferred_reasoning: Option<(devo_core::ItemId, u64, String)>,
    pub(crate) turn_inline: Option<TurnInlineState>,
}

impl SessionStreamState {
    pub(crate) fn take_deferred_items(&mut self) -> DeferredItems {
        DeferredItems {
            assistant: self.deferred_assistant.take(),
            reasoning: self.deferred_reasoning.take(),
        }
    }
}

/// Per-session state owned exclusively by a `SessionActor` task.
pub(crate) struct SessionActorState {
    pub(crate) runtime_context: Arc<SessionRuntimeContext>,
    pub(crate) record: Option<SessionRecord>,
    pub(crate) summary: SessionMetadata,
    pub(crate) config: SessionConfig,
    pub(crate) core: SessionState,
    pub(crate) stream: Arc<TokioMutex<SessionStreamState>>,
    pub(crate) active_turn: Option<TurnMetadata>,
    pub(crate) latest_turn: Option<TurnMetadata>,
    pub(crate) loaded_item_count: u64,
    pub(crate) history_items: Vec<SessionHistoryItem>,
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    pub(crate) latest_compaction_snapshot: Option<devo_core::CompactionSnapshotLine>,
    pub(crate) turn_records_by_id: HashMap<TurnId, TurnRecord>,
    pub(crate) pending_turn_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
    pub(crate) steer_input_queue: Arc<StdMutex<VecDeque<devo_protocol::PendingInputItem>>>,
    pub(crate) agent_tool_policy: devo_protocol::AgentToolPolicy,
    pub(crate) max_turns: Option<u32>,
    pub(crate) next_item_seq: u64,
    pub(crate) first_user_input: Option<String>,
    pub(crate) tool_registry: Option<Arc<ToolRegistry>>,
    /// Session-scoped ledger of files read/written by tools (used by `edit`).
    pub(crate) file_read_ledger: Arc<devo_core::tools::FileReadLedger>,
    pub(crate) session_approval_cache: crate::execution::ApprovalGrantCache,
    pub(crate) turn_approval_cache: crate::execution::ApprovalGrantCache,
    pub(crate) session_context_recorded: bool,
}

impl SessionActorState {
    pub(crate) fn session_id(&self) -> SessionId {
        self.summary.session_id
    }

    pub(crate) fn parent_session_id(&self) -> Option<SessionId> {
        self.summary.parent_session_id
    }

    pub(crate) fn approval_cache_snapshot(&self) -> ApprovalCacheSnapshot {
        ApprovalCacheSnapshot {
            session_approval_cache: self.session_approval_cache.clone(),
            turn_approval_cache: self.turn_approval_cache.clone(),
        }
    }

    pub(crate) fn spawn_snapshot(&self) -> SpawnSnapshot {
        let fork_turns_all = true;
        let stable_items = if fork_turns_all {
            let active_turn_id = self.active_turn.as_ref().map(|turn| turn.turn_id);
            self.persisted_turn_items
                .iter()
                .filter(|item| active_turn_id.is_none_or(|turn_id| item.turn_id != turn_id))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        SpawnSnapshot {
            parent_summary: self.summary.clone(),
            parent_config: self.config.clone(),
            stable_items,
            parent_latest_turn: self.latest_turn.clone(),
            parent_active_turn_id: self
                .active_turn
                .as_ref()
                .map(|turn| turn.turn_id)
                .or_else(|| self.latest_turn.as_ref().map(|turn| turn.turn_id)),
            parent_tool_registry: self.tool_registry.clone(),
            runtime_context: Arc::clone(&self.runtime_context),
            pending_turn_queue: Arc::clone(&self.pending_turn_queue),
            steer_input_queue: Arc::clone(&self.steer_input_queue),
        }
    }

    pub(crate) fn from_runtime_session(session: RuntimeSession) -> Self {
        let core = Arc::try_unwrap(session.core_session)
            .unwrap_or_else(|_| {
                panic!("session core_session should have a single owner when starting actor")
            })
            .into_inner();
        Self {
            runtime_context: session.runtime_context,
            record: session.record,
            summary: session.summary,
            config: session.config,
            core,
            stream: Arc::new(TokioMutex::new(SessionStreamState {
                deferred_assistant: session.deferred_assistant,
                deferred_reasoning: session.deferred_reasoning,
                turn_inline: None,
            })),
            active_turn: session.active_turn,
            latest_turn: session.latest_turn,
            loaded_item_count: session.loaded_item_count,
            history_items: session.history_items,
            persisted_turn_items: session.persisted_turn_items,
            latest_compaction_snapshot: session.latest_compaction_snapshot,
            turn_records_by_id: session.turn_records_by_id,
            pending_turn_queue: session.pending_turn_queue,
            steer_input_queue: session.steer_input_queue,
            agent_tool_policy: session.agent_tool_policy,
            max_turns: session.max_turns,
            next_item_seq: session.next_item_seq,
            first_user_input: session.first_user_input,
            tool_registry: session.tool_registry,
            file_read_ledger: session.file_read_ledger,
            session_approval_cache: session.session_approval_cache,
            turn_approval_cache: session.turn_approval_cache,
            session_context_recorded: session.session_context_recorded,
        }
    }

    pub(crate) fn to_runtime_session_from_stream(
        &self,
        stream: &SessionStreamState,
    ) -> RuntimeSession {
        RuntimeSession {
            runtime_context: Arc::clone(&self.runtime_context),
            record: self.record.clone(),
            summary: self.summary.clone(),
            config: self.config.clone(),
            core_session: Arc::new(tokio::sync::Mutex::new(self.core.snapshot_for_export())),
            active_turn: self.active_turn.clone(),
            latest_turn: self.latest_turn.clone(),
            loaded_item_count: self.loaded_item_count,
            history_items: self.history_items.clone(),
            persisted_turn_items: self.persisted_turn_items.clone(),
            latest_compaction_snapshot: self.latest_compaction_snapshot.clone(),
            turn_records_by_id: self.turn_records_by_id.clone(),
            pending_turn_queue: Arc::clone(&self.pending_turn_queue),
            steer_input_queue: Arc::clone(&self.steer_input_queue),
            agent_tool_policy: self.agent_tool_policy,
            max_turns: self.max_turns,
            deferred_assistant: stream.deferred_assistant.clone(),
            deferred_reasoning: stream.deferred_reasoning.clone(),
            next_item_seq: self.next_item_seq,
            first_user_input: self.first_user_input.clone(),
            tool_registry: self.tool_registry.clone(),
            file_read_ledger: Arc::clone(&self.file_read_ledger),
            session_approval_cache: self.session_approval_cache.clone(),
            turn_approval_cache: self.turn_approval_cache.clone(),
            session_context_recorded: self.session_context_recorded,
        }
    }
}

impl From<RuntimeSession> for SessionActorState {
    fn from(session: RuntimeSession) -> Self {
        Self::from_runtime_session(session)
    }
}
