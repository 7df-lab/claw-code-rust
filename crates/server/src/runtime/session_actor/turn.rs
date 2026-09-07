use std::sync::Arc;

use tokio::sync::mpsc;

use crate::runtime::ServerRuntime;
use crate::runtime::session_actor::TurnWorkingSet;
use crate::runtime::subagent_usage::UsageTotals;
use crate::runtime::turn_exec::{
    ExecuteTurnRequest, FinalizeTurnParams, QUERY_EVENT_CHANNEL_CAPACITY, TurnModelQueryParams,
    spawn_turn_event_stream,
};
use devo_core::TurnStatus;

/// Runs one turn on the caller's task using a checked-out [`TurnWorkingSet`].
///
/// Returns whether goal continuation should be considered after merge.
/// Post-turn scheduling is the caller's responsibility so this future does not
/// recursively type-check against queue/follow-up spawn paths.
pub(crate) async fn execute_turn_task(
    mut working: TurnWorkingSet,
    runtime: Arc<ServerRuntime>,
    request: ExecuteTurnRequest,
) -> bool {
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

    let spawn_snapshot = Arc::new(working.state.spawn_snapshot());
    runtime
        .register_turn_spawn_snapshot(session_id, turn.turn_id, Arc::clone(&spawn_snapshot))
        .await;

    runtime
        .register_active_stream(session_id, Arc::clone(&working.state.stream))
        .await;

    runtime
        .prepare_turn_execution_for_actor(
            &mut working.state,
            &turn,
            &display_input,
            input_mode.emits_user_message(),
        )
        .await;

    let (event_tx, event_rx) = mpsc::channel(QUERY_EVENT_CHANNEL_CAPACITY);
    let event_tool_registry = runtime.tool_registry_for_actor_state(&working.state);
    let usage_parent_session_id = working.state.parent_session_id();
    let global_compaction = runtime
        .deps
        .config_store
        .lock()
        .expect("app config store mutex should not be poisoned")
        .effective_config()
        .compaction_token_limit;
    let usage_context_window = Some(crate::runtime::context_occupancy::occupancy_window_tokens(
        working.state.core.config.effective_context_window_override,
        Some(&turn_config.model),
        global_compaction,
    ));
    if usage_parent_session_id.is_none() {
        runtime
            .begin_parent_usage_turn_with_base(
                session_id,
                turn.turn_id,
                UsageTotals::from_session_summary(&working.state.summary),
                usage_context_window,
            )
            .await;
    }

    let stream = Arc::clone(&working.state.stream);
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
            state: &mut working.state,
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
            state: &mut working.state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id,
        })
        .await;

    // Merge before clearing the runtime registry so admission (compact /
    // turn/start) cannot see a free registry while the actor still holds
    // `active_turn` from BeginActiveTurn.
    let inline = {
        let mut stream = working.state.stream.lock().await;
        stream.turn_inline.take()
    };
    if let Some(inline) = inline {
        inline.merge_into(&mut working.state);
    }

    let should_auto_continue_goal = working
        .state
        .latest_turn
        .as_ref()
        .is_some_and(|turn| matches!(turn.status, TurnStatus::Completed | TurnStatus::Failed));

    if let Some(handle) = runtime.session(session_id).await {
        handle.merge_turn(working).await;
    }

    runtime.clear_turn_spawn_snapshot(session_id, turn_id).await;
    runtime.unregister_active_stream(session_id).await;
    runtime
        .clear_active_turn_interrupt_handles(session_id)
        .await;
    runtime.clear_active_turn_runtime_handles(session_id).await;
    runtime.broadcast_recovery_state(session_id).await;

    should_auto_continue_goal
}
