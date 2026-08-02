use super::super::*;

pub(crate) struct RuntimeSessionTurnCutOptions {
    pub(crate) session_id: SessionId,
    pub(crate) user_turn_index: Option<u32>,
    pub(crate) rollback_mode: SessionRollbackMode,
    pub(crate) cwd_override: Option<PathBuf>,
    pub(crate) title_override: Option<String>,
    pub(crate) created_at: chrono::DateTime<Utc>,
}

/// Resolve occupancy and latest-query usage for a history cut.
///
/// Prefers a compaction snapshot that still applies to the kept turns; otherwise
/// uses the last kept turn's stored occupancy / query usage.
pub(crate) fn resolve_cut_occupancy_and_usage(
    kept_turn_ids: &std::collections::HashSet<devo_core::TurnId>,
    last_kept_turn_id: Option<devo_core::TurnId>,
    turn_records_by_id: &std::collections::HashMap<devo_core::TurnId, devo_core::TurnRecord>,
    latest_compaction_snapshot: Option<&devo_core::CompactionSnapshotLine>,
) -> (
    Option<devo_protocol::canonical::item::ContextOccupancy>,
    Option<devo_protocol::TurnUsage>,
    Option<devo_core::CompactionSnapshotLine>,
) {
    let cut_turn_record =
        last_kept_turn_id.and_then(|turn_id| turn_records_by_id.get(&turn_id).cloned());
    let applicable_compaction = latest_compaction_snapshot
        .filter(|snapshot| kept_turn_ids.contains(&snapshot.turn_id))
        .cloned();
    let occupancy = applicable_compaction
        .as_ref()
        .and_then(|snapshot| snapshot.context_occupancy.clone())
        .or_else(|| {
            cut_turn_record
                .as_ref()
                .and_then(|turn| turn.context_occupancy.clone())
        });
    let latest_query_usage = cut_turn_record.as_ref().and_then(|turn| {
        turn.latest_query_usage
            .clone()
            .or_else(|| turn.usage.clone())
    });
    (occupancy, latest_query_usage, applicable_compaction)
}

pub(crate) enum RuntimeSessionToolRegistryUpdate {
    KeepCurrent,
    ReplaceIfCwdMatches {
        cwd: PathBuf,
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
    },
}

impl ServerRuntime {
    pub(crate) async fn start_session_with_registry(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: SessionStartParams,
        tool_registry: Option<Arc<devo_core::tools::ToolRegistry>>,
    ) -> serde_json::Value {
        let now = Utc::now();
        let session_id = SessionId::new();
        let runtime_context = match self.deps.context_for_workspace(&params.cwd).await {
            Ok(context) => context,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to initialize session workspace: {error}"),
                );
            }
        };
        let requested_model = params
            .model_binding_id
            .as_deref()
            .or(params.model.as_deref());
        let initial_turn_config = runtime_context.resolve_turn_config(requested_model, None);
        let model = initial_turn_config.model.slug.clone();
        let model_binding_id = initial_turn_config.model_binding_id.clone();
        let record = (!params.ephemeral).then(|| {
            self.rollout_store.create_session_record(
                session_id,
                now,
                params.cwd.clone(),
                params.additional_directories.clone(),
                params.title.clone(),
                Some(model.clone()),
                model_binding_id.clone(),
                None,
                runtime_context.provider.name().to_string(),
                None,
            )
        });
        let summary = crate::SessionMetadata {
            session_id,
            cwd: params.cwd.clone(),
            additional_directories: params.additional_directories.clone(),
            created_at: now,
            updated_at: now,
            last_activity_at: now,
            title: params.title.clone(),
            title_state: params
                .title
                .as_ref()
                .map(|_| SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate))
                .unwrap_or(SessionTitleState::Unset),
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: params.ephemeral,
            model: Some(model.clone()),
            model_binding_id: model_binding_id.clone(),
            reasoning_effort_selection: None,
            reasoning_effort: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: 0,
            last_query_usage: None,
            last_query_total_tokens: 0,
            last_context_occupancy: None,
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: Default::default(),
            effective_context_window: None,
            permission_preset: None,
        };
        let global_compaction_limit = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .compaction_token_limit;
        let applied_compaction_limit = crate::runtime::context_occupancy::resolved_compaction_limit(
            global_compaction_limit,
            &initial_turn_config.model,
        );
        let mut summary = summary;
        summary.effective_context_window = Some(applied_compaction_limit);
        if let Some(record) = &record
            && let Err(error) = self.rollout_store.append_session_meta(record)
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session metadata: {error}"),
            );
        }
        let mut core_session = runtime_context.new_session_state(
            session_id,
            params.cwd.clone(),
            params.additional_directories.clone(),
        );
        crate::runtime::context_occupancy::apply_resolved_compaction_limit(
            &mut core_session.config,
            applied_compaction_limit as usize,
        );
        let config = core_session.config.clone();
        let pending_turn_queue = Arc::clone(&core_session.pending_turn_queue);
        let steer_input_queue = Arc::clone(&core_session.steer_input_queue);
        let rollout_path_for_db = record.as_ref().map(|entry| entry.rollout_path.clone());
        let actor_state = SessionActorState {
            runtime_context,
            record,
            summary: summary.clone(),
            config,
            core: core_session,
            stream: Arc::new(tokio::sync::Mutex::new(
                crate::runtime::session_actor::state::SessionStreamState::default(),
            )),
            active_turn: None,
            latest_turn: None,
            loaded_item_count: 0,
            history_items: Vec::new(),
            persisted_turn_items: Vec::new(),
            latest_compaction_snapshot: None,
            turn_records_by_id: std::collections::HashMap::new(),
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: Default::default(),
            max_turns: None,
            next_item_seq: 1,
            first_user_input: None,
            tool_registry,
            file_read_ledger: Arc::new(devo_core::tools::FileReadLedger::new()),
            session_approval_cache: crate::execution::ApprovalGrantCache::default(),
            turn_approval_cache: crate::execution::ApprovalGrantCache::default(),
            session_context_recorded: false,
        };
        self.insert_session_actor(actor_state).await;
        self.subscribe_connection_to_session(connection_id, session_id, None)
            .await;
        self.runtime_arc()
            .after_root_session_insert(session_id)
            .await;

        // Persist session metadata to SQLite (skip for ephemeral sessions)
        if !summary.ephemeral
            && let Err(err) = self
                .deps
                .db
                .upsert_session(&summary, rollout_path_for_db.as_deref())
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to persist session metadata to database"
            );
        }

        tracing::info!(
            connection_id,
            session_id = %session_id,
            cwd = %summary.cwd.display(),
            ephemeral = summary.ephemeral,
            model = ?summary.model,
            has_title = summary.title.is_some(),
            "started session"
        );
        self.broadcast_event(ServerEvent::SessionStarted(SessionEventPayload {
            session: summary.clone(),
        }))
        .await;
        self.run_session_hook(
            session_id,
            devo_core::HookEvent::SessionStart,
            serde_json::Map::from_iter([
                ("source".to_string(), serde_json::json!("startup")),
                ("model".to_string(), serde_json::json!(model)),
            ]),
        )
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionStartResult { session: summary },
        })
        .expect("serialize session/start response")
    }

    pub(crate) async fn list_session_summaries(&self) -> Vec<SessionMetadata> {
        let mut sessions_by_id = match self.deps.db.list_root_sessions() {
            Ok(sessions) => sessions
                .into_iter()
                .map(|session| (session.session_id, session))
                .collect::<std::collections::HashMap<_, _>>(),
            Err(error) => {
                tracing::warn!(error = %error, "failed to list root sessions from database");
                std::collections::HashMap::new()
            }
        };

        for handle in self.list_session_handles().await {
            let Some(runtime_summary) = handle.summary().await else {
                continue;
            };
            if runtime_summary.ephemeral || runtime_summary.agent_path.is_some() {
                continue;
            }
            sessions_by_id.insert(runtime_summary.session_id, runtime_summary);
        }

        let mut sessions = sessions_by_id.into_values().collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .last_activity_at
                .cmp(&left.last_activity_at)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        sessions
    }

    pub(crate) async fn handle_session_metadata_update(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionMetadataUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/metadata/update params: {error}"),
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
        let Some(mut updated_session) = session_handle
            .update_session_metadata(
                params.model.clone(),
                params.model_binding_id.clone(),
                params.reasoning_effort_selection.clone(),
                params.collaboration_mode,
            )
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if let Some(record) = session_handle.record().await.flatten() {
            if let Err(error) = self.rollout_store.append_session_meta(&record) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist session metadata update: {error}"),
                );
            }
            updated_session = session_handle.summary().await.unwrap_or(updated_session);
        }

        // Persist updated session metadata to SQLite
        if !updated_session.ephemeral
            && let Err(err) = self.deps.db.upsert_session(&updated_session, None)
        {
            tracing::warn!(
                session_id = %params.session_id,
                error = %err,
                "failed to update session metadata in database"
            );
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionMetadataUpdateResult {
                session: updated_session,
            },
        })
        .expect("serialize session/metadata/update response")
    }

    pub(crate) async fn handle_session_permissions_update(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionPermissionsUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/permissions/update params: {error}"),
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
        let Some(summary) = session_handle.summary().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let profile = safety_profile_from_protocol(
            params.preset,
            summary.cwd.clone(),
            summary.additional_directories.clone(),
        );
        if !session_handle
            .apply_permission_profile(profile.clone())
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }
        if let Some(record) = session_handle.record().await.flatten() {
            if let Err(error) = self.rollout_store.append_session_meta(&record) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist session permissions update: {error}"),
                );
            }
        }

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionPermissionsUpdateResult {
                session_id: params.session_id,
                preset: params.preset,
                reviewer: protocol_reviewer_from_safety(profile.reviewer),
            },
        })
        .expect("serialize session/permissions/update response")
    }

    pub(crate) async fn handle_session_compaction_update(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionCompactionUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/compaction/update params: {error}"),
                );
            }
        };
        if params.effective_context_window == 0 {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "effectiveContextWindow must be at least 1",
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
        let Some(summary) = session_handle.summary().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let model = {
            let runtime_context = session_handle.runtime_context().await;
            let catalog = runtime_context
                .as_ref()
                .map(|context| Arc::clone(&context.model_catalog));
            summary
                .model
                .as_deref()
                .and_then(|slug| {
                    catalog
                        .as_ref()
                        .and_then(|catalog| catalog.get(slug).cloned())
                        .or_else(|| self.deps.model_catalog.get(slug).cloned())
                })
                .or_else(|| {
                    summary.model_binding_id.as_deref().and_then(|binding| {
                        catalog
                            .as_ref()
                            .and_then(|catalog| catalog.get(binding).cloned())
                            .or_else(|| self.deps.model_catalog.get(binding).cloned())
                    })
                })
        };
        let Some(model) = model else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "session model is unavailable for compaction update",
            );
        };
        let context_window_tokens = u64::from(model.context_window.max(1));
        let recommended_token_limit =
            u64::from(model.effective_context_window()).min(context_window_tokens);

        {
            let mut store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            if let Err(error) = store.set_compaction_token_limit(params.effective_context_window) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist compaction_token_limit: {error}"),
                );
            }
        }
        self.deps.invalidate_workspace_contexts();

        let global = Some(params.effective_context_window);
        let handles = self.list_session_handles().await;
        for handle in handles {
            let Some(session_summary) = handle.summary().await else {
                continue;
            };
            let runtime_context = handle.runtime_context().await;
            let catalog = runtime_context
                .as_ref()
                .map(|context| Arc::clone(&context.model_catalog));
            let Some(session_model) = session_summary
                .model
                .as_deref()
                .and_then(|slug| {
                    catalog
                        .as_ref()
                        .and_then(|catalog| catalog.get(slug).cloned())
                        .or_else(|| self.deps.model_catalog.get(slug).cloned())
                })
                .or_else(|| {
                    session_summary
                        .model_binding_id
                        .as_deref()
                        .and_then(|binding| {
                            catalog
                                .as_ref()
                                .and_then(|catalog| catalog.get(binding).cloned())
                                .or_else(|| self.deps.model_catalog.get(binding).cloned())
                        })
                })
            else {
                continue;
            };
            let applied = crate::runtime::context_occupancy::resolved_compaction_limit(
                global,
                &session_model,
            );
            match handle
                .apply_effective_context_window(applied as usize)
                .await
            {
                Some(Ok(())) => {
                    self.broadcast_event(ServerEvent::SessionEffectiveContextWindowUpdated(
                        SessionEffectiveContextWindowUpdatedPayload {
                            session_id: session_summary.session_id,
                            effective_context_window: applied,
                        },
                    ))
                    .await;
                }
                Some(Err(_)) | None => continue,
            }
        }

        let applied = crate::runtime::context_occupancy::resolved_compaction_limit(global, &model);

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionCompactionUpdateResult {
                session_id: params.session_id,
                effective_context_window: applied,
                recommended_token_limit,
                context_window_tokens,
            },
        })
        .expect("serialize session/compaction/update response")
    }

    pub(crate) async fn handle_session_sandbox_profile_update(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionSandboxProfileUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/sandbox_profile/update params: {error}"),
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
        let applied = match session_handle
            .apply_sandbox_profile(params.profile.clone())
            .await
        {
            Some(applied) => applied,
            None => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                );
            }
        };
        let profile = match applied {
            Ok(profile) => profile,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid sandbox profile '{}': {error}", params.profile),
                );
            }
        };

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionSandboxProfileUpdateResult {
                session_id: params.session_id,
                profile,
            },
        })
        .expect("serialize session/sandbox_profile/update response")
    }

    pub(crate) async fn handle_session_title_update(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionTitleUpdateParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/title/update params: {error}"),
                );
            }
        };
        let new_title = params.title.trim();
        if new_title.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "session title cannot be empty",
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

        let previous_title = session_handle
            .summary()
            .await
            .and_then(|summary| summary.title);
        let Some(mut summary) = session_handle
            .set_session_title_user_rename(new_title.to_string())
            .await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        if let Some(record) = session_handle.record().await.flatten() {
            if let Err(error) = self.rollout_store.append_title_update(
                &record,
                new_title.to_string(),
                record.title_state.clone(),
                previous_title,
            ) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist session title update: {error}"),
                );
            }
            summary = session_handle.summary().await.unwrap_or(summary);
        }

        // Persist updated session metadata to SQLite
        self.persist_session_summary_if_persistent(params.session_id, &summary)
            .await;

        self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
            session: summary.clone(),
        }))
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionTitleUpdateResult { session: summary },
        })
        .expect("serialize session/title/update response")
    }

    pub(crate) async fn handle_session_resume(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionResumeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/resume params: {error}"),
                );
            }
        };
        self.restore_existing_session_with_tool_registry_update(
            connection_id,
            request_id,
            params,
            RuntimeSessionToolRegistryUpdate::KeepCurrent,
        )
        .await
    }

    pub(crate) async fn restore_existing_session_with_tool_registry_update(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: SessionResumeParams,
        tool_registry_update: RuntimeSessionToolRegistryUpdate,
    ) -> serde_json::Value {
        let session_handle = match self
            .runtime_arc()
            .get_or_load_parent_session(params.session_id)
            .await
        {
            Ok(handle) => handle,
            Err(crate::runtime::session_cache::LoadSessionError::SessionNotFound) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                );
            }
            Err(crate::runtime::session_cache::LoadSessionError::SubagentNotResumable {
                parent_session_id,
            }) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!(
                        "subagent sessions cannot be resumed directly; resume the parent session {parent_session_id} instead"
                    ),
                );
            }
            Err(crate::runtime::session_cache::LoadSessionError::RolloutMissing) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    "session metadata exists but rollout file is missing; session cannot be restored",
                );
            }
            Err(crate::runtime::session_cache::LoadSessionError::RestoreFailed(message)) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to restore session: {message}"),
                );
            }
        };
        let _state_change_guard = session_handle.lock_state_change().await;
        match tool_registry_update {
            RuntimeSessionToolRegistryUpdate::KeepCurrent => {}
            RuntimeSessionToolRegistryUpdate::ReplaceIfCwdMatches { cwd, tool_registry } => {
                let summary = session_handle.summary().await;
                if summary.as_ref().is_none_or(|summary| summary.cwd != cwd) {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "session cwd does not match the stored session cwd",
                    );
                }
                if !session_handle.set_tool_registry(tool_registry).await {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::SessionNotFound,
                        "session does not exist",
                    );
                }
            }
        }
        let Some(resume_snapshot) = session_handle.resume_snapshot().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let session_summary = resume_snapshot.summary;
        let latest_turn = resume_snapshot.latest_turn;
        let loaded_item_count = resume_snapshot.loaded_item_count;
        let history_items = resume_snapshot.history_items;
        let pending_texts = resume_snapshot.pending_texts;
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        self.run_session_hook(
            params.session_id,
            devo_core::HookEvent::SessionStart,
            serde_json::Map::from_iter([("source".to_string(), serde_json::json!("resume"))]),
        )
        .await;
        tracing::info!(
            connection_id,
            session_id = %params.session_id,
            loaded_item_count,
            has_latest_turn = latest_turn.is_some(),
            pending_count = pending_texts.len(),
            "resumed session"
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionResumeResult {
                session: session_summary,
                latest_turn,
                loaded_item_count,
                history_items,
                pending_texts,
            },
        })
        .expect("serialize session/resume response")
    }

    pub(crate) async fn handle_session_fork(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionForkParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/fork params: {error}"),
                );
            }
        };
        let Some(source_handle) = self.session(params.session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let Some(source) = source_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let source = &source;
        let now = Utc::now();
        let forked_id = SessionId::new();
        let mut forked_runtime = match self
            .build_runtime_session_from_user_turn_cut(
                source,
                RuntimeSessionTurnCutOptions {
                    session_id: forked_id,
                    user_turn_index: params.user_turn_index,
                    rollback_mode: SessionRollbackMode::ThroughUserTurn,
                    cwd_override: params.cwd.clone(),
                    title_override: params.title.clone(),
                    created_at: now,
                },
            )
            .await
        {
            Ok(runtime) => runtime,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        forked_runtime.summary.parent_session_id = Some(params.session_id);
        if !forked_runtime.summary.ephemeral {
            let record = self.rollout_store.create_session_record(
                forked_id,
                now,
                forked_runtime.summary.cwd.clone(),
                forked_runtime.summary.additional_directories.clone(),
                forked_runtime.summary.title.clone(),
                forked_runtime.summary.model.clone(),
                forked_runtime.summary.model_binding_id.clone(),
                forked_runtime.summary.reasoning_effort_selection.clone(),
                forked_runtime.runtime_context.provider.name().to_string(),
                Some(params.session_id),
            );
            if let Err(error) = self.rollout_store.append_session_meta(&record) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist forked session metadata: {error}"),
                );
            }
            forked_runtime.record = Some(record);
        }
        let summary = forked_runtime.summary.clone();
        let rollout_path_for_db = forked_runtime
            .record
            .as_ref()
            .map(|entry| entry.rollout_path.clone());
        self.insert_session_actor(SessionActorState::from_runtime_session(forked_runtime))
            .await;
        self.subscribe_connection_to_session(connection_id, forked_id, None)
            .await;
        self.runtime_arc()
            .after_root_session_insert(forked_id)
            .await;
        if !summary.ephemeral
            && let Err(err) = self
                .deps
                .db
                .upsert_session(&summary, rollout_path_for_db.as_deref())
        {
            tracing::warn!(
                session_id = %forked_id,
                error = %err,
                "failed to persist forked session metadata to database"
            );
        }
        if !summary.ephemeral {
            let stats = crate::db::SessionStats {
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_tokens: 0,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
                last_input_tokens: 0,
                turn_count: 0,
                prompt_token_estimate: summary.prompt_token_estimate,
                last_context_occupancy: summary.last_context_occupancy.clone(),
            };
            if let Err(err) = self.deps.db.update_stats(&forked_id, &stats) {
                tracing::warn!(
                    session_id = %forked_id,
                    error = %err,
                    "failed to persist forked session token stats to database"
                );
            }
        }
        tracing::info!(
            connection_id,
            source_session_id = %params.session_id,
            forked_session_id = %forked_id,
            cwd = %summary.cwd.display(),
            ephemeral = summary.ephemeral,
            model = ?summary.model,
            "forked session"
        );
        self.broadcast_event(ServerEvent::SessionStarted(SessionEventPayload {
            session: summary.clone(),
        }))
        .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionForkResult {
                session: summary,
                forked_from_session_id: params.session_id,
            },
        })
        .expect("serialize session/fork response")
    }

    pub(crate) async fn handle_session_rollback(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionRollbackParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/rollback params: {error}"),
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
        if self
            .runtime_active_turn_id(params.session_id)
            .await
            .is_some()
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot rollback while a turn is active",
            );
        }
        let _state_change_guard = session_handle.lock_state_change().await;
        if self
            .runtime_active_turn_id(params.session_id)
            .await
            .is_some()
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot rollback while a turn is active",
            );
        }
        let Some(source) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let source = &source;
        let mut rebuilt = match self
            .build_runtime_session_from_user_turn_cut(
                source,
                RuntimeSessionTurnCutOptions {
                    session_id: params.session_id,
                    user_turn_index: Some(params.user_turn_index),
                    rollback_mode: params.mode,
                    cwd_override: None,
                    title_override: source.summary.title.clone(),
                    created_at: source.summary.created_at,
                },
            )
            .await
        {
            Ok(runtime) => runtime,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        let record = source.record.clone();
        let (retained_turn_ids, retained_item_ids) =
            retained_ids_for_persisted_items(&rebuilt.persisted_turn_items);
        let latest_turn_id = rebuilt.latest_turn.as_ref().map(|turn| turn.turn_id);
        if let Some(record) = record.clone()
            && let Err(error) = self.rollout_store.append_session_rollback(
                &record,
                retained_turn_ids,
                retained_item_ids,
                latest_turn_id,
            )
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist session rollback: {error}"),
            );
        }
        rebuilt.record = record;
        let summary = rebuilt.summary.clone();
        let latest_turn = rebuilt.latest_turn.clone();
        let loaded_item_count = rebuilt.loaded_item_count;
        let history_items = rebuilt.history_items.clone();
        if !summary.ephemeral {
            let stats = crate::db::SessionStats {
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_tokens: 0,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
                last_input_tokens: 0,
                turn_count: 0,
                prompt_token_estimate: summary.prompt_token_estimate,
                last_context_occupancy: summary.last_context_occupancy.clone(),
            };
            if let Err(err) = self.deps.db.update_stats(&params.session_id, &stats) {
                tracing::warn!(
                    session_id = %params.session_id,
                    error = %err,
                    "failed to persist rolled-back session token stats to database"
                );
            }
        }
        session_handle
            .replace_state(SessionActorState::from_runtime_session(rebuilt))
            .await;
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: SessionRollbackResult {
                session: summary,
                latest_turn,
                loaded_item_count,
                history_items,
                pending_texts: Vec::new(),
            },
        })
        .expect("serialize session/rollback response")
    }

    pub(crate) async fn build_runtime_session_from_user_turn_cut(
        &self,
        source: &RuntimeSession,
        options: RuntimeSessionTurnCutOptions,
    ) -> Result<RuntimeSession, String> {
        let RuntimeSessionTurnCutOptions {
            session_id,
            user_turn_index,
            rollback_mode,
            cwd_override,
            title_override,
            created_at,
        } = options;
        let source_core_session = source.core_session.lock().await;
        let kept_items = kept_items_for_user_turn_cut(
            &source.persisted_turn_items,
            user_turn_index,
            rollback_mode,
        )?;

        let cwd = cwd_override.unwrap_or_else(|| source.summary.cwd.clone());
        let additional_directories = source.summary.additional_directories.clone();
        let runtime_context = if cwd == source.summary.cwd {
            Arc::clone(&source.runtime_context)
        } else {
            self.deps
                .context_for_workspace(&cwd)
                .await
                .map_err(|error| format!("failed to initialize session workspace: {error}"))?
        };
        let mut core_session = runtime_context.new_session_state(
            session_id,
            cwd.clone(),
            additional_directories.clone(),
        );
        core_session.config = source_core_session.config.clone();
        core_session.session_context = source_core_session.session_context.clone();
        core_session.collaboration_mode = source_core_session.collaboration_mode;
        core_session.latest_turn_context = None;
        // Fork/rollback starts a new cumulative ledger at the cut point.
        core_session.total_input_tokens = 0;
        core_session.total_output_tokens = 0;
        core_session.total_tokens = 0;
        core_session.total_cache_creation_tokens = 0;
        core_session.total_cache_read_tokens = 0;
        core_session.last_input_tokens = 0;
        core_session.last_turn_tokens = 0;

        let mut rebuilt_history_items = Vec::new();
        let mut rebuilt_messages = Vec::new();
        let mut tool_names_by_id = HashMap::new();
        for item in &kept_items {
            crate::persistence::apply_turn_item(
                &mut rebuilt_messages,
                &mut rebuilt_history_items,
                &mut tool_names_by_id,
                &item.turn_kind,
                item.turn_item.clone(),
            );
        }
        core_session.messages = rebuilt_messages;
        core_session.prompt_messages = None;
        core_session.turn_count = kept_items
            .iter()
            .filter(|item| matches!(item.turn_item, TurnItem::UserMessage(_)))
            .count();

        let last_kept_turn_id = kept_items.last().map(|item| item.turn_id);
        let kept_turn_ids: HashSet<_> = kept_items.iter().map(|item| item.turn_id).collect();
        let (cut_occupancy, latest_query_usage, applicable_compaction) =
            resolve_cut_occupancy_and_usage(
                &kept_turn_ids,
                last_kept_turn_id,
                &source.turn_records_by_id,
                source.latest_compaction_snapshot.as_ref(),
            );
        let cut_turn_record =
            last_kept_turn_id.and_then(|turn_id| source.turn_records_by_id.get(&turn_id).cloned());
        let prompt_token_estimate = cut_occupancy
            .as_ref()
            .map(|occupancy| occupancy.total_tokens as usize)
            .or_else(|| {
                latest_query_usage
                    .as_ref()
                    .map(devo_protocol::TurnUsage::display_total_tokens)
            })
            .unwrap_or(0);
        core_session.prompt_token_estimate = prompt_token_estimate;

        let latest_turn = if let Some(last_turn_id) = last_kept_turn_id {
            source
                .latest_turn
                .clone()
                .filter(|turn| turn.turn_id == last_turn_id)
                .or_else(|| {
                    let model = source
                        .summary
                        .model
                        .clone()
                        .unwrap_or_else(|| runtime_context.default_model.clone());
                    // Synthetic fork metadata follows normal turn semantics:
                    // `model` remains the catalog slug, while `request_model`
                    // is recomputed from the active provider binding.
                    let request_model = runtime_context
                        .resolve_turn_config(
                            source
                                .summary
                                .model_binding_id
                                .as_deref()
                                .or(Some(model.as_str())),
                            source.summary.reasoning_effort_selection.clone(),
                        )
                        .request_model;
                    let sequence = kept_items
                        .iter()
                        .filter(|item| matches!(item.turn_item, TurnItem::UserMessage(_)))
                        .count() as u32;
                    Some(TurnMetadata {
                        turn_id: last_turn_id,
                        session_id,
                        sequence,
                        status: TurnStatus::Completed,
                        kind: devo_protocol::TurnKind::Regular,
                        model,
                        model_binding_id: source.summary.model_binding_id.clone(),
                        reasoning_effort_selection: source
                            .summary
                            .reasoning_effort_selection
                            .clone(),
                        reasoning_effort: source.summary.reasoning_effort,
                        request_model,
                        request_thinking: source.summary.reasoning_effort_selection.clone(),
                        started_at: source.summary.created_at,
                        completed_at: Some(source.summary.updated_at),
                        usage: cut_turn_record.as_ref().and_then(|turn| turn.usage.clone()),
                        stop_reason: None,
                        failure_reason: None,
                    })
                })
        } else {
            None
        };

        let updated_at = Utc::now();
        let summary = crate::SessionMetadata {
            session_id,
            cwd: cwd.clone(),
            additional_directories,
            created_at,
            updated_at,
            last_activity_at: updated_at,
            title: title_override.or_else(|| source.summary.title.clone()),
            title_state: source.summary.title_state.clone(),
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: source.summary.ephemeral,
            model: source.summary.model.clone(),
            model_binding_id: source.summary.model_binding_id.clone(),
            reasoning_effort_selection: source.summary.reasoning_effort_selection.clone(),
            reasoning_effort: source.summary.reasoning_effort,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate,
            last_query_usage: latest_query_usage.clone(),
            last_query_total_tokens: cut_occupancy
                .as_ref()
                .map(|occupancy| occupancy.total_tokens as usize)
                .or_else(|| {
                    latest_query_usage
                        .as_ref()
                        .map(devo_protocol::TurnUsage::display_total_tokens)
                })
                .unwrap_or(0),
            last_context_occupancy: cut_occupancy.clone(),
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: core_session.collaboration_mode,
            effective_context_window: None,
            permission_preset: None,
        };
        drop(source_core_session);

        let turn_records_by_id = source
            .turn_records_by_id
            .iter()
            .filter(|(turn_id, _)| kept_turn_ids.contains(turn_id))
            .map(|(turn_id, record)| (*turn_id, record.clone()))
            .collect();

        core_session.pending_turn_queue = Arc::clone(&source.pending_turn_queue);
        core_session.steer_input_queue = Arc::clone(&source.steer_input_queue);
        let config = core_session.config.clone();
        let pending_turn_queue = Arc::clone(&source.pending_turn_queue);
        let steer_input_queue = Arc::clone(&source.steer_input_queue);
        Ok(RuntimeSession {
            runtime_context,
            record: None,
            summary,
            config,
            core_session: Arc::new(Mutex::new(core_session)),
            active_turn: None,
            latest_turn,
            loaded_item_count: u64::try_from(kept_items.len()).unwrap_or(u64::MAX),
            history_items: rebuilt_history_items,
            persisted_turn_items: kept_items,
            latest_compaction_snapshot: applicable_compaction,
            turn_records_by_id,
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: source.agent_tool_policy,
            max_turns: source.max_turns,
            deferred_assistant: None,
            deferred_reasoning: None,
            next_item_seq: u64::try_from(source.persisted_turn_items.len().saturating_add(1))
                .unwrap_or(u64::MAX),
            first_user_input: source.first_user_input.clone(),
            tool_registry: source.tool_registry.clone(),
            file_read_ledger: Arc::clone(&source.file_read_ledger),
            session_approval_cache: source.session_approval_cache.clone(),
            turn_approval_cache: source.turn_approval_cache.clone(),
            session_context_recorded: source.session_context_recorded,
        })
    }
}

fn kept_items_for_user_turn_cut(
    persisted_turn_items: &[crate::execution::PersistedTurnItem],
    user_turn_index: Option<u32>,
    rollback_mode: SessionRollbackMode,
) -> Result<Vec<crate::execution::PersistedTurnItem>, String> {
    let Some(user_turn_index) = user_turn_index else {
        return Ok(persisted_turn_items.to_vec());
    };

    let mut user_turn_ids: Vec<TurnId> = Vec::new();
    for item in persisted_turn_items {
        if matches!(item.turn_item, TurnItem::UserMessage(_))
            && user_turn_ids.last().copied() != Some(item.turn_id)
        {
            user_turn_ids.push(item.turn_id);
        }
    }
    let selected_idx = usize::try_from(user_turn_index)
        .map_err(|_| "selected turn index is invalid".to_string())?;
    let Some(selected_turn_id) = user_turn_ids.get(selected_idx).copied() else {
        return Err("selected turn does not exist".to_string());
    };

    match rollback_mode {
        SessionRollbackMode::ThroughUserTurn => Ok(persisted_turn_items
            .iter()
            .take_while(|item| item.turn_id != selected_turn_id)
            .cloned()
            .chain(
                persisted_turn_items
                    .iter()
                    .skip_while(|item| item.turn_id != selected_turn_id)
                    .take_while(|item| item.turn_id == selected_turn_id)
                    .cloned(),
            )
            .collect()),
        SessionRollbackMode::BeforeUserTurn => Ok(persisted_turn_items
            .iter()
            .take_while(|item| item.turn_id != selected_turn_id)
            .cloned()
            .collect()),
    }
}

fn retained_ids_for_persisted_items(
    items: &[crate::execution::PersistedTurnItem],
) -> (Vec<TurnId>, Vec<devo_core::ItemId>) {
    let mut retained_turn_ids = Vec::new();
    let mut retained_item_ids = Vec::new();
    for item in items {
        if !retained_turn_ids.contains(&item.turn_id) {
            retained_turn_ids.push(item.turn_id);
        }
        if !retained_item_ids.contains(&item.item_id) {
            retained_item_ids.push(item.item_id);
        }
    }
    (retained_turn_ids, retained_item_ids)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn user_item(turn_id: TurnId, text: &str) -> crate::execution::PersistedTurnItem {
        crate::execution::PersistedTurnItem {
            turn_id,
            turn_kind: devo_core::TurnKind::Regular,
            item_id: devo_core::ItemId::new(),
            turn_item: TurnItem::UserMessage(devo_core::TextItem {
                text: text.to_string(),
            }),
        }
    }

    fn assistant_item(turn_id: TurnId, text: &str) -> crate::execution::PersistedTurnItem {
        crate::execution::PersistedTurnItem {
            turn_id,
            turn_kind: devo_core::TurnKind::Regular,
            item_id: devo_core::ItemId::new(),
            turn_item: TurnItem::AgentMessage(devo_core::TextItem {
                text: text.to_string(),
            }),
        }
    }

    #[test]
    fn kept_items_for_user_turn_cut_keeps_selected_turn_in_legacy_mode() {
        let first_turn_id = TurnId::new();
        let second_turn_id = TurnId::new();
        let items = vec![
            user_item(first_turn_id, "first user"),
            assistant_item(first_turn_id, "first answer"),
            user_item(second_turn_id, "second user"),
            assistant_item(second_turn_id, "second answer"),
        ];

        let kept = kept_items_for_user_turn_cut(
            &items,
            Some(/*user_turn_index*/ 1),
            SessionRollbackMode::ThroughUserTurn,
        )
        .expect("keep selected turn");

        assert_eq!(kept, items);
    }

    #[test]
    fn kept_items_for_user_turn_cut_can_drop_selected_turn() {
        let first_turn_id = TurnId::new();
        let second_turn_id = TurnId::new();
        let items = vec![
            user_item(first_turn_id, "first user"),
            assistant_item(first_turn_id, "first answer"),
            user_item(second_turn_id, "second user"),
            assistant_item(second_turn_id, "second answer"),
        ];

        let kept = kept_items_for_user_turn_cut(
            &items,
            Some(/*user_turn_index*/ 1),
            SessionRollbackMode::BeforeUserTurn,
        )
        .expect("drop selected turn");

        assert_eq!(kept, items[..2]);
    }

    #[test]
    fn kept_items_for_user_turn_cut_can_drop_the_first_turn() {
        let turn_id = TurnId::new();
        let items = vec![
            user_item(turn_id, "first user"),
            assistant_item(turn_id, "first answer"),
        ];

        let kept = kept_items_for_user_turn_cut(
            &items,
            Some(/*user_turn_index*/ 0),
            SessionRollbackMode::BeforeUserTurn,
        )
        .expect("drop first turn");

        assert_eq!(kept, Vec::new());
    }

    #[test]
    fn cut_occupancy_uses_cut_turn_not_tip() {
        use pretty_assertions::assert_eq;

        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let occupancy_a = devo_protocol::canonical::item::ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 20_000,
        );
        let occupancy_b = devo_protocol::canonical::item::ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 80_000,
        );
        let mut records = std::collections::HashMap::new();
        records.insert(
            turn_a,
            devo_core::TurnRecord {
                id: turn_a,
                session_id: SessionId::new(),
                sequence: 1,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                status: TurnStatus::Completed,
                kind: devo_core::TurnKind::Regular,
                model: "m".into(),
                model_binding_id: None,
                reasoning_effort_selection: None,
                request_model: "m".into(),
                request_thinking: None,
                input_token_estimate: None,
                usage: Some(devo_protocol::TurnUsage {
                    input_tokens: 30,
                    output_tokens: 5,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: Some(35),
                }),
                latest_query_usage: Some(devo_protocol::TurnUsage {
                    input_tokens: 30,
                    output_tokens: 5,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: Some(35),
                }),
                context_occupancy: Some(occupancy_a.clone()),
                stop_reason: None,
                failure_reason: None,
                error: None,
                session_context: None,
                turn_context: None,
                schema_version: 4,
            },
        );
        records.insert(
            turn_b,
            devo_core::TurnRecord {
                id: turn_b,
                session_id: SessionId::new(),
                sequence: 2,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                status: TurnStatus::Completed,
                kind: devo_core::TurnKind::Regular,
                model: "m".into(),
                model_binding_id: None,
                reasoning_effort_selection: None,
                request_model: "m".into(),
                request_thinking: None,
                input_token_estimate: None,
                usage: None,
                latest_query_usage: Some(devo_protocol::TurnUsage {
                    input_tokens: 90,
                    output_tokens: 10,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    reasoning_output_tokens: None,
                    total_tokens: Some(100),
                }),
                context_occupancy: Some(occupancy_b.clone()),
                stop_reason: None,
                failure_reason: None,
                error: None,
                session_context: None,
                turn_context: None,
                schema_version: 4,
            },
        );

        let kept = [turn_a].into_iter().collect();
        let (occupancy, usage, compact) =
            resolve_cut_occupancy_and_usage(&kept, Some(turn_a), &records, None);
        assert_eq!(occupancy, Some(occupancy_a.clone()));
        assert_eq!(usage.map(|u| u.input_tokens), Some(30));
        assert!(compact.is_none());

        // Compaction on a discarded tip turn must not override the cut turn.
        let tip_only_snapshot = devo_core::CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: turn_b,
            summary_item_id: ItemId::new(),
            preserved_item_ids: Vec::new(),
            context_occupancy: Some(occupancy_b.clone()),
        };
        let (occupancy, usage, compact) = resolve_cut_occupancy_and_usage(
            &kept,
            Some(turn_a),
            &records,
            Some(&tip_only_snapshot),
        );
        assert_eq!(occupancy, Some(occupancy_a));
        assert_eq!(usage.map(|u| u.input_tokens), Some(30));
        assert!(compact.is_none());

        let kept_all = [turn_a, turn_b].into_iter().collect();
        let compact_occupancy =
            devo_protocol::canonical::item::ContextOccupancy::from_category_tokens(
                /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
                /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 5_000,
            );
        let snapshot = devo_core::CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: turn_b,
            summary_item_id: ItemId::new(),
            preserved_item_ids: Vec::new(),
            context_occupancy: Some(compact_occupancy.clone()),
        };
        let (occupancy, usage, compact) =
            resolve_cut_occupancy_and_usage(&kept_all, Some(turn_b), &records, Some(&snapshot));
        assert_eq!(occupancy, Some(compact_occupancy));
        assert_eq!(usage.map(|u| u.input_tokens), Some(90));
        assert_eq!(compact.as_ref().map(|s| s.turn_id), Some(turn_b));
    }
}
