use std::sync::Arc;

use chrono::Utc;
use devo_core::{SessionId, TurnError, TurnStatus, TurnUsage};
use devo_protocol::{
    SessionHistoryItem, SessionHistoryItemKind, SessionHistoryMetadata, TurnFailedPayload,
};

use super::super::ServerRuntime;
use super::super::subagent_usage::ParentUsageSnapshot;
use super::failure::{turn_error_payload_from_error, turn_failure_reason_from_error};
use super::types::{TurnEventStreamSummary, TurnQueryOutcome};
use crate::db::{QueueType, SessionStats};
use crate::persistence::build_turn_record;
use crate::runtime::session_actor::SessionActorState;
use crate::{SessionRuntimeStatus, SessionStatusChangedPayload, TurnEventPayload};

fn terminal_usages(
    event_summary: Option<&TurnEventStreamSummary>,
    snapshot: Option<&ParentUsageSnapshot>,
) -> (Option<TurnUsage>, Option<TurnUsage>) {
    let turn_usage = snapshot
        .and_then(|snapshot| reported_usage(snapshot.turn_usage.to_turn_usage()))
        .or_else(|| {
            event_summary
                .and_then(|summary| summary.turn_usage.clone())
                .and_then(reported_usage)
        });
    let latest_query_usage = snapshot
        .and_then(|snapshot| reported_usage(snapshot.latest_query_usage.to_turn_usage()))
        .or_else(|| {
            event_summary
                .and_then(|summary| summary.latest_query_usage.clone())
                .and_then(reported_usage)
        });
    (turn_usage, latest_query_usage)
}

fn reported_usage(usage: TurnUsage) -> Option<TurnUsage> {
    (usage.display_total_tokens() > 0).then_some(usage)
}

pub(crate) struct FinalizeTurnParams<'a> {
    pub state: &'a mut SessionActorState,
    pub session_id: SessionId,
    pub turn: crate::TurnMetadata,
    pub query_outcome: TurnQueryOutcome,
    pub event_summary: Option<TurnEventStreamSummary>,
    pub usage_parent_session_id: Option<SessionId>,
}

impl ServerRuntime {
    pub(crate) async fn finalize_executed_turn(self: &Arc<Self>, params: FinalizeTurnParams<'_>) {
        let FinalizeTurnParams {
            state,
            session_id,
            turn,
            query_outcome,
            event_summary,
            usage_parent_session_id,
        } = params;
        let TurnQueryOutcome {
            result,
            mut session_total_input_tokens,
            mut session_total_output_tokens,
            mut session_total_tokens,
            mut session_total_cache_creation_tokens,
            mut session_total_cache_read_tokens,
            session_last_input_tokens,
            session_prompt_token_estimate,
        } = query_outcome;
        let terminal_stop_reason = event_summary
            .as_ref()
            .and_then(|summary| summary.stop_reason.clone());
        if usage_parent_session_id.is_some() {
            // Completed legs were already accumulated by the event stream.
            // Only fold any trailing in-flight delta (e.g. interrupted mid-stream).
            let _ = self
                .commit_subagent_inflight_usage(session_id, turn.turn_id)
                .await;
        }
        let usage_snapshot = if usage_parent_session_id.is_none() {
            self.parent_usage_snapshot(session_id, turn.turn_id).await
        } else {
            None
        };
        let (turn_usage, latest_query_usage) =
            terminal_usages(event_summary.as_ref(), usage_snapshot.as_ref());
        let terminal_error = result
            .as_ref()
            .err()
            .filter(|error| !matches!(error, devo_core::AgentError::Aborted))
            .map(turn_error_payload_from_error)
            .map(|error| TurnError {
                code: error.code,
                message: error.message,
                recovery_hint: error.recovery_hint,
            });
        if let Some(snapshot) = usage_snapshot {
            session_total_input_tokens = snapshot.session_totals.input_tokens;
            session_total_output_tokens = snapshot.session_totals.output_tokens;
            session_total_tokens = snapshot.session_totals.total_tokens;
            session_total_cache_creation_tokens =
                snapshot.session_totals.cache_creation_input_tokens;
            session_total_cache_read_tokens = snapshot.session_totals.cache_read_input_tokens;
        }
        // Leave ActiveTurnRegistry turn metadata until after MergeTurn so
        // admission cannot race a free registry against an unmerged working set.
        match &result {
            Ok(()) => {
                self.run_session_hook_for_actor_state(
                    state,
                    session_id,
                    devo_core::HookEvent::Stop,
                    serde_json::Map::from_iter([(
                        "stop_hook_active".to_string(),
                        serde_json::Value::Bool(false),
                    )]),
                )
                .await;
            }
            Err(error) => {
                self.run_session_hook_for_actor_state(
                    state,
                    session_id,
                    devo_core::HookEvent::StopFailure,
                    serde_json::Map::from_iter([
                        (
                            "error".to_string(),
                            serde_json::Value::String(error.to_string()),
                        ),
                        (
                            "error_details".to_string(),
                            serde_json::Value::String(error.to_string()),
                        ),
                    ]),
                )
                .await;
            }
        }

        let final_turn = self
            .persist_terminal_turn_state(
                state,
                session_id,
                &turn,
                &result,
                turn_usage,
                latest_query_usage.clone(),
                terminal_stop_reason,
                session_total_input_tokens,
                session_total_output_tokens,
                session_total_tokens,
                session_total_cache_creation_tokens,
                session_total_cache_read_tokens,
                session_last_input_tokens,
                session_prompt_token_estimate,
            )
            .await;
        if matches!(final_turn.status, TurnStatus::Interrupted) {
            state.core.mark_last_turn_interrupted();
        } else {
            state.core.last_turn_interrupted = false;
        }
        self.clear_steer_input_queue(state, session_id).await;
        self.append_terminal_turn_record(
            state,
            session_id,
            &final_turn,
            latest_query_usage,
            terminal_error.clone(),
        )
        .await;
        append_terminal_history_items(state, &final_turn, terminal_error.as_ref());
        self.finalize_turn_workspace_changes(session_id, &final_turn)
            .await;
        self.emit_terminal_turn_events(
            state,
            session_id,
            &final_turn,
            &result,
            terminal_error.as_ref(),
        )
        .await;
        self.record_terminal_turn_status(
            final_turn.turn_id,
            super::super::TerminalTurnSnapshot::from_turn(&final_turn),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn persist_terminal_turn_state(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        session_id: SessionId,
        turn: &crate::TurnMetadata,
        result: &Result<(), devo_core::AgentError>,
        turn_usage: Option<devo_core::TurnUsage>,
        latest_query_usage: Option<devo_core::TurnUsage>,
        terminal_stop_reason: Option<devo_core::StopReason>,
        session_total_input_tokens: usize,
        session_total_output_tokens: usize,
        session_total_tokens: usize,
        session_total_cache_creation_tokens: usize,
        session_total_cache_read_tokens: usize,
        session_last_input_tokens: usize,
        session_prompt_token_estimate: usize,
    ) -> crate::TurnMetadata {
        let mut final_turn = turn.clone();
        final_turn.completed_at = Some(Utc::now());
        final_turn.status = match result {
            Ok(()) => TurnStatus::Completed,
            Err(devo_core::AgentError::Aborted) => TurnStatus::Interrupted,
            Err(_) => TurnStatus::Failed,
        };
        final_turn.usage = turn_usage;
        final_turn.stop_reason = terminal_stop_reason;
        final_turn.failure_reason = result
            .as_ref()
            .err()
            .and_then(turn_failure_reason_from_error);
        state.latest_turn = Some(final_turn.clone());
        state.active_turn = None;
        state.summary.status = SessionRuntimeStatus::Idle;
        state.summary.updated_at = Utc::now();
        state.summary.last_activity_at = state.summary.updated_at;
        state.summary.total_input_tokens = session_total_input_tokens;
        state.summary.total_output_tokens = session_total_output_tokens;
        state.summary.total_tokens = session_total_tokens;
        state.summary.total_cache_creation_tokens = session_total_cache_creation_tokens;
        state.summary.total_cache_read_tokens = session_total_cache_read_tokens;
        state.summary.prompt_token_estimate = session_prompt_token_estimate;
        let last_input_for_stats = latest_query_usage
            .as_ref()
            .map(|usage| usage.input_tokens as usize)
            .unwrap_or(session_last_input_tokens);
        if let Some(usage) = latest_query_usage {
            // Context length uses latest-query display total, not session
            // cumulative total_input/output/tokens.
            state.summary.last_query_usage = Some(usage.clone());
            state.summary.last_query_total_tokens = usage.display_total_tokens();
            state.core.last_input_tokens = usage.input_tokens as usize;
            state.core.last_turn_tokens = usage.display_total_tokens();
            if let Some(raw) = state.core.raw_context_breakdown {
                let window = effective_context_window_tokens(state, self);
                let occupancy = super::super::context_occupancy::occupancy_from_raw(
                    window,
                    raw,
                    usage.display_total_tokens() as u64,
                );
                state.core.last_turn_tokens = occupancy.total_tokens as usize;
                state.summary.last_context_occupancy = Some(occupancy);
            }
        }
        state.core.total_input_tokens = session_total_input_tokens;
        state.core.total_output_tokens = session_total_output_tokens;
        state.core.total_tokens = session_total_tokens;
        state.core.total_cache_creation_tokens = session_total_cache_creation_tokens;
        state.core.total_cache_read_tokens = session_total_cache_read_tokens;
        if !state.summary.ephemeral {
            let stats = SessionStats {
                total_input_tokens: session_total_input_tokens,
                total_output_tokens: session_total_output_tokens,
                total_tokens: session_total_tokens,
                total_cache_creation_tokens: session_total_cache_creation_tokens,
                total_cache_read_tokens: session_total_cache_read_tokens,
                // Persist latest-query input only — turn-aggregate usage would
                // inflate auto-compact after resume via hydrate.
                last_input_tokens: last_input_for_stats,
                turn_count: state.summary.updated_at.timestamp() as usize,
                prompt_token_estimate: session_prompt_token_estimate,
                last_context_occupancy: state.summary.last_context_occupancy.clone(),
            };
            if let Err(err) = self.deps.db.update_stats(&session_id, &stats) {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to persist token stats to database"
                );
            }
        }
        final_turn
    }

    async fn clear_steer_input_queue(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
    ) {
        let is_ephemeral = state.summary.ephemeral;
        let steer_input_queue = Arc::clone(&state.steer_input_queue);
        let leftover: Vec<devo_core::PendingInputItem> = {
            let mut queue = steer_input_queue
                .lock()
                .expect("steer input queue mutex should not be poisoned");
            queue.drain(..).collect()
        };
        if !leftover.is_empty() {
            // Any unconsumed steer degrades into the session turn queue
            // (01 §4.3); normally `SessionState::end_turn` already moved
            // them, this covers terminal paths that skip it.
            let mut queue = state
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            for item in &leftover {
                queue.push_back(item.clone());
            }
        }
        if !is_ephemeral {
            // The database mirrors the same degradation, but only for ids that
            // remain in the shared in-memory turn queue. A steer already
            // drained by follow-up scheduling was consumed into its next turn;
            // retaining or moving its steer row would leave an orphan and make
            // a restart resurrect a duplicate. Every other steer row is dropped.
            let queued_ids: std::collections::HashSet<devo_core::PendingInputId> = state
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned")
                .iter()
                .map(|item| item.id)
                .collect();
            match self.deps.db.drain_pending(&session_id, QueueType::Steer) {
                Ok(rows) => {
                    for item in &rows {
                        if !queued_ids.contains(&item.id) {
                            continue;
                        }
                        if let Err(error) =
                            self.deps
                                .db
                                .push_pending(&session_id, QueueType::Turn, item)
                        {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "failed to degrade steer input into the turn queue"
                            );
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "failed to drain steer input messages from database"
                    );
                }
            }
        }
        for item in &leftover {
            self.broadcast_queue_updated(
                session_id,
                devo_protocol::native::queue::QueueChange::Added,
                devo_protocol::native::ids::QueueItemId::from_legacy_uuid(uuid::Uuid::from(
                    item.id,
                )),
                None,
            )
            .await;
        }
    }

    async fn append_terminal_turn_record(
        self: &Arc<Self>,
        state: &mut SessionActorState,
        session_id: SessionId,
        final_turn: &crate::TurnMetadata,
        latest_query_usage: Option<devo_core::TurnUsage>,
        terminal_error: Option<TurnError>,
    ) {
        let record = state.record.clone();
        let turn_context = state.core.latest_turn_context.clone();
        let session_context = state.core.session_context.clone();
        let mut turn_record = build_turn_record(
            final_turn,
            None,
            turn_context,
            latest_query_usage,
            state.summary.last_context_occupancy.clone(),
        );
        turn_record.error = terminal_error;
        state
            .turn_records_by_id
            .insert(turn_record.id, turn_record.clone());
        if let Some(record) = record
            && let Err(error) = self.rollout_store.append_turn_deduped(
                &record,
                &mut state.session_context_recorded,
                turn_record,
                session_context,
            )
        {
            tracing::warn!(session_id = %session_id, error = %error, "failed to persist terminal turn line");
        }
    }

    async fn emit_terminal_turn_events(
        self: &Arc<Self>,
        state: &SessionActorState,
        session_id: SessionId,
        final_turn: &crate::TurnMetadata,
        result: &Result<(), devo_core::AgentError>,
        terminal_error: Option<&TurnError>,
    ) {
        if let Err(error) = result {
            if matches!(error, devo_core::AgentError::Aborted) {
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %final_turn.turn_id,
                    status = ?final_turn.status,
                    "turn execution interrupted"
                );
                self.broadcast_event(crate::ServerEvent::TurnInterrupted(TurnEventPayload {
                    session_id,
                    turn: final_turn.clone(),
                }))
                .await;
            } else {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %final_turn.turn_id,
                    status = ?final_turn.status,
                    error = %error,
                    "turn execution failed"
                );
                self.broadcast_event(crate::ServerEvent::TurnFailed(TurnFailedPayload {
                    session_id,
                    turn: final_turn.clone(),
                    error: terminal_error.map(|error| devo_protocol::TurnErrorPayload {
                        code: error.code.clone(),
                        message: error.message.clone(),
                        recovery_hint: error.recovery_hint.clone(),
                    }),
                }))
                .await;
            }
        } else {
            tracing::info!(
                session_id = %session_id,
                turn_id = %final_turn.turn_id,
                status = ?final_turn.status,
                total_input_tokens = final_turn.usage.as_ref().map(|usage| usage.input_tokens),
                total_output_tokens = final_turn.usage.as_ref().map(|usage| usage.output_tokens),
                "turn execution completed"
            );
        }
        self.handle_subagent_turn_completed_for_actor_state(state, session_id, final_turn)
            .await;
        self.broadcast_event(crate::ServerEvent::TurnCompleted(TurnEventPayload {
            session_id,
            turn: final_turn.clone(),
        }))
        .await;
        self.broadcast_event(crate::ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        if let Some(occupancy) = state.summary.last_context_occupancy.clone() {
            self.broadcast_event(crate::ServerEvent::ContextUsageUpdated(
                crate::ContextUsageUpdatedPayload {
                    session_id,
                    occupancy,
                },
            ))
            .await;
        }
    }
}

fn effective_context_window_tokens(state: &SessionActorState, runtime: &ServerRuntime) -> u64 {
    let model = state
        .core
        .latest_turn_context
        .as_ref()
        .map(|context| &context.model)
        .or_else(|| {
            state
                .summary
                .model
                .as_deref()
                .and_then(|slug| runtime.deps.model_catalog.get(slug))
        });
    let global = runtime
        .deps
        .config_store
        .lock()
        .expect("app config store mutex should not be poisoned")
        .effective_config()
        .compaction_token_limit;
    super::super::context_occupancy::occupancy_window_tokens(
        state.core.config.effective_context_window_override,
        model,
        global,
    )
}

fn append_terminal_history_items(
    state: &mut SessionActorState,
    final_turn: &crate::TurnMetadata,
    terminal_error: Option<&TurnError>,
) {
    if let Some(error) = terminal_error {
        let title = error
            .recovery_hint
            .clone()
            .unwrap_or_else(|| error.code.clone());
        state.history_items.push(SessionHistoryItem::new(
            None,
            SessionHistoryItemKind::Error,
            title,
            error.message.clone(),
        ));
    }

    let outcome = match final_turn.status {
        TurnStatus::Failed => "failed",
        TurnStatus::Interrupted => "interrupted",
        TurnStatus::Completed => "",
        TurnStatus::Pending | TurnStatus::Running | TurnStatus::WaitingApproval => return,
    };
    let duration_secs = final_turn.completed_at.and_then(|completed| {
        let seconds = (completed - final_turn.started_at).num_seconds();
        (seconds > 0).then_some(seconds as u64)
    });
    state.history_items.push(SessionHistoryItem {
        tool_call_id: None,
        kind: SessionHistoryItemKind::TurnSummary,
        title: final_turn.model.clone(),
        body: outcome.to_string(),
        tool_io: None,
        metadata: Some(SessionHistoryMetadata::TurnSummary {
            collaboration_mode: state.core.collaboration_mode,
        }),
        duration_ms: duration_secs,
    });
}

#[cfg(test)]
mod tests {
    use devo_core::{SessionId, TurnId, TurnUsage};
    use pretty_assertions::assert_eq;

    use super::super::super::subagent_usage::{ParentUsageSnapshot, UsageTotals};
    use super::super::types::TurnEventStreamSummary;
    use super::terminal_usages;

    #[test]
    fn terminal_usage_keeps_turn_total_separate_from_latest_query() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let summary = TurnEventStreamSummary {
            turn_usage: Some(TurnUsage {
                input_tokens: 1_300,
                output_tokens: 80,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(1_380),
            }),
            latest_query_usage: Some(TurnUsage {
                input_tokens: 700,
                output_tokens: 30,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(730),
            }),
            stop_reason: None,
        };
        let snapshot = ParentUsageSnapshot {
            session_id,
            turn_id,
            turn_usage: UsageTotals {
                input_tokens: 1_300,
                output_tokens: 80,
                total_tokens: 1_380,
                ..UsageTotals::default()
            },
            latest_query_usage: UsageTotals {
                input_tokens: 700,
                output_tokens: 30,
                total_tokens: 730,
                ..UsageTotals::default()
            },
            session_totals: UsageTotals::default(),
            context_window: None,
        };

        let (turn_usage, latest_query_usage) = terminal_usages(Some(&summary), Some(&snapshot));

        assert_eq!(turn_usage, summary.turn_usage);
        assert_eq!(latest_query_usage, summary.latest_query_usage);
    }

    #[test]
    fn terminal_usage_ignores_unreported_zero_snapshot() {
        let snapshot = ParentUsageSnapshot {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            turn_usage: UsageTotals::default(),
            latest_query_usage: UsageTotals::default(),
            session_totals: UsageTotals {
                input_tokens: 1_000,
                output_tokens: 100,
                total_tokens: 1_100,
                ..UsageTotals::default()
            },
            context_window: None,
        };

        assert_eq!(terminal_usages(None, Some(&snapshot)), (None, None));
    }

    #[test]
    fn terminal_usage_falls_back_to_reported_event_when_snapshot_is_empty() {
        let summary = TurnEventStreamSummary {
            turn_usage: Some(TurnUsage {
                input_tokens: 500,
                output_tokens: 20,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(520),
            }),
            latest_query_usage: Some(TurnUsage {
                input_tokens: 300,
                output_tokens: 10,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(310),
            }),
            stop_reason: None,
        };
        let snapshot = ParentUsageSnapshot {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            turn_usage: UsageTotals::default(),
            latest_query_usage: UsageTotals::default(),
            session_totals: UsageTotals::default(),
            context_window: None,
        };

        let (turn_usage, latest_query_usage) = terminal_usages(Some(&summary), Some(&snapshot));

        assert_eq!(turn_usage, summary.turn_usage);
        assert_eq!(latest_query_usage, summary.latest_query_usage);
    }
}
