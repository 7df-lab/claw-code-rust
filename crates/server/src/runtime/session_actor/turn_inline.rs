use std::sync::Arc;

use devo_core::CompactionSnapshotLine;
use devo_core::SessionRecord;
use devo_core::TurnId;
use devo_core::TurnKind;
use devo_core::TurnUsage;
use devo_protocol::CollaborationMode;

use crate::execution::ApprovalGrantCache;
use crate::execution::PersistedTurnItem;
use crate::session::SessionHistoryItem;
use crate::session::SessionMetadata;
use crate::turn::TurnMetadata;

use super::SessionActorState;
use super::snapshots::HookContextSnapshot;

/// Mutable session fields updated during an active turn without mailbox round-trips.
///
/// Transient scratch state registered in `ActiveTurnRegistry` while the turn
/// task owns a [`super::TurnWorkingSet`]. Merges into durable actor state when
/// the turn completes via `MergeTurn`.
pub(crate) struct TurnInlineState {
    pub(crate) turn_id: TurnId,
    pub(crate) turn_kind: TurnKind,
    pub(crate) next_item_seq: u64,
    pub(crate) loaded_item_count: u64,
    pub(crate) persisted_turn_items: Vec<PersistedTurnItem>,
    pub(crate) history_items: Vec<SessionHistoryItem>,
    pub(crate) record: Option<SessionRecord>,
    pub(crate) session_approval_cache: ApprovalGrantCache,
    pub(crate) turn_approval_cache: ApprovalGrantCache,
    pub(crate) summary: SessionMetadata,
    pub(crate) active_turn_usage: Option<TurnUsage>,
    pub(crate) collaboration_mode: CollaborationMode,
    pub(crate) hook_context: HookContextSnapshot,
    pub(crate) latest_compaction_snapshot: Option<CompactionSnapshotLine>,
    /// Live sandbox profile shared with tool execution (L2-DES-CONV-002
    /// Phase 3): the settings override path mutates it mid-turn and the tool
    /// router reads it per spawn. `hook_context.config.sandbox_profile` is
    /// updated alongside so admission checks see the same value.
    pub(crate) sandbox_profile_live: Arc<std::sync::Mutex<Option<String>>>,
    /// Live model/effort/compaction-limit overrides shared with the core
    /// query loop (L2-DES-CONV-002 Phase 4).
    pub(crate) live_turn_settings: devo_core::SharedLiveTurnSettings,
    /// Last assembled provider request for this turn. Auto-review clones this
    /// prefix so the reviewer shares the main-turn prompt cache.
    pub(crate) last_model_request: devo_core::SharedLastModelRequest,
}

impl TurnInlineState {
    pub(crate) fn new(state: &SessionActorState, turn: &TurnMetadata) -> Self {
        Self {
            turn_id: turn.turn_id,
            turn_kind: turn.kind.clone(),
            next_item_seq: state.next_item_seq,
            loaded_item_count: state.loaded_item_count,
            persisted_turn_items: Vec::new(),
            history_items: Vec::new(),
            record: state.record.clone(),
            session_approval_cache: state.session_approval_cache.clone(),
            turn_approval_cache: state.turn_approval_cache.clone(),
            summary: state.summary.clone(),
            active_turn_usage: state
                .active_turn
                .as_ref()
                .and_then(|turn| turn.usage.clone()),
            collaboration_mode: state.core.collaboration_mode,
            hook_context: HookContextSnapshot {
                runtime_context: Arc::clone(&state.runtime_context),
                record: state.record.clone(),
                summary: state.summary.clone(),
                config: state.config.clone(),
            },
            latest_compaction_snapshot: None,
            sandbox_profile_live: Arc::new(std::sync::Mutex::new(
                state.config.sandbox_profile.clone(),
            )),
            live_turn_settings: Default::default(),
            last_model_request: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub(crate) fn allocate_item_seq(&mut self) -> u64 {
        let item_seq = self.next_item_seq;
        self.next_item_seq = self.next_item_seq.saturating_add(1);
        self.loaded_item_count = self.loaded_item_count.saturating_add(1);
        item_seq
    }

    pub(crate) fn merge_into(self, state: &mut SessionActorState) {
        state.next_item_seq = self.next_item_seq;
        state.loaded_item_count = self.loaded_item_count;
        state.persisted_turn_items.extend(self.persisted_turn_items);
        state.history_items.extend(self.history_items);
        state.session_approval_cache = self.session_approval_cache;
        state.turn_approval_cache = self.turn_approval_cache;
        state.summary.total_input_tokens = self.summary.total_input_tokens;
        state.summary.total_output_tokens = self.summary.total_output_tokens;
        state.summary.total_tokens = self.summary.total_tokens;
        state.summary.total_cache_creation_tokens = self.summary.total_cache_creation_tokens;
        state.summary.total_cache_read_tokens = self.summary.total_cache_read_tokens;
        state.summary.last_query_usage = self.summary.last_query_usage.clone();
        state.summary.last_query_total_tokens = self.summary.last_query_total_tokens;
        state.core.total_input_tokens = self.summary.total_input_tokens;
        state.core.total_output_tokens = self.summary.total_output_tokens;
        state.core.total_tokens = self.summary.total_tokens;
        state.core.total_cache_creation_tokens = self.summary.total_cache_creation_tokens;
        state.core.total_cache_read_tokens = self.summary.total_cache_read_tokens;
        if let Some(snapshot) = self.latest_compaction_snapshot {
            state.latest_compaction_snapshot = Some(snapshot);
        }
        if let Some(active_turn) = state.active_turn.as_mut()
            && active_turn.turn_id == self.turn_id
        {
            active_turn.usage = self.active_turn_usage;
        }
    }
}
