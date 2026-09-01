use std::sync::Arc;

use devo_core::TurnId;
use devo_protocol::SessionId;

use super::state::SpawnSnapshot;
use crate::runtime::ServerRuntime;

impl ServerRuntime {
    pub(crate) fn runtime_arc(&self) -> Arc<ServerRuntime> {
        self.self_weak
            .upgrade()
            .expect("server runtime weak reference should remain alive")
    }

    pub(crate) async fn session(&self, session_id: SessionId) -> Option<super::SessionHandle> {
        self.sessions.lock().await.get(&session_id).cloned()
    }

    pub(crate) async fn insert_session_actor(
        &self,
        state: super::SessionActorState,
    ) -> super::SessionHandle {
        let runtime = self.runtime_arc();
        let session_id = state.session_id();
        if let Some(existing) = self.sessions.lock().await.get(&session_id).cloned() {
            return existing;
        }

        let handle = super::SessionHandle::spawn(session_id, state, runtime);
        let mut sessions = self.sessions.lock().await;
        if let Some(existing) = sessions.get(&session_id).cloned() {
            drop(sessions);
            handle.shutdown().await;
            return existing;
        }
        sessions.insert(session_id, handle.clone());
        drop(sessions);
        let runtime = self.runtime_arc();
        tokio::spawn(async move {
            runtime.rearm_title_polish_if_needed(session_id).await;
        });
        handle
    }

    pub(crate) async fn remove_session_actor(
        &self,
        session_id: SessionId,
    ) -> Option<super::SessionHandle> {
        let handle = self.sessions.lock().await.remove(&session_id)?;
        // Drop in-memory waiters without rewriting Waiting items to Interrupted.
        // App restart must still be able to restore unanswered questions.
        self.session_interactive
            .drain_pending_user_inputs_for_session(session_id)
            .await;
        self.session_interactive.clear_session(session_id).await;
        self.active_turns.remove_session(session_id).await;
        Some(handle)
    }

    pub(crate) async fn list_session_handles(&self) -> Vec<super::SessionHandle> {
        self.sessions.lock().await.values().cloned().collect()
    }

    #[allow(dead_code)]
    pub(crate) async fn list_session_summaries_from_actors(
        &self,
    ) -> Vec<crate::session::SessionMetadata> {
        let handles = self.list_session_handles().await;
        let mut summaries = Vec::with_capacity(handles.len());
        for handle in handles {
            if let Some(summary) = handle.summary().await {
                summaries.push(summary);
            }
        }
        summaries.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        summaries
    }

    /// Reads turn reservation state, preferring runtime caches while a turn
    /// task holds the working copy (shared queue Arcs stay usable without a
    /// mailbox hop). Callers may mutate `pending_turn_queue` and
    /// `steer_input_queue` through the returned shared mutexes.
    ///
    /// Stream presence **or** runtime turn metadata means a turn is admitted.
    /// Finalization clears `runtime_active_turn_id` before `MergeTurn`
    /// returns; falling through to the mailbox in that window is safe because
    /// the actor no longer runs unbounded turn I/O.
    pub(crate) async fn session_turn_reservation_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<super::snapshots::TurnReservationSnapshot> {
        let runtime_turn = self.active_turns.active_turn_metadata(session_id).await;
        let stream_busy = self.active_stream_state(session_id).await.is_some();
        if stream_busy || runtime_turn.is_some() {
            let handle = self.session(session_id).await?;
            let Some(spawn) = self.active_spawn_snapshot_for_session(session_id).await else {
                // Turn is admitted but the spawn snapshot is not available yet.
                // Prefer None over a stale mailbox read of pre-admission state.
                return None;
            };
            return Some(super::snapshots::TurnReservationSnapshot {
                max_turns: handle.max_turns(),
                active_turn: runtime_turn,
                latest_turn: spawn.parent_latest_turn,
                ephemeral: spawn.parent_summary.ephemeral,
                parent_session_id: spawn.parent_summary.parent_session_id,
                summary: spawn.parent_summary,
                runtime_context: spawn.runtime_context,
                pending_turn_queue: spawn.pending_turn_queue,
                steer_input_queue: spawn.steer_input_queue,
            });
        }
        let handle = self.session(session_id).await?;
        handle.turn_reservation_snapshot().await
    }

    pub(crate) async fn register_turn_spawn_snapshot(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        snapshot: Arc<SpawnSnapshot>,
    ) {
        self.active_turns
            .register_spawn_snapshot(session_id, turn_id, snapshot)
            .await;
    }

    pub(crate) async fn clear_turn_spawn_snapshot(&self, session_id: SessionId, turn_id: TurnId) {
        self.active_turns
            .clear_spawn_snapshot(session_id, turn_id)
            .await;
    }

    /// Snapshot registered at turn start for zero-hop control-plane access.
    pub(crate) async fn active_spawn_snapshot_for_session(
        &self,
        session_id: SessionId,
    ) -> Option<SpawnSnapshot> {
        self.active_turns
            .spawn_snapshot_for_session(session_id)
            .await
    }

    pub(crate) async fn register_active_stream(
        &self,
        session_id: SessionId,
        stream: Arc<tokio::sync::Mutex<super::state::SessionStreamState>>,
    ) {
        self.active_turns.register_stream(session_id, stream).await;
    }

    pub(crate) async fn unregister_active_stream(&self, session_id: SessionId) {
        self.active_turns.unregister_stream(session_id).await;
    }

    pub(crate) async fn active_stream_state(
        &self,
        session_id: SessionId,
    ) -> Option<Arc<tokio::sync::Mutex<super::state::SessionStreamState>>> {
        self.active_turns.stream_state(session_id).await
    }

    pub(crate) async fn session_record_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<devo_core::SessionRecord> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return inline.record.clone();
            }
        }
        let handle = self.session(session_id).await?;
        handle.record().await.flatten()
    }

    pub(crate) async fn session_summary_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<crate::session::SessionMetadata> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Some(inline.summary.clone());
            }
        }
        let handle = self.session(session_id).await?;
        handle.summary().await
    }

    /// Reads the session's collaboration mode, preferring the in-flight turn's
    /// inline snapshot over a mailbox round-trip.
    ///
    /// Callers invoked from within `session_id`'s own actor turn (e.g. while
    /// finalizing that turn) must go through this path: the actor's mailbox
    /// is not polled again until the turn finishes, so asking its own
    /// `SessionHandle` for a reply here would deadlock.
    pub(crate) async fn session_collaboration_mode(
        &self,
        session_id: SessionId,
    ) -> Option<devo_protocol::CollaborationMode> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Some(inline.collaboration_mode);
            }
        }
        let handle = self.session(session_id).await?;
        handle.collaboration_mode().await
    }

    /// Reads a session's parent id, preferring the in-flight turn inline snapshot
    /// and agent-registry hierarchy over a mailbox round-trip.
    #[allow(dead_code)]
    pub(crate) async fn session_parent_id_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<Option<SessionId>> {
        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                return Some(inline.summary.parent_session_id);
            }
        }
        {
            let registries = self.agent_registries.lock().await;
            for registry in registries.values() {
                if let Some(parent_id) = registry.child_to_parent.get(&session_id).copied() {
                    return Some(Some(parent_id));
                }
            }
        }
        let handle = self.session(session_id).await?;
        handle.parent_session_id().await
    }
}
