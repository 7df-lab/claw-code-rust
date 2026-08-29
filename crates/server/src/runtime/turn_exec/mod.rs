mod context_compaction;
mod event_stream;
mod failure;
mod finalize;
mod followup;
mod item_stream;
mod query;
mod tool_display;
mod tool_results;
mod trace;
mod types;

pub(crate) use context_compaction::{
    manual_compaction_completed_event, manual_compaction_started_event,
};
pub(crate) use event_stream::{QUERY_EVENT_CHANNEL_CAPACITY, spawn_turn_event_stream};
pub(crate) use finalize::FinalizeTurnParams;
pub(crate) use query::TurnModelQueryParams;
pub(crate) use types::ExecuteTurnRequest;

use std::sync::Arc;

use anyhow::Context;
use devo_core::SessionId;

use super::*;

/// Schedules queue drain / goal continuation after a turn merges.
///
/// Must stay a sync function so callers' async opaque types do not recursively
/// include this spawn's future (rustc Send-cycle with `execute_turn`).
pub(crate) fn spawn_post_turn_scheduling(
    runtime: Arc<ServerRuntime>,
    session_id: SessionId,
    should_auto_continue_goal: bool,
) {
    tokio::spawn(async move {
        runtime
            .maybe_schedule_final_title_generation(session_id, None)
            .await;
        if runtime.chain_queued_followup_turn(session_id).await {
            return;
        }
        if runtime.spawn_next_turn_from_queue(session_id).await {
            return;
        }
        if runtime.child_parent_and_path(session_id).await.is_some()
            && runtime.child_can_accept_next_turn(session_id).await
        {
            let _ = runtime.drain_child_mailbox_into_user_turns(session_id).await;
            return;
        }
        if should_auto_continue_goal {
            runtime
                .maybe_start_goal_continuation_turn(session_id)
                .await;
        }
    });
}

impl ServerRuntime {
    /// Execute one turn on a spawned working copy; the session actor stays free.
    pub(in crate::runtime) async fn execute_turn(self: Arc<Self>, request: ExecuteTurnRequest) {
        let Some(handle) = self.session(request.session_id).await else {
            return;
        };
        handle.execute_turn(Arc::clone(&self), request).await;
    }

    pub(crate) async fn persist_turn_line_deduped(
        self: &Arc<Self>,
        session_id: devo_core::SessionId,
        turn: &crate::TurnMetadata,
    ) -> anyhow::Result<()> {
        let handle = self
            .session(session_id)
            .await
            .context("session not found")?;
        handle
            .persist_turn_line(Arc::clone(self), turn.clone())
            .await
    }

    pub(super) async fn prepare_turn_execution_for_actor(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        turn: &crate::TurnMetadata,
        display_input: &str,
        emits_user_message: bool,
    ) {
        self.capture_turn_workspace_baseline(
            state.session_id(),
            turn.turn_id,
            state.summary.cwd.clone(),
        )
        .await;
        state.turn_approval_cache = crate::execution::ApprovalGrantCache::default();
        if emits_user_message {
            self.emit_turn_item(
                state.session_id(),
                turn.turn_id,
                crate::ItemKind::UserMessage,
                devo_core::TurnItem::UserMessage(devo_core::TextItem {
                    text: display_input.to_string(),
                }),
                serde_json::json!({ "title": "You", "text": display_input }),
            )
            .await;
        }
    }

    pub(in crate::runtime) fn tool_registry_for_actor_state(
        &self,
        state: &SessionActorState,
    ) -> Arc<devo_core::tools::ToolRegistry> {
        state
            .tool_registry
            .clone()
            .unwrap_or_else(|| state.runtime_context.tool_registry())
    }
}

#[cfg(test)]
mod tests;
