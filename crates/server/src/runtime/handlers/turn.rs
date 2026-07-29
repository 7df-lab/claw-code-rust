use super::super::*;
use crate::TurnInputDisposition;
use crate::TurnQueueRemoveParams;
use crate::TurnQueueRemoveResult;
use crate::TurnQueueSteerParams;
use crate::TurnQueueSteerResult;

fn pending_turn_metadata(
    collaboration_mode: devo_protocol::CollaborationMode,
    model: Option<String>,
    model_binding_id: Option<String>,
) -> Option<serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    if collaboration_mode != devo_protocol::CollaborationMode::Build {
        metadata.insert(
            "collaboration_mode".to_string(),
            serde_json::json!(collaboration_mode),
        );
    }
    if let Some(model_binding_id) = model_binding_id {
        metadata.insert(
            "model_binding_id".to_string(),
            serde_json::Value::String(model_binding_id),
        );
    }
    if let Some(model) = model {
        metadata.insert("model".to_string(), serde_json::Value::String(model));
    }
    (!metadata.is_empty()).then_some(serde_json::Value::Object(metadata))
}

impl ServerRuntime {
    pub(crate) async fn handle_turn_start_for_connection(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnStartParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/start params: {error}"),
                );
            }
        };
        self.handle_turn_start_with_queue_policy(
            connection_id,
            request_id,
            params,
            TurnStartQueuePolicy::Queue,
        )
        .await
    }

    pub(crate) async fn handle_turn_start_with_queue_policy(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: TurnStartParams,
        queue_policy: TurnStartQueuePolicy,
    ) -> serde_json::Value {
        if params.input.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn input is empty",
            );
        }
        let Some(display_input) = render_input_items(&params.input) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn input is empty",
            );
        };
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let state_change_guard = session_handle.lock_state_change().await;
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let workspace_root = params
            .cwd
            .clone()
            .unwrap_or_else(|| reservation.summary.cwd.clone());
        let runtime_context = if params
            .cwd
            .as_ref()
            .is_some_and(|cwd| cwd != &reservation.summary.cwd)
        {
            match self.deps.context_for_workspace(&workspace_root).await {
                Ok(runtime_context) => runtime_context,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize session workspace: {error}"),
                    );
                }
            }
        } else {
            reservation.runtime_context
        };
        if let Some(binding_id) = params.model_binding_id.as_deref() {
            let binding_error = {
                let config_store = runtime_context
                    .config_store
                    .lock()
                    .expect("app config store mutex should not be poisoned");
                let provider_config = &config_store.effective_config().provider;
                match provider_config.model_bindings.get(binding_id) {
                    None => Some(format!("model binding `{binding_id}` does not exist")),
                    Some(binding) if !binding.enabled => {
                        Some(format!("model binding `{binding_id}` is disabled"))
                    }
                    Some(binding) => match provider_config.providers.get(&binding.provider) {
                        None => Some(format!(
                            "model binding `{binding_id}` references missing provider `{}`",
                            binding.provider
                        )),
                        Some(provider) if !provider.enabled => Some(format!(
                            "model binding `{binding_id}` references disabled provider `{}`",
                            binding.provider
                        )),
                        Some(_) => None,
                    },
                }
            };
            if let Some(error) = binding_error {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, error);
            }
        }
        let Some(resolved_input) = (match runtime_context
            .resolve_input_items(&params.input, Some(workspace_root.as_path()))
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
                    format!("failed to resolve turn input: {error}"),
                );
            }
        }) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn input is empty",
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
        let now = Utc::now();
        let mut cwd_change = None;
        if let Some(active_turn) = reservation.active_turn.as_ref() {
            if queue_policy == TurnStartQueuePolicy::RejectActive {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "session already has an active prompt turn",
                );
            }
            let active_turn_id = active_turn.turn_id;
            let queued_model = params
                .model
                .clone()
                .or_else(|| reservation.summary.model.clone());
            let queued_model_binding_id = params
                .model_binding_id
                .clone()
                .or_else(|| reservation.summary.model_binding_id.clone());
            let item = devo_core::PendingInputItem::new(
                devo_core::PendingInputKind::UserInput {
                    input: params.input.clone(),
                    display_text: display_input.clone(),
                    prompt_text: resolved_input.prompt_text.clone(),
                    prompt_messages: resolved_input.prompt_messages.clone(),
                },
                pending_turn_metadata(
                    params.collaboration_mode,
                    queued_model,
                    queued_model_binding_id,
                ),
                now,
            );
            let queued_input_id = item.id;
            // Push into the shared queue directly instead of the actor
            // mailbox: a busy actor does not service its mailbox until the
            // turn finishes, and callers must see their entry synchronously
            // (01 §4.3 last-write-wins). The actor reads the same shared
            // queue at drain time.
            reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned")
                .push_back(item.clone());
            if !reservation.ephemeral
                && let Err(err) =
                    self.deps
                        .db
                        .push_pending(&params.session_id, QueueType::Turn, &item)
            {
                tracing::warn!(
                    session_id = %params.session_id,
                    error = %err,
                    "failed to persist pending turn message to database"
                );
            }
            let sid = params.session_id;
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime.broadcast_updated_queue(sid).await;
            });
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: TurnStartResult::Queued {
                    active_turn_id,
                    queued_input_id,
                    status: TurnStatus::Pending,
                    accepted_at: now,
                },
            })
            .expect("serialize queued turn/start response");
        }
        if let Some(cwd) = params.cwd.clone() {
            let old_cwd = reservation.summary.cwd.clone();
            if old_cwd != cwd {
                cwd_change = Some((old_cwd, cwd.clone()));
                session_handle
                    .update_session_workspace(cwd.clone(), Arc::clone(&runtime_context))
                    .await;
            }
        }
        if let Some(permission_mode) = params
            .approval_policy
            .as_deref()
            .and_then(permission_mode_from_approval_policy)
        {
            session_handle
                .update_core_permission_mode(permission_mode)
                .await;
        }
        let requested_model = requested_model_selection(
            params.model_binding_id.as_deref(),
            params.model.as_deref(),
            &reservation.summary,
        );
        let requested_reasoning_effort_selection = params
            .reasoning_effort_selection
            .clone()
            .or_else(|| reservation.summary.reasoning_effort_selection.clone());
        let turn_config = runtime_context
            .resolve_turn_config(requested_model, requested_reasoning_effort_selection);
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: params.session_id,
            sequence: reservation
                .latest_turn
                .as_ref()
                .map_or(1, |turn| turn.sequence + 1),
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Regular,
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
        session_handle
            .begin_active_turn(turn.clone(), turn_config.clone())
            .await;
        drop(state_change_guard);
        if let Some((old_cwd, new_cwd)) = cwd_change {
            self.run_session_hook(
                params.session_id,
                devo_core::HookEvent::CwdChanged,
                serde_json::Map::from_iter([
                    (
                        "old_cwd".to_string(),
                        serde_json::Value::String(old_cwd.display().to_string()),
                    ),
                    (
                        "new_cwd".to_string(),
                        serde_json::Value::String(new_cwd.display().to_string()),
                    ),
                ]),
            )
            .await;
        }
        self.maybe_start_title_generation_from_user_input(params.session_id, &display_input)
            .await;
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
                format!("failed to persist turn start: {error}"),
            );
        }

        self.broadcast_event(ServerEvent::InputQueueUpdated(
            devo_core::InputQueueUpdatedPayload {
                session_id: params.session_id,
                pending_count: 0,
                pending_texts: vec![],
            },
        ))
        .await;
        let runtime = Arc::clone(self);
        let turn_for_task = turn.clone();
        let display_input_for_task = display_input.clone();
        let input_for_task = resolved_input.prompt_text.clone();
        let input_messages_for_task = resolved_input.prompt_messages.clone();
        let turn_config_for_task = turn_config.clone();
        let collaboration_mode = params.collaboration_mode;
        let session_id = params.session_id;
        self.spawn_active_turn_task(params.session_id, turn.clone(), connection_id, async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn: turn_for_task,
                    turn_config: turn_config_for_task,
                    display_input: display_input_for_task,
                    input: input_for_task,
                    input_messages: input_messages_for_task,
                    collaboration_mode,
                    input_mode: TurnInputMode::VisibleUserMessage,
                })
                .await;
        })
        .await;

        tracing::info!(
            session_id = %params.session_id,
            turn_id = %turn.turn_id,
            sequence = turn.sequence,
            request_model = %turn.request_model,
            input_chars = resolved_input.prompt_text.len(),
            "started turn"
        );
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: params.session_id,
                status: SessionRuntimeStatus::ActiveTurn,
            },
        ))
        .await;
        self.broadcast_event(ServerEvent::TurnStarted(TurnEventPayload {
            session_id: params.session_id,
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
        .expect("serialize turn/start response")
    }

    pub(crate) async fn handle_turn_shell_command_for_connection(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: ShellCommandParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/shell_command params: {error}"),
                );
            }
        };
        let command = params.command.trim().to_string();
        if command.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "shell command is empty",
            );
        }
        let Some(session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let _state_change_guard = session_handle.lock_state_change().await;

        let requested_runtime_context = match params.cwd.as_ref() {
            Some(cwd) => match self.deps.context_for_workspace(cwd).await {
                Ok(runtime_context) => Some(runtime_context),
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to initialize session workspace: {error}"),
                    );
                }
            },
            None => None,
        };
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if reservation.active_turn.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot run shell command while a turn is active",
            );
        }
        let now = Utc::now();
        let mut cwd_change = None;
        let cwd = params
            .cwd
            .clone()
            .unwrap_or_else(|| reservation.summary.cwd.clone());
        if let Some(cwd) = params.cwd.clone() {
            let old_cwd = reservation.summary.cwd.clone();
            if old_cwd != cwd {
                cwd_change = Some((old_cwd, cwd.clone()));
                if let Some(runtime_context) = requested_runtime_context.as_ref() {
                    session_handle
                        .update_session_workspace(cwd.clone(), Arc::clone(runtime_context))
                        .await;
                }
            }
        }
        let model = reservation.summary.model.clone().unwrap_or_default();
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id: params.session_id,
            sequence: reservation
                .latest_turn
                .as_ref()
                .map_or(1, |turn| turn.sequence + 1),
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::Other("shell_command".to_string()),
            model: model.clone(),
            model_binding_id: reservation.summary.model_binding_id.clone(),
            reasoning_effort_selection: reservation.summary.reasoning_effort_selection.clone(),
            reasoning_effort: reservation.summary.reasoning_effort,
            request_model: model,
            request_thinking: reservation.summary.reasoning_effort_selection.clone(),
            started_at: now,
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        session_handle
            .begin_active_turn(
                turn.clone(),
                reservation.runtime_context.resolve_turn_config(
                    session_model_selection(&reservation.summary),
                    reservation.summary.reasoning_effort_selection.clone(),
                ),
            )
            .await;
        if let Some((old_cwd, new_cwd)) = cwd_change {
            self.run_session_hook(
                params.session_id,
                devo_core::HookEvent::CwdChanged,
                serde_json::Map::from_iter([
                    (
                        "old_cwd".to_string(),
                        serde_json::Value::String(old_cwd.display().to_string()),
                    ),
                    (
                        "new_cwd".to_string(),
                        serde_json::Value::String(new_cwd.display().to_string()),
                    ),
                ]),
            )
            .await;
        }

        if let Some(persistence) = session_handle.turn_persistence_snapshot().await
            && persistence.record.is_some()
            && let Err(error) = self
                .persist_turn_line_deduped(params.session_id, &turn)
                .await
        {
            self.clear_active_turn_reservation(&session_handle, turn.turn_id)
                .await;
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist shell command turn start: {error}"),
            );
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
            turn: turn.clone(),
        }))
        .await;

        let runtime = Arc::clone(self);
        let command_for_task = command.clone();
        let turn_for_task = turn.clone();
        self.spawn_active_turn_task(params.session_id, turn.clone(), connection_id, async move {
            runtime
                .execute_shell_command_turn(params.session_id, turn_for_task, command_for_task, cwd)
                .await;
        })
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ShellCommandResult {
                turn_id: turn.turn_id,
                status: turn.status,
                accepted_at: now,
            },
        })
        .expect("serialize turn/shell_command response")
    }

    pub(crate) async fn handle_turn_steer(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/steer params: {error}"),
                );
            }
        };
        if params.input.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn steer input is empty",
            );
        }
        let Some(display_input) = render_input_items(&params.input) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::EmptyInput,
                "turn steer input is empty",
            );
        };
        let Some(_session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let workspace_root = reservation.summary.cwd.clone();
        let runtime_context = reservation.runtime_context;
        let resolved_input = match runtime_context
            .resolve_input_items(&params.input, Some(workspace_root.as_path()))
        {
            Ok(Some(resolved_input)) => resolved_input,
            Ok(None) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::EmptyInput,
                    "turn steer input is empty",
                );
            }
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
                    format!("failed to resolve turn steer input: {error}"),
                );
            }
        };

        // The turn having just ended is the ONLY race that degrades (01
        // §4.3): the input must not be lost, so it lands in the session
        // queue instead of erroring. Genuine steerability violations
        // (wrong turn, wrong kind) still error below.
        if reservation.active_turn.is_none() {
            let item = devo_core::PendingInputItem::new(
                devo_core::PendingInputKind::UserInput {
                    input: params.input.clone(),
                    display_text: display_input.clone(),
                    prompt_text: resolved_input.prompt_text,
                    prompt_messages: resolved_input.prompt_messages,
                },
                None,
                chrono::Utc::now(),
            );
            reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned")
                .push_back(item.clone());
            if !reservation.ephemeral
                && let Err(err) =
                    self.deps
                        .db
                        .push_pending(&params.session_id, QueueType::Turn, &item)
            {
                tracing::warn!(
                    session_id = %params.session_id,
                    error = %err,
                    "failed to persist degraded steer input to database"
                );
            }
            self.broadcast_queue_updated(
                params.session_id,
                devo_protocol::canonical::queue::QueueChange::Added,
                devo_protocol::canonical::ids::QueueItemId::from_legacy_uuid(uuid::Uuid::from(
                    item.id,
                )),
                None,
            )
            .await;
            self.broadcast_updated_queue(params.session_id).await;
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: TurnSteerResult {
                    turn_id: params.expected_turn_id,
                    disposition: TurnInputDisposition::Queued,
                },
            })
            .expect("serialize turn/steer degraded response");
        }
        let active_turn = reservation.active_turn.as_ref().expect("checked above");
        let turn_id = active_turn.turn_id;
        if turn_id != params.expected_turn_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTurnMismatch,
                "active turn did not match expectedTurnId",
            );
        }
        if active_turn.kind != devo_core::TurnKind::Regular {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "cannot steer a non-regular turn",
            );
        }

        self.emit_turn_item(
            params.session_id,
            turn_id,
            ItemKind::UserMessage,
            TurnItem::SteerInput(TextItem {
                text: display_input.clone(),
            }),
            serde_json::json!({ "title": "You", "text": display_input.clone() }),
        )
        .await;
        let item = devo_core::PendingInputItem::new(
            devo_core::PendingInputKind::UserInput {
                input: params.input.clone(),
                display_text: display_input,
                prompt_text: resolved_input.prompt_text,
                prompt_messages: resolved_input.prompt_messages,
            },
            None,
            chrono::Utc::now(),
        );
        reservation
            .steer_input_queue
            .lock()
            .expect("steer input queue mutex should not be poisoned")
            .push_back(item.clone());

        if !reservation.ephemeral
            && let Err(err) = self
                .deps
                .db
                .push_pending(&params.session_id, QueueType::Steer, &item)
        {
            tracing::warn!(
                session_id = %params.session_id,
                error = %err,
                "failed to persist steer input to database"
            );
        }

        self.emit_to_connection(
            connection_id,
            "serverRequest/resolved",
            ServerEvent::ServerRequestResolved(ServerRequestResolvedPayload {
                session_id: params.session_id,
                request_id: "steer-accepted".into(),
                turn_id: Some(turn_id),
            }),
        )
        .await;
        tracing::info!(
            connection_id,
            session_id = %params.session_id,
            turn_id = %turn_id,
            input_items = params.input.len(),
            "accepted turn steer request"
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnSteerResult {
                turn_id,
                disposition: TurnInputDisposition::Steered,
            },
        })
        .expect("serialize turn/steer response")
    }

    pub(crate) async fn handle_turn_queue_remove(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnQueueRemoveParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/queue/remove params: {error}"),
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
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let is_ephemeral = reservation.ephemeral;
        let removed = session_handle
            .remove_queued_turn_input(params.queued_input_id)
            .await
            .unwrap_or(false);
        if removed
            && !is_ephemeral
            && let Err(error) = self.deps.db.remove_pending_by_id(
                &params.session_id,
                QueueType::Turn,
                &params.queued_input_id,
            )
        {
            tracing::warn!(
                session_id = %params.session_id,
                queued_input_id = %params.queued_input_id,
                error = %error,
                "failed to remove pending turn message from database"
            );
        }
        if removed {
            self.broadcast_updated_queue(params.session_id).await;
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnQueueRemoveResult { removed },
        })
        .expect("serialize turn/queue/remove response")
    }

    pub(crate) async fn handle_turn_queue_steer(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: TurnQueueSteerParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid turn/queue/steer params: {error}"),
                );
            }
        };
        let Some(_session_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(active_turn) = reservation.active_turn.as_ref() else {
            // The turn just ended (01 §4.3): the queued entry stays queued —
            // the message is never lost.
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: TurnQueueSteerResult {
                    turn_id: params.expected_turn_id,
                    disposition: TurnInputDisposition::Queued,
                },
            })
            .expect("serialize turn/queue/steer degraded response");
        };
        if active_turn.turn_id != params.expected_turn_id {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ExpectedTurnMismatch,
                "active turn did not match expectedTurnId",
            );
        }
        if active_turn.kind != devo_core::TurnKind::Regular {
            return self.error_response(
                request_id,
                ProtocolErrorCode::ActiveTurnNotSteerable,
                "cannot steer a non-regular turn",
            );
        }
        let turn_id = active_turn.turn_id;
        let is_ephemeral = reservation.ephemeral;
        let queued = {
            let mut queue = reservation
                .pending_turn_queue
                .lock()
                .expect("pending turn queue mutex should not be poisoned");
            let Some(index) = queue
                .iter()
                .position(|item| item.id == params.queued_input_id)
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "queued input does not exist or cannot be steered",
                );
            };
            let display_input = match &queue[index].kind {
                devo_core::PendingInputKind::UserText { text } => text.clone(),
                devo_core::PendingInputKind::UserInput { display_text, .. } => display_text.clone(),
                _ => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "queued input does not exist or cannot be steered",
                    );
                }
            };
            let item = queue
                .remove(index)
                .expect("queued item index should remain valid");
            (display_input, item)
        };
        let (display_input, item) = queued;

        reservation
            .steer_input_queue
            .lock()
            .expect("steer input queue mutex should not be poisoned")
            .push_back(item.clone());

        if !is_ephemeral {
            if let Err(error) = self.deps.db.remove_pending_by_id(
                &params.session_id,
                QueueType::Turn,
                &params.queued_input_id,
            ) {
                tracing::warn!(
                    session_id = %params.session_id,
                    queued_input_id = %params.queued_input_id,
                    error = %error,
                    "failed to remove steered queued message from database"
                );
            }
            if let Err(error) =
                self.deps
                    .db
                    .push_pending(&params.session_id, QueueType::Steer, &item)
            {
                tracing::warn!(
                    session_id = %params.session_id,
                    queued_input_id = %params.queued_input_id,
                    error = %error,
                    "failed to persist steered queued message to database"
                );
            }
        }

        self.emit_turn_item(
            params.session_id,
            turn_id,
            ItemKind::UserMessage,
            TurnItem::SteerInput(TextItem {
                text: display_input.clone(),
            }),
            serde_json::json!({ "title": "You", "text": display_input }),
        )
        .await;
        self.broadcast_updated_queue(params.session_id).await;
        self.emit_to_connection(
            connection_id,
            "serverRequest/resolved",
            ServerEvent::ServerRequestResolved(ServerRequestResolvedPayload {
                session_id: params.session_id,
                request_id: "queued-steer-accepted".into(),
                turn_id: Some(turn_id),
            }),
        )
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: TurnQueueSteerResult {
                turn_id,
                disposition: TurnInputDisposition::Steered,
            },
        })
        .expect("serialize turn/queue/steer response")
    }

    pub(crate) async fn handle_events_subscribe(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: EventsSubscribeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid events/subscribe params: {error}"),
                );
            }
        };
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            connection.subscriptions.push(SubscriptionFilter {
                session_id: params.session_id,
                event_types: params.event_types.unwrap_or_default().into_iter().collect(),
                include_child_agents: params.include_child_agents,
            });
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: EventsSubscribeResult {
                subscription_id: format!("sub-{connection_id}-1").into(),
            },
        })
        .expect("serialize events/subscribe response")
    }
}
