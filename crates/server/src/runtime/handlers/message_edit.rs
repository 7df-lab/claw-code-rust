use super::super::*;
use super::message_edit_restore::{
    apply_safe_workspace_restore, candidate_files, core_restore_policy,
    discover_restore_candidates, restore_completed_payload, restore_started_payload,
};

impl ServerRuntime {
    pub(crate) async fn handle_message_edit_previous(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: crate::MessageEditPreviousParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid message/editPrevious params: {error}"),
                );
            }
        };
        let edited_input = match params
            .edited_content_parts
            .iter()
            .cloned()
            .map(serde_json::from_value::<crate::InputItem>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(input) => input,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidContentParts,
                    format!("invalid message/editPrevious edited content: {error}"),
                );
            }
        };
        let Some(display_input) = render_input_items(&edited_input) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidContentParts,
                "message/editPrevious edited content is empty",
            );
        };

        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(hook_context) = session_handle.hook_context_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let workspace_root = hook_context.summary.cwd.clone();
        let runtime_context = hook_context.runtime_context;
        let Some(resolved_input) = (match runtime_context
            .resolve_input_items(&edited_input, Some(workspace_root.as_path()))
        {
            Ok(resolved_input) => resolved_input,
            Err(error) => {
                let code = match error {
                    devo_core::SkillError::SkillNotFound { .. }
                    | devo_core::SkillError::AmbiguousSkillName { .. }
                    | devo_core::SkillError::SkillDisabled { .. } => {
                        ProtocolErrorCode::InvalidParams
                    }
                    devo_core::SkillError::SkillParseFailed { .. }
                    | devo_core::SkillError::SkillRootUnavailable { .. }
                    | devo_core::SkillError::DuplicateSkillId { .. } => {
                        ProtocolErrorCode::InternalError
                    }
                };
                return self.error_response(
                    request_id,
                    code,
                    format!("failed to resolve message/editPrevious input: {error}"),
                );
            }
        }) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidContentParts,
                "message/editPrevious edited content is empty",
            );
        };
        let prompt_hook_report = self
            .run_session_hook(
                params.session_id,
                devo_core::HookEvent::UserPromptSubmit,
                serde_json::Map::from_iter([(
                    "prompt".to_string(),
                    serde_json::Value::String(resolved_input.prompt_text.clone()),
                )]),
            )
            .await;
        if let Some(reason) = prompt_hook_report.first_blocking_reason() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("prompt blocked by hook: {reason}"),
            );
        }
        let edited_mentions = match params
            .edited_mentions
            .iter()
            .cloned()
            .map(serde_json::from_value::<devo_core::Mention>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(mentions) => mentions,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidMentions,
                    format!("invalid message/editPrevious mentions: {error}"),
                );
            }
        };
        if params.edit_mode != devo_protocol::EditMode::Normal {
            return self.error_response(
                request_id,
                ProtocolErrorCode::WorkspaceRestoreFailedToStart,
                "message/editPrevious queued-only edits are not implemented",
            );
        }
        let Some(session) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if session.active_turn.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnEditRejected,
                "cannot edit the previous message while a turn is active",
            );
        }

        let expected_target_message_id = params
            .expected_target_message_id
            .or(params.target_message_id);
        let Some(target) = session
            .persisted_turn_items
            .iter()
            .rev()
            .find(|item| matches!(item.turn_item, TurnItem::UserMessage(_)))
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::OlderMessageRequiresFork,
                "no immediately previous user message is available to edit",
            );
        };
        if let Some(expected) = expected_target_message_id
            && expected != target.item_id
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTargetMessageMismatch,
                "expected target message does not match the current editable message",
            );
        }
        let requested_restore_policy = params
            .workspace_restore_policy
            .unwrap_or(crate::MessageEditWorkspaceRestorePolicy::Safe);
        let workspace_restore_policy = core_restore_policy(requested_restore_policy);
        let Some(record) = session.record.clone() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "message/editPrevious requires a durable session",
            );
        };
        let target_message_id = target.item_id;
        let target_turn_id = target.turn_id;
        let target_turn_items = session
            .persisted_turn_items
            .iter()
            .filter(|item| item.turn_id == target_turn_id)
            .cloned()
            .collect::<Vec<_>>();
        let sequence = session
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.sequence + 1);
        let requested_model =
            requested_model_selection(None, None, &session.summary).map(str::to_string);
        let requested_reasoning_effort_selection =
            session.summary.reasoning_effort_selection.clone();
        let runtime_context = Arc::clone(&session.runtime_context);
        let collaboration_mode = {
            let core_session = session.core_session.lock().await;
            core_session.collaboration_mode
        };
        drop(session);

        let turn_config = runtime_context.resolve_turn_config(
            requested_model.as_deref(),
            requested_reasoning_effort_selection.clone(),
        );
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let replacement_message_id = ItemId::new();
        let records = devo_core::create_edit_records(
            params.session_id,
            target_message_id,
            Some(target_turn_id),
            replacement_message_id,
            vec![devo_core::ContentPart::Text(display_input.clone())],
            edited_mentions,
            workspace_restore_policy,
        );
        let [first_record, second_record]: [devo_core::DurableRecord; 2] = match records.try_into()
        {
            Ok(records) => records,
            Err(_) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "message/editPrevious did not create the expected durable records",
                );
            }
        };
        let (
            devo_core::DurableRecord::MessageEditRecorded(mut edit_record),
            devo_core::DurableRecord::TurnSuperseded(mut superseded_record),
        ) = (first_record, second_record)
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "message/editPrevious created unexpected durable records",
            );
        };
        edit_record.requested_by_client_id = params.client_edit_id.clone();
        let restore_plan = if workspace_restore_policy == devo_core::WorkspaceRestorePolicy::Skip {
            None
        } else {
            let restore_candidates =
                discover_restore_candidates(&target_turn_items, target_turn_id);
            let restore_candidate_files = candidate_files(&restore_candidates);
            let (restore_record, restore_id) = devo_core::plan_workspace_restore(
                params.session_id,
                target_turn_id,
                restore_candidate_files,
                workspace_restore_policy,
            );
            let devo_core::DurableRecord::TurnWorkspaceRestoreStarted(restore_started_record) =
                restore_record
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "message/editPrevious created unexpected workspace restore record",
                );
            };
            superseded_record.restore_id = Some(restore_id);
            Some((restore_started_record, restore_candidates))
        };
        let replacement_turn_id = superseded_record.replacement_turn_id;
        let now = Utc::now();
        let replacement_turn = TurnMetadata {
            turn_id: replacement_turn_id,
            session_id: params.session_id,
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
        if let Err(error) = self
            .rollout_store
            .append_message_edit_recorded(&record, edit_record.clone())
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist message edit record: {error}"),
            );
        }
        if let Err(error) = self
            .rollout_store
            .append_turn_superseded(&record, superseded_record.clone())
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist turn superseded record: {error}"),
            );
        }
        let restore_event_payloads = if let Some((restore_started_record, restore_candidates)) =
            restore_plan
        {
            if let Err(error) = self
                .rollout_store
                .append_workspace_restore_started(&record, restore_started_record.clone())
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::WorkspaceRestoreFailedToStart,
                    format!("failed to persist workspace restore start: {error}"),
                );
            }
            let restore_outcomes =
                apply_safe_workspace_restore(&workspace_root, &restore_candidates).await;
            let restore_completed_record = devo_core::complete_workspace_restore(
                params.session_id,
                restore_started_record.restore_id,
                restore_outcomes,
            );
            let devo_core::DurableRecord::TurnWorkspaceRestoreCompleted(restore_completed_record) =
                restore_completed_record
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "message/editPrevious created unexpected workspace restore completion",
                );
            };
            if let Err(error) = self
                .rollout_store
                .append_workspace_restore_completed(&record, restore_completed_record.clone())
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist workspace restore completion: {error}"),
                );
            }
            Some((
                restore_started_payload(
                    &restore_started_record,
                    &edit_record.edit_id.0.to_string(),
                ),
                restore_completed_payload(
                    &restore_completed_record,
                    &edit_record.edit_id.0.to_string(),
                    superseded_record.superseded_turn_id,
                ),
            ))
        } else {
            None
        };
        if let Err(error) = self
            .persist_turn_line_deduped(params.session_id, &replacement_turn)
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist replacement turn start: {error}"),
            );
        }
        let replacement_item_seq = self.allocate_item_sequence(params.session_id).await;
        let replacement_item = TurnItem::UserMessage(TextItem {
            text: display_input.clone(),
        });
        if let Err(error) = self.rollout_store.append_item(
            &record,
            build_item_record(
                params.session_id,
                replacement_turn_id,
                replacement_message_id,
                replacement_item_seq,
                replacement_item.clone(),
                Some(TurnStatus::Running),
                None,
            ),
        ) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist replacement message item: {error}"),
            );
        }

        let Some(mut session) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        session
            .persisted_turn_items
            .retain(|item| item.turn_id != target_turn_id);
        let mut rebuilt_messages = Vec::new();
        let mut rebuilt_history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &session.persisted_turn_items {
            crate::persistence::apply_turn_item(
                &mut rebuilt_messages,
                &mut rebuilt_history_items,
                &mut tool_names_by_id,
                &item.turn_kind,
                item.turn_item.clone(),
            );
        }
        if let Some(history_item) = history_item_from_turn_item(&replacement_item) {
            rebuilt_history_items.push(history_item);
        }
        session.history_items = rebuilt_history_items;
        session
            .persisted_turn_items
            .push(crate::execution::PersistedTurnItem {
                turn_id: replacement_turn_id,
                turn_kind: replacement_turn.kind.clone(),
                item_id: replacement_message_id,
                turn_item: replacement_item.clone(),
            });
        let branch_turn_count = session
            .persisted_turn_items
            .iter()
            .filter(|item| matches!(item.turn_item, TurnItem::UserMessage(_)))
            .count()
            .saturating_sub(1);
        session.latest_compaction_snapshot = None;
        session.summary.status = SessionRuntimeStatus::ActiveTurn;
        session.summary.updated_at = now;
        session.summary.last_activity_at = now;
        session.active_turn = Some(replacement_turn.clone());
        {
            let mut core_session = session.core_session.lock().await;
            core_session.messages = rebuilt_messages;
            core_session.prompt_messages = None;
            core_session.turn_count = branch_turn_count;
            if resolved_input.prompt_messages.is_empty() {
                core_session.push_message(Message::user(resolved_input.prompt_text.clone()));
            } else {
                for prompt_message in &resolved_input.prompt_messages {
                    core_session.push_message(Message::user(prompt_message.clone()));
                }
            }
        }
        session_handle
            .replace_state(
                crate::runtime::session_actor::SessionActorState::from_runtime_session(session),
            )
            .await;

        let runtime = Arc::clone(self);
        let replacement_turn_for_task = replacement_turn.clone();
        let turn_config_for_task = turn_config.clone();
        let display_input_for_task = display_input.clone();
        let input_for_task = resolved_input.prompt_text.clone();
        let input_messages_for_task = resolved_input.prompt_messages.clone();
        let session_id = params.session_id;
        let replacement_goal = {
            let stores = self.goal_stores.lock().await;
            stores
                .get(&params.session_id)
                .and_then(GoalStore::get)
                .map(Goal::to_thread_goal)
                .unwrap_or(devo_protocol::ThreadGoal {
                    thread_id: session_id,
                    objective: "message edit replacement".to_string(),
                    status: devo_protocol::ThreadGoalStatus::Complete,
                    token_budget: None,
                    tokens_used: 0,
                    time_used_seconds: 0,
                    created_at: now.timestamp(),
                    updated_at: now.timestamp(),
                })
        };
        self.spawn_active_turn_task(
            params.session_id,
            replacement_turn.clone(),
            None,
            async move {
                runtime
                    .execute_turn(ExecuteTurnRequest {
                        session_id,
                        turn: replacement_turn_for_task,
                        turn_config: turn_config_for_task,
                        display_input: display_input_for_task,
                        input: input_for_task,
                        input_messages: input_messages_for_task,
                        collaboration_mode,
                        input_mode: TurnInputMode::HiddenGoalContinuation {
                            goal: replacement_goal,
                        },
                    })
                    .await;
            },
        )
        .await;
        self.broadcast_event(ServerEvent::MessageEditRecorded(
            crate::MessageEditRecordedPayload {
                session_id: params.session_id,
                edit_id: edit_record.edit_id.0.to_string(),
                target_message_id,
                replacement_message_id,
                edit_state: "accepted".to_string(),
                content_preview: display_input.clone(),
                mentions: params.edited_mentions.clone(),
                timestamp: edit_record.created_at,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnSuperseded(crate::TurnSupersededPayload {
            session_id: params.session_id,
            superseded_turn_id: superseded_record.superseded_turn_id,
            replacement_turn_id,
            edit_id: superseded_record.edit_id.0.to_string(),
            reason: superseded_record.reason.clone(),
            timestamp: superseded_record.created_at,
        }))
        .await;
        if let Some((started_payload, completed_payload)) = restore_event_payloads {
            self.broadcast_event(ServerEvent::WorkspaceRestoreStarted(started_payload))
                .await;
            self.broadcast_event(ServerEvent::WorkspaceRestoreCompleted(completed_payload))
                .await;
        }
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: params.session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id: params.session_id,
            turn: replacement_turn,
        }))
        .await;
        self.emit_item_started(
            params.session_id,
            replacement_turn_id,
            replacement_message_id,
            // The replacement message id is reused as-is; no new item
            // sequence is allocated on this path.
            None,
            ItemKind::UserMessage,
            serde_json::json!({ "title": "You", "text": display_input.clone() }),
        )
        .await;
        self.emit_item_completed(
            params.session_id,
            replacement_turn_id,
            replacement_message_id,
            None,
            ItemKind::UserMessage,
            serde_json::json!({ "title": "You", "text": display_input }),
        )
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: crate::MessageEditPreviousResult {
                edit_id: edit_record.edit_id.0.to_string(),
                replacement_message_id,
                replacement_turn_id: Some(replacement_turn_id),
                edit_state: "accepted".to_string(),
            },
        })
        .expect("serialize message/editPrevious response")
    }
}
