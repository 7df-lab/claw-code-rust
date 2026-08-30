//! Turn working copy checked out of the session actor for unbounded I/O.
//!
//! The session actor remains free to drain short mailbox commands while the
//! turn task owns this copy. Durable conversation state returns only through
//! [`SessionActorState::merge_turn_working_set`].

use super::state::SessionActorState;
use crate::turn::TurnMetadata;

/// Turn-owned session state for one in-flight execution.
///
/// Shares `stream`, queue Arcs, and `file_read_ledger` with the actor so the
/// control plane and item stream stay coherent. Conversation mutations happen
/// only on the embedded `state` until merge.
pub(crate) struct TurnWorkingSet {
    pub(crate) state: SessionActorState,
}

impl SessionActorState {
    /// Builds a working copy for turn execution without removing actor state.
    ///
    /// Installs `TurnInlineState` on the shared stream. Queue Arcs are shared so
    /// pending/steer RPCs remain mailbox-free.
    pub(crate) fn checkout_turn_working_set(&self, _turn: &TurnMetadata) -> TurnWorkingSet {
        TurnWorkingSet {
            state: SessionActorState {
                runtime_context: std::sync::Arc::clone(&self.runtime_context),
                record: self.record.clone(),
                summary: self.summary.clone(),
                config: self.config.clone(),
                core: self.core.snapshot_for_export(),
                stream: std::sync::Arc::clone(&self.stream),
                active_turn: self.active_turn.clone(),
                latest_turn: self.latest_turn.clone(),
                loaded_item_count: self.loaded_item_count,
                history_items: self.history_items.clone(),
                persisted_turn_items: self.persisted_turn_items.clone(),
                latest_compaction_snapshot: self.latest_compaction_snapshot.clone(),
                turn_records_by_id: self.turn_records_by_id.clone(),
                pending_turn_queue: std::sync::Arc::clone(&self.pending_turn_queue),
                steer_input_queue: std::sync::Arc::clone(&self.steer_input_queue),
                agent_tool_policy: self.agent_tool_policy,
                max_turns: self.max_turns,
                next_item_seq: self.next_item_seq,
                first_user_input: self.first_user_input.clone(),
                tool_registry: self.tool_registry.clone(),
                file_read_ledger: std::sync::Arc::clone(&self.file_read_ledger),
                session_approval_cache: self.session_approval_cache.clone(),
                turn_approval_cache: self.turn_approval_cache.clone(),
                session_context_recorded: self.session_context_recorded,
            },
        }
    }

    /// Installs turn-owned fields from a completed working copy.
    ///
    /// Session-plane config/settings that may have landed via persist-first
    /// `notify_*` during the turn are preserved on the actor.
    pub(crate) fn merge_turn_working_set(&mut self, working: TurnWorkingSet) {
        let working = working.state;

        let session_config = self.config.clone();
        let session_core_config = self.core.config.clone();
        let session_permission_preset = self.summary.permission_preset;
        let session_model = self.summary.model.clone();
        let session_model_binding_id = self.summary.model_binding_id.clone();
        let session_effort = self.summary.reasoning_effort_selection.clone();
        let session_effective_context_window = self.summary.effective_context_window;
        let session_title = self.summary.title.clone();
        let session_title_state = self.summary.title_state.clone();
        let session_record = self.record.clone();

        self.core = working.core;
        self.core.config = session_core_config;
        self.config = session_config;

        self.summary = working.summary;
        if session_permission_preset.is_some() {
            self.summary.permission_preset = session_permission_preset;
        }
        if session_model.is_some() {
            self.summary.model = session_model;
            self.summary.model_binding_id = session_model_binding_id;
        } else if session_model_binding_id.is_some() {
            self.summary.model_binding_id = session_model_binding_id;
        }
        if session_effort.is_some() {
            self.summary.reasoning_effort_selection = session_effort;
        }
        if session_effective_context_window.is_some() {
            self.summary.effective_context_window = session_effective_context_window;
        }
        if session_title.is_some() {
            self.summary.title = session_title;
            self.summary.title_state = session_title_state;
        }

        self.active_turn = working.active_turn;
        self.latest_turn = working.latest_turn;
        self.history_items = working.history_items;
        self.persisted_turn_items = working.persisted_turn_items;
        self.next_item_seq = working.next_item_seq;
        self.loaded_item_count = working.loaded_item_count;
        self.session_approval_cache = working.session_approval_cache;
        self.turn_approval_cache = working.turn_approval_cache;
        if working.latest_compaction_snapshot.is_some() {
            self.latest_compaction_snapshot = working.latest_compaction_snapshot;
        }
        self.session_context_recorded = working.session_context_recorded;
        self.turn_records_by_id = working.turn_records_by_id;
        self.first_user_input = working
            .first_user_input
            .or_else(|| self.first_user_input.clone());

        match (session_record, working.record) {
            (Some(actor_record), Some(mut turn_record)) => {
                turn_record.title = actor_record.title.or(turn_record.title);
                turn_record.title_state = actor_record.title_state;
                if actor_record.permission_preset.is_some() {
                    turn_record.permission_preset = actor_record.permission_preset;
                }
                if actor_record.model.is_some() {
                    turn_record.model = actor_record.model;
                    turn_record.model_binding_id = actor_record.model_binding_id;
                } else if actor_record.model_binding_id.is_some() {
                    turn_record.model_binding_id = actor_record.model_binding_id;
                }
                if actor_record.reasoning_effort_selection.is_some() {
                    turn_record.reasoning_effort_selection =
                        actor_record.reasoning_effort_selection;
                }
                turn_record.updated_at = turn_record.updated_at.max(actor_record.updated_at);
                self.record = Some(turn_record);
            }
            (actor_record, turn_record) => {
                self.record = turn_record.or(actor_record);
            }
        }
    }
}
