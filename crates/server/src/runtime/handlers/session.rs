use super::super::*;

use devo_core::SessionSettingsField;
use devo_protocol::native::rpc_session::RollbackMode;

/// Default page size for canonical `session/list` when no limit is given.
const CANONICAL_SESSION_LIST_DEFAULT_LIMIT: usize = 50;

fn session_list_cwd_matches(cwds: &[std::path::PathBuf], cwd: &std::path::PathBuf) -> bool {
    if cwds.is_empty() {
        return true;
    }
    cwds.iter().any(|filter| {
        if filter == cwd {
            return true;
        }
        normalize_session_list_cwd(filter) == normalize_session_list_cwd(cwd)
    })
}

fn normalize_session_list_cwd(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

pub(crate) struct RuntimeSessionTurnCutOptions {
    pub(crate) session_id: SessionId,
    pub(crate) user_turn_index: Option<u32>,
    pub(crate) rollback_mode: RollbackMode,
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
    Option<devo_protocol::native::item::ContextOccupancy>,
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
        summary.permission_preset = Some(protocol_preset_from_safety(
            core_session.config.permission_profile.preset,
        ));
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

    /// Native `session/metadata/update` (L2-DES-APP-008 DD-4/DD-5,
    /// L2-DES-CONV-002 Phase 2): persist-first — settings field lines are
    /// written synchronously and the response is built from the rollout,
    /// never waiting on the session actor; the actor is notified best-effort
    /// (mailbox FIFO guarantees application before the next turn). Ephemeral
    /// sessions have no rollout: field lines are skipped (there is nothing
    /// to persist by design) and the snapshot is built from the SQLite
    /// index instead.
    pub(crate) async fn handle_native_session_metadata_update(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionMetadataUpdateParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/metadata/update params: {error}"),
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
        // Title patch: `Value` renames through the session actor and persists
        // the title update before the canonical snapshot is read below.
        match &params.title {
            devo_protocol::native::patch::PatchField::Missing => {}
            devo_protocol::native::patch::PatchField::Null => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "clearing the session title is not supported (titles cannot be empty)",
                );
            }
            devo_protocol::native::patch::PatchField::Value(title) => {
                let new_title = title.trim();
                if new_title.is_empty() {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "session title cannot be empty",
                    );
                }
                let session_handle = match self.get_or_load_parent_session(legacy_session_id).await
                {
                    Ok(handle) => handle,
                    Err(crate::runtime::session_cache::LoadSessionError::SessionNotFound)
                    | Err(crate::runtime::session_cache::LoadSessionError::RolloutMissing) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::SessionNotFound,
                            "session does not exist",
                        );
                    }
                    Err(
                        crate::runtime::session_cache::LoadSessionError::SubagentNotResumable {
                            parent_session_id,
                        },
                    ) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!(
                                "subagent sessions cannot be renamed directly; rename the parent session {parent_session_id} instead"
                            ),
                        );
                    }
                    Err(crate::runtime::session_cache::LoadSessionError::RestoreFailed(
                        message,
                    )) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to load session for metadata update: {message}"),
                        );
                    }
                };
                {
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
                    self.persist_session_summary_if_persistent(legacy_session_id, &summary)
                        .await;
                    self.broadcast_event(ServerEvent::SessionTitleUpdated(SessionEventPayload {
                        session: summary,
                    }))
                    .await;
                }
            }
        }
        let Some(session_handle) = self.session(legacy_session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        // Persist-first: never wait on the session actor, and never take
        // the state-change gate for a settings patch. Title generation and
        // finalize hold that gate across mailbox waits; taking it here
        // stalls the TUI's pre-turn `session/metadata/update` until the
        // 10s client timeout, after which `turn/start` hits the same gate.
        // Mailbox-free rollout resolution: SQLite index first, rollout scan
        // fallback (same sources as the subscription snapshot path). The
        // index metadata also supplies the current model/binding/effort
        // values, needed because the actor's metadata command overwrites
        // absent fields unless they are re-sent with their current values.
        let session_index = self
            .deps
            .db
            .get_session_index(&legacy_session_id)
            .ok()
            .flatten();
        let current_model_slug = session_index
            .as_ref()
            .and_then(|index| index.metadata.model.clone());
        let current_binding_id = session_index
            .as_ref()
            .and_then(|index| index.metadata.model_binding_id.clone());
        let current_effort = session_index
            .as_ref()
            .and_then(|index| index.metadata.reasoning_effort_selection.clone());
        let mut index_metadata = session_index.as_ref().map(|index| index.metadata.clone());
        let rollout_path = session_index
            .and_then(|index| index.rollout_path)
            .or_else(|| {
                self.rollout_store
                    .find_rollout_by_session_id(&legacy_session_id)
                    .ok()
                    .flatten()
            });
        // Ephemeral sessions have neither rollout nor an index row: the only
        // metadata source left is the actor summary (a mailbox read — the
        // blocking is scoped to the ephemeral degrade; durable paths never
        // wait on the actor).
        if index_metadata.is_none() && rollout_path.is_none() {
            index_metadata = session_handle.summary().await;
        }
        // Ephemeral degrade: no rollout → no field lines and an index-built
        // snapshot; durable → history-backed snapshot with version checks.
        let (session_version, session_model_slug, session_cwd, session_additional_dirs, current) =
            if let Some(rollout_path) = &rollout_path {
                let history = match devo_core::read_canonical_history(rollout_path) {
                    Ok(history) => history,
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to read session history: {error}"),
                        );
                    }
                };
                let Some(session) = history.session else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::SessionNotFound,
                        "session history has no metadata",
                    );
                };
                if params.expected_version != 0 && params.expected_version != session.version {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::WorkspaceVersionConflict,
                        format!(
                            "session version drift: expected {}, current {}",
                            params.expected_version, session.version
                        ),
                    );
                }
                (
                    session.version,
                    session.model.model.clone(),
                    session.cwd.clone(),
                    session.additional_directories.clone(),
                    session.settings.clone(),
                )
            } else {
                let Some(index_metadata) = index_metadata.as_ref() else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::SessionNotFound,
                        "session is not durable and has no index metadata",
                    );
                };
                if params.expected_version > 1 {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::WorkspaceVersionConflict,
                        format!(
                            "session version drift: expected {}, current 1",
                            params.expected_version
                        ),
                    );
                }
                let settings = devo_protocol::native::session::SessionSettings {
                    permission_profile: match index_metadata.permission_preset {
                        Some(devo_protocol::PermissionPreset::Default) | None => {
                            devo_protocol::native::model::PermissionProfile::Default
                        }
                        Some(devo_protocol::PermissionPreset::AutoReview) => {
                            devo_protocol::native::model::PermissionProfile::AutoReview
                        }
                        Some(devo_protocol::PermissionPreset::FullAccess) => {
                            devo_protocol::native::model::PermissionProfile::FullAccess
                        }
                    },
                    reasoning_effort: index_metadata
                        .reasoning_effort_selection
                        .as_deref()
                        .and_then(|selection| selection.parse().ok()),
                    mode: Some(
                        serde_json::to_value(index_metadata.collaboration_mode)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_default(),
                    ),
                    sandbox_profile: None,
                    effective_context_window: None,
                };
                (
                    1,
                    index_metadata.model.clone().unwrap_or_default(),
                    index_metadata.cwd.clone(),
                    index_metadata.additional_directories.clone(),
                    settings,
                )
            };
        let _ = session_version;
        let mut overlay_profile: Option<devo_safety::RuntimePermissionProfile> = None;
        let mut overlay_sandbox: Option<String> = None;
        let mut overlay_effort: Option<String> = None;
        let mut overlay_model: Option<String> = None;
        let mut overlay_compact_limit: Option<usize> = None;
        let mut overlay_mode: Option<devo_protocol::CollaborationMode> = None;
        let mut applied_window: Option<u64> = None;
        if let Some(settings) = &params.settings {
            if let Some(profile) = settings.permission_profile
                && profile != current.permission_profile
            {
                let preset = match profile {
                    devo_protocol::native::model::PermissionProfile::Default => {
                        devo_protocol::PermissionPreset::Default
                    }
                    devo_protocol::native::model::PermissionProfile::AutoReview => {
                        devo_protocol::PermissionPreset::AutoReview
                    }
                    devo_protocol::native::model::PermissionProfile::FullAccess => {
                        devo_protocol::PermissionPreset::FullAccess
                    }
                };
                if let Some(path) = &rollout_path
                    && let Err(error) = self.rollout_store.append_session_settings_at(
                        path,
                        legacy_session_id,
                        SessionSettingsField::PermissionPreset,
                        serde_json::to_value(preset).expect("serialize permission preset setting"),
                    )
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to persist permission preset settings line: {error}"),
                    );
                }
                let profile = safety_profile_from_protocol(
                    preset,
                    session_cwd.clone(),
                    session_additional_dirs.clone(),
                );
                session_handle.notify_permission_profile(profile.clone());
                overlay_profile = Some(profile);
            }
            if settings.sandbox_profile != current.sandbox_profile
                && let Some(name) = &settings.sandbox_profile
            {
                let native_name = match crate::sandbox_profile::normalize_sandbox_profile_name(
                    name,
                    &session_cwd,
                ) {
                    Ok(name) => name,
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("invalid sandbox profile '{name}': {error}"),
                        );
                    }
                };
                if let Some(path) = &rollout_path
                    && let Err(error) = self.rollout_store.append_session_settings_at(
                        path,
                        legacy_session_id,
                        SessionSettingsField::SandboxProfile,
                        serde_json::Value::String(native_name.clone()),
                    )
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to persist sandbox profile settings line: {error}"),
                    );
                }
                session_handle.notify_sandbox_profile(native_name.clone());
                overlay_sandbox = Some(native_name);
            }
            let current_effort = current.reasoning_effort.map(|effort| effort.to_string());
            if let Some(effort) = &settings.reasoning_effort
                && current_effort.as_ref() != Some(effort)
            {
                if let Some(path) = &rollout_path
                    && let Err(error) = self.rollout_store.append_session_settings_at(
                        path,
                        legacy_session_id,
                        SessionSettingsField::ReasoningEffortSelection,
                        serde_json::to_value(Some(effort.clone()))
                            .expect("serialize reasoning effort setting"),
                    )
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to persist reasoning effort settings line: {error}"),
                    );
                }
                overlay_effort = Some(effort.clone());
            }
            if settings.mode != current.mode
                && let Some(mode_id) = &settings.mode
            {
                let mode = match serde_json::from_value::<devo_protocol::CollaborationMode>(
                    serde_json::Value::String(mode_id.clone()),
                ) {
                    Ok(mode) => mode,
                    Err(_) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("invalid collaboration mode '{mode_id}'"),
                        );
                    }
                };
                if let Some(path) = &rollout_path
                    && let Err(error) = self.rollout_store.append_session_settings_at(
                        path,
                        legacy_session_id,
                        SessionSettingsField::CollaborationMode,
                        serde_json::to_value(mode).expect("serialize collaboration mode setting"),
                    )
                {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to persist collaboration mode settings line: {error}"),
                    );
                }
                overlay_mode = Some(mode);
            }
            if settings.effective_context_window != current.effective_context_window
                && let Some(window) = settings.effective_context_window
            {
                if window == 0 {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        "effectiveContextWindow must be at least 1",
                    );
                }
                // Durability target is the global config.toml (L2-DES-CONV-002
                // DD-6); no field line is written. The canonical path applies
                // only to the addressed session — the legacy compaction
                // handler keeps the all-sessions fan-out until Phase C.
                {
                    let mut store = self
                        .deps
                        .config_store
                        .lock()
                        .expect("app config store mutex should not be poisoned");
                    if let Err(error) = store.set_compaction_token_limit(window) {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to persist compaction_token_limit: {error}"),
                        );
                    }
                }
                self.deps.invalidate_workspace_contexts();
                // Resolve the model through the same two-catalog chain the
                // legacy handler used: the workspace runtime context's
                // catalog first, then the deps catalog (mailbox-free).
                let workspace_catalog = self
                    .deps
                    .context_for_workspace(&session_cwd)
                    .await
                    .ok()
                    .map(|context| Arc::clone(&context.model_catalog));
                let model = workspace_catalog
                    .as_ref()
                    .and_then(|catalog| catalog.get(&session_model_slug).cloned())
                    .or_else(|| self.deps.model_catalog.get(&session_model_slug).cloned());
                if let Some(model) = model {
                    let applied = crate::runtime::context_occupancy::resolved_compaction_limit(
                        Some(window),
                        &model,
                    );
                    session_handle.notify_effective_context_window(applied as usize);
                    overlay_compact_limit = Some(applied as usize);
                    applied_window = Some(applied);
                }
            }
        }
        if let Some(binding) = &params.model
            && binding.model != session_model_slug
        {
            if let Some(path) = &rollout_path
                && let Err(error) = self.rollout_store.append_session_settings_at(
                    path,
                    legacy_session_id,
                    SessionSettingsField::Model,
                    serde_json::to_value(Some(binding.model.clone()))
                        .expect("serialize model setting"),
                )
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist model settings line: {error}"),
                );
            }
            overlay_model = Some(binding.model.clone());
        }
        if let Some(model_binding_id) = &params.model_binding_id
            && let Some(path) = &rollout_path
            && let Err(error) = self.rollout_store.append_session_settings_at(
                path,
                legacy_session_id,
                SessionSettingsField::ModelBindingId,
                serde_json::to_value(Some(model_binding_id.clone()))
                    .expect("serialize model binding setting"),
            )
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to persist model binding settings line: {error}"),
            );
        }
        // One consolidated metadata notification carrying every field's new
        // or current value: the actor overwrites absent fields on non-
        // mode-only updates, so partial notifications would wipe them.
        if overlay_model.is_some()
            || overlay_effort.is_some()
            || overlay_mode.is_some()
            || params.model_binding_id.is_some()
        {
            session_handle.notify_session_metadata(
                Some(
                    overlay_model
                        .clone()
                        .or(current_model_slug)
                        .unwrap_or_else(|| session_model_slug.clone()),
                ),
                params.model_binding_id.clone().or(current_binding_id),
                overlay_effort.clone().or(current_effort),
                overlay_mode,
            );
        }

        // Phase 3: deliver the override to the running turn's inline state,
        // if a turn is active. Admission reads the inline config on every
        // authorization and the tool router reads the live sandbox handle on
        // every spawn, so the change applies at the next decision point.
        let mut applied_to_active_turn = false;
        if (overlay_profile.is_some()
            || overlay_sandbox.is_some()
            || overlay_effort.is_some()
            || overlay_model.is_some()
            || overlay_compact_limit.is_some())
            && let Some(stream) = self.active_stream_state(legacy_session_id).await
        {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                if let Some(profile) = &overlay_profile {
                    inline.hook_context.config.permission_mode = profile.permission_mode();
                    inline.hook_context.config.permission_profile = profile.clone();
                    let implied = Some(profile.implied_sandbox_profile().to_string());
                    inline.hook_context.config.sandbox_profile = implied.clone();
                    *inline
                        .sandbox_profile_live
                        .lock()
                        .expect("sandbox profile live mutex poisoned") = implied;
                    // A new policy invalidates implicit cached approvals,
                    // matching the actor-side behavior for idle updates.
                    inline.session_approval_cache = Default::default();
                    inline.turn_approval_cache = Default::default();
                }
                if let Some(name) = &overlay_sandbox {
                    inline.hook_context.config.sandbox_profile = Some(name.clone());
                    *inline
                        .sandbox_profile_live
                        .lock()
                        .expect("sandbox profile live mutex poisoned") = Some(name.clone());
                }
                // Phase 4: model/effort changes replace the live turn config
                // (re-resolved so provider routing follows the new model);
                // compaction-limit changes move the next budget check.
                if overlay_effort.is_some()
                    || overlay_model.is_some()
                    || overlay_compact_limit.is_some()
                {
                    let mut live = inline
                        .live_turn_settings
                        .lock()
                        .expect("live settings mutex poisoned");
                    if let Some(model_slug) = &overlay_model {
                        let effort = overlay_effort.clone().or_else(|| {
                            live.turn_config
                                .as_ref()
                                .and_then(|config| config.reasoning_effort_selection.clone())
                        });
                        live.turn_config = Some(
                            inline
                                .hook_context
                                .runtime_context
                                .resolve_turn_config(Some(model_slug), effort),
                        );
                    } else if let Some(effort) = &overlay_effort {
                        // Base the override on the seeded live config; if the
                        // turn runner has not seeded yet, resolve a fresh base
                        // from the session's current model so the overlay is
                        // never silently dropped.
                        let base = live.turn_config.clone().unwrap_or_else(|| {
                            inline
                                .hook_context
                                .runtime_context
                                .resolve_turn_config(inline.summary.model.as_deref(), None)
                        });
                        let mut config = base;
                        config.reasoning_effort_selection = Some(effort.clone());
                        live.turn_config = Some(config);
                    }
                    if let Some(limit) = overlay_compact_limit {
                        live.auto_compact_token_limit = Some(limit);
                    }
                    live.generation = live.generation.saturating_add(1);
                }
                applied_to_active_turn = true;
            }
        }

        // Keep the SQLite session index in step with the settings write so
        // the session list reflects new values without waiting for turn
        // activity. Built from index metadata + the applied patch values
        // (no actor round-trip).
        if let Some(index_metadata) = index_metadata.as_mut() {
            let mut touched = false;
            if let Some(profile) = &overlay_profile {
                index_metadata.permission_preset =
                    Some(protocol_preset_from_safety(profile.preset));
                touched = true;
            }
            if let Some(model) = &overlay_model {
                index_metadata.model = Some(model.clone());
                touched = true;
            }
            if let Some(binding_id) = &params.model_binding_id {
                index_metadata.model_binding_id = Some(binding_id.clone());
                touched = true;
            }
            if let Some(effort) = &overlay_effort {
                index_metadata.reasoning_effort_selection = Some(effort.clone());
                touched = true;
            }
            if let Some(mode) = overlay_mode {
                index_metadata.collaboration_mode = mode;
                touched = true;
            }
            if touched {
                index_metadata.updated_at = Utc::now();
                if let Err(error) = self.deps.db.upsert_session(index_metadata, None) {
                    tracing::warn!(
                        session_id = %legacy_session_id,
                        error = %error,
                        "failed to refresh session index after settings update"
                    );
                }
            }
        }

        // The response reflects the persisted state: for durable sessions
        // it is rebuilt from the rollout (field lines fold into the canonical
        // snapshot); for ephemeral sessions it is built from the SQLite
        // index with the applied patch.
        let mut session = if let Some(rollout_path) = &rollout_path {
            let history = match devo_core::read_canonical_history(rollout_path) {
                Ok(history) => history,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InternalError,
                        format!("failed to re-read session history: {error}"),
                    );
                }
            };
            let Some(session) = history.session else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session history has no metadata",
                );
            };
            *session
        } else {
            let Some(index_metadata) = index_metadata.as_ref() else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session is not durable and has no index metadata",
                );
            };
            Self::native_session_from_index_metadata(index_metadata, legacy_session_id)
        };
        // The compaction limit's durability target is config.toml, so the
        // rollout re-read does not reflect it; echo the clamped applied value.
        if let Some(applied) = applied_window {
            session.settings.effective_context_window = Some(applied);
        }
        // Compaction settings are global, so update loaded sibling sessions
        // after the addressed session has been persisted. This keeps the
        // canonical settings patch behavior identical for every session
        // without reintroducing a standalone compaction RPC.
        if let Some(global) = params
            .settings
            .as_ref()
            .and_then(|settings| settings.effective_context_window)
        {
            for handle in self.list_session_handles().await {
                if handle.id() == legacy_session_id {
                    continue;
                }
                let session_model = self
                    .deps
                    .db
                    .get_session_index(&handle.id())
                    .ok()
                    .flatten()
                    .and_then(|index| index.metadata.model.or(index.metadata.model_binding_id))
                    .and_then(|slug| self.deps.model_catalog.get(&slug).cloned());
                let Some(session_model) = session_model else {
                    continue;
                };
                let applied_for_session =
                    crate::runtime::context_occupancy::resolved_compaction_limit(
                        Some(global),
                        &session_model,
                    );
                handle.notify_effective_context_window(applied_for_session as usize);
                self.broadcast_event(ServerEvent::SessionEffectiveContextWindowUpdated(
                    SessionEffectiveContextWindowUpdatedPayload {
                        session_id: handle.id(),
                        effective_context_window: applied_for_session,
                    },
                ))
                .await;
            }
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult {
                session,
                applied_to_active_turn,
            },
        })
        .expect("serialize canonical session/metadata/update response")
    }

    /// Builds a canonical session snapshot from SQLite index metadata, used for
    /// ephemeral sessions that have no rollout file (L2-DES-CONV-002 Phase 2
    /// degrade path). Applied patch values are not folded here; the caller
    /// overlays them (the index is refreshed separately).
    fn native_session_from_index_metadata(
        metadata: &SessionMetadata,
        session_id: SessionId,
    ) -> devo_protocol::native::session::Session {
        devo_protocol::native::session::Session {
            id: devo_protocol::native::ids::SessionId::from_string(session_id.to_string()),
            version: 1,
            cwd: metadata.cwd.clone(),
            additional_directories: metadata.additional_directories.clone(),
            parent: metadata.parent_session_id.map(|parent| {
                devo_protocol::native::session::SessionParent::Fork {
                    session_id: devo_protocol::native::ids::SessionId::from_string(
                        parent.to_string(),
                    ),
                    at_turn_id: None,
                }
            }),
            ephemeral: metadata.ephemeral,
            created_at: metadata.created_at,
            status: devo_protocol::native::session::SessionStatus::Idle,
            flags: Vec::new(),
            archived: false,
            active_turn_id: None,
            queued_count: 0,
            title: metadata.title.clone(),
            model: devo_protocol::native::model::ModelBinding {
                provider: metadata
                    .model_binding_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                model: metadata.model.clone().unwrap_or_default(),
                reasoning_effort: metadata
                    .reasoning_effort_selection
                    .as_deref()
                    .and_then(|selection| selection.parse().ok()),
            },
            settings: devo_protocol::native::session::SessionSettings {
                permission_profile: match metadata.permission_preset {
                    Some(devo_protocol::PermissionPreset::Default) | None => {
                        devo_protocol::native::model::PermissionProfile::Default
                    }
                    Some(devo_protocol::PermissionPreset::AutoReview) => {
                        devo_protocol::native::model::PermissionProfile::AutoReview
                    }
                    Some(devo_protocol::PermissionPreset::FullAccess) => {
                        devo_protocol::native::model::PermissionProfile::FullAccess
                    }
                },
                reasoning_effort: metadata
                    .reasoning_effort_selection
                    .as_deref()
                    .and_then(|selection| selection.parse().ok()),
                mode: Some(
                    serde_json::to_value(metadata.collaboration_mode)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default(),
                ),
                sandbox_profile: None,
                effective_context_window: None,
            },
            git_info: None,
            preview: String::new(),
            last_activity_at: metadata.last_activity_at,
            transcript_size_bytes: None,
            usage: devo_protocol::native::usage::SessionUsage {
                total: devo_protocol::native::usage::UsageTotals {
                    total_tokens: metadata.total_tokens as u64,
                    input_tokens: metadata.total_input_tokens as u64,
                    output_tokens: metadata.total_output_tokens as u64,
                    reasoning_tokens: 0,
                    cache_read_input_tokens: metadata.total_cache_read_tokens as u64,
                    cache_creation_input_tokens: metadata.total_cache_creation_tokens as u64,
                    call_count: 0,
                    metered_call_count: 0,
                    failed_call_count: 0,
                    cancelled_call_count: 0,
                    estimated_cost: None,
                },
                by_purpose: Vec::new(),
                legacy: None,
                updated_at: metadata.updated_at,
            },
        }
    }

    /// Native `session/new` (L2-DES-APP-008 Phase B): creates a durable
    /// session in `cwd` with idempotency-key replay, returning the canonical
    /// session snapshot built from the rollout (single source of truth).
    pub(crate) async fn handle_native_session_new(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionNewParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/new params: {error}"),
                    );
                }
            };
        if let Some(existing) = self
            .session_new_idempotency
            .lock()
            .await
            .get(&params.idempotency_key)
            .copied()
            && let Some(result) = self
                .native_session_snapshot_response(request_id.clone(), existing)
                .await
        {
            return result;
        }
        let response = self
            .start_session_with_registry(
                connection_id,
                request_id.clone(),
                SessionStartParams {
                    cwd: params.cwd.clone(),
                    additional_directories: Vec::new(),
                    ephemeral: false,
                    title: None,
                    model: None,
                    model_binding_id: None,
                },
                None,
            )
            .await;
        if let Ok(success) =
            serde_json::from_value::<SuccessResponse<SessionStartResult>>(response.clone())
        {
            self.subscribe_connection_to_session(
                connection_id,
                success.result.session.session_id,
                None,
            )
            .await;
            self.session_new_idempotency
                .lock()
                .await
                .insert(params.idempotency_key, success.result.session.session_id);
            if let Some(result) = self
                .native_session_snapshot_response(request_id, success.result.session.session_id)
                .await
            {
                return result;
            }
        }
        response
    }

    /// Builds a `session/new`-shaped response from the rollout-backed
    /// canonical session snapshot; `None` when the rollout is not readable.
    async fn native_session_snapshot_response(
        &self,
        request_id: serde_json::Value,
        session_id: SessionId,
    ) -> Option<serde_json::Value> {
        let session = self.native_session_snapshot(session_id).await?;
        Some(
            serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_session::SessionNewResult { session },
            })
            .expect("serialize canonical session/new response"),
        )
    }

    /// Reads the rollout-backed canonical session snapshot; `None` when the
    /// rollout is missing or unreadable.
    async fn native_session_snapshot(
        &self,
        session_id: SessionId,
    ) -> Option<devo_protocol::native::session::Session> {
        let rollout_path = self
            .deps
            .db
            .get_session_index(&session_id)
            .ok()
            .flatten()
            .and_then(|index| index.rollout_path)
            .or_else(|| {
                self.rollout_store
                    .find_rollout_by_session_id(&session_id)
                    .ok()
                    .flatten()
            })?;
        let history = devo_core::read_canonical_history(&rollout_path).ok()?;
        history.session.map(|session| *session)
    }

    /// Native `session/read` (L2-DES-APP-008): one session's
    /// rollout-backed canonical snapshot.
    pub(crate) async fn handle_native_session_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/read params: {error}"),
                    );
                }
            };
        let Ok(session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server",
            );
        };
        let Some(session) = self.native_session_snapshot(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionReadResult { session },
        })
        .expect("serialize canonical session/read response")
    }

    /// Native `session/list` (L2-DES-APP-008): offset-paged canonical
    /// session snapshots, newest activity first. Sessions whose rollout
    /// snapshot is unreadable are skipped.
    pub(crate) async fn handle_native_session_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/list params: {error}"),
                    );
                }
            };
        let start: usize = match params.cursor.as_deref() {
            None => 0,
            Some(cursor) => match cursor.parse() {
                Ok(start) => start,
                Err(_) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/list cursor: {cursor}"),
                    );
                }
            },
        };
        let limit = params
            .limit
            .map_or(CANONICAL_SESSION_LIST_DEFAULT_LIMIT, |limit| limit as usize);
        let search = params.search.as_deref().map(str::to_lowercase);
        let mut sessions = Vec::new();
        for summary in self.list_session_summaries().await {
            if !session_list_cwd_matches(&params.cwds, &summary.cwd) {
                continue;
            }
            if let Some(search) = search.as_ref()
                && !summary
                    .title
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(search)
            {
                continue;
            }
            let mut session = self
                .native_session_snapshot(summary.session_id)
                .await
                .unwrap_or_else(|| {
                    Self::native_session_from_index_metadata(&summary, summary.session_id)
                });
            let rollout_path = self
                .deps
                .db
                .get_session_index(&summary.session_id)
                .ok()
                .flatten()
                .and_then(|index| index.rollout_path)
                .or_else(|| {
                    self.rollout_store
                        .find_rollout_by_session_id(&summary.session_id)
                        .ok()
                        .flatten()
                });
            session.transcript_size_bytes = rollout_path
                .and_then(|path| std::fs::metadata(path).ok())
                .map(|metadata| metadata.len());
            sessions.push(session);
        }
        let next_start = start.saturating_add(limit);
        let next_cursor = (next_start < sessions.len()).then(|| next_start.to_string());
        let data = sessions.into_iter().skip(start).take(limit).collect();
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::page::Page { data, next_cursor },
        })
        .expect("serialize canonical session/list response")
    }

    /// Native `session/delete` (L2-DES-APP-008): deletes the session tree
    /// and broadcasts the session-deleted event, same side effects as the
    /// ACP adapter path.
    pub(crate) async fn handle_native_session_delete(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionDeleteParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/delete params: {error}"),
                    );
                }
            };
        let session_id = match SessionId::try_from(params.session_id.as_str()) {
            Ok(session_id) => session_id,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid canonical session id: {error}"),
                );
            }
        };
        let deleted_session_ids = match self.delete_session_tree(session_id).await {
            Ok(deleted_session_ids) => deleted_session_ids,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to delete session: {error}"),
                );
            }
        };
        if !deleted_session_ids.is_empty() {
            self.broadcast_event(ServerEvent::SessionDeleted(SessionDeletedPayload {
                session_id,
                deleted_session_ids,
            }))
            .await;
        }
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionDeleteResult {},
        })
        .expect("serialize canonical session/delete response")
    }

    pub(crate) async fn handle_session_resume(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        // Dual-shape boundary (L2-DES-APP-008 DD-4): the canonical shape is
        // detected by its camelCase `sessionId` key.
        if params.get("sessionId").is_some() {
            return self
                .handle_native_session_resume(connection_id, request_id, params)
                .await;
        }
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

    /// Native `session/resume` (L2-DES-APP-008 Phase B): hydrates the
    /// session via the legacy flow and answers with the rollout-backed
    /// canonical session snapshot. Transcript restore is intentionally not
    /// part of this result — canonical clients page `session/items/list` or
    /// use `subscription/*` snapshots (Phase C rework of the TUI restore
    /// flow).
    async fn handle_native_session_resume(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionResumeParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/resume params: {error}"),
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
            .restore_existing_session_with_tool_registry_update(
                connection_id,
                request_id.clone(),
                SessionResumeParams {
                    session_id: legacy_session_id,
                },
                RuntimeSessionToolRegistryUpdate::KeepCurrent,
            )
            .await;
        if response.get("error").is_some() {
            return response;
        }
        self.native_session_snapshot_response(request_id, legacy_session_id)
            .await
            .unwrap_or(response)
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
        // Dual-shape boundary (L2-DES-APP-008 DD-4): the canonical shape is
        // detected by its camelCase `sessionId` key.
        if params.get("sessionId").is_some() {
            return self
                .handle_native_session_fork(connection_id, request_id, params)
                .await;
        }
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
        self.handle_session_fork_translated(connection_id, request_id, params)
            .await
    }

    /// Native `session/fork` (L2-DES-APP-008 Phase B): forks at
    /// `atTurnId` (or the session tip when absent). The turn id is mapped to
    /// the legacy user-turn index with the same rule the fork machinery
    /// uses (turns containing a `UserMessage` item, in order).
    async fn handle_native_session_fork(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionForkParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical session/fork params: {error}"),
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
        let user_turn_index = match &params.at_turn_id {
            None => None,
            Some(at_turn_id) => {
                let Ok(legacy_turn_id) = TurnId::try_from(at_turn_id.as_str()) else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::ForkTurnNotFound,
                        "turn id is not addressable by this server",
                    );
                };
                let Some(source_handle) = self.session(legacy_session_id).await else {
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
                let mut user_turn_ids: Vec<TurnId> = Vec::new();
                for item in &source.persisted_turn_items {
                    if matches!(item.turn_item, devo_core::TurnItem::UserMessage(_))
                        && user_turn_ids.last().copied() != Some(item.turn_id)
                    {
                        user_turn_ids.push(item.turn_id);
                    }
                }
                let Some(index) = user_turn_ids
                    .iter()
                    .position(|turn_id| *turn_id == legacy_turn_id)
                else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::ForkTurnNotFound,
                        "atTurnId does not name a user turn in this session",
                    );
                };
                Some(u32::try_from(index).unwrap_or(u32::MAX))
            }
        };
        let response = self
            .handle_session_fork_translated(
                connection_id,
                request_id.clone(),
                SessionForkParams {
                    session_id: legacy_session_id,
                    title: None,
                    cwd: None,
                    user_turn_index,
                },
            )
            .await;
        let Ok(success) =
            serde_json::from_value::<SuccessResponse<SessionForkResult>>(response.clone())
        else {
            return response;
        };
        self.native_session_snapshot_response(request_id, success.result.session.session_id)
            .await
            .unwrap_or(response)
    }

    async fn handle_session_fork_translated(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: SessionForkParams,
    ) -> serde_json::Value {
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
                    rollback_mode: RollbackMode::ThroughUserTurn,
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
    rollback_mode: RollbackMode,
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
        RollbackMode::ThroughUserTurn => Ok(persisted_turn_items
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
        RollbackMode::BeforeUserTurn => Ok(persisted_turn_items
            .iter()
            .take_while(|item| item.turn_id != selected_turn_id)
            .cloned()
            .collect()),
    }
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
            RollbackMode::ThroughUserTurn,
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
            RollbackMode::BeforeUserTurn,
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
            RollbackMode::BeforeUserTurn,
        )
        .expect("drop first turn");

        assert_eq!(kept, Vec::new());
    }

    #[test]
    fn cut_occupancy_uses_cut_turn_not_tip() {
        use pretty_assertions::assert_eq;

        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let occupancy_a = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 20_000,
        );
        let occupancy_b = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
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
        let compact_occupancy = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
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
