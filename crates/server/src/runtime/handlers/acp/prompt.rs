//! ACP `session/prompt` and `session/cancel` handlers.
//!
//! Portable ACP clients submit turns through blocking `session/prompt`: the JSON-RPC
//! response is deferred until the turn reaches a terminal state, while progress is
//! streamed on `session/update`. Devo's TUI uses Native `turn/start` instead; this
//! module serves external ACP integrations.

use super::super::acp_slash_commands::AcpSlashCommandPromptResult;
use super::*;

impl ServerRuntime {
    /// Handles ACP `session/prompt` for non-Devo clients.
    ///
    /// Returns `None` when the prompt turn was accepted and the JSON-RPC response
    /// will be sent asynchronously after the turn finishes (ACP blocking semantics).
    /// Streaming progress is delivered through `session/update` on the subscribed
    /// connection while the background task waits for turn completion.
    pub(crate) async fn handle_acp_session_prompt(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> Option<serde_json::Value> {
        // --- Validate request ---------------------------------------------------
        let params: AcpPromptParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return Some(acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/prompt params: {error}"),
                ));
            }
        };
        // ACP prompt turns are exclusive per session; queueing is not supported here.
        if self.session_has_active_turn(params.session_id).await {
            return Some(acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "session already has an active prompt turn",
            ));
        }
        // Ensure this connection receives `session/update` for the prompt turn.
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        // --- Slash commands embedded in the prompt ----------------------------
        match self
            .handle_acp_slash_command_prompt(
                connection_id,
                request_id.clone(),
                params.session_id,
                &params.prompt,
            )
            .await
        {
            AcpSlashCommandPromptResult::NotCommand => {}
            AcpSlashCommandPromptResult::Response(response) => {
                return Some(response);
            }
            AcpSlashCommandPromptResult::Pending => return None,
        }
        // --- Start a regular turn from ACP prompt content ---------------------
        let session_id = params.session_id;
        let input = match input_items_from_acp_prompt(params.prompt) {
            Ok(input) => input,
            Err(error) => {
                return Some(acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    error,
                ));
            }
        };
        // Reuse the Devo turn engine with ACP-default params (no model/sandbox
        // extensions on the prompt RPC). Reject if another turn is already active.
        let legacy_response = self
            .handle_turn_start_with_queue_policy(
                Some(connection_id),
                request_id.clone(),
                TurnStartParams {
                    session_id,
                    input,
                    model: None,
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    sandbox: None,
                    approval_policy: None,
                    cwd: None,
                    collaboration_mode: CollaborationMode::Build,
                    execution_mode: TurnExecutionMode::Regular,
                },
                TurnStartQueuePolicy::RejectActive,
            )
            .await;
        let legacy: SuccessResponse<TurnStartResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return Some(legacy_error_to_acp(request_id, legacy_response)),
            };
        let Some(turn_id) = legacy.result.turn_id() else {
            return Some(acp_error_response(
                request_id,
                AcpErrorCode::ServerError,
                "session/prompt cannot queue behind an active turn",
            ));
        };
        // --- Defer JSON-RPC response until the turn completes -----------------
        // The client keeps reading `session/update` notifications while this
        // task blocks on terminal turn status, then receives `AcpPromptResult`.
        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            let stop_reason = runtime
                .wait_for_acp_prompt_stop_reason(session_id, turn_id)
                .await;
            runtime
                .send_raw_to_connection(
                    connection_id,
                    acp_success_response(
                        request_id,
                        AcpPromptResult {
                            stop_reason,
                            meta: None,
                        },
                    ),
                )
                .await;
        });
        None
    }

    /// Handles ACP `session/cancel`.
    ///
    /// Design intent: `session/cancel` is the session-scoped "stop current
    /// work" primitive. Prefer it over adding per-feature cancel RPCs.
    ///
    /// Manual `/compact` is admitted as an active `ManualCompaction` turn, so
    /// cancel routes through the shared turn-cancellation application service (and
    /// dual-emits `session/compaction/failed` with `"compaction canceled"`).
    /// Idle sessions are an idempotent no-op.
    ///
    /// Turn-scoped automatic compaction still rides the turn cancel token.
    pub(crate) async fn handle_acp_session_cancel(self: &Arc<Self>, params: serde_json::Value) {
        let params: AcpCancelParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                tracing::warn!(%error, "invalid session/cancel params");
                return;
            }
        };
        if let Some(turn_id) = self.runtime_active_turn_id(params.session_id).await {
            self.signal_active_turn_interrupt(params.session_id).await;
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                let _ = runtime
                    .interrupt_turn(
                        serde_json::Value::Null,
                        serde_json::to_value(TurnInterruptParams {
                            session_id: params.session_id,
                            turn_id,
                            reason: Some("cancelled by ACP client".to_string()),
                        })
                        .expect("serialize turn interrupt params"),
                    )
                    .await;
            });
            return;
        }

        tracing::debug!(
            session_id = %params.session_id,
            "session/cancel had no active turn"
        );
    }

    async fn session_has_active_turn(&self, session_id: SessionId) -> bool {
        self.runtime_active_turn_id(session_id).await.is_some()
    }

    /// Waits for a prompt turn to reach a terminal state and maps it to ACP `stopReason`.
    pub(crate) async fn wait_for_acp_prompt_stop_reason(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> AcpStopReason {
        let receiver = self.subscribe_terminal_turn_status(turn_id).await;
        if let Some(status) = self.recent_terminal_turn_status(turn_id).await {
            self.record_terminal_turn_status(turn_id, status).await;
        } else if !self.sessions.lock().await.contains_key(&session_id) {
            return AcpStopReason::Cancelled;
        }
        let status = match receiver.await {
            Ok(status) => status,
            Err(_) => return AcpStopReason::Refusal,
        };
        acp_stop_reason_from_terminal_turn(status)
    }
}

/// Maps internal turn completion metadata to the ACP `stopReason` enum.
fn acp_stop_reason_from_terminal_turn(snapshot: TerminalTurnSnapshot) -> AcpStopReason {
    match snapshot.status {
        TurnStatus::Completed => match snapshot.stop_reason {
            Some(devo_core::StopReason::MaxTokens) => AcpStopReason::MaxTokens,
            Some(
                devo_core::StopReason::EndTurn
                | devo_core::StopReason::ToolUse
                | devo_core::StopReason::StopSequence,
            )
            | None => AcpStopReason::EndTurn,
        },
        TurnStatus::Interrupted => AcpStopReason::Cancelled,
        TurnStatus::Failed => match snapshot.failure_reason {
            Some(devo_protocol::TurnFailureReason::MaxTurnRequests) => {
                AcpStopReason::MaxTurnRequests
            }
            None => AcpStopReason::Refusal,
        },
        TurnStatus::Pending | TurnStatus::Running | TurnStatus::WaitingApproval => {
            AcpStopReason::Refusal
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn acp_stop_reason_maps_terminal_turn_metadata() {
        let mut turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 1,
            status: TurnStatus::Completed,
            kind: devo_protocol::TurnKind::Regular,
            model: "test-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".to_string(),
            request_thinking: None,
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            usage: None,
            stop_reason: Some(devo_core::StopReason::MaxTokens),
            failure_reason: None,
        };
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::MaxTokens
        );

        turn.status = TurnStatus::Failed;
        turn.stop_reason = None;
        turn.failure_reason = Some(devo_protocol::TurnFailureReason::MaxTurnRequests);
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::MaxTurnRequests
        );

        turn.status = TurnStatus::Interrupted;
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::Cancelled
        );
        turn.status = TurnStatus::Failed;
        turn.failure_reason = None;
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::Refusal
        );
        turn.status = TurnStatus::Completed;
        assert_eq!(
            acp_stop_reason_from_terminal_turn(TerminalTurnSnapshot::from_turn(&turn)),
            AcpStopReason::EndTurn
        );
    }
}
