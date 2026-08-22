use std::sync::Arc;

use tokio::sync::mpsc;

use crate::runtime::ServerRuntime;
use crate::runtime::session_actor::state::SessionActorState;
use crate::runtime::subagent_usage::UsageTotals;
use crate::runtime::turn_exec::{
    ExecuteTurnRequest, FinalizeTurnParams, QUERY_EVENT_CHANNEL_CAPACITY, TurnModelQueryParams,
    spawn_turn_event_stream,
};

/// Executes a turn inline on the session actor.
///
/// The actor does not poll its mailbox until this function returns. Code that
/// must remain responsive while a turn runs (for example queue operations,
/// steering, or future rollback preview) must not wait for an actor command.
/// It must use the runtime reservation fast path and its shared queues instead.
pub(super) async fn execute_turn_in_actor(
    state: &mut SessionActorState,
    runtime: Arc<ServerRuntime>,
    request: ExecuteTurnRequest,
) {
    let ExecuteTurnRequest {
        session_id,
        turn,
        turn_config,
        display_input,
        input,
        input_messages,
        collaboration_mode,
        input_mode,
    } = request;

    let spawn_snapshot = Arc::new(state.spawn_snapshot());
    runtime
        .register_turn_spawn_snapshot(session_id, turn.turn_id, Arc::clone(&spawn_snapshot))
        .await;

    {
        let mut stream = state.stream.lock().await;
        stream.turn_inline = Some(super::turn_inline::TurnInlineState::new(state, &turn));
    }
    runtime
        .register_active_stream(session_id, Arc::clone(&state.stream))
        .await;

    runtime
        .prepare_turn_execution_for_actor(
            state,
            &turn,
            &display_input,
            input_mode.emits_user_message(),
        )
        .await;

    let (event_tx, event_rx) = mpsc::channel(QUERY_EVENT_CHANNEL_CAPACITY);
    let event_tool_registry = runtime.tool_registry_for_actor_state(state);
    let usage_parent_session_id = state.parent_session_id();
    let usage_context_window = Some(turn_config.model.context_window as u64);
    // Only root sessions own a parent-turn usage ledger. Child turns publish
    // through `publish_subagent_turn_usage` into their parent's ledger; starting
    // a ledger keyed by the child session id is incorrect and can strand usage
    // updates on the wrong session.
    if usage_parent_session_id.is_none() {
        runtime
            .begin_parent_usage_turn_with_base(
                session_id,
                turn.turn_id,
                UsageTotals::from_session_summary(&state.summary),
                usage_context_window,
            )
            .await;
    }

    let stream = Arc::clone(&state.stream);
    let event_task = spawn_turn_event_stream(
        Arc::clone(&runtime),
        stream,
        session_id,
        turn.clone(),
        collaboration_mode,
        event_tool_registry,
        usage_parent_session_id,
        usage_context_window,
        event_rx,
    );

    let query_outcome = runtime
        .run_turn_model_query(TurnModelQueryParams {
            state,
            turn_id: turn.turn_id,
            turn_config: &turn_config,
            input: &input,
            input_messages: &input_messages,
            collaboration_mode,
            input_mode,
            usage_parent_session_id,
            event_tx,
        })
        .await;
    let event_summary = event_task.await.ok();

    let turn_id = turn.turn_id;
    runtime
        .finalize_executed_turn(FinalizeTurnParams {
            state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id,
        })
        .await;

    runtime.clear_turn_spawn_snapshot(session_id, turn_id).await;
    runtime.unregister_active_stream(session_id).await;
    let inline = {
        let mut stream = state.stream.lock().await;
        stream.turn_inline.take()
    };
    if let Some(inline) = inline {
        inline.merge_into(state);
    }
}
