use std::collections::HashMap;
use std::sync::Arc;

use devo_core::SessionId;
use devo_core::TurnId;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::turn::TurnMetadata;

use super::session_actor::state::{SessionStreamState, SpawnSnapshot};

/// Runtime coordination for an in-flight turn (cancel, abort, spawn/stream mirrors).
///
/// Durable session fields remain on `SessionActorState`; turn I/O runs on a
/// spawned task with a `TurnWorkingSet` and merges back through `MergeTurn`.
pub(crate) struct ActiveTurnExecution {
    pub turn: Option<TurnMetadata>,
    pub cancel_token: Option<CancellationToken>,
    pub abort_handle: Option<tokio::task::AbortHandle>,
    pub connection_id: Option<u64>,
    pub spawn_snapshots: HashMap<TurnId, Arc<SpawnSnapshot>>,
    pub stream: Option<Arc<tokio::sync::Mutex<SessionStreamState>>>,
}

/// Unified registry replacing the scattered `active_turn_*`, `active_tasks`,
/// `active_stream_states`, and `active_spawn_snapshots` maps.
#[derive(Default)]
pub(crate) struct ActiveTurnRegistry {
    turns: Mutex<HashMap<SessionId, ActiveTurnExecution>>,
}

impl ActiveTurnRegistry {
    fn entry(_session_id: SessionId) -> ActiveTurnExecution {
        ActiveTurnExecution {
            turn: None,
            cancel_token: None,
            abort_handle: None,
            connection_id: None,
            spawn_snapshots: HashMap::new(),
            stream: None,
        }
    }

    pub(crate) async fn active_turn_id(&self, session_id: SessionId) -> Option<TurnId> {
        self.turns
            .lock()
            .await
            .get(&session_id)
            .and_then(|execution| execution.turn.as_ref().map(|turn| turn.turn_id))
    }

    pub(crate) async fn active_turn_metadata(&self, session_id: SessionId) -> Option<TurnMetadata> {
        self.turns
            .lock()
            .await
            .get(&session_id)
            .and_then(|execution| execution.turn.clone())
    }

    pub(crate) async fn active_connection_id(&self, session_id: SessionId) -> Option<u64> {
        self.turns
            .lock()
            .await
            .get(&session_id)
            .and_then(|execution| execution.connection_id)
    }

    pub(crate) async fn connection_map(&self) -> HashMap<SessionId, u64> {
        self.turns
            .lock()
            .await
            .iter()
            .filter_map(|(session_id, execution)| {
                execution
                    .connection_id
                    .map(|connection_id| (*session_id, connection_id))
            })
            .collect()
    }

    pub(crate) async fn cancel_token(&self, session_id: SessionId) -> Option<CancellationToken> {
        self.turns
            .lock()
            .await
            .get(&session_id)
            .and_then(|execution| execution.cancel_token.clone())
    }

    pub(crate) async fn insert_cancel_token(
        &self,
        session_id: SessionId,
        token: CancellationToken,
    ) {
        self.turns
            .lock()
            .await
            .entry(session_id)
            .or_insert_with(|| Self::entry(session_id))
            .cancel_token = Some(token);
    }

    pub(crate) async fn register_turn_metadata(&self, session_id: SessionId, turn: TurnMetadata) {
        self.turns
            .lock()
            .await
            .entry(session_id)
            .or_insert_with(|| Self::entry(session_id))
            .turn = Some(turn);
    }

    /// Claims a session for an in-flight turn when no active execution exists.
    pub(crate) async fn try_claim_session(
        &self,
        session_id: SessionId,
        turn: TurnMetadata,
    ) -> bool {
        let mut turns = self.turns.lock().await;
        if turns.contains_key(&session_id) {
            return false;
        }
        turns
            .entry(session_id)
            .or_insert_with(|| Self::entry(session_id))
            .turn = Some(turn);
        true
    }

    pub(crate) async fn set_abort_handle(
        &self,
        session_id: SessionId,
        abort_handle: tokio::task::AbortHandle,
    ) {
        if let Some(execution) = self.turns.lock().await.get_mut(&session_id) {
            execution.abort_handle = Some(abort_handle);
        }
    }

    pub(crate) async fn set_connection_id(&self, session_id: SessionId, connection_id: u64) {
        self.turns
            .lock()
            .await
            .entry(session_id)
            .or_insert_with(|| Self::entry(session_id))
            .connection_id = Some(connection_id);
    }

    pub(crate) async fn abort_task(&self, session_id: SessionId) -> bool {
        let abort_handle = self
            .turns
            .lock()
            .await
            .get_mut(&session_id)
            .and_then(|execution| execution.abort_handle.take());
        if let Some(abort_handle) = abort_handle {
            abort_handle.abort();
            true
        } else {
            false
        }
    }

    pub(crate) async fn remove_cancel_token(&self, session_id: SessionId) {
        let mut turns = self.turns.lock().await;
        if let Some(execution) = turns.get_mut(&session_id) {
            execution.cancel_token = None;
            Self::maybe_remove_empty(&mut turns, session_id);
        }
    }

    pub(crate) async fn remove_abort_handle(&self, session_id: SessionId) {
        let mut turns = self.turns.lock().await;
        if let Some(execution) = turns.get_mut(&session_id) {
            execution.abort_handle = None;
            Self::maybe_remove_empty(&mut turns, session_id);
        }
    }

    /// Clears cancellation, metadata, and connection routing while a turn ends.
    ///
    /// Stream state and spawn snapshots remain registered until the turn task
    /// unregisters them after `MergeTurn`.
    pub(crate) async fn clear_interrupt_handles(&self, session_id: SessionId) {
        let mut turns = self.turns.lock().await;
        if let Some(execution) = turns.get_mut(&session_id) {
            execution.turn = None;
            execution.cancel_token = None;
            execution.abort_handle = None;
            execution.connection_id = None;
            Self::maybe_remove_empty(&mut turns, session_id);
        }
    }

    /// Drops all runtime state for a session turn, including stream mirrors.
    pub(crate) async fn clear_runtime_handles(&self, session_id: SessionId) {
        self.turns.lock().await.remove(&session_id);
    }

    pub(crate) async fn register_spawn_snapshot(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        snapshot: Arc<SpawnSnapshot>,
    ) {
        self.turns
            .lock()
            .await
            .entry(session_id)
            .or_insert_with(|| Self::entry(session_id))
            .spawn_snapshots
            .insert(turn_id, snapshot);
    }

    pub(crate) async fn clear_spawn_snapshot(&self, session_id: SessionId, turn_id: TurnId) {
        let mut turns = self.turns.lock().await;
        if let Some(execution) = turns.get_mut(&session_id) {
            execution.spawn_snapshots.remove(&turn_id);
            Self::maybe_remove_empty(&mut turns, session_id);
        }
    }

    pub(crate) async fn spawn_snapshot_for_session(
        &self,
        session_id: SessionId,
    ) -> Option<SpawnSnapshot> {
        let turns = self.turns.lock().await;
        let execution = turns.get(&session_id)?;
        execution
            .spawn_snapshots
            .values()
            .next()
            .map(|snapshot| (**snapshot).clone())
    }

    pub(crate) async fn register_stream(
        &self,
        session_id: SessionId,
        stream: Arc<tokio::sync::Mutex<SessionStreamState>>,
    ) {
        self.turns
            .lock()
            .await
            .entry(session_id)
            .or_insert_with(|| Self::entry(session_id))
            .stream = Some(stream);
    }

    pub(crate) async fn unregister_stream(&self, session_id: SessionId) {
        let mut turns = self.turns.lock().await;
        if let Some(execution) = turns.get_mut(&session_id) {
            execution.stream = None;
            Self::maybe_remove_empty(&mut turns, session_id);
        }
    }

    pub(crate) async fn stream_state(
        &self,
        session_id: SessionId,
    ) -> Option<Arc<tokio::sync::Mutex<SessionStreamState>>> {
        self.turns
            .lock()
            .await
            .get(&session_id)
            .and_then(|execution| execution.stream.clone())
    }

    pub(crate) async fn remove_session(&self, session_id: SessionId) {
        self.turns.lock().await.remove(&session_id);
    }

    pub(crate) async fn has_session(&self, session_id: SessionId) -> bool {
        self.turns.lock().await.contains_key(&session_id)
    }

    pub(crate) async fn cancel_token_for_host_or_session(
        &self,
        host_session_id: SessionId,
        session_id: SessionId,
    ) -> CancellationToken {
        let turns = self.turns.lock().await;
        turns
            .get(&host_session_id)
            .or_else(|| turns.get(&session_id))
            .and_then(|execution| execution.cancel_token.clone())
            .unwrap_or_else(CancellationToken::new)
    }

    pub(crate) async fn drop_connection_id(&self, connection_id: u64) {
        let mut turns = self.turns.lock().await;
        for execution in turns.values_mut() {
            if execution.connection_id == Some(connection_id) {
                execution.connection_id = None;
            }
        }
    }

    pub(crate) async fn copy_connection_from_parent(
        &self,
        child_session_id: SessionId,
        parent_session_id: SessionId,
    ) {
        let connection_id = self.active_connection_id(parent_session_id).await;
        if let Some(connection_id) = connection_id {
            self.set_connection_id(child_session_id, connection_id)
                .await;
        }
    }

    fn maybe_remove_empty(
        turns: &mut HashMap<SessionId, ActiveTurnExecution>,
        session_id: SessionId,
    ) {
        let should_remove = turns.get(&session_id).is_some_and(|execution| {
            execution.turn.is_none()
                && execution.cancel_token.is_none()
                && execution.abort_handle.is_none()
                && execution.connection_id.is_none()
                && execution.spawn_snapshots.is_empty()
                && execution.stream.is_none()
        });
        if should_remove {
            turns.remove(&session_id);
        }
    }
}
