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

struct SessionCompactRequest {
    session_id: SessionId,
}

impl ServerRuntime {
    /// Native `session/compact/start` (L2-DES-APP-008 Phase B): lean
    /// params and a canonical turn snapshot result, produced by translating
    /// into the legacy flow and projecting the admitted compaction turn.
    pub(crate) async fn handle_native_session_compact_start(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionCompactStartParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/compact/start params: {error}"),
                    );
                }
            };
        let Ok(legacy_session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server",
            );
        };
        let response = self
            .handle_session_compact_translated(
                request_id.clone(),
                SessionCompactRequest {
                    session_id: legacy_session_id,
                },
            )
            .await;
        let Ok(success) =
            serde_json::from_value::<SuccessResponse<TurnStartResult>>(response.clone())
        else {
            return response;
        };
        let TurnStartResult::Started { turn_id, .. } = success.result else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot compact while a turn is active or queued",
            );
        };
        // `spawn_active_turn_task` has already registered runtime metadata.
        // Compaction may not yet have a stream/spawn snapshot, so the mailbox
        // reservation can miss the active turn. Read the registry the same
        // way native `turn/start` does.
        let Some(metadata) = self
            .active_turns
            .active_turn_metadata(legacy_session_id)
            .await
            .filter(|turn| turn.turn_id == turn_id)
        else {
            return response;
        };
        let turn = devo_protocol::native::wire_projector::native_turn_from_metadata(&metadata);
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_turn::TurnStartResult { turn },
        })
        .expect("serialize canonical session/compact/start response")
    }

    async fn handle_session_compact_translated(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: SessionCompactRequest,
    ) -> serde_json::Value {
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        // Busy rejection must not wait on the session actor: turns execute
        // inline on the actor, so a mailbox round-trip here would deadlock
        // while a turn is running. `runtime_active_turn_id` reads the runtime
        // turn cache only; the mailbox-based `try_begin_active_turn` below
        // stays the authoritative admission check once the session is idle.
        if self
            .runtime_active_turn_id(params.session_id)
            .await
            .is_some()
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot compact while a turn is active or queued",
            );
        }

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
        if let Some(spawn) = session_handle.spawn_snapshot().await {
            self.register_turn_spawn_snapshot(session_id, turn.turn_id, Arc::new(spawn))
                .await;
        }
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
                            /*compaction_item_id*/ None,
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
                /*compaction_item_id*/ None,
            )
            .await;
            return;
        };
        self.broadcast_event(ServerEvent::SessionCompactionStarted(
            devo_protocol::SessionCompactionStartedPayload {
                session: started_summary,
                turn_id: turn.turn_id,
                trigger: devo_protocol::native::item::CompactionTrigger::Manual,
            },
        ))
        .await;
        // Surface "Compacting context" in the Desktop transcript as soon as
        // manual compaction begins — not only after summarization finishes.
        let compaction_item_id = devo_core::ItemId::new();
        self.broadcast_event(super::super::turn_exec::manual_compaction_started_event(
            session_id,
            turn.turn_id,
            compaction_item_id,
            /*item_seq*/ None,
        ))
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

        // Snapshot under the gate, then release it before the model call so
        // admission / queue / metadata RPCs stay responsive (L2-DES-SERVER-002).
        let (items, token_info, model_slug, request_model, max_tokens, provider_route, budget) = {
            let _state_change_guard = session_handle.lock_state_change().await;
            let Some(runtime_session) = session_handle.export_runtime_session().await else {
                tracing::warn!(session_id = %session_id, "session compaction failed: session unavailable");
                self.finalize_manual_compaction_turn(
                    &session_handle,
                    session_id,
                    turn,
                    CompactionTurnOutcome::Failed {
                        message: "compaction failed: session unavailable".to_string(),
                    },
                    Some(compaction_item_id),
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
            let budget = core_session.config.token_budget.clone();
            let provider_route = turn_config.provider_route.clone();
            drop(core_session);
            drop(runtime_session);
            (
                items,
                token_info,
                model_slug,
                request_model,
                max_tokens,
                provider_route,
                budget,
            )
        };

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
            {
                // Resolve provider without holding the session gate.
                let Some(runtime_session) = session_handle.export_runtime_session().await else {
                    self.finalize_manual_compaction_turn(
                        &session_handle,
                        session_id,
                        turn,
                        CompactionTurnOutcome::Failed {
                            message: "compaction failed: session unavailable".to_string(),
                        },
                        Some(compaction_item_id),
                    )
                    .await;
                    return;
                };
                runtime_session
                    .runtime_context
                    .provider_for_route(provider_route)
            },
            session_id,
            Some(turn.turn_id),
            devo_protocol::native::usage::UsagePurpose::Compaction,
        );
        let summarizer =
            DefaultHistorySummarizer::with_models(provider, model_slug, request_model, max_tokens);

        let config = CompactionConfig {
            budget,
            // Proactive: user-requested /compact; preserve latest user suffix.
            kind: CompactionKind::Proactive,
        };

        let result = compact_history(
            &items,
            &token_info,
            &summarizer,
            &config,
            Some(&cancel_token),
        )
        .await;

        // Summarize is done: detach abort so interrupt cannot kill mid-terminalize.
        // Cancel token still works for any remaining cooperative checks.
        self.detach_active_turn_abort(session_id).await;

        // Apply under the gate so replace_state cannot race admission/edit.
        let state_change_guard = session_handle.lock_state_change().await;

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
                    Some(compaction_item_id),
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
                        Some(compaction_item_id),
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
                        Some(compaction_item_id),
                    )
                    .await;
                    return;
                };
                // A failed write must leave the previous prompt installed.
                if let Some(record) = runtime_session.record.clone() {
                    let persist = CompactionSummaryPersist {
                        session_id,
                        turn_id: turn.turn_id,
                        summary_item_id: compaction_item_id,
                        item_seq: runtime_session.next_item_seq,
                        summary_turn_item: summary_turn_item_from_compacted(&compacted_items),
                        snapshot: build_compaction_snapshot_line(
                            session_id,
                            turn.turn_id,
                            compaction_item_id,
                            preserved_item_ids_from_compacted(
                                &runtime_session.persisted_turn_items,
                                &compacted_items,
                            ),
                            runtime_session.summary.last_context_occupancy.clone(),
                        ),
                    };
                    let runtime = Arc::clone(&self);
                    let committed = tokio::task::spawn_blocking(move || {
                        append_compaction_summary_and_snapshot(
                            &runtime.rollout_store,
                            &record,
                            persist,
                        )
                    })
                    .await;
                    if let Err(error) = committed.unwrap_or_else(|error| Err(error.into())) {
                        drop(state_change_guard);
                        self.finalize_manual_compaction_turn(
                            &session_handle,
                            session_id,
                            turn,
                            CompactionTurnOutcome::Failed {
                                message: format!("compaction persistence failed: {error}"),
                            },
                            Some(compaction_item_id),
                        )
                        .await;
                        return;
                    }
                }
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
                let preserved_item_ids = preserved_item_ids_from_compacted(
                    &runtime_session.persisted_turn_items,
                    &compacted_items,
                );
                let new_messages = devo_core::history::response_items_to_messages(&compacted_items);
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
                        let global = self
                            .deps
                            .config_store
                            .lock()
                            .expect("app config store mutex should not be poisoned")
                            .effective_config()
                            .compaction_token_limit;
                        let model = runtime_session
                            .summary
                            .model
                            .as_deref()
                            .and_then(|slug| {
                                runtime_session
                                    .runtime_context
                                    .model_catalog
                                    .get(slug)
                                    .or_else(|| self.deps.model_catalog.get(slug))
                            })
                            .or_else(|| {
                                runtime_session
                                    .summary
                                    .model_binding_id
                                    .as_deref()
                                    .and_then(|binding| {
                                        runtime_session
                                            .runtime_context
                                            .model_catalog
                                            .get(binding)
                                            .or_else(|| self.deps.model_catalog.get(binding))
                                    })
                            });
                        let window = runtime_session
                            .summary
                            .effective_context_window
                            .or_else(|| {
                                model.map(|model| {
                                    super::super::context_occupancy::resolved_compaction_limit(
                                        global, model,
                                    )
                                })
                            })
                            .unwrap_or(0);
                        let occupancy = super::super::context_occupancy::occupancy_after_compaction(
                            window,
                            previous_occupancy.as_ref(),
                            conversation_tokens,
                            core_session.raw_context_breakdown,
                        );
                        // Keep auto-compact pressure on the post-compact tip so
                        // resume / next query do not re-trigger from pre-compact
                        // latest-query totals.
                        core_session.last_turn_tokens = occupancy.total_tokens as usize;
                        core_session.last_input_tokens = compacted_prompt_token_estimate;
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
                        last_input_tokens: runtime_session.summary.prompt_token_estimate,
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
                let item_id = compaction_item_id;
                let item_seq = runtime_session.next_item_seq;
                runtime_session.loaded_item_count += 1;
                runtime_session.next_item_seq += 1;

                self.broadcast_event(super::super::turn_exec::manual_compaction_completed_event(
                    session_id, turn_id, item_id, item_seq,
                ))
                .await;

                let summary_turn_item = summary_turn_item_from_compacted(&compacted_items);
                let compact_summary = match &summary_turn_item {
                    TurnItem::ContextCompaction(TextItem { text }) => text.clone(),
                    _ => String::new(),
                };
                if runtime_session.record.is_some() {
                    let snapshot = build_compaction_snapshot_line(
                        session_id,
                        turn_id,
                        item_id,
                        preserved_item_ids.clone(),
                        runtime_session.summary.last_context_occupancy.clone(),
                    );
                    runtime_session.latest_compaction_snapshot = Some(snapshot.clone());
                    runtime_session
                        .persisted_turn_items
                        .push(compaction_persisted_turn_item(
                            turn_id,
                            devo_core::TurnKind::ManualCompaction,
                            item_id,
                            summary_turn_item.clone(),
                        ));
                    if let Some(history_item) =
                        crate::projection::history_item_from_turn_item(&summary_turn_item)
                    {
                        runtime_session.history_items.push(history_item);
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
                    devo_protocol::SessionCompactionCompletedPayload {
                        session: summary,
                        turn_id: completed_turn.turn_id,
                        item_id: Some(item_id),
                    },
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
                    Some(compaction_item_id),
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
                    Some(compaction_item_id),
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
        compaction_item_id: Option<ItemId>,
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

        // Close the early-emitted started item so Desktop does not leave a
        // dangling "Compacting context" divider when compact does not replace.
        if let Some(item_id) = compaction_item_id {
            match &outcome {
                CompactionTurnOutcome::Skipped => {
                    self.broadcast_event(
                        super::super::turn_exec::manual_compaction_completed_event(
                            session_id,
                            turn.turn_id,
                            item_id,
                            /*item_seq*/ 0,
                        ),
                    )
                    .await;
                }
                CompactionTurnOutcome::Failed { message } => {
                    self.broadcast_event(
                        super::super::turn_exec::manual_compaction_item_failed_event(
                            session_id,
                            turn.turn_id,
                            item_id,
                            message.clone(),
                        ),
                    )
                    .await;
                }
                CompactionTurnOutcome::Canceled => {
                    self.broadcast_event(
                        super::super::turn_exec::manual_compaction_item_failed_event(
                            session_id,
                            turn.turn_id,
                            item_id,
                            "compaction canceled".to_string(),
                        ),
                    )
                    .await;
                }
            }
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
                    devo_protocol::SessionCompactionCompletedPayload {
                        session: summary,
                        turn_id: turn.turn_id,
                        item_id: compaction_item_id,
                    },
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
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_core::CommandExecutionItem;

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
            preserved_item_ids_from_compacted(&persisted_turn_items, &compacted_items),
            vec![command_item_id, command_item_id]
        );
    }
}
