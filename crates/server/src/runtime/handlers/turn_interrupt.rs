use std::sync::Arc;
use std::time::Duration;

use super::super::*;

const TURN_INTERRUPT_TERMINAL_TIMEOUT: Duration = Duration::from_secs(5);

impl ServerRuntime {
    pub(crate) async fn interrupt_turn(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnInterruptParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid interrupt params: {error}"),
                );
            }
        };
        self.handle_turn_interrupt_translated(request_id, params)
            .await
    }

    async fn handle_turn_interrupt_translated(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: TurnInterruptParams,
    ) -> serde_json::Value {
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        // Turns that run inline on the session actor finalize themselves when the
        // cancel token fires (`finalize_executed_turn` records terminal status).
        // Research (and similar) turns run on a spawned task outside the actor:
        // aborting that task does not record a terminal status, so we must claim
        // `active_turn` via the mailbox and finalize here.
        if self.runtime_active_turn_id(params.session_id).await != Some(params.turn_id) {
            if let Some(snapshot) = self.recent_terminal_turn_status(params.turn_id).await {
                return self.turn_interrupt_success(request_id, params.turn_id, snapshot.status);
            }
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnNotFound,
                "turn is not active",
            );
        }

        let terminal_rx = self.subscribe_terminal_turn_status(params.turn_id).await;
        if let Some(snapshot) = self.recent_terminal_turn_status(params.turn_id).await {
            self.record_terminal_turn_status(params.turn_id, snapshot.clone())
                .await;
            return self.turn_interrupt_success(request_id, params.turn_id, snapshot.status);
        }
        eprintln!("dbg interrupt: before signal_active_turn_interrupt");

        // Cancel before any session-actor mailbox round-trip: the actor may be blocked
        // waiting for a permission response and cannot process commands until cancelled.
        // Cancel via a clone rather than `remove`: see the comment in
        // `interrupt_child_runtime_work` for why removing here races with
        // `run_turn_model_query` fetching the same token.
        self.signal_active_turn_interrupt(params.session_id).await;
        eprintln!("dbg interrupt: after signal");

        let removed_len = self
            .session_interactive
            .clear_pending_user_inputs_for_turn(params.session_id, params.turn_id)
            .await;
        eprintln!("dbg interrupt: after clear_pending");
        if removed_len > 0 {
            tracing::info!(
                session_id = %params.session_id,
                turn_id = %params.turn_id,
                removed_len,
                "cleared pending request_user_input requests for interrupted turn"
            );
        }

        Arc::clone(self)
            .interrupt_all_child_agents(params.session_id)
            .await;
        eprintln!("dbg interrupt: after child agents");

        // Out-of-actor turns (research): actor is free, so we can claim active_turn.
        // In-actor turns: finalize already cleared it; fall through to terminal wait.
        if let Some(interrupted_turn) = session_handle.interrupt_active_turn().await.flatten() {
            eprintln!("dbg interrupt: claimed active turn");
            if interrupted_turn.turn_id != params.turn_id {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnNotFound,
                    "turn does not exist",
                );
            }
            return self
                .finalize_claimed_interrupted_turn(
                    request_id,
                    &session_handle,
                    params.session_id,
                    interrupted_turn,
                )
                .await;
        }
        eprintln!("dbg interrupt: no claim, waiting for terminal");

        let snapshot =
            match tokio::time::timeout(TURN_INTERRUPT_TERMINAL_TIMEOUT, terminal_rx).await {
                Ok(Ok(snapshot)) => snapshot,
                Ok(Err(_)) | Err(_) => {
                    if let Some(snapshot) = self.recent_terminal_turn_status(params.turn_id).await {
                        snapshot
                    } else if let Some(orphaned) = self
                        .recover_orphaned_manual_compaction_interrupt(
                            &session_handle,
                            params.session_id,
                            params.turn_id,
                        )
                        .await
                    {
                        return self.turn_interrupt_success(
                            request_id,
                            params.turn_id,
                            orphaned.status,
                        );
                    } else {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::TurnNotFound,
                            "turn is not active",
                        );
                    }
                }
            };

        tracing::info!(
            session_id = %params.session_id,
            turn_id = %params.turn_id,
            status = ?snapshot.status,
            "interrupted turn"
        );

        self.turn_interrupt_success(request_id, params.turn_id, snapshot.status)
    }

    /// Safety net when interrupt abort raced past a compaction task that already
    /// cleared `active_turn` but never recorded a terminal status.
    pub(crate) async fn recover_orphaned_manual_compaction_interrupt(
        self: &Arc<Self>,
        session_handle: &crate::runtime::session_actor::SessionHandle,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Option<TerminalTurnSnapshot> {
        let meta = self.active_turns.active_turn_metadata(session_id).await?;
        if meta.turn_id != turn_id || meta.kind != devo_core::TurnKind::ManualCompaction {
            return None;
        }
        if let Some(snapshot) = self.recent_terminal_turn_status(turn_id).await {
            return Some(snapshot);
        }

        // Wake any compaction task still holding `state_change_gate` inside
        // `compact_history` so admission can proceed after recovery.
        if let Some(cancel_token) = self.active_turns.cancel_token(session_id).await {
            cancel_token.cancel();
        }
        self.active_turns.abort_task(session_id).await;

        let mut interrupted_turn = meta;
        interrupted_turn.status = TurnStatus::Interrupted;
        interrupted_turn.completed_at = Some(Utc::now());
        session_handle
            .set_session_idle(Some(interrupted_turn.clone()))
            .await;
        self.clear_active_turn_runtime_handles(session_id).await;

        tracing::warn!(
            session_id = %session_id,
            turn_id = %turn_id,
            "recovered orphaned manual compaction interrupt"
        );
        self.broadcast_event(ServerEvent::SessionCompactionFailed(
            SessionCompactionFailedPayload {
                session_id,
                message: "compaction canceled".to_string(),
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnInterrupted(TurnEventPayload {
            session_id,
            turn: interrupted_turn.clone(),
        }))
        .await;
        self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: interrupted_turn.clone(),
        }))
        .await;
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        let snapshot = TerminalTurnSnapshot::from_turn(&interrupted_turn);
        self.record_terminal_turn_status(turn_id, snapshot.clone())
            .await;
        Some(snapshot)
    }

    async fn finalize_claimed_interrupted_turn(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        session_handle: &crate::runtime::session_actor::SessionHandle,
        session_id: SessionId,
        interrupted_turn: TurnMetadata,
    ) -> serde_json::Value {
        eprintln!("dbg finalize: before clear_active_turn_runtime_handles");
        self.clear_active_turn_runtime_handles(session_id).await;
        eprintln!("dbg finalize: before take_deferred_items");

        let deferred = session_handle.take_deferred_items().await;
        eprintln!("dbg finalize: after take_deferred_items");
        if let Some((item_id, item_seq, text)) = deferred.assistant
            && !text.trim().is_empty()
        {
            self.complete_item(
                session_id,
                interrupted_turn.turn_id,
                item_id,
                item_seq,
                ItemKind::AgentMessage,
                TurnItem::AgentMessage(TextItem { text: text.clone() }),
                serde_json::json!({ "title": "Assistant", "text": text }),
            )
            .await;
        }
        if let Some((item_id, item_seq, text)) = deferred.reasoning {
            self.complete_item(
                session_id,
                interrupted_turn.turn_id,
                item_id,
                item_seq,
                ItemKind::Reasoning,
                TurnItem::Reasoning(TextItem { text: text.clone() }),
                serde_json::json!({ "title": "Reasoning", "text": text }),
            )
            .await;
        }
        eprintln!("dbg finalize: before turn_persistence_snapshot");
        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.record.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(session_id, &interrupted_turn)
                .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist interrupted turn: {error}"),
            );
        }
        eprintln!("dbg finalize: after persist");

        tracing::info!(
            session_id = %session_id,
            turn_id = %interrupted_turn.turn_id,
            status = ?interrupted_turn.status,
            "interrupted turn"
        );
        self.finalize_turn_workspace_changes(session_id, &interrupted_turn)
            .await;
        eprintln!("dbg finalize: after workspace changes");
        if interrupted_turn.kind == devo_core::TurnKind::ManualCompaction {
            // Manual compact dual-emits compaction lifecycle for existing UI;
            // abort may drop the compaction task before it can emit this itself.
            self.broadcast_event(ServerEvent::SessionCompactionFailed(
                SessionCompactionFailedPayload {
                    session_id,
                    message: "compaction canceled".to_string(),
                },
            ))
            .await;
        }
        self.broadcast_event(ServerEvent::TurnInterrupted(TurnEventPayload {
            session_id,
            turn: interrupted_turn.clone(),
        }))
        .await;
        self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: interrupted_turn.clone(),
        }))
        .await;
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        self.record_terminal_turn_status(
            interrupted_turn.turn_id,
            TerminalTurnSnapshot::from_turn(&interrupted_turn),
        )
        .await;

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            runtime.spawn_next_turn_from_queue(session_id).await;
        });

        self.turn_interrupt_success(
            request_id,
            interrupted_turn.turn_id,
            interrupted_turn.status,
        )
    }

    fn turn_interrupt_success(
        &self,
        request_id: serde_json::Value,
        turn_id: TurnId,
        status: TurnStatus,
    ) -> serde_json::Value {
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnInterruptResult { turn_id, status },
        })
        .expect("serialize interrupt response")
    }
}
