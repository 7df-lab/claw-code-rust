use std::sync::Arc;

use devo_core::SessionId;
use tokio_util::sync::CancellationToken;

use crate::turn::TurnMetadata;

use super::ServerRuntime;

impl ServerRuntime {
    /// Registers cancellation, metadata, and optional connection ownership for a
    /// turn that is about to run on a background task.
    pub(crate) async fn register_active_turn_execution(
        &self,
        session_id: SessionId,
        turn: TurnMetadata,
        connection_id: Option<u64>,
    ) -> CancellationToken {
        let cancel_token = CancellationToken::new();
        self.active_turns
            .insert_cancel_token(session_id, cancel_token.clone())
            .await;
        self.register_runtime_active_turn(session_id, turn).await;
        if let Some(connection_id) = connection_id {
            self.active_turns
                .set_connection_id(session_id, connection_id)
                .await;
        }
        cancel_token
    }

    pub(crate) async fn attach_active_turn_abort_handle(
        &self,
        session_id: SessionId,
        abort_handle: tokio::task::AbortHandle,
    ) {
        self.active_turns
            .set_abort_handle(session_id, abort_handle)
            .await;
    }

    /// Drop the abort handle so an interrupt cannot kill terminalization mid-flight.
    ///
    /// Cancellation via the turn token remains active; only hard abort is detached.
    pub(crate) async fn detach_active_turn_abort(&self, session_id: SessionId) {
        self.active_turns.remove_abort_handle(session_id).await;
    }

    /// Cancels and aborts the active turn for `session_id` without clearing the
    /// full runtime handle (used while waiting for terminal status).
    pub(crate) async fn signal_active_turn_interrupt(&self, session_id: SessionId) {
        if let Some(cancel_token) = self.active_turns.cancel_token(session_id).await {
            cancel_token.cancel();
        }
        self.active_turns.abort_task(session_id).await;
    }

    pub(crate) async fn spawn_active_turn_task<F>(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: TurnMetadata,
        connection_id: Option<u64>,
        task: F,
    ) where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.register_active_turn_execution(session_id, turn, connection_id)
            .await;
        let runtime = Arc::clone(self);
        let join_handle = tokio::spawn(task);
        runtime
            .attach_active_turn_abort_handle(session_id, join_handle.abort_handle())
            .await;
    }
}
