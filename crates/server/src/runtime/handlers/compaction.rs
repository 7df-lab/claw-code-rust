use super::super::*;

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

        let session_handle = match self.session(params.session_id).await {
            Some(session) => session,
            None => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                );
            }
        };

        let Some(summary) = session_handle.summary().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let runtime = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(panic) =
                AssertUnwindSafe(runtime.run_session_compaction(params.session_id, session_handle))
                    .catch_unwind()
                    .await
            {
                tracing::error!(
                    session_id = %params.session_id,
                    panic = ?panic,
                    "session compaction task panicked"
                );
            }
        });
        tracing::info!(session_id = %params.session_id, "accepted async session compaction request");

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionCompactResult { session: summary },
        })
        .expect("serialize session/compact response")
    }

    pub(crate) async fn run_session_compaction(
        self: Arc<Self>,
        session_id: SessionId,
        session_handle: crate::runtime::session_actor::SessionHandle,
    ) {
        tracing::info!(session_id = %session_id, "session compaction task started");
        let Some(started_summary) = session_handle.summary().await else {
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

        let result = {
            let Some(runtime_session) = session_handle.export_runtime_session().await else {
                tracing::warn!(session_id = %session_id, "session compaction failed: session unavailable");
                self.broadcast_event(ServerEvent::SessionCompactionFailed(
                    SessionCompactionFailedPayload {
                        session_id,
                        message: "compaction failed: session unavailable".to_string(),
                    },
                ))
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
                model = %model_slug,
                request_model = %request_model,
                item_count = items.len(),
                input_tokens = token_info.input_tokens,
                cached_input_tokens = token_info.cached_input_tokens,
                output_tokens = token_info.output_tokens,
                "starting compaction summarization"
            );
            let summarizer = DefaultHistorySummarizer::with_models(
                runtime_session
                    .runtime_context
                    .provider_for_route(turn_config.provider_route.clone()),
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

            compact_history(&items, &token_info, &summarizer, &config).await
        };

        match result {
            Ok(CompactAction::Replaced(compacted_items)) => {
                let Some(mut runtime_session) = session_handle.export_runtime_session().await
                else {
                    return;
                };
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
                    ) = {
                        let mut core_session = runtime_session.core_session.lock().await;
                        core_session.set_prompt_messages(new_messages);
                        let compacted_prompt_token_estimate = core_session
                            .prompt_source_messages()
                            .iter()
                            .map(|message| {
                                serde_json::to_string(message).map_or(0, |json| json.len())
                            })
                            .sum::<usize>()
                            .div_ceil(4);
                        core_session.prompt_token_estimate = compacted_prompt_token_estimate;
                        (
                            core_session.total_input_tokens,
                            core_session.total_output_tokens,
                            core_session.total_tokens,
                            core_session.total_cache_creation_tokens,
                            core_session.total_cache_read_tokens,
                            compacted_prompt_token_estimate,
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
                }

                if let Some(turn_id) = runtime_session
                    .latest_turn
                    .as_ref()
                    .map(|t| t.turn_id)
                    .or_else(|| runtime_session.active_turn.as_ref().map(|t| t.turn_id))
                {
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

                    let summary_turn_item =
                        Self::summary_turn_item_from_compacted(&compacted_items);
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
                        if let Err(error) = self.rollout_store.append_compaction_snapshot(
                            &record,
                            runtime_session
                                .latest_compaction_snapshot
                                .clone()
                                .expect("compaction snapshot should be set"),
                        ) {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "failed to persist compaction snapshot"
                            );
                        }
                    }
                    let summary = runtime_session.summary.clone();
                    session_handle
                        .replace_state(
                            crate::runtime::session_actor::SessionActorState::from_runtime_session(
                                runtime_session,
                            ),
                        )
                        .await;
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
                    tracing::info!(session_id = %session_id, "session compaction completed with replacement");
                    self.broadcast_event(ServerEvent::SessionCompactionCompleted(
                        SessionEventPayload { session: summary },
                    ))
                    .await;
                    return;
                }

                let summary = runtime_session.summary.clone();
                session_handle
                    .replace_state(
                        crate::runtime::session_actor::SessionActorState::from_runtime_session(
                            runtime_session,
                        ),
                    )
                    .await;
                tracing::info!(session_id = %session_id, "session compaction completed with replacement");
                self.broadcast_event(ServerEvent::SessionCompactionCompleted(
                    SessionEventPayload { session: summary },
                ))
                .await;
            }
            Ok(CompactAction::Skipped) => {
                let Some(summary) = session_handle.summary().await else {
                    return;
                };
                tracing::info!(session_id = %session_id, "session compaction completed without replacement");
                self.broadcast_event(ServerEvent::SessionCompactionCompleted(
                    SessionEventPayload { session: summary },
                ))
                .await;
            }
            Err(error) => {
                tracing::warn!(session_id = %session_id, error = %error, "session compaction failed");
                self.broadcast_event(ServerEvent::SessionCompactionFailed(
                    SessionCompactionFailedPayload {
                        session_id,
                        message: format!("compaction failed: {error}"),
                    },
                ))
                .await;
            }
        }
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
