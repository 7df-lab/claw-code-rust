use std::collections::HashMap;
use std::time::Duration;

use super::*;

mod coordinator;
pub(in crate::runtime) mod handlers;
mod lifecycle;

const AGENT_NAME_ADJECTIVES: &[&str] = &[
    "brave", "clever", "silent", "happy", "gentle", "swift", "bright", "lazy", "wild", "calm",
    "fuzzy", "tiny", "bold", "lucky", "mighty",
];
const AGENT_NAME_NOUNS: &[&str] = &[
    "apple", "banana", "orange", "peach", "mango", "tiger", "panda", "fox", "rabbit", "eagle",
    "koala", "lion", "whale", "otter", "wolf",
];

impl ServerRuntime {
    async fn spawn_agent_inner(
        self: &Arc<Self>,
        params: devo_protocol::SpawnAgentParams,
    ) -> Result<devo_protocol::SpawnAgentResult, ToolCallError> {
        let parent_session_id = params.session_id;
        let child_session_id = SessionId::new();
        let now = Utc::now();
        if let Some(fork_turns) = params.fork_turns.as_deref()
            && !matches!(fork_turns, "none" | "all")
        {
            return Err(ToolCallError::InvalidInput(
                "fork_turns must be \"none\" or \"all\"".to_string(),
            ));
        }
        let effective_tool_policy = params.tool_policy;
        let fork_turns = params.fork_turns.as_deref().unwrap_or("all");
        if params.max_turns == Some(0) {
            return Err(ToolCallError::InvalidInput(
                "max_turns must be positive when provided".to_string(),
            ));
        }

        let parent_handle = self.session(parent_session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {parent_session_id}"))
        })?;
        let parent_snapshot = if let Some(snapshot) = self
            .active_spawn_snapshot_for_session(parent_session_id)
            .await
        {
            snapshot
        } else {
            parent_handle.spawn_snapshot().await.ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot parent session: {parent_session_id}"
                ))
            })?
        };
        let stable_items = if fork_turns == "all" {
            parent_snapshot.stable_items
        } else {
            Vec::new()
        };

        let parent_summary = parent_snapshot.parent_summary;
        let parent_config = parent_snapshot.parent_config;
        let parent_latest_turn = parent_snapshot.parent_latest_turn;
        let parent_active_turn_id = parent_snapshot.parent_active_turn_id;
        let parent_tool_registry = parent_snapshot.parent_tool_registry;
        let runtime_context = parent_snapshot.runtime_context;
        let parent_usage_turn_id =
            parent_active_turn_id.or_else(|| parent_latest_turn.as_ref().map(|turn| turn.turn_id));

        let nickname = self
            .generate_unique_agent_name(parent_session_id, child_session_id)
            .await?;
        let role = "default".to_string();
        let parent_path = parent_summary
            .agent_path
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let agent_path = AgentPath::new(parent_path).join(&nickname).0;
        let model = parent_summary.model.clone();
        let model_binding_id = parent_summary.model_binding_id.clone();
        let reasoning_effort_selection = parent_summary.reasoning_effort_selection.clone();

        let mut record = self.rollout_store.create_session_record(
            child_session_id,
            now,
            parent_summary.cwd.clone(),
            parent_summary.additional_directories.clone(),
            Some(nickname.clone()),
            model.clone(),
            model_binding_id.clone(),
            reasoning_effort_selection.clone(),
            runtime_context.provider.name().to_string(),
            Some(parent_session_id),
        );
        record.agent_path = Some(agent_path.clone());
        record.agent_nickname = Some(nickname.clone());
        record.agent_role = Some(role.clone());
        record.first_user_message = Some(params.message.clone());
        let record = if params.ephemeral {
            None
        } else {
            self.rollout_store
                .append_session_meta(&record)
                .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
            Some(record)
        };

        let rollout_path_for_db = record.as_ref().map(|entry| entry.rollout_path.clone());
        let mut core_session = runtime_context.new_session_state(
            child_session_id,
            parent_summary.cwd.clone(),
            parent_summary.additional_directories.clone(),
        );
        core_session.config = parent_config.clone();
        let mut rebuilt_history_items = Vec::new();
        let mut rebuilt_messages = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &stable_items {
            crate::persistence::apply_turn_item(
                &mut rebuilt_messages,
                &mut rebuilt_history_items,
                &mut tool_names_by_id,
                &item.turn_kind,
                item.turn_item.clone(),
            );
        }
        core_session.messages = rebuilt_messages;
        core_session.turn_count = stable_items
            .iter()
            .filter(|item| matches!(item.turn_item, TurnItem::UserMessage(_)))
            .count();
        let pending_turn_queue = Arc::clone(&core_session.pending_turn_queue);
        let steer_input_queue = Arc::clone(&core_session.steer_input_queue);
        let latest_turn = if stable_items.is_empty() {
            None
        } else {
            parent_latest_turn.map(|mut turn| {
                turn.session_id = child_session_id;
                turn
            })
        };
        let summary = SessionMetadata {
            session_id: child_session_id,
            cwd: parent_summary.cwd.clone(),
            additional_directories: parent_summary.additional_directories.clone(),
            created_at: now,
            updated_at: now,
            last_activity_at: now,
            title: Some(nickname.clone()),
            title_state: SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate),
            parent_session_id: Some(parent_session_id),
            agent_path: Some(agent_path.clone()),
            agent_nickname: Some(nickname.clone()),
            agent_role: Some(role.clone()),
            ephemeral: params.ephemeral,
            model: model.clone(),
            model_binding_id: model_binding_id.clone(),
            reasoning_effort_selection,
            reasoning_effort: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: core_session.prompt_token_estimate,
            last_query_usage: None,
            last_query_total_tokens: 0,
            last_context_occupancy: None,
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: Default::default(),
            effective_context_window: parent_summary.effective_context_window.or_else(|| {
                parent_config
                    .effective_context_window_override
                    .map(|limit| limit as u64)
            }),
            permission_preset: parent_summary.permission_preset,
        };
        let child_session = RuntimeSession {
            runtime_context,
            record,
            summary: summary.clone(),
            config: parent_config,
            core_session: Arc::new(Mutex::new(core_session)),
            active_turn: None,
            latest_turn,
            loaded_item_count: u64::try_from(stable_items.len()).unwrap_or(u64::MAX),
            history_items: rebuilt_history_items,
            persisted_turn_items: stable_items,
            latest_compaction_snapshot: None,
            turn_records_by_id: std::collections::HashMap::new(),
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: effective_tool_policy,
            max_turns: params.max_turns,
            deferred_assistant: None,
            deferred_reasoning: None,
            next_item_seq: 1,
            first_user_input: Some(params.message.clone()),
            tool_registry: parent_tool_registry,
            file_read_ledger: std::sync::Arc::new(devo_core::tools::FileReadLedger::new()),
            session_approval_cache: crate::execution::ApprovalGrantCache::default(),
            turn_approval_cache: crate::execution::ApprovalGrantCache::default(),
            session_context_recorded: false,
        };
        let child_state = SessionActorState::from_runtime_session(child_session);
        self.insert_session_actor(child_state).await;
        self.agent_mailboxes
            .lock()
            .await
            .entry(parent_session_id)
            .or_default();
        self.agent_mailboxes
            .lock()
            .await
            .entry(child_session_id)
            .or_default();
        self.agent_output_buffers
            .lock()
            .await
            .entry(parent_session_id)
            .or_default();
        self.register_child_agent(
            parent_session_id,
            child_session_id,
            SubagentMetadata {
                session_id: child_session_id,
                parent_session_id,
                agent_path: agent_path.clone(),
                nickname: nickname.clone(),
                role: role.clone(),
                status: SubagentStatus::Spawning,
                spawned_at: now,
                closed_at: None,
                last_task_message: Some(params.message.clone()),
                close_requested: false,
            },
        )
        .await;
        if let Some(parent_turn_id) = parent_usage_turn_id {
            self.record_subagent_status_event(
                parent_session_id,
                child_session_id,
                SubagentStatus::Spawning,
                parent_turn_id,
            )
            .await;
        }
        self.register_subagent_usage_owner(
            parent_session_id,
            child_session_id,
            parent_usage_turn_id,
        )
        .await;
        if !summary.ephemeral
            && let Err(error) = self.deps.db.upsert_session(
                &summary,
                rollout_path_for_db.as_deref().map(std::path::Path::new),
            )
        {
            tracing::warn!(
                session_id = %child_session_id,
                error = %error,
                "failed to persist child session metadata to database"
            );
        }
        let start_runtime = Arc::clone(self);
        let start_summary = summary;
        let start_message = params.message.clone();
        tracing::debug!(
            parent_session_id = %parent_session_id,
            child_session_id = %child_session_id,
            agent_path = %agent_path,
            "subagent startup task spawned"
        );
        tokio::spawn(async move {
            start_runtime
                .broadcast_event(ServerEvent::SessionStarted(SessionEventPayload {
                    session: start_summary,
                }))
                .await;
            start_runtime
                .run_subagent_start_hook(child_session_id)
                .await;
            if start_runtime
                .agent_close_requested(parent_session_id, child_session_id)
                .await
            {
                return;
            }
            match start_runtime
                .start_runtime_turn(
                    child_session_id,
                    start_message.clone(),
                    start_message,
                    /*queued_metadata*/ None,
                )
                .await
            {
                Ok(_) => {
                    if start_runtime
                        .agent_close_requested(parent_session_id, child_session_id)
                        .await
                    {
                        let _ = start_runtime
                            .close_child_agent(parent_session_id, child_session_id)
                            .await;
                        return;
                    }
                    start_runtime
                        .set_agent_status(
                            parent_session_id,
                            child_session_id,
                            SubagentStatus::Running,
                        )
                        .await;
                }
                Err(error) => {
                    let error_message = error.to_string();
                    tracing::warn!(
                        parent_session_id = %parent_session_id,
                        child_session_id = %child_session_id,
                        error = %error_message,
                        "failed to start child agent turn"
                    );
                    if start_runtime
                        .agent_close_requested(parent_session_id, child_session_id)
                        .await
                    {
                        return;
                    }
                    start_runtime
                        .fail_child_agent_startup(
                            parent_session_id,
                            child_session_id,
                            error_message,
                        )
                        .await;
                }
            }
        });

        Ok(devo_protocol::SpawnAgentResult {
            task_id: devo_protocol::TaskId::from(child_session_id),
            child_session_id,
            agent_path,
            agent_nickname: nickname,
            status: SubagentStatus::Spawning.as_str().to_string(),
        })
    }

    async fn mailbox(&self, session_id: SessionId) -> SubagentMailbox {
        self.agent_mailboxes
            .lock()
            .await
            .entry(session_id)
            .or_default()
            .clone()
    }

    async fn output_buffer(&self, parent_session_id: SessionId) -> SubagentOutputBuffer {
        self.agent_output_buffers
            .lock()
            .await
            .entry(parent_session_id)
            .or_default()
            .clone()
    }

    async fn register_child_agent(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        metadata: SubagentMetadata,
    ) {
        self.agent_registries
            .lock()
            .await
            .entry(parent_session_id)
            .or_insert_with(AgentRegistry::new)
            .register(parent_session_id, child_session_id, metadata);
    }

    async fn set_agent_status(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        status: SubagentStatus,
    ) {
        if let Some(registry) = self
            .agent_registries
            .lock()
            .await
            .get_mut(&parent_session_id)
        {
            registry.update_status(child_session_id, status);
        }
    }

    async fn start_runtime_turn(
        self: &Arc<Self>,
        session_id: SessionId,
        display_input: String,
        input_text: String,
        queued_metadata: Option<serde_json::Value>,
    ) -> Result<TurnMetadata, ToolCallError> {
        let session_handle = self.session(session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {session_id}"))
        })?;
        let _state_change_guard = session_handle.lock_state_change().await;

        let reservation = self
            .session_turn_reservation_snapshot(session_id)
            .await
            .ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot session reservation: {session_id}"
                ))
            })?;

        if reservation.max_turns.is_some_and(|max_turns| {
            max_turns == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.sequence >= max_turns)
                || reservation
                    .latest_turn
                    .as_ref()
                    .is_some_and(|turn| turn.sequence >= max_turns)
        }) {
            return Err(ToolCallError::InvalidInput(
                "agent maximum turn count reached".to_string(),
            ));
        }

        if let Some(active_turn) = reservation.active_turn.clone() {
            let item = devo_protocol::PendingInputItem::new(
                devo_protocol::PendingInputKind::UserText { text: input_text },
                queued_metadata,
                Utc::now(),
            );
            session_handle
                .enqueue_pending_turn_input(item.clone())
                .await;
            if !reservation.ephemeral
                && let Err(error) = self
                    .deps
                    .db
                    .push_pending(&session_id, QueueType::Turn, &item)
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to persist agent follow-up pending message"
                );
            }
            return Ok(active_turn);
        }

        let turn_config = reservation.runtime_context.resolve_turn_config(
            session_model_selection(&reservation.summary),
            reservation.summary.reasoning_effort_selection.clone(),
        );
        let resolved_request = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());

        let request_model = turn_config.provider_request_model(&resolved_request.request_model);
        let now = Utc::now();
        let sequence = reservation
            .latest_turn
            .as_ref()
            .map_or(1, |turn| turn.sequence + 1);
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
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

        session_handle
            .begin_active_turn(turn.clone(), turn_config.clone())
            .await;

        if let Err(error) = self.append_turn_start(session_id, &turn).await {
            let _ = session_handle
                .clear_active_turn_if_matches(turn.turn_id)
                .await;
            return Err(error);
        }

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

        let runtime = Arc::clone(self);
        let turn_for_task = turn.clone();
        let turn_config_for_task = turn_config.clone();
        if let Some(parent_session_id) = reservation.parent_session_id {
            self.active_turns
                .copy_connection_from_parent(session_id, parent_session_id)
                .await;
        }
        self.spawn_active_turn_task(session_id, turn.clone(), None, async move {
            runtime
                .execute_turn(ExecuteTurnRequest {
                    session_id,
                    turn: turn_for_task,
                    turn_config: turn_config_for_task,
                    display_input,
                    input: input_text,
                    input_messages: Vec::new(),
                    collaboration_mode: devo_protocol::CollaborationMode::Build,
                    input_mode: TurnInputMode::VisibleUserMessage,
                })
                .await;
        })
        .await;
        Ok(turn)
    }

    async fn append_turn_start(
        self: &Arc<Self>,
        session_id: SessionId,
        turn: &TurnMetadata,
    ) -> Result<(), ToolCallError> {
        let session_handle = self.session(session_id).await.ok_or_else(|| {
            ToolCallError::InvalidInput(format!("session not found: {session_id}"))
        })?;
        let persistence_snapshot = session_handle
            .turn_persistence_snapshot()
            .await
            .ok_or_else(|| {
                ToolCallError::InvalidInput(format!(
                    "failed to snapshot turn persistence: {session_id}"
                ))
            })?;
        // Child agents can be the first durable write for their rollout; route through
        // actor-owned dedupe so SessionContextUpdated is recorded even if the process
        // crashes before terminal turn finalization.
        if persistence_snapshot.record.is_some() {
            self.persist_turn_line_deduped(session_id, turn)
                .await
                .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
        }
        Ok(())
    }

    async fn queue_agent_message(
        &self,
        from_session_id: SessionId,
        target: &str,
        content: String,
    ) -> Result<AgentRoute, ToolCallError> {
        let route = self.resolve_agent_route(from_session_id, target).await?;
        let message = devo_protocol::AgentMailboxMessage {
            message_id: String::new(),
            from_session_id,
            to_session_id: route.to_session_id,
            from_agent_path: route.from_agent_path.clone(),
            to_agent_path: route.to_agent_path.clone(),
            content,
            sequence: 0,
            created_at: Utc::now(),
        };
        self.mailbox(route.to_session_id)
            .await
            .send(message)
            .await
            .map_err(|error| ToolCallError::InternalError(error.to_string()))?;
        Ok(route)
    }

    pub(in crate::runtime) async fn child_can_accept_next_turn(
        &self,
        session_id: SessionId,
    ) -> bool {
        let Some(reservation) = self.session_turn_reservation_snapshot(session_id).await else {
            return false;
        };
        !reservation.max_turns.is_some_and(|max_turns| {
            max_turns == 0
                || reservation
                    .active_turn
                    .as_ref()
                    .is_some_and(|turn| turn.sequence >= max_turns)
                || reservation
                    .latest_turn
                    .as_ref()
                    .is_some_and(|turn| turn.sequence >= max_turns)
        })
    }
    pub(in crate::runtime) async fn drain_child_mailbox_into_user_turns(
        self: &Arc<Self>,
        child_session_id: SessionId,
    ) -> Result<(), ToolCallError> {
        let messages = self.mailbox(child_session_id).await.drain().await;
        for message in messages {
            let parent_turn_id = self
                .active_turn_id_for_session(message.from_session_id)
                .await;
            if self
                .active_turn_id_for_session(child_session_id)
                .await
                .is_none()
            {
                self.register_subagent_usage_owner(
                    message.from_session_id,
                    child_session_id,
                    parent_turn_id,
                )
                .await;
            }
            self.start_runtime_turn(
                child_session_id,
                message.content.clone(),
                message.content,
                Some(subagent_usage_owner_pending_metadata(
                    message.from_session_id,
                    parent_turn_id,
                )),
            )
            .await?;
            if let Some((parent_session_id, _)) = self.child_parent_and_path(child_session_id).await
            {
                self.set_agent_status(parent_session_id, child_session_id, SubagentStatus::Running)
                    .await;
            }
        }
        Ok(())
    }

    pub(in crate::runtime) async fn resolve_child_agent(
        &self,
        parent_session_id: SessionId,
        target: &str,
    ) -> Result<SubagentMetadata, ToolCallError> {
        let registries = self.agent_registries.lock().await;
        let Some(registry) = registries.get(&parent_session_id) else {
            return Err(ToolCallError::InvalidInput(format!(
                "agent not found: {target}"
            )));
        };
        let Some(child_session_id) = registry.find_child(parent_session_id, target) else {
            return Err(ToolCallError::InvalidInput(format!(
                "agent not found: {target}"
            )));
        };
        registry
            .get(child_session_id)
            .cloned()
            .ok_or_else(|| ToolCallError::InvalidInput(format!("agent not found: {target}")))
    }

    pub(in crate::runtime) async fn agent_info(
        &self,
        parent_session_id: SessionId,
        target: &str,
    ) -> Result<devo_protocol::AgentInfo, ToolCallError> {
        Ok(self
            .resolve_child_agent(parent_session_id, target)
            .await?
            .to_agent_info())
    }

    async fn resolve_agent_route(
        &self,
        from_session_id: SessionId,
        target: &str,
    ) -> Result<AgentRoute, ToolCallError> {
        if let Ok(child) = self.resolve_child_agent(from_session_id, target).await {
            let from_path = self.session_agent_path(from_session_id).await;
            return Ok(AgentRoute {
                to_session_id: child.session_id,
                from_agent_path: from_path,
                to_agent_path: child.agent_path,
            });
        }
        Err(ToolCallError::InvalidInput(format!(
            "agent not found: {target}"
        )))
    }

    async fn resolve_wait_agent_targets(
        &self,
        parent_session_id: SessionId,
        target: Option<&str>,
    ) -> Result<Vec<SessionId>, ToolCallError> {
        let registries = self.agent_registries.lock().await;
        let Some(registry) = registries.get(&parent_session_id) else {
            return Ok(Vec::new());
        };
        if let Some(target) = target {
            let Some(child_session_id) = registry.find_child(parent_session_id, target) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {target}"
                )));
            };
            return Ok(vec![child_session_id]);
        }
        Ok(registry.children_of(parent_session_id))
    }

    async fn session_agent_path(&self, session_id: SessionId) -> String {
        let Some(session_handle) = self.session(session_id).await else {
            return "root".to_string();
        };
        let Some(summary) = session_handle.summary().await else {
            return "root".to_string();
        };
        summary
            .agent_path
            .clone()
            .unwrap_or_else(|| "root".to_string())
    }

    pub(super) async fn handle_subagent_turn_completed(
        &self,
        child_session_id: SessionId,
        turn: &TurnMetadata,
    ) {
        let Some((parent_session_id, status)) = self
            .resolve_terminal_subagent_status(child_session_id, turn)
            .await
        else {
            return;
        };
        let detail = self
            .subagent_terminal_status_detail(child_session_id, turn.turn_id, status)
            .await;
        self.finish_subagent_turn_completion(
            parent_session_id,
            child_session_id,
            turn,
            status,
            detail,
        )
        .await;
        if subagent_stop_hook_applies(status) {
            self.run_subagent_stop_hook(child_session_id).await;
        }
    }

    /// Same as `handle_subagent_turn_completed`, but reads any data owned by
    /// the currently-executing session actor directly from `state` instead of
    /// round-tripping through the session actor mailbox.
    ///
    /// Must be used when `child_session_id` is the session actor currently
    /// executing this code: that actor's mailbox is not being polled until
    /// the in-flight turn finishes, so a mailbox round-trip here would
    /// deadlock forever waiting on itself.
    pub(super) async fn handle_subagent_turn_completed_for_actor_state(
        &self,
        state: &SessionActorState,
        child_session_id: SessionId,
        turn: &TurnMetadata,
    ) {
        let Some((parent_session_id, status)) = self
            .resolve_terminal_subagent_status(child_session_id, turn)
            .await
        else {
            return;
        };
        let detail = subagent_terminal_status_detail_from_stable_items(
            &state.persisted_turn_items,
            turn.turn_id,
            status,
        );
        self.finish_subagent_turn_completion(
            parent_session_id,
            child_session_id,
            turn,
            status,
            detail,
        )
        .await;
        if subagent_stop_hook_applies(status) {
            self.run_subagent_stop_hook_for_actor_state(state, child_session_id)
                .await;
        }
    }

    async fn resolve_terminal_subagent_status(
        &self,
        child_session_id: SessionId,
        turn: &TurnMetadata,
    ) -> Option<(SessionId, SubagentStatus)> {
        let (parent_session_id, _agent_path) = self.child_parent_and_path(child_session_id).await?;
        let status = match turn.status {
            TurnStatus::Completed => SubagentStatus::Completed,
            TurnStatus::Interrupted => SubagentStatus::Interrupted,
            TurnStatus::Failed => SubagentStatus::Failed,
            TurnStatus::Pending | TurnStatus::Running | TurnStatus::WaitingApproval => {
                SubagentStatus::Running
            }
        };
        let status = if self
            .agent_close_requested(parent_session_id, child_session_id)
            .await
        {
            SubagentStatus::Closed
        } else {
            status
        };
        Some((parent_session_id, status))
    }

    async fn finish_subagent_turn_completion(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        turn: &TurnMetadata,
        status: SubagentStatus,
        detail: Option<String>,
    ) {
        self.set_agent_status(parent_session_id, child_session_id, status)
            .await;
        self.record_subagent_status_event_with_text(
            parent_session_id,
            child_session_id,
            status,
            turn.turn_id,
            detail,
        )
        .await;
    }

    async fn subagent_terminal_status_detail(
        &self,
        child_session_id: SessionId,
        turn_id: TurnId,
        status: SubagentStatus,
    ) -> Option<String> {
        if status != SubagentStatus::Failed {
            return None;
        }
        let session_handle = self.sessions.lock().await.get(&child_session_id).cloned()?;
        let snapshot = session_handle.spawn_snapshot().await?;
        subagent_terminal_status_detail_from_stable_items(&snapshot.stable_items, turn_id, status)
    }

    pub(super) async fn child_parent_and_path(
        &self,
        child_session_id: SessionId,
    ) -> Option<(SessionId, String)> {
        let registries = self.agent_registries.lock().await;
        registries.values().find_map(|registry| {
            let parent_session_id = registry.child_to_parent.get(&child_session_id).copied()?;
            let agent_path = registry.get(child_session_id)?.agent_path.clone();
            Some((parent_session_id, agent_path))
        })
    }

    pub(super) async fn record_subagent_output_event(&self, event: &ServerEvent) {
        let ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::AgentMessageDelta,
            payload,
        } = event
        else {
            return;
        };
        if payload.delta.is_empty() {
            return;
        }
        let child_session_id = payload.context.session_id;
        let Some((parent_session_id, agent_path)) =
            self.child_parent_and_path(child_session_id).await
        else {
            return;
        };
        let buffer = self.output_buffer(parent_session_id).await;
        let output_event = devo_protocol::AgentOutputEvent {
            sequence: 0,
            child_session_id,
            agent_path,
            turn_id: payload.context.turn_id,
            kind: devo_protocol::AgentOutputEventKind::AssistantMessage,
            text: Some(payload.delta.clone()),
            status: None,
            created_at: Utc::now(),
        };
        // Streaming text must not block behind wait_agent's buffer lock. If the
        // lock is busy, skip this delta for the wait buffer; the TUI already
        // received it via outbound notifications, and a later delta/status will
        // refresh the coalesced text.
        let _ = buffer.try_push_text_delta(output_event);
    }

    async fn record_subagent_status_event(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        status: SubagentStatus,
        turn_id: TurnId,
    ) {
        self.record_subagent_status_event_with_text(
            parent_session_id,
            child_session_id,
            status,
            turn_id,
            None,
        )
        .await;
    }

    async fn record_subagent_status_event_with_text(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
        status: SubagentStatus,
        turn_id: TurnId,
        text: Option<String>,
    ) {
        let agent_path = match self.child_parent_and_path(child_session_id).await {
            Some((event_parent_session_id, agent_path))
                if event_parent_session_id == parent_session_id =>
            {
                agent_path
            }
            _ => self.session_agent_path(child_session_id).await,
        };
        self.output_buffer(parent_session_id)
            .await
            .push(devo_protocol::AgentOutputEvent {
                sequence: 0,
                child_session_id,
                agent_path,
                turn_id: Some(turn_id),
                kind: devo_protocol::AgentOutputEventKind::Status,
                text,
                status: Some(status.as_str().to_string()),
                created_at: Utc::now(),
            })
            .await;
    }

    async fn agent_close_requested(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> bool {
        self.agent_registries
            .lock()
            .await
            .get(&parent_session_id)
            .and_then(|registry| registry.get(child_session_id))
            .is_some_and(|metadata| metadata.close_requested)
    }

    pub(super) async fn interrupt_all_child_agents(self: Arc<Self>, parent_session_id: SessionId) {
        let child_session_ids = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&parent_session_id)
                .map(|registry| registry.children_of(parent_session_id))
                .unwrap_or_default()
        };
        for child_session_id in child_session_ids {
            let _ = self.interrupt_child_runtime_work(child_session_id).await;
            self.set_agent_status(
                parent_session_id,
                child_session_id,
                SubagentStatus::Interrupted,
            )
            .await;
        }
    }

    async fn close_child_agent(
        self: &Arc<Self>,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> Result<String, ToolCallError> {
        let already_terminal = {
            let mut registries = self.agent_registries.lock().await;
            let Some(registry) = registries.get_mut(&parent_session_id) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {child_session_id}"
                )));
            };
            let Some(metadata) = registry.agents.get_mut(&child_session_id) else {
                return Err(ToolCallError::InvalidInput(format!(
                    "agent not found: {child_session_id}"
                )));
            };
            let terminal = matches!(
                metadata.status,
                SubagentStatus::Completed
                    | SubagentStatus::Failed
                    | SubagentStatus::Interrupted
                    | SubagentStatus::Canceled
                    | SubagentStatus::Closed
            );
            metadata.close_requested = true;
            if !terminal {
                metadata.status = SubagentStatus::Closed;
                metadata.closed_at = Some(Utc::now());
            }
            terminal
        };
        // A running turn's cancellation is now observed by the session actor's
        // own turn-completion handling (`handle_subagent_turn_completed_for_actor_state`),
        // which independently resolves and records the terminal "closed" status
        // once it sees `close_requested`. Track whether a turn was actually in
        // flight so we don't also send a duplicate closed notification below.
        let had_active_turn = self.active_turns.has_session(child_session_id).await;
        let interrupted_turn = self.interrupt_child_runtime_work(child_session_id).await;
        if already_terminal && interrupted_turn.is_none() {
            let status = self
                .resolve_child_agent(parent_session_id, &child_session_id.to_string())
                .await?
                .status
                .as_str()
                .to_string();
            return Ok(status);
        }

        if let Some(turn) = interrupted_turn {
            self.broadcast_event(ServerEvent::TurnInterrupted(TurnEventPayload {
                session_id: child_session_id,
                turn: turn.clone(),
            }))
            .await;
            self.broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                session_id: child_session_id,
                turn: turn.clone(),
            }))
            .await;
            self.handle_subagent_turn_completed(child_session_id, &turn)
                .await;
        } else if !had_active_turn {
            self.send_closed_notification(parent_session_id, child_session_id)
                .await;
        }
        self.broadcast_event(ServerEvent::SessionStatusChanged(
            SessionStatusChangedPayload {
                session_id: child_session_id,
                status: SessionRuntimeStatus::Idle,
            },
        ))
        .await;
        Ok(SubagentStatus::Closed.as_str().to_string())
    }

    async fn send_closed_notification(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) {
        self.record_subagent_status_event(
            parent_session_id,
            child_session_id,
            SubagentStatus::Closed,
            TurnId::new(),
        )
        .await;
        self.run_subagent_stop_hook(child_session_id).await;
    }

    async fn generate_unique_agent_name(
        &self,
        parent_session_id: SessionId,
        child_session_id: SessionId,
    ) -> Result<String, ToolCallError> {
        let used_names = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&parent_session_id)
                .map(|registry| {
                    registry
                        .children_of(parent_session_id)
                        .into_iter()
                        .filter_map(|child_id| registry.get(child_id))
                        .map(|metadata| metadata.nickname.clone())
                        .collect::<std::collections::HashSet<_>>()
                })
                .unwrap_or_default()
        };
        let max_count = AGENT_NAME_ADJECTIVES.len() * AGENT_NAME_NOUNS.len();
        if used_names.len() >= max_count {
            return Err(ToolCallError::InvalidInput(
                "no unique generated agent names available".to_string(),
            ));
        }
        let start = generated_name_start_index(child_session_id, max_count);
        for offset in 0..max_count {
            let index = (start + offset) % max_count;
            let adjective = AGENT_NAME_ADJECTIVES[index / AGENT_NAME_NOUNS.len()];
            let noun = AGENT_NAME_NOUNS[index % AGENT_NAME_NOUNS.len()];
            let candidate = format!("{adjective}-{noun}");
            if !used_names.contains(&candidate) {
                return Ok(candidate);
            }
        }
        Err(ToolCallError::InvalidInput(
            "no unique generated agent names available".to_string(),
        ))
    }
}

struct AgentRoute {
    to_session_id: SessionId,
    from_agent_path: String,
    to_agent_path: String,
}

fn subagent_stop_hook_applies(status: SubagentStatus) -> bool {
    matches!(
        status,
        SubagentStatus::Completed
            | SubagentStatus::Failed
            | SubagentStatus::Interrupted
            | SubagentStatus::Canceled
            | SubagentStatus::Closed
    )
}

fn subagent_terminal_status_detail_from_stable_items(
    stable_items: &[crate::execution::PersistedTurnItem],
    turn_id: TurnId,
    status: SubagentStatus,
) -> Option<String> {
    if status != SubagentStatus::Failed {
        return None;
    }
    stable_items.iter().rev().find_map(|item| {
        if item.turn_id != turn_id {
            return None;
        }
        match &item.turn_item {
            TurnItem::AgentMessage(TextItem { text }) if !text.trim().is_empty() => {
                Some(text.trim().to_string())
            }
            TurnItem::UserMessage(_)
            | TurnItem::SteerInput(_)
            | TurnItem::HookPrompt(_)
            | TurnItem::AgentMessage(_)
            | TurnItem::Plan(_)
            | TurnItem::Reasoning(_)
            | TurnItem::ToolCall(_)
            | TurnItem::ToolProgress(_)
            | TurnItem::ToolResult(_)
            | TurnItem::CommandExecution(_)
            | TurnItem::ApprovalRequest(_)
            | TurnItem::ApprovalDecision(_)
            | TurnItem::WebSearch(_)
            | TurnItem::ImageGeneration(_)
            | TurnItem::ContextCompaction(_)
            | TurnItem::TurnSummary(_) => None,
        }
    })
}

fn generated_name_start_index(child_session_id: SessionId, max_count: usize) -> usize {
    child_session_id
        .to_string()
        .bytes()
        .fold(0usize, |acc, byte| {
            acc.wrapping_mul(31).wrapping_add(usize::from(byte))
        })
        % max_count
}
