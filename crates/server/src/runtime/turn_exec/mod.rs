mod context_compaction;
mod event_stream;
mod failure;
mod finalize;
mod followup;
mod item_stream;
mod query;
mod shell;
mod tool_display;
mod tool_results;
mod trace;
mod types;

pub(crate) use event_stream::{QUERY_EVENT_CHANNEL_CAPACITY, spawn_turn_event_stream};
pub(crate) use finalize::FinalizeTurnParams;
pub(crate) use query::TurnModelQueryParams;
pub(crate) use types::ExecuteTurnRequest;

use std::sync::Arc;

use anyhow::Context;

use super::*;

impl ServerRuntime {
    /// Execute one turn end-to-end via the session actor.
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
