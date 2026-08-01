use super::super::*;
use std::panic::AssertUnwindSafe;

use devo_protocol::TurnFailedPayload;
use devo_protocol::approx_tokens_from_byte_count;
use futures::FutureExt;

enum CompactionTurnOutcome {
    Skipped,
    Failed { message: String },
    Canceled,
}

impl ServerRuntime {
    pub(crate) async fn handle_session_compact(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionCompactParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/compact params: {error}"),
                );
            }
        };

        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let _state_change_guard = session_handle.lock_state_change().await;
        let Some(reservation) = session_handle.turn_reservation_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let requested_model = session_model_selection(&reservation.summary);
        let requested_reasoning_effort_selection =
            reservation.summary.reasoning_effort_selection.clone();
        let turn_config = reservation
            .runtime_context
            .resolve_turn_config(requested_model, requested_reasoning_effort_selection);
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let now = Utc::now();
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: params.session_id,
            sequence: reservation
                .latest_turn
                .as_ref()
                .map_or(1, |turn| turn.sequence + 1),
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::ManualCompaction,
            model: turn_config.model.slug.clone(),
            model_binding_id: turn_config.model_binding_id.clone(),
            reasoning_effort_selection: turn_config.reasoning_effort_selection.clone(),
            reasoning_effort: resolved_request.effective_reasoning_effort,
            request_model,
            request_thinking: resolved_request.request_thinking,
            started_at: now,
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };

        if !session_handle
            .try_begin_active_turn(turn.clone(), turn_config)
            .await
            .unwrap_or(false)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot compact while a turn is active or queued",
            );
        }

        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.record.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(params.session_id, &turn)
                .await
        {
            let _ = session_handle
                .clear_active_turn_if_matches(turn.turn_id)
                .await;
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist compaction turn start: {error}"),
            );
        }

        let runtime = Arc::clone(self);
        let session_id = params.session_id;
        let turn_for_task = turn.clone();
        let session_handle_for_task = session_handle.clone();
        self.spawn_active_turn_task(
            session_id,
            turn.clone(),
            /*connection_id*/ None,
            async move {
                let runtime_for_panic = Arc::clone(&runtime);
                let session_handle_for_panic = session_handle_for_task.clone();
                let turn_for_panic = turn_for_task.clone();
                if let Err(panic) = AssertUnwindSafe(runtime.run_session_compaction(
                    session_id,
                    session_handle_for_task,
                    turn_for_task,
                ))
                .catch_unwind()
                .await
                {
                    tracing::error!(
                        session_id = %session_id,
                        turn_id = %turn_for_panic.turn_id,
                        panic = ?panic,
                        "session compaction task panicked"
                    );
                    runtime_for_panic
                        .finalize_manual_compaction_turn(
                            &session_handle_for_panic,
                            session_id,
                            turn_for_panic.clone(),
                            CompactionTurnOutcome::Failed {
                                message: "compaction failed: panicked".to_string(),
                            },
                        )
                        .await;
                    // If the panic happened after claim, finalize is a no-op — still
                    // recover so the session is not left without terminal events.
                    if runtime_for_panic
                        .recent_terminal_turn_status(turn_for_panic.turn_id)
                        .await
                        .is_none()
                    {
                        let _ = runtime_for_panic
                            .recover_orphaned_manual_compaction_interrupt(
                                &session_handle_for_panic,
                                session_id,
                                turn_for_panic.turn_id,
                            )
                            .await;
                        if runtime_for_panic
                            .recent_terminal_turn_status(turn_for_panic.turn_id)
                            .await
                            .is_none()
                        {
                            // Actor claim may have cleared runtime metadata too; force
                            // a Failed terminal so admission reopens.
                            let mut failed = turn_for_panic;
                            failed.status = TurnStatus::Failed;
                            failed.completed_at = Some(Utc::now());
                            session_handle_for_panic
                                .set_session_idle(Some(failed.clone()))
                                .await;
                            runtime_for_panic
                                .clear_active_turn_runtime_handles(session_id)
                                .await;
                            runtime_for_panic
                                .broadcast_event(ServerEvent::SessionCompactionFailed(
                                    SessionCompactionFailedPayload {
                                        session_id,
                                        message: "compaction failed: panicked".to_string(),
                                    },
                                ))
                                .await;
                            runtime_for_panic
                                .broadcast_event(ServerEvent::TurnFailed(TurnFailedPayload {
                                    session_id,
                                    turn: failed.clone(),
                                    error: None,
                                }))
                                .await;
                            runtime_for_panic
                                .broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                                    session_id,
                                    turn: failed.clone(),
                                }))
                                .await;
                            runtime_for_panic
                                .broadcast_event(ServerEvent::SessionStatusChanged(
                                    SessionStatusChangedPayload {
                                        session_id,
                                        status: SessionRuntimeStatus::Idle,
                                    },
                                ))
                                .await;
                            runtime_for_panic
                                .record_terminal_turn_status(
                                    failed.turn_id,
                                    TerminalTurnSnapshot::from_turn(&failed),
                                )
                                .await;
                        }
                    }
                }
            },
        )
        .await;

        tracing::info!(
            session_id = %session_id,
            turn_id = %turn.turn_id,
            sequence = turn.sequence,
            "started manual compaction turn"
        );
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id,
            turn: turn.clone(),
        }))
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnStartResult::Started {
                turn_id: turn.turn_id,
                status: turn.status.clone(),
                accepted_at: now,
            },
        })
        .expect("serialize session/compact response")
    }

    pub(crate) async fn run_session_compaction(
        self: Arc<Self>,
        session_id: SessionId,
        session_handle: crate::runtime::session_actor::SessionHandle,
        turn: TurnMetadata,
    ) {
        tracing::info!(
            session_id = %session_id,
            turn_id = %turn.turn_id,
            "session compaction task started"
        );
        let Some(started_summary) = session_handle.summary().await else {
            self.finalize_manual_compaction_turn(
                &session_handle,
                session_id,
                turn,
                CompactionTurnOutcome::Failed {
                    message: "compaction failed: session unavailable".to_string(),
                },
            )
            .await;
            return;
        };
        self.broadcast_event(ServerEvent::SessionCompactionStarted(SessionEventPayload {
            session: started_summary,
        }))
        .await;
        self.run_session_hook(
            session_id,
            devo_core::HookEvent::PreCompact,
            serde_json::Map::from_iter([
                ("trigger".to_string(), serde_json::json!("manual")),
                ("custom_instructions".to_string(), serde_json::Value::Null),
            ]),
        )
        .await;

        let cancel_token = self
            .active_turns
            .cancel_token(session_id)
            .await
            .unwrap_or_else(CancellationToken::new);

        // Compaction computes a replacement from a history snapshot. Keep the
        // session mutation gate for the whole summarize-and-apply operation so
        // rollback, turn admission, and metadata edits cannot make that
        // replacement stale while the model call is in flight.
        let state_change_guard = session_handle.lock_state_change().await;
        let result = {
            let Some(runtime_session) = session_handle.export_runtime_session().await else {
                tracing::warn!(session_id = %session_id, "session compaction failed: session unavailable");
                drop(state_change_guard);
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Failed {
                        message: "compaction failed: session unavailable".to_string(),
                    },
                )
                .await;
                return;
            };
            let core_session = runtime_session.core_session.lock().await;

            let items: Vec<ResponseItem> = core_session
                .messages
                .iter()
                .flat_map(|msg| message_to_response_items(msg.clone()))
                .collect();

            let token_info = TokenInfo {
                input_tokens: core_session.total_input_tokens,
                cached_input_tokens: core_session.total_cache_read_tokens,
                output_tokens: core_session.total_output_tokens,
            };

            let model_selection = session_model_selection(&runtime_session.summary)
                .unwrap_or(&runtime_session.runtime_context.default_model);
            let turn_config = runtime_session.runtime_context.resolve_turn_config(
                Some(model_selection),
                /*reasoning_effort_selection*/ None,
            );
            let resolved_request = turn_config.model.resolve_reasoning_effort_selection(None);
            let model_slug = resolved_request.request_model;
            let request_model = turn_config.provider_request_model(&model_slug);
            let max_tokens = runtime_session
                .runtime_context
                .model_catalog
                .get(&model_slug)
                .and_then(|m| m.max_tokens.map(|t| t as usize))
                .unwrap_or(4096);

            tracing::debug!(
                session_id = %session_id,
                turn_id = %turn.turn_id,
                model = %model_slug,
                request_model = %request_model,
                item_count = items.len(),
                input_tokens = token_info.input_tokens,
                cached_input_tokens = token_info.cached_input_tokens,
                output_tokens = token_info.output_tokens,
                "starting compaction summarization"
            );
            let provider = self.usage_ledger.instrumented_provider(
                runtime_session
                    .runtime_context
                    .provider_for_route(turn_config.provider_route.clone()),
                session_id,
                Some(turn.turn_id),
                devo_protocol::canonical::usage::UsagePurpose::Compaction,
            );
            let summarizer = DefaultHistorySummarizer::with_models(
                provider,
                model_slug,
                request_model,
                max_tokens,
            );

            let config = CompactionConfig {
                budget: core_session.config.token_budget.clone(),
                // Proactive: user-requested /compact; preserve latest user suffix.
                // Example: [user1, asst1, user2, asst2, user3] -> [summary, user3].
                kind: CompactionKind::Proactive,
            };

            // Drop the core_session lock before the long summarizer await.
            drop(core_session);
            drop(runtime_session);

            compact_history(
                &items,
                &token_info,
                &summarizer,
                &config,
                Some(&cancel_token),
            )
            .await
        };

        // Summarize is done: detach abort so interrupt cannot kill mid-terminalize.
        // Cancel token still works for any remaining cooperative checks.
        self.detach_active_turn_abort(session_id).await;

        match result {
            Err(devo_core::history::compaction::CompactionError::Canceled) => {
                drop(state_change_guard);
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    "session compaction canceled"
                );
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Canceled,
                )
                .await;
            }
            Ok(CompactAction::Replaced(compacted_items)) => {
                if cancel_token.is_cancelled() {
                    drop(state_change_guard);
                    self.finalize_manual_compaction_turn(
                        &session_handle,
                        session_id,
                        turn,
                        CompactionTurnOutcome::Canceled,
                    )
                    .await;
                    return;
                }
                let Some(mut runtime_session) = session_handle.export_runtime_session().await
                else {
                    drop(state_change_guard);
                    self.finalize_manual_compaction_turn(
                        &session_handle,
                        session_id,
                        turn,
                        CompactionTurnOutcome::Failed {
                            message: "compaction failed: session unavailable".to_string(),
                        },
                    )
                    .await;
                    return;
                };
                // Claim terminalization before mutating history so an interrupt that
                // already took `active_turn` cannot race with replace_state.
                if session_handle
                    .clear_active_turn_if_matches(turn.turn_id)
                    .await
                    != Some(true)
                {
                    drop(state_change_guard);
                    return;
                }
                let preserved_item_ids = Self::preserved_item_ids_from_compacted(
                    &runtime_session.persisted_turn_items,
                    &compacted_items,
                );
                let new_messages: Vec<Message> = compacted_items
                    .iter()
                    .filter_map(|item| match item {
                        ResponseItem::Message(msg) => Some(msg.clone()),
                        _ => None,
                    })
                    .collect();

                {
                    let (
                        compacted_total_input_tokens,
                        compacted_total_output_tokens,
                        compacted_total_tokens,
                        compacted_total_cache_creation_tokens,
                        compacted_total_cache_read_tokens,
                        compacted_prompt_token_estimate,
                        compacted_occupancy,
                    ) = {
                        let previous_occupancy =
                            runtime_session.summary.last_context_occupancy.clone();
                        let mut core_session = runtime_session.core_session.lock().await;
                        core_session.set_prompt_messages(new_messages);
                        let prompt_bytes = core_session
                            .prompt_source_messages()
                            .iter()
                            .map(|message| {
                                serde_json::to_string(message).map_or(0, |json| json.len())
                            })
                            .sum::<usize>();
                        let conversation_tokens = approx_tokens_from_byte_count(prompt_bytes);
                        let compacted_prompt_token_estimate =
                            conversation_tokens.try_into().unwrap_or(usize::MAX);
                        core_session.prompt_token_estimate = compacted_prompt_token_estimate;
                        let window = runtime_session
                            .summary
                            .model
                            .as_deref()
                            .and_then(|slug| self.deps.model_catalog.get(slug))
                            .map(|model| u64::from(model.effective_context_window()))
                            .unwrap_or(0);
                        let occupancy = super::super::context_occupancy::occupancy_after_compaction(
                            window,
                            previous_occupancy.as_ref(),
                            conversation_tokens,
                            core_session.raw_context_breakdown,
                        );
                        (
                            core_session.total_input_tokens,
                            core_session.total_output_tokens,
                            core_session.total_tokens,
                            core_session.total_cache_creation_tokens,
                            core_session.total_cache_read_tokens,
                            compacted_prompt_token_estimate,
                            occupancy,
                        )
                    };
                    runtime_session.summary.total_input_tokens = compacted_total_input_tokens;
                    runtime_session.summary.total_output_tokens = compacted_total_output_tokens;
                    runtime_session.summary.total_tokens = compacted_total_tokens;
                    runtime_session.summary.total_cache_creation_tokens =
                        compacted_total_cache_creation_tokens;
                    runtime_session.summary.total_cache_read_tokens =
                        compacted_total_cache_read_tokens;
                    runtime_session.summary.prompt_token_estimate = compacted_prompt_token_estimate;
                    runtime_session.summary.last_query_total_tokens =
                        compacted_occupancy.total_tokens as usize;
                    runtime_session.summary.last_context_occupancy =
                        Some(compacted_occupancy.clone());
                }

                if !runtime_session.summary.ephemeral {
                    let stats = crate::db::SessionStats {
                        total_input_tokens: runtime_session.summary.total_input_tokens,
                        total_output_tokens: runtime_session.summary.total_output_tokens,
                        total_tokens: runtime_session.summary.total_tokens,
                        total_cache_creation_tokens: runtime_session
                            .summary
                            .total_cache_creation_tokens,
                        total_cache_read_tokens: runtime_session.summary.total_cache_read_tokens,
                        last_input_tokens: 0,
                        turn_count: runtime_session.summary.updated_at.timestamp() as usize,
                        prompt_token_estimate: runtime_session.summary.prompt_token_estimate,
                        last_context_occupancy: runtime_session
                            .summary
                            .last_context_occupancy
                            .clone(),
                    };
                    if let Err(err) = self.deps.db.update_stats(&session_id, &stats) {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %err,
                            "failed to persist compaction token stats to database"
                        );
                    }
                }

                let turn_id = turn.turn_id;
                let item_id = devo_core::ItemId::new();
                let item_seq = runtime_session.next_item_seq;
                runtime_session.loaded_item_count += 1;
                runtime_session.next_item_seq += 1;

                let payload = serde_json::json!({ "title": "Context Compaction" });
                self.broadcast_event(ServerEvent::ItemStarted(ItemEventPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: Some(item_id),
                        seq: item_seq,
                        item_seq: Some(item_seq),
                    },
                    item: ItemEnvelope {
                        item_id,
                        item_kind: ItemKind::ContextCompaction,
                        payload: payload.clone(),
                    },
                }))
                .await;

                self.broadcast_event(ServerEvent::ItemCompleted(ItemEventPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: Some(item_id),
                        seq: item_seq,
                        item_seq: Some(item_seq),
                    },
                    item: ItemEnvelope {
                        item_id,
                        item_kind: ItemKind::ContextCompaction,
                        payload,
                    },
                }))
                .await;

                let summary_turn_item = Self::summary_turn_item_from_compacted(&compacted_items);
                let compact_summary = match &summary_turn_item {
                    TurnItem::ContextCompaction(TextItem { text }) => text.clone(),
                    _ => String::new(),
                };
                if let Some(record) = runtime_session.record.clone() {
                    runtime_session.latest_compaction_snapshot =
                        Some(devo_core::CompactionSnapshotLine {
                            timestamp: Utc::now(),
                            session_id,
                            turn_id,
                            summary_item_id: item_id,
                            preserved_item_ids: preserved_item_ids.clone(),
                            context_occupancy: runtime_session
                                .summary
                                .last_context_occupancy
                                .clone(),
                        });
                    runtime_session.persisted_turn_items.push(
                        crate::execution::PersistedTurnItem {
                            turn_id,
                            turn_kind: devo_core::TurnKind::ManualCompaction,
                            item_id,
                            turn_item: summary_turn_item.clone(),
                        },
                    );

                    let item_record = crate::persistence::build_item_record(
                        session_id,
                        turn_id,
                        item_id,
                        item_seq,
                        summary_turn_item,
                        None,
                        None,
                    );
                    if let Err(error) = self.rollout_store.append_item(&record, item_record) {
                        tracing::warn!(
                            session_id = %session_id,
                            error = %error,
                            "failed to persist compaction summary item"
                        );
                    }
                    if let Some(snapshot) = runtime_session.latest_compaction_snapshot.clone() {
                        if let Err(error) = self
                            .rollout_store
                            .append_compaction_snapshot(&record, snapshot)
                        {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "failed to persist compaction snapshot"
                            );
                        }
                    } else {
                        tracing::warn!(
                            session_id = %session_id,
                            "compaction snapshot missing after summary item write"
                        );
                    }
                }

                let mut completed_turn = turn.clone();
                completed_turn.status = TurnStatus::Completed;
                completed_turn.completed_at = Some(Utc::now());
                runtime_session.active_turn = None;
                runtime_session.latest_turn = Some(completed_turn.clone());
                runtime_session.summary.status = SessionRuntimeStatus::Idle;
                let summary = runtime_session.summary.clone();
                session_handle
                    .replace_state(
                        crate::runtime::session_actor::SessionActorState::from_runtime_session(
                            runtime_session,
                        ),
                    )
                    .await;
                drop(state_change_guard);
                self.clear_active_turn_runtime_handles(session_id).await;
                if let Some(persistence) = session_handle.turn_persistence_snapshot().await
                    && persistence.record.is_some()
                    && let Err(error) = self
                        .persist_turn_line_deduped(session_id, &completed_turn)
                        .await
                {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %completed_turn.turn_id,
                        error = %error,
                        "failed to persist compaction turn completion"
                    );
                }
                self.run_session_hook(
                    session_id,
                    devo_core::HookEvent::PostCompact,
                    serde_json::Map::from_iter([
                        ("trigger".to_string(), serde_json::json!("manual")),
                        (
                            "compact_summary".to_string(),
                            serde_json::Value::String(compact_summary),
                        ),
                    ]),
                )
                .await;
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %completed_turn.turn_id,
                    "session compaction completed with replacement"
                );
                if let Some(occupancy) = summary.last_context_occupancy.clone() {
                    self.broadcast_event(ServerEvent::ContextUsageUpdated(
                        crate::ContextUsageUpdatedPayload {
                            session_id,
                            occupancy,
                        },
                    ))
                    .await;
                }
                self.broadcast_event(ServerEvent::SessionCompactionCompleted(
                    SessionEventPayload { session: summary },
                ))
                .await;
                self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn: completed_turn.clone(),
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
                    completed_turn.turn_id,
                    TerminalTurnSnapshot::from_turn(&completed_turn),
                )
                .await;
            }
            Ok(CompactAction::Skipped) => {
                drop(state_change_guard);
                tracing::info!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    "session compaction completed without replacement"
                );
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Skipped,
                )
                .await;
            }
            Err(error) => {
                drop(state_change_guard);
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn.turn_id,
                    error = %error,
                    "session compaction failed"
                );
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Failed {
                        message: format!("compaction failed: {error}"),
                    },
                )
                .await;
            }
        }
    }

    /// Terminalize a manual compaction turn when the task still owns `active_turn`.
    ///
    /// If interrupt already claimed the turn via `interrupt_active_turn`, this is a
    /// no-op so we do not double-emit terminal events.
    async fn finalize_manual_compaction_turn(
        self: &Arc<Self>,
        session_handle: &crate::runtime::session_actor::SessionHandle,
        session_id: SessionId,
        mut turn: TurnMetadata,
        outcome: CompactionTurnOutcome,
    ) {
        // Ensure interrupt abort cannot drop us between claim and event emit.
        self.detach_active_turn_abort(session_id).await;

        let now = Utc::now();
        turn.completed_at = Some(now);
        turn.status = match &outcome {
            CompactionTurnOutcome::Skipped => TurnStatus::Completed,
            CompactionTurnOutcome::Failed { .. } => TurnStatus::Failed,
            CompactionTurnOutcome::Canceled => TurnStatus::Interrupted,
        };

        // Atomic claim: interrupt may have already taken `active_turn`.
        if session_handle
            .clear_active_turn_if_matches(turn.turn_id)
            .await
            != Some(true)
        {
            return;
        }
        session_handle.set_session_idle(Some(turn.clone())).await;
        self.clear_active_turn_runtime_handles(session_id).await;

        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.record.is_some()
            && let Err(error) = self.persist_turn_line_deduped(session_id, &turn).await
        {
            tracing::warn!(
                session_id = %session_id,
                turn_id = %turn.turn_id,
                error = %error,
                "failed to persist compaction turn terminal line"
            );
        }

        match outcome {
            CompactionTurnOutcome::Skipped => {
                let Some(summary) = session_handle.summary().await else {
                    tracing::warn!(
                        session_id = %session_id,
                        turn_id = %turn.turn_id,
                        "compaction skipped but session summary unavailable"
                    );
                    self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                        session_id,
                        turn: turn.clone(),
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
                        turn.turn_id,
                        TerminalTurnSnapshot::from_turn(&turn),
                    )
                    .await;
                    return;
                };
                if let Some(occupancy) = summary.last_context_occupancy.clone() {
                    self.broadcast_event(ServerEvent::ContextUsageUpdated(
                        crate::ContextUsageUpdatedPayload {
                            session_id,
                            occupancy,
                        },
                    ))
                    .await;
                }
                self.broadcast_event(ServerEvent::SessionCompactionCompleted(
                    SessionEventPayload { session: summary },
                ))
                .await;
                self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn: turn.clone(),
                }))
                .await;
            }
            CompactionTurnOutcome::Failed { message } => {
                self.broadcast_event(ServerEvent::SessionCompactionFailed(
                    SessionCompactionFailedPayload {
                        session_id,
                        message,
                    },
                ))
                .await;
                self.broadcast_event(ServerEvent::TurnFailed(TurnFailedPayload {
                    session_id,
                    turn: turn.clone(),
                    error: None,
                }))
                .await;
                self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn: turn.clone(),
                }))
                .await;
            }
            CompactionTurnOutcome::Canceled => {
                self.broadcast_event(ServerEvent::SessionCompactionFailed(
                    SessionCompactionFailedPayload {
                        session_id,
                        message: "compaction canceled".to_string(),
                    },
                ))
                .await;
                self.broadcast_event(ServerEvent::TurnInterrupted(TurnEventPayload {
                    session_id,
                    turn: turn.clone(),
                }))
                .await;
                self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn: turn.clone(),
                }))
                .await;
            }
        }

        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        self.record_terminal_turn_status(turn.turn_id, TerminalTurnSnapshot::from_turn(&turn))
            .await;
    }

    fn preserved_item_ids_from_compacted(
        persisted_turn_items: &[crate::execution::PersistedTurnItem],
        compacted_items: &[ResponseItem],
    ) -> Vec<ItemId> {
        let mut normalized_persisted_items = Vec::new();
        for item in persisted_turn_items {
            if !crate::persistence::prompt_visible_persisted_turn_item(item) {
                continue;
            }

            // The compactor returns a summary followed by the prompt-visible suffix it
            // kept verbatim. Normalize persisted items into that same response shape
            // without allocating a short intermediate Vec for every journal item.
            match &item.turn_item {
                TurnItem::UserMessage(TextItem { text })
                | TurnItem::SteerInput(TextItem { text }) => {
                    normalized_persisted_items.push((
                        item.item_id,
                        ResponseItem::Message(Message::user(text.clone())),
                    ));
                }
                TurnItem::AgentMessage(TextItem { text })
                | TurnItem::Plan(TextItem { text })
                | TurnItem::WebSearch(TextItem { text })
                | TurnItem::ImageGeneration(TextItem { text })
                | TurnItem::ContextCompaction(TextItem { text })
                | TurnItem::HookPrompt(TextItem { text }) => {
                    normalized_persisted_items.push((
                        item.item_id,
                        ResponseItem::Message(Message::assistant_text(text.clone())),
                    ));
                }
                TurnItem::Reasoning(_) => {}
                TurnItem::ToolCall(ToolCallItem {
                    tool_call_id,
                    tool_name,
                    input,
                }) => {
                    normalized_persisted_items.push((
                        item.item_id,
                        ResponseItem::ToolCall {
                            id: tool_call_id.clone(),
                            name: tool_name.clone(),
                            input: input.clone(),
                        },
                    ));
                }
                TurnItem::ToolResult(ToolResultItem {
                    tool_call_id,
                    output,
                    is_error,
                    ..
                }) => {
                    normalized_persisted_items.push((
                        item.item_id,
                        ResponseItem::ToolCallOutput {
                            tool_use_id: tool_call_id.clone(),
                            content: match output {
                                serde_json::Value::String(text) => text.clone(),
                                other => other.to_string(),
                            },
                            is_error: *is_error,
                        },
                    ));
                }
                TurnItem::CommandExecution(CommandExecutionItem {
                    tool_call_id,
                    tool_name,
                    input,
                    output,
                    is_error,
                    ..
                }) => {
                    normalized_persisted_items.push((
                        item.item_id,
                        ResponseItem::ToolCall {
                            id: tool_call_id.clone(),
                            name: tool_name.clone(),
                            input: input.clone(),
                        },
                    ));
                    normalized_persisted_items.push((
                        item.item_id,
                        ResponseItem::ToolCallOutput {
                            tool_use_id: tool_call_id.clone(),
                            content: match output {
                                serde_json::Value::String(text) => text.clone(),
                                other => other.to_string(),
                            },
                            is_error: *is_error,
                        },
                    ));
                }
                TurnItem::ToolProgress(_)
                | TurnItem::ApprovalRequest(_)
                | TurnItem::ApprovalDecision(_)
                | TurnItem::TurnSummary(_) => {}
            }
        }
        let preserved = compacted_items.get(1..).unwrap_or(&[]);
        if preserved.is_empty() {
            return Vec::new();
        }
        let preserved_len = preserved.len();
        if normalized_persisted_items.len() < preserved_len {
            return Vec::new();
        }
        let suffix =
            &normalized_persisted_items[normalized_persisted_items.len() - preserved_len..];
        if suffix.iter().map(|(_, item)| item).eq(preserved.iter()) {
            suffix.iter().map(|(item_id, _)| *item_id).collect()
        } else {
            Vec::new()
        }
    }

    fn summary_turn_item_from_compacted(compacted_items: &[ResponseItem]) -> TurnItem {
        let summary_text = compacted_items
            .first()
            .and_then(|item| match item {
                ResponseItem::Message(message) => {
                    message.content.iter().find_map(|block| match block {
                        devo_core::ContentBlock::Text { text } => Some(text.clone()),
                        devo_core::ContentBlock::Reasoning { .. }
                        | devo_core::ContentBlock::ProviderReasoning { .. }
                        | devo_core::ContentBlock::ToolUse { .. }
                        | devo_core::ContentBlock::HostedToolUse { .. }
                        | devo_core::ContentBlock::ToolResult { .. } => None,
                    })
                }
                ResponseItem::Reason { text } => Some(text.clone()),
                ResponseItem::ToolCall { .. } | ResponseItem::ToolCallOutput { .. } => None,
            })
            .unwrap_or_default();
        TurnItem::ContextCompaction(TextItem { text: summary_text })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn preserved_item_ids_match_complete_command_execution_pair() {
        let command_item_id = ItemId::new();
        let command_input = serde_json::json!({ "cmd": "printf ok" });
        let command_output = serde_json::Value::String("ok".to_string());
        let persisted_turn_items = vec![crate::execution::PersistedTurnItem {
            turn_id: TurnId::new(),
            turn_kind: devo_core::TurnKind::Regular,
            item_id: command_item_id,
            turn_item: TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: "call-1".to_string(),
                tool_name: "exec_command".to_string(),
                command: "printf ok".to_string(),
                input: command_input.clone(),
                output: command_output.clone(),
                is_error: false,
            }),
        }];
        let compacted_items = vec![
            ResponseItem::Message(Message::assistant_text("summary")),
            ResponseItem::ToolCall {
                id: "call-1".to_string(),
                name: "exec_command".to_string(),
                input: command_input,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "call-1".to_string(),
                content: "ok".to_string(),
                is_error: false,
            },
        ];

        assert_eq!(
            ServerRuntime::preserved_item_ids_from_compacted(
                &persisted_turn_items,
                &compacted_items
            ),
            vec![command_item_id, command_item_id]
        );
    }
}
