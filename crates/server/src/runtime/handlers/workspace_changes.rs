use super::super::*;

impl ServerRuntime {
    /// Native `workspace/changes/read` (L2-DES-APP-008): the desktop
    /// client's diff read model; translates into the legacy machinery and
    /// projects views to the canonical camelCase shape.
    pub(crate) async fn handle_native_workspace_changes_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_workspace::WorkspaceChangesReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical workspace/changes/read params: {error}"),
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
        let turn_id: Option<TurnId> = params.turn_id.as_ref().and_then(|turn_id| {
            serde_json::from_value(serde_json::Value::String(turn_id.as_str().to_string())).ok()
        });
        let views = self
            .workspace_changes_read_views(WorkspaceChangesReadParams {
                session_id,
                cwd: params.cwd,
                scopes: params.scopes,
                base_branch: params.base_branch,
                turn_id,
                diff_detail: params.diff_detail,
                max_diff_bytes: params.max_diff_bytes,
                ignore_whitespace: params.ignore_whitespace,
                paths: params.paths,
                include_file_sides: params.include_file_sides,
            })
            .await;
        match views {
            Ok(views) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: devo_protocol::native::rpc_workspace::WorkspaceChangesReadResult {
                    views: views
                        .into_iter()
                        .map(devo_protocol::native::rpc_workspace::WorkspaceChangeView::from)
                        .collect(),
                },
            })
            .expect("serialize canonical workspace/changes/read response"),
            Err((code, message)) => self.error_response(request_id, code, message),
        }
    }

    pub(crate) async fn handle_workspace_changes_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        self.handle_native_workspace_changes_read(request_id, params)
            .await
    }

    async fn workspace_changes_read_views(
        self: &Arc<Self>,
        params: WorkspaceChangesReadParams,
    ) -> Result<Vec<WorkspaceChangeView>, (ProtocolErrorCode, String)> {
        if params.scopes.is_empty() {
            return Err((
                ProtocolErrorCode::InvalidParams,
                "workspace/changes/read requires at least one scope".to_string(),
            ));
        }

        let Some(session_handle) = self
            .get_or_load_parent_session(params.session_id)
            .await
            .ok()
        else {
            return Err((
                ProtocolErrorCode::SessionNotFound,
                "session does not exist".to_string(),
            ));
        };
        // Prefer the registry/spawn fast path when a turn is active so reads
        // never wait on turn I/O; falls through to the mailbox when idle.
        let reservation = self
            .session_turn_reservation_snapshot(params.session_id)
            .await
            .or(session_handle.turn_reservation_snapshot().await);
        let Some(cwd) = params
            .cwd
            .clone()
            .or_else(|| reservation.as_ref().map(|r| r.summary.cwd.clone()))
        else {
            return Err((
                ProtocolErrorCode::InvalidParams,
                "workspace/changes/read requires cwd when session cwd is unavailable".to_string(),
            ));
        };
        let active_turn_id = reservation
            .as_ref()
            .and_then(|r| r.active_turn.as_ref().map(|turn| turn.turn_id));
        let latest_turn_id = reservation
            .as_ref()
            .and_then(|r| r.latest_turn.as_ref().map(|turn| turn.turn_id));
        let ignore_whitespace = params.ignore_whitespace.unwrap_or(false);
        let path_filter = params
            .paths
            .as_ref()
            .filter(|paths| !paths.is_empty())
            .cloned();
        let path_scoped_full =
            path_filter.is_some() && matches!(params.diff_detail, WorkspaceDiffDetail::Full);

        let mut views = Vec::with_capacity(params.scopes.len());
        for scope in params.scopes {
            let view = if path_scoped_full {
                let turn_checkpoint = if matches!(scope, WorkspaceChangeScope::Turn) {
                    let turn_id = params.turn_id.or(active_turn_id).or(latest_turn_id);
                    match turn_id {
                        Some(turn_id) => self.turn_checkpoint_id(turn_id).await,
                        None => None,
                    }
                } else {
                    None
                };
                crate::workspace_changes::path_scoped_full_view(
                    cwd.clone(),
                    scope,
                    path_filter.clone().unwrap_or_default(),
                    params.base_branch.clone(),
                    ignore_whitespace,
                    params.max_diff_bytes,
                    turn_checkpoint,
                    params.include_file_sides.unwrap_or(false),
                )
                .await
            } else {
                match scope {
                    WorkspaceChangeScope::Branch => {
                        crate::workspace_changes::branch_view(
                            cwd.clone(),
                            params.base_branch.clone(),
                            ignore_whitespace,
                            params.diff_detail,
                            params.max_diff_bytes,
                        )
                        .await
                    }
                    WorkspaceChangeScope::Staged => {
                        crate::workspace_changes::staged_view(
                            cwd.clone(),
                            ignore_whitespace,
                            params.diff_detail,
                            params.max_diff_bytes,
                        )
                        .await
                    }
                    WorkspaceChangeScope::Unstaged => {
                        crate::workspace_changes::unstaged_view(
                            cwd.clone(),
                            ignore_whitespace,
                            params.diff_detail,
                            params.max_diff_bytes,
                        )
                        .await
                    }
                    WorkspaceChangeScope::Uncommitted => {
                        crate::workspace_changes::uncommitted_view(
                            cwd.clone(),
                            ignore_whitespace,
                            params.diff_detail,
                            params.max_diff_bytes,
                        )
                        .await
                    }
                    WorkspaceChangeScope::Turn => {
                        let turn_id = params.turn_id.or(active_turn_id).or(latest_turn_id);
                        match turn_id {
                            Some(turn_id) => {
                                self.read_turn_workspace_changes(
                                    params.session_id,
                                    turn_id,
                                    cwd.clone(),
                                    params.diff_detail,
                                    params.max_diff_bytes,
                                )
                                .await
                            }
                            None => crate::workspace_changes::unsupported_view(
                                WorkspaceChangeScope::Turn,
                                cwd.clone(),
                                WorkspaceChangeAttribution::WorkspaceNet,
                                "turn_id_not_available",
                            ),
                        }
                    }
                }
            };
            views.push(view);
        }

        Ok(views)
    }

    async fn turn_checkpoint_id(&self, turn_id: TurnId) -> Option<String> {
        let baseline = self
            .active_workspace_baselines
            .lock()
            .await
            .get(&turn_id)
            .cloned()?;
        Some(baseline.checkpoint_id().to_string())
    }

    async fn read_turn_workspace_changes(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        cwd: PathBuf,
        diff_detail: WorkspaceDiffDetail,
        max_diff_bytes: Option<u64>,
    ) -> WorkspaceChangeView {
        if let Some(baseline) = self
            .active_workspace_baselines
            .lock()
            .await
            .get(&turn_id)
            .cloned()
        {
            return match crate::workspace_changes::read_active_turn_view(
                baseline,
                diff_detail,
                max_diff_bytes,
            )
            .await
            {
                Ok(view) => view,
                Err(error) => crate::workspace_changes::error_view(
                    WorkspaceChangeScope::Turn,
                    cwd,
                    WorkspaceChangeAttribution::WorkspaceNet,
                    error.to_string(),
                ),
            };
        }

        match crate::workspace_changes::read_finalized_turn_view(
            self.metadata.server_home.as_path(),
            session_id,
            turn_id,
            diff_detail,
            max_diff_bytes,
        ) {
            Ok(Some(view)) => view,
            Ok(None) => crate::workspace_changes::unsupported_view(
                WorkspaceChangeScope::Turn,
                cwd,
                WorkspaceChangeAttribution::WorkspaceNet,
                "turn_baseline_not_available",
            ),
            Err(error) => crate::workspace_changes::error_view(
                WorkspaceChangeScope::Turn,
                cwd,
                WorkspaceChangeAttribution::WorkspaceNet,
                error.to_string(),
            ),
        }
    }
}
