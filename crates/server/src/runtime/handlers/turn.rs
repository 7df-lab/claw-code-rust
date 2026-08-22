use super::super::*;

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
        // Dual-shape boundary (L2-DES-APP-008 DD-4): the canonical shape is
        // detected by its required `idempotencyKey`.
        if params.get("idempotencyKey").is_some() {
            return self
                .handle_native_turn_start(connection_id, request_id, params)
                .await;
        }
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

    /// Native `turn/start` (L2-DES-APP-008 Phase B): lean params (input +
    /// idempotency key; per-turn model/settings moved to settings updates),
    /// busy sessions reject with `TURN_ALREADY_RUNNING` (clients use
    /// `session/queue/push`), and the result carries the canonical turn
    /// snapshot. Idempotent replays return the originally started turn.
    async fn handle_native_turn_start(
        self: &Arc<Self>,
        connection_id: Option<u64>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::TurnStartParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical turn/start params: {error}"),
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
        // Input conversion: canonical `UserInput` → internal `InputItem`.
        let mut input = Vec::with_capacity(params.input.len());
        for item in &params.input {
            use devo_protocol::native::item::UserInput;
            let converted = match item {
                UserInput::Text { text } => devo_protocol::InputItem::Text { text: text.clone() },
                UserInput::LocalImage { path, .. } => {
                    devo_protocol::InputItem::LocalImage { path: path.clone() }
                }
                UserInput::Mention { uri } => devo_protocol::InputItem::Mention {
                    path: uri.clone(),
                    name: None,
                },
                UserInput::Skill { name } => devo_protocol::InputItem::Skill {
                    name: name.clone(),
                    path: std::path::PathBuf::new(),
                },
                UserInput::Image { .. } | UserInput::Audio { .. } => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "image and audio inputs are not served by canonical turn/start yet",
                    );
                }
            };
            input.push(converted);
        }
        // Idempotent replay: return the originally started turn snapshot.
        let idempotency_key = (legacy_session_id, params.idempotency_key.clone());
        if let Some(turn) = self
            .turn_start_idempotency
            .lock()
            .await
            .get(&idempotency_key)
            .cloned()
        {
            return serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_turn::TurnStartResult { turn },
            })
            .expect("serialize canonical turn/start response");
        }

        let legacy_params = TurnStartParams {
            session_id: legacy_session_id,
            input,
            model: None,
            model_binding_id: None,
            reasoning_effort_selection: None,
            sandbox: None,
            approval_policy: None,
            cwd: None,
            collaboration_mode: Default::default(),
            execution_mode: Default::default(),
        };
        let response = self
            .handle_turn_start_with_queue_policy(
                connection_id,
                request_id.clone(),
                legacy_params,
                TurnStartQueuePolicy::RejectActive,
            )
            .await;
        let Ok(success) =
            serde_json::from_value::<SuccessResponse<TurnStartResult>>(response.clone())
        else {
            return response;
        };
        let TurnStartResult::Started { turn_id, .. } = success.result else {
            // A turn raced in between the reservation check and admission;
            // canonical busy semantics reject instead of queueing.
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "session already has an active prompt turn",
            );
        };
        // `spawn_active_turn_task` has already queued `ExecuteTurn`, so the
        // actor mailbox is unresponsive until that turn ends. Read the
        // runtime registry instead of `session_turn_reservation_snapshot`
        // (mailbox) or the TUI's second `turn/start` times out while the
        // turn continues in the background.
        let Some(metadata) = self
            .active_turns
            .active_turn_metadata(legacy_session_id)
            .await
            .filter(|turn| turn.turn_id == turn_id)
        else {
            return response;
        };
        let turn = devo_protocol::native::wire_projector::native_turn_from_metadata(&metadata);
        self.turn_start_idempotency
            .lock()
            .await
            .insert(idempotency_key, turn.clone());
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_turn::TurnStartResult { turn },
        })
        .expect("serialize canonical turn/start response")
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
        // Registry presence is mailbox-free: `spawn_active_turn_task`
        // records the turn before `ExecuteTurn` registers a stream. Native
        // busy clients must reject here instead of waiting on the actor.
        if queue_policy == TurnStartQueuePolicy::RejectActive
            && self
                .runtime_active_turn_id(params.session_id)
                .await
                .is_some()
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "session already has an active prompt turn",
            );
        }
        // A busy session needs no state-change gate to enqueue: the queue
        // mutex is the serialization point for queue ops, and the gate can
        // be held for the rest of a turn (final title generation parking
        // on the busy actor mailbox) or across a compaction provider call,
        // which would park every push behind it without responding.
        let Some(mut reservation) = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        // Only admitting a new turn must serialize against rollback,
        // message edit, and compaction via the gate. Re-read the
        // reservation under the gate: a turn may have started meanwhile.
        let state_change_guard = if reservation.active_turn.is_none() {
            let guard = session_handle.lock_state_change().await;
            let Some(fresh) = self
                .session_turn_reservation_snapshot(params.session_id)
                .await
            else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                );
            };
            reservation = fresh;
            Some(guard)
        } else {
            None
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
            // The gate-free enqueue can race the post-turn drain: if the
            // active turn ended between the snapshot and this push and the
            // followup chain already found an empty queue, the entry would
            // strand. Kick an idle-only drain; it no-ops otherwise.
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime.drain_queue_if_idle(sid).await;
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

        if let Some(spawn) = session_handle.spawn_snapshot().await {
            self.register_turn_spawn_snapshot(params.session_id, turn.turn_id, Arc::new(spawn))
                .await;
        }

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
}
