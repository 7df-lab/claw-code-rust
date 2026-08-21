use super::*;

const ACP_SESSION_LIST_PAGE_SIZE: usize = 50;

/// Accept the pre-ACP session creation shape used by existing Devo clients.
///
/// ACP v1 describes `mcpServers` as part of the session lifecycle request
/// contract, while the older Devo API omitted it when no MCP servers were
/// configured. The adapter normalizes that omission to the ACP empty-list
/// value before deserialization; supplied MCP entries still go through the
/// normal ACP validation path.
fn normalize_missing_mcp_servers(mut params: serde_json::Value) -> serde_json::Value {
    if let Some(object) = params.as_object_mut() {
        object
            .entry("mcpServers")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    params
}

impl ServerRuntime {
    pub(crate) async fn handle_acp_session_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpListSessionsParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/list params: {error}"),
                );
            }
        };
        if let Some(cwd) = params.cwd.as_ref()
            && !cwd.is_absolute()
        {
            return acp_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "session/list cwd must be an absolute path",
            );
        }
        let start = match params.cursor.as_deref().map(decode_session_list_cursor) {
            Some(Ok(start)) => start,
            Some(Err(error)) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
            None => 0,
        };
        let sessions = self.list_session_summaries().await;
        let mut filtered_sessions = Vec::new();
        for session in sessions.iter().filter(|session| {
            params
                .cwd
                .as_ref()
                .is_none_or(|cwd| session.cwd.as_path() == cwd.as_path())
        }) {
            if !session.cwd.is_absolute() {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InternalError,
                    format!(
                        "stored session {} cwd is not an absolute path",
                        session.session_id
                    ),
                );
            }
            filtered_sessions.push(acp_session_info_from_metadata(session));
        }
        if start > filtered_sessions.len() {
            return acp_error_response(
                request_id,
                AcpErrorCode::InvalidParams,
                "session/list cursor is out of range",
            );
        }
        let next_start = start.saturating_add(ACP_SESSION_LIST_PAGE_SIZE);
        let next_cursor =
            (next_start < filtered_sessions.len()).then(|| encode_session_list_cursor(next_start));
        let sessions = filtered_sessions
            .into_iter()
            .skip(start)
            .take(ACP_SESSION_LIST_PAGE_SIZE)
            .collect();
        acp_success_response(
            request_id,
            AcpListSessionsResult {
                sessions,
                next_cursor,
                meta: None,
            },
        )
    }

    pub(crate) async fn handle_acp_session_load(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = normalize_missing_mcp_servers(params);
        let params: AcpLoadSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/load params: {error}"),
                );
            }
        };
        if let Err(error) =
            validate_acp_session_roots("session/load", &params.cwd, &params.additional_directories)
        {
            return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
        }
        if let Err((code, error)) = self
            .validate_acp_existing_session_cwd("session/load", params.session_id, &params.cwd)
            .await
        {
            return acp_error_response(request_id, code, error);
        }
        let tool_registry = match self
            .acp_session_tool_registry("session/load", &params.mcp_servers, &params.cwd)
            .await
        {
            Ok(tool_registry) => tool_registry,
            Err(error) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
        };
        let legacy_response = self
            .restore_existing_session_with_tool_registry_update(
                connection_id,
                request_id.clone(),
                SessionResumeParams {
                    session_id: params.session_id,
                },
                RuntimeSessionToolRegistryUpdate::ReplaceIfCwdMatches {
                    cwd: params.cwd.clone(),
                    tool_registry,
                },
            )
            .await;
        let mut legacy: SuccessResponse<SessionResumeResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let updated_summary = match self
            .apply_acp_session_additional_directories(
                params.session_id,
                params.additional_directories.clone(),
            )
            .await
        {
            Ok(summary) => summary,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        legacy.result.session = updated_summary;
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        self.send_acp_history_updates(
            connection_id,
            params.session_id,
            &legacy.result.history_items,
        )
        .await;
        let config_options = match self.acp_session_config_options(params.session_id).await {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpLoadSessionResult {
                modes: None,
                config_options: Some(config_options),
                meta: None,
            },
        )
    }

    pub(crate) async fn handle_acp_session_new(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = normalize_missing_mcp_servers(params);
        let params: AcpNewSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/new params: {error}"),
                );
            }
        };
        if let Err(error) =
            validate_acp_session_roots("session/new", &params.cwd, &params.additional_directories)
        {
            return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
        }
        let tool_registry = match self
            .acp_session_tool_registry("session/new", &params.mcp_servers, &params.cwd)
            .await
        {
            Ok(tool_registry) => tool_registry,
            Err(error) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
        };
        let legacy_response = self
            .start_session_with_registry(
                connection_id,
                request_id.clone(),
                SessionStartParams {
                    cwd: params.cwd,
                    additional_directories: params.additional_directories,
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                tool_registry,
            )
            .await;
        let legacy: SuccessResponse<SessionStartResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let mut meta = serde_json::Map::new();
        meta.insert(
            DEVO_SESSION_META.to_string(),
            serde_json::to_value(&legacy.result.session).expect("serialize session metadata"),
        );
        self.subscribe_connection_to_session(connection_id, legacy.result.session.session_id, None)
            .await;
        let config_options = match self
            .acp_session_config_options(legacy.result.session.session_id)
            .await
        {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpNewSessionResult {
                session_id: legacy.result.session.session_id,
                modes: None,
                config_options: Some(config_options),
                meta: Some(meta),
            },
        )
    }

    pub(crate) async fn handle_acp_session_resume(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = normalize_missing_mcp_servers(params);
        let params: AcpResumeSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/resume params: {error}"),
                );
            }
        };
        if let Err(error) = validate_acp_session_roots(
            "session/resume",
            &params.cwd,
            &params.additional_directories,
        ) {
            return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
        }
        if let Err((code, error)) = self
            .validate_acp_existing_session_cwd("session/resume", params.session_id, &params.cwd)
            .await
        {
            return acp_error_response(request_id, code, error);
        }
        let tool_registry = match self
            .acp_session_tool_registry("session/resume", &params.mcp_servers, &params.cwd)
            .await
        {
            Ok(tool_registry) => tool_registry,
            Err(error) => {
                return acp_error_response(request_id, AcpErrorCode::InvalidParams, error);
            }
        };
        let legacy_response = self
            .restore_existing_session_with_tool_registry_update(
                connection_id,
                request_id.clone(),
                SessionResumeParams {
                    session_id: params.session_id,
                },
                RuntimeSessionToolRegistryUpdate::ReplaceIfCwdMatches {
                    cwd: params.cwd.clone(),
                    tool_registry,
                },
            )
            .await;
        let mut legacy: SuccessResponse<SessionResumeResult> =
            match serde_json::from_value(legacy_response.clone()) {
                Ok(legacy) => legacy,
                Err(_) => return legacy_error_to_acp(request_id, legacy_response),
            };
        let updated_summary = match self
            .apply_acp_session_additional_directories(
                params.session_id,
                params.additional_directories.clone(),
            )
            .await
        {
            Ok(summary) => summary,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        legacy.result.session = updated_summary;
        let mut meta = serde_json::Map::new();
        meta.insert(
            DEVO_SESSION_META.to_string(),
            serde_json::to_value(&legacy.result.session).expect("serialize session metadata"),
        );
        meta.insert(
            DEVO_SESSION_RESUME_META.to_string(),
            serde_json::to_value(&legacy.result).expect("serialize session resume result"),
        );
        self.subscribe_connection_to_session(connection_id, params.session_id, None)
            .await;
        let config_options = match self.acp_session_config_options(params.session_id).await {
            Ok(config_options) => config_options,
            Err(error) => return acp_error_response(request_id, AcpErrorCode::ServerError, error),
        };
        acp_success_response(
            request_id,
            AcpResumeSessionResult {
                modes: None,
                config_options: Some(config_options),
                meta: Some(meta),
            },
        )
    }

    pub(crate) async fn handle_acp_session_close(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpCloseSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/close params: {error}"),
                );
            }
        };
        if !self.sessions.lock().await.contains_key(&params.session_id) {
            return acp_error_response(
                request_id,
                AcpErrorCode::ResourceNotFound,
                "session does not exist",
            );
        }
        self.handle_acp_session_cancel(
            serde_json::to_value(AcpCancelParams {
                session_id: params.session_id,
                meta: None,
            })
            .expect("serialize ACP cancel params"),
        )
        .await;
        self.remove_session_actor(params.session_id).await;
        acp_success_response(request_id, AcpCloseSessionResult::default())
    }

    pub(crate) async fn handle_acp_session_delete(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpDeleteSessionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/delete params: {error}"),
                );
            }
        };
        let deleted_session_ids = match self.delete_session_tree(params.session_id).await {
            Ok(deleted_session_ids) => deleted_session_ids,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InternalError,
                    format!("failed to delete session: {error}"),
                );
            }
        };
        if !deleted_session_ids.is_empty() {
            self.broadcast_event(ServerEvent::SessionDeleted(SessionDeletedPayload {
                session_id: params.session_id,
                deleted_session_ids,
            }))
            .await;
        }
        acp_success_response(request_id, AcpDeleteSessionResult::default())
    }

    pub(crate) async fn delete_session_tree(
        self: &Arc<Self>,
        root_session_id: SessionId,
    ) -> Result<Vec<SessionId>, String> {
        let session_ids = self.collect_session_delete_tree(root_session_id).await;
        for session_id in &session_ids {
            self.await_session_turn_interrupt_before_delete(*session_id)
                .await;
        }

        let mut deleted_session_ids = Vec::new();
        for session_id in session_ids {
            let removed = self.sessions.lock().await.remove(&session_id);
            self.clear_deleted_session_runtime_state(session_id).await;
            let persisted = self
                .deps
                .db
                .get_session(&session_id)
                .map_err(|error| format!("failed to inspect session before delete: {error}"))?;
            let deleted_rollout = self
                .rollout_store
                .delete_session_rollouts(&session_id)
                .map_err(|error| format!("failed to delete session rollout: {error}"))?;
            if persisted.is_some() {
                self.deps
                    .db
                    .clear_pending(&session_id, crate::db::QueueType::Turn)
                    .map_err(|error| format!("failed to clear pending turn queue: {error}"))?;
                self.deps
                    .db
                    .clear_pending(&session_id, crate::db::QueueType::Steer)
                    .map_err(|error| format!("failed to clear pending steer queue: {error}"))?;
                self.deps
                    .db
                    .delete_session(&session_id)
                    .map_err(|error| format!("failed to delete session metadata: {error}"))?;
            }
            if removed.is_some() || persisted.is_some() || deleted_rollout {
                deleted_session_ids.push(session_id);
            }
        }
        Ok(deleted_session_ids)
    }

    async fn collect_session_delete_tree(&self, root_session_id: SessionId) -> Vec<SessionId> {
        let session_ids_in_runtime: Vec<SessionId> = {
            let sessions = self.sessions.lock().await;
            sessions.keys().copied().collect()
        };
        let mut parent_by_session = Vec::new();
        for session_id in session_ids_in_runtime {
            let parent_id = self.session_parent_id_snapshot(session_id).await.flatten();
            parent_by_session.push((session_id, parent_id));
        }
        parent_by_session.sort_by_key(|(session_id, _parent_id)| session_id.to_string());

        let mut seen = std::collections::HashSet::new();
        let mut session_ids = Vec::new();
        seen.insert(root_session_id);
        session_ids.push(root_session_id);
        let mut index = 0;
        while index < session_ids.len() {
            let parent_session_id = session_ids[index];
            for (session_id, parent_id) in &parent_by_session {
                if *parent_id == Some(parent_session_id) && seen.insert(*session_id) {
                    session_ids.push(*session_id);
                }
            }
            index += 1;
        }
        session_ids
    }

    async fn await_session_turn_interrupt_before_delete(self: &Arc<Self>, session_id: SessionId) {
        let Some(turn_id) = self.runtime_active_turn_id(session_id).await else {
            return;
        };
        let receiver = self.subscribe_terminal_turn_status(turn_id).await;
        if self.recent_terminal_turn_status(turn_id).await.is_some() {
            return;
        }
        self.signal_active_turn_interrupt(session_id).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), receiver).await;
    }

    async fn clear_deleted_session_runtime_state(&self, session_id: SessionId) {
        self.signal_active_turn_interrupt(session_id).await;
        self.active_turns.clear_runtime_handles(session_id).await;
        if let Some(turn_id) = self
            .active_goal_continuation_turns
            .lock()
            .await
            .remove(&session_id)
        {
            self.goal_continuation_turn_goals
                .lock()
                .await
                .remove(&turn_id);
        }
        self.goal_stores.lock().await.remove(&session_id);
        self.agent_mailboxes.lock().await.remove(&session_id);
        self.agent_output_buffers.lock().await.remove(&session_id);
        self.agent_wait_cursors.lock().await.remove(&session_id);
        {
            let mut registries = self.agent_registries.lock().await;
            registries.remove(&session_id);
            for registry in registries.values_mut() {
                registry.unregister(session_id);
            }
        }
    }

    pub(crate) async fn handle_acp_session_set_mode(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let _params: AcpSetModeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/set_mode params: {error}"),
                );
            }
        };
        acp_error_response(
            request_id,
            AcpErrorCode::MethodNotFound,
            "session/set_mode is not supported",
        )
    }

    pub(crate) async fn handle_acp_session_set_config_option(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: AcpSetConfigOptionParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid session/set_config_option params: {error}"),
                );
            }
        };
        let config_options = match self.set_acp_session_config_option(params).await {
            Ok(config_options) => config_options,
            Err((code, message)) => return acp_error_response(request_id, code, message),
        };
        acp_success_response(
            request_id,
            AcpSetConfigOptionResult {
                config_options,
                meta: None,
            },
        )
    }
}
