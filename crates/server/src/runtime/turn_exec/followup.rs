use std::sync::Arc;

use chrono::Utc;
use devo_core::{SessionId, TurnId, TurnStatus};

use super::super::*;
use super::types::{ExecuteTurnRequest, QueuedTurnInput};

/// Whether a queued-input drain may pop while a turn is still active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum QueueDrainPolicy {
    /// Pop regardless of active-turn state (post-turn chain, interrupt
    /// recovery): the caller knows the turn is winding down.
    Always,
    /// Pop only when no turn is active, so a race-recovery kick can never
    /// double-admit a turn.
    OnlyWhenIdle,
}

impl ServerRuntime {
    /// Pop the first queued input and start a new turn in a background task.
    /// Used from the interrupt handler where the calling function must return
    /// its response immediately.
    pub(in crate::runtime) async fn spawn_next_turn_from_queue(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> bool {
        self.spawn_next_turn_from_queue_with_policy(session_id, QueueDrainPolicy::Always)
            .await
    }

    /// Starts a queued turn only when the session is idle. Gate-free busy
    /// enqueues (`handle_turn_start_with_queue_policy`) can race the
    /// post-turn drain: if the active turn ended between the pusher's
    /// snapshot and the enqueue and the followup chain already found an
    /// empty queue, this kick is what keeps the entry from stranding.
    pub(in crate::runtime) async fn drain_queue_if_idle(self: &Arc<Self>, session_id: SessionId) {
        let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
            return;
        };
        if reservation.active_turn.is_some()
            || reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned")
                .is_empty()
        {
            return;
        }
        self.spawn_next_turn_from_queue_with_policy(session_id, QueueDrainPolicy::OnlyWhenIdle)
            .await;
    }

    /// After hydrate/resume, start the next queued turn when the session is idle.
    pub(in crate::runtime) async fn resume_pending_queue_if_idle(
        self: &Arc<Self>,
        session_id: SessionId,
    ) {
        self.drain_queue_if_idle(session_id).await;
    }

    async fn spawn_next_turn_from_queue_with_policy(
        self: &Arc<Self>,
        session_id: SessionId,
        policy: QueueDrainPolicy,
    ) -> bool {
        let Some(session_handle) = self.session(session_id).await else {
            return false;
        };
        let state_change_guard = session_handle.lock_state_change().await;
        let Some(queued) = self
            .pop_next_queued_turn_input(
                session_id,
                /*require_idle_session*/ matches!(policy, QueueDrainPolicy::OnlyWhenIdle),
            )
            .await
        else {
            return false;
        };
        let Some((turn, turn_config)) = self.prepare_queued_turn_start(session_id, &queued).await
        else {
            return false;
        };
        if let Some((parent_session_id, parent_turn_id)) = queued.subagent_usage_owner {
            self.register_subagent_usage_owner(parent_session_id, session_id, parent_turn_id)
                .await;
        }
        self.activate_queued_turn(session_id, &turn, &turn_config)
            .await;
        drop(state_change_guard);
        self.broadcast_event(crate::ServerEvent::TurnStarted(TurnEventPayload {
            session_id,
            turn: turn.clone(),
        }))
        .await;
        // `drained` carries queueItemId + startedTurnId, emitted in the same
        // session-actor operation as the matching turn/started so handles
        // bind atomically (01 §4.3).
        self.broadcast_queue_updated(
            session_id,
            devo_protocol::native::queue::QueueChange::Drained,
            queued.queued_item_id,
            Some(devo_protocol::native::ids::TurnId::from_legacy_uuid(
                uuid::Uuid::from(turn.turn_id),
            )),
        )
        .await;
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn,
                    turn_config,
                    display_input: queued.display_input,
                    input: queued.input_text,
                    input_messages: queued.input_messages,
                    collaboration_mode: queued.collaboration_mode,
                    input_mode: TurnInputMode::VisibleUserMessage,
                })
                .await;
        });
        true
    }

    /// After a turn completes, chain directly into the next queued input when present.
    pub(crate) async fn chain_queued_followup_turn(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> bool {
        let Some(session_handle) = self.session(session_id).await else {
            return false;
        };
        let state_change_guard = session_handle.lock_state_change().await;
        let Some(queued) = self
            .pop_next_queued_turn_input(session_id, /*require_idle_session*/ false)
            .await
        else {
            return false;
        };
        let Some((turn, turn_config)) = self.prepare_queued_turn_start(session_id, &queued).await
        else {
            return false;
        };
        if let Some((parent_session_id, parent_turn_id)) = queued.subagent_usage_owner {
            self.register_subagent_usage_owner(parent_session_id, session_id, parent_turn_id)
                .await;
        }
        self.activate_queued_turn(session_id, &turn, &turn_config)
            .await;
        drop(state_change_guard);
        self.broadcast_event(crate::ServerEvent::TurnStarted(TurnEventPayload {
            session_id,
            turn: turn.clone(),
        }))
        .await;
        self.broadcast_queue_updated(
            session_id,
            devo_protocol::native::queue::QueueChange::Drained,
            queued.queued_item_id.clone(),
            Some(devo_protocol::native::ids::TurnId::from_legacy_uuid(
                uuid::Uuid::from(turn.turn_id),
            )),
        )
        .await;
        Box::pin(Arc::clone(self).execute_turn(ExecuteTurnRequest {
            session_id,
            turn,
            turn_config,
            display_input: queued.display_input,
            input: queued.input_text,
            input_messages: queued.input_messages,
            collaboration_mode: queued.collaboration_mode,
            input_mode: TurnInputMode::VisibleUserMessage,
        }))
        .await;
        true
    }

    async fn pop_next_queued_turn_input(
        self: &Arc<Self>,
        session_id: SessionId,
        require_idle_session: bool,
    ) -> Option<QueuedTurnInput> {
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        let popped = session_handle
            .pop_queued_turn_input(require_idle_session)
            .await
            .flatten()?;
        if let Err(error) = self.deps.db.remove_pending_by_id(
            &session_id,
            crate::db::QueueType::Turn,
            &popped.queued_input_id,
        ) {
            tracing::warn!(
                session_id = %session_id,
                queued_input_id = %popped.queued_input_id,
                error = %error,
                "failed to remove dequeued turn input from database"
            );
        }
        Some(QueuedTurnInput {
            queued_item_id: devo_protocol::native::ids::QueueItemId::from_legacy_uuid(
                uuid::Uuid::from(popped.queued_input_id),
            ),
            display_input: popped.display_input,
            input_text: popped.input_text,
            input_messages: popped.input_messages,
            collaboration_mode: popped.collaboration_mode,
            model_selection: popped.model_selection,
            subagent_usage_owner: popped.subagent_usage_owner,
        })
    }

    async fn prepare_queued_turn_start(
        self: &Arc<Self>,
        session_id: SessionId,
        queued: &QueuedTurnInput,
    ) -> Option<(crate::TurnMetadata, devo_core::TurnConfig)> {
        let session_handle = self.sessions.lock().await.get(&session_id).cloned()?;
        let reservation = session_handle.turn_reservation_snapshot().await?;
        let model_override = queued
            .model_selection
            .as_deref()
            .or_else(|| session_model_selection(&reservation.summary));
        let turn_config = reservation.runtime_context.resolve_turn_config(
            model_override,
            reservation.summary.reasoning_effort_selection.clone(),
        );
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let sequence = reservation
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.sequence + 1);
        let now = Utc::now();
        let turn = crate::TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence,
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Regular,
            model: turn_config.model.slug.clone(),
            model_binding_id: turn_config.model_binding_id.clone(),
            reasoning_effort_selection: turn_config.reasoning_effort_selection.clone(),
            reasoning_effort: resolved_request.effective_reasoning_effort,
            request_model,
            request_thinking: resolved_request.request_thinking.clone(),
            started_at: now,
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        Some((turn, turn_config))
    }

    async fn activate_queued_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &crate::TurnMetadata,
        turn_config: &devo_core::TurnConfig,
    ) {
        if let Some(session_handle) = self.sessions.lock().await.get(&session_id).cloned() {
            session_handle
                .activate_queued_turn(turn.clone(), turn_config.clone())
                .await;
        }
    }
}
