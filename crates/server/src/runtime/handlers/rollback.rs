//! Two-phase `session/rollback/preview|commit` implementation (P4d).
//!
//! Plans are connection-bound and short-lived. Commit revalidates both the
//! session history and Git worktree before applying any change.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use devo_core::SessionId;
use devo_protocol::canonical::ids::RestorePlanId;
use devo_protocol::canonical::rpc_session::{
    RestorePlan, SessionRollbackCommitParams, SessionRollbackCommitResult,
    SessionRollbackPreviewParams,
};
use tokio::sync::Notify;

use super::super::*;
use super::rollback_plan::*;
use super::session::RuntimeSessionTurnCutOptions;

impl ServerRuntime {
    pub(crate) async fn drop_restore_plans_for_connection(&self, connection_id: u64) {
        let mut plans = self.restore_plans.lock().await;
        let owned_plan_ids = plans
            .iter()
            .filter(|(_, plan)| plan.connection_id == connection_id)
            .map(|(plan_id, _)| plan_id.clone())
            .collect::<Vec<_>>();
        let mut recovery_attempts = Vec::new();
        for plan_id in owned_plan_ids {
            let Some(plan) = plans.get_mut(&plan_id) else {
                continue;
            };
            if matches!(&plan.status, RestorePlanStatus::InFlight) {
                plan.owner_disconnected = true;
                continue;
            }
            let recovery_action = recovery_action(&plan.status);
            let removed = plans.remove(&plan_id).expect("restore plan still exists");
            removed.notify.notify_waiters();
            if let Some(action) = recovery_action {
                recovery_attempts.push((removed, action));
            }
        }
        drop(plans);
        for (plan, action) in recovery_attempts {
            let runtime = self.runtime_arc();
            tokio::spawn(async move {
                runtime
                    .reconcile_disconnected_restore_plan(plan, action)
                    .await;
            });
        }
    }

    pub(crate) async fn handle_session_rollback_preview(
        &self,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionRollbackPreviewParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/rollback/preview params: {error}"),
                );
            }
        };
        let Ok(session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "invalid session id",
            );
        };
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot preview rollback while a turn is active",
            );
        }
        let Some(session_handle) = self.session(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let _state_change_guard = session_handle.lock_state_change().await;
        if self.runtime_active_turn_id(session_id).await.is_some() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::TurnAlreadyRunning,
                "cannot preview rollback while a turn is active",
            );
        }
        let Some(source) = session_handle.export_runtime_session().await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };
        let rollback_mode = legacy_rollback_mode(params.mode);
        let rebuilt = match self
            .build_runtime_session_from_user_turn_cut(
                &source,
                RuntimeSessionTurnCutOptions {
                    session_id,
                    user_turn_index: Some(params.user_turn_index),
                    rollback_mode: rollback_mode.clone(),
                    cwd_override: None,
                    title_override: source.summary.title.clone(),
                    created_at: source.summary.created_at,
                },
            )
            .await
        {
            Ok(rebuilt) => rebuilt,
            Err(message) => {
                return self.error_response(request_id, ProtocolErrorCode::InvalidParams, message);
            }
        };
        let dropped_turn_ids =
            dropped_turn_ids(&source.persisted_turn_items, &rebuilt.persisted_turn_items);
        let checkpoint = match source.record.as_ref() {
            Some(record) => {
                let rollout_store = self.rollout_store.clone();
                let record = record.clone();
                let checkpoints = tokio::task::spawn_blocking(move || {
                    rollout_store.workspace_checkpoints(&record)
                })
                .await;
                match checkpoints {
                    Ok(Ok(checkpoints)) => dropped_turn_ids.first().and_then(|turn_id| {
                        checkpoints.into_iter().rev().find(|checkpoint| {
                            checkpoint.turn_id == *turn_id
                                && checkpoint.backend.as_deref() == Some("git_ghost_commit")
                                && checkpoint.workspace_root.is_some()
                        })
                    }),
                    Ok(Err(error)) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to read workspace checkpoints: {error}"),
                        );
                    }
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("workspace checkpoint read task failed: {error}"),
                        );
                    }
                }
            }
            None => None,
        };
        let (workspace_version, affected_files) = match checkpoint.as_ref() {
            Some(checkpoint) => {
                let workspace_root = PathBuf::from(
                    checkpoint
                        .workspace_root
                        .as_deref()
                        .expect("Git checkpoint has workspace root"),
                );
                match crate::workspace_changes::preview_git_rollback(
                    workspace_root,
                    checkpoint.checkpoint_id.clone(),
                )
                .await
                {
                    Ok(preview) => (preview.workspace_version, preview.affected_files),
                    Err(error) => {
                        return self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to preview workspace rollback: {error}"),
                        );
                    }
                }
            }
            None => (HISTORY_ONLY_WORKSPACE_VERSION.to_string(), Vec::new()),
        };
        let restore_plan_id = RestorePlanId::new();
        let public_plan = RestorePlan {
            restore_plan_id: restore_plan_id.clone(),
            affected_files,
            dropped_turn_count: u32::try_from(dropped_turn_ids.len()).unwrap_or(u32::MAX),
            workspace_version,
        };
        let mut plans = self.restore_plans.lock().await;
        let now = Utc::now();
        plans.retain(|_, plan| {
            matches!(
                &plan.status,
                RestorePlanStatus::InFlight
                    | RestorePlanStatus::WorkspaceRestoreRetry { .. }
                    | RestorePlanStatus::WorkspaceCompletionPending { .. }
                    | RestorePlanStatus::HistoryPending { .. }
            ) || plan.expires_at + RESTORE_PLAN_TOMBSTONE_TTL > now
        });
        if plans.len() >= RESTORE_PLAN_STORE_LIMIT {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                "restore plan store is full; retry after an existing plan expires",
            );
        }
        plans.insert(
            restore_plan_id.to_string(),
            StoredRestorePlan {
                connection_id,
                owner_disconnected: false,
                session_id,
                user_turn_index: params.user_turn_index,
                rollback_mode,
                history_fingerprint: history_fingerprint(&source.persisted_turn_items),
                checkpoint,
                public_plan: public_plan.clone(),
                expires_at: now + RESTORE_PLAN_TTL,
                status: RestorePlanStatus::Ready,
                notify: Arc::new(Notify::new()),
            },
        );
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: public_plan,
        })
        .expect("serialize session/rollback/preview response")
    }

    pub(crate) async fn handle_session_rollback_commit(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: SessionRollbackCommitParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid session/rollback/commit params: {error}"),
                );
            }
        };
        let (plan, action) = loop {
            let mut plans = self.restore_plans.lock().await;
            let Some(stored) = plans.get_mut(params.restore_plan_id.as_str()) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::RestorePlanNotFound,
                    "restore plan does not exist",
                );
            };
            if stored.connection_id != connection_id {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::RestorePlanNotFound,
                    "restore plan does not exist",
                );
            }
            if Utc::now() > stored.expires_at
                && matches!(
                    &stored.status,
                    RestorePlanStatus::Ready | RestorePlanStatus::Completed(_)
                )
            {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::RestorePlanExpired,
                    "restore plan has expired",
                );
            }
            if params.expected_workspace_version != stored.public_plan.workspace_version {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::WorkspaceVersionConflict,
                    "expected workspace version does not match the restore plan",
                );
            }
            let action = match &stored.status {
                RestorePlanStatus::Ready => CommitAction::Full,
                RestorePlanStatus::WorkspaceRestoreRetry {
                    expected_workspace_version,
                } => CommitAction::WorkspaceRestoreRetry {
                    expected_workspace_version: expected_workspace_version.clone(),
                },
                RestorePlanStatus::WorkspaceCompletionPending {
                    completed,
                    restored_file_count,
                } => CommitAction::WorkspaceCompletionPending {
                    completed: completed.clone(),
                    restored_file_count: *restored_file_count,
                },
                RestorePlanStatus::HistoryPending {
                    restored_file_count,
                } => CommitAction::HistoryPending {
                    restored_file_count: *restored_file_count,
                },
                RestorePlanStatus::Completed(result) => {
                    return rollback_commit_response(request_id, result.clone());
                }
                RestorePlanStatus::InFlight => {
                    let notified = Arc::clone(&stored.notify).notified_owned();
                    drop(plans);
                    notified.await;
                    continue;
                }
            };
            stored.status = RestorePlanStatus::InFlight;
            let plan = stored.clone();
            drop(plans);
            break (plan, action);
        };

        let runtime = Arc::clone(self);
        let plan_id = params.restore_plan_id.to_string();
        let failed_task_plan_id = plan_id.clone();
        let attempt_request_id = request_id.clone();
        let attempt_action = action.clone();
        let attempt = tokio::spawn(async move {
            let attempt = runtime
                .execute_rollback_commit_attempt(attempt_request_id, &plan, attempt_action)
                .await;
            let mut plans = runtime.restore_plans.lock().await;
            let disconnected_recovery = if plans
                .get(&plan_id)
                .is_some_and(|stored| stored.owner_disconnected)
            {
                let recovery = attempt
                    .as_ref()
                    .err()
                    .and_then(|failure| recovery_action(&failure.next_status));
                if let Some(stored) = plans.remove(&plan_id) {
                    stored.notify.notify_waiters();
                }
                recovery
            } else {
                if let Some(stored) = plans.get_mut(&plan_id) {
                    stored.status = match &attempt {
                        Ok(result) => RestorePlanStatus::Completed(result.clone()),
                        Err(failure) => failure.next_status.clone(),
                    };
                    stored.notify.notify_waiters();
                }
                None
            };
            drop(plans);
            if let Some(recovery_action) = disconnected_recovery {
                runtime
                    .reconcile_disconnected_restore_plan(plan.clone(), recovery_action)
                    .await;
            }
            attempt
        })
        .await;
        match attempt {
            Ok(Ok(result)) => rollback_commit_response(request_id, result),
            Ok(Err(failure)) => failure.response,
            Err(error) => {
                let mut plans = self.restore_plans.lock().await;
                if plans
                    .get(&failed_task_plan_id)
                    .is_some_and(|stored| stored.owner_disconnected)
                {
                    if let Some(stored) = plans.remove(&failed_task_plan_id) {
                        stored.notify.notify_waiters();
                    }
                } else if let Some(stored) = plans.get_mut(&failed_task_plan_id) {
                    stored.status = status_for_retry(&action);
                    stored.notify.notify_waiters();
                }
                self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("rollback commit task failed: {error}"),
                )
            }
        }
    }

    async fn reconcile_disconnected_restore_plan(
        &self,
        plan: StoredRestorePlan,
        action: CommitAction,
    ) {
        tracing::warn!(
            restore_plan_id = %plan.public_plan.restore_plan_id,
            session_id = %plan.session_id,
            "restore plan owner disconnected; attempting one background recovery"
        );
        if let Err(recovery_failure) = self
            .execute_rollback_commit_attempt(serde_json::Value::Null, &plan, action)
            .await
        {
            tracing::error!(
                restore_plan_id = %plan.public_plan.restore_plan_id,
                session_id = %plan.session_id,
                response = %recovery_failure.response,
                "disconnected restore plan recovery did not complete"
            );
        }
    }

    async fn execute_rollback_commit_attempt(
        &self,
        request_id: serde_json::Value,
        plan: &StoredRestorePlan,
        action: CommitAction,
    ) -> Result<SessionRollbackCommitResult, CommitAttemptFailure> {
        let retry_status = status_for_retry(&action);
        if self.runtime_active_turn_id(plan.session_id).await.is_some() {
            return Err(CommitAttemptFailure {
                response: self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "cannot commit rollback while a turn is active",
                ),
                next_status: retry_status.clone(),
            });
        }
        let Some(session_handle) = self.session(plan.session_id).await else {
            return Err(CommitAttemptFailure {
                response: self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                ),
                next_status: retry_status,
            });
        };
        let _state_change_guard = session_handle.lock_state_change().await;
        if self.runtime_active_turn_id(plan.session_id).await.is_some() {
            return Err(CommitAttemptFailure {
                response: self.error_response(
                    request_id,
                    ProtocolErrorCode::TurnAlreadyRunning,
                    "cannot commit rollback while a turn is active",
                ),
                next_status: retry_status,
            });
        }
        let Some(source) = session_handle.export_runtime_session().await else {
            return Err(CommitAttemptFailure {
                response: self.error_response(
                    request_id,
                    ProtocolErrorCode::SessionNotFound,
                    "session does not exist",
                ),
                next_status: retry_status,
            });
        };
        if history_fingerprint(&source.persisted_turn_items) != plan.history_fingerprint {
            return Err(CommitAttemptFailure {
                response: self.error_response(
                    request_id,
                    ProtocolErrorCode::WorkspaceVersionConflict,
                    "session history changed after rollback preview",
                ),
                next_status: retry_status,
            });
        }
        if matches!(action, CommitAction::Full)
            && let Some(checkpoint) = plan.checkpoint.as_ref()
        {
            let workspace_root = PathBuf::from(
                checkpoint
                    .workspace_root
                    .as_deref()
                    .expect("Git checkpoint has workspace root"),
            );
            let workspace_matches = crate::workspace_changes::git_workspace_matches_version(
                workspace_root,
                plan.public_plan.workspace_version.clone(),
            )
            .await;
            match workspace_matches {
                Ok(true) => {}
                Ok(false) => {
                    return Err(CommitAttemptFailure {
                        response: self.error_response(
                            request_id,
                            ProtocolErrorCode::WorkspaceVersionConflict,
                            "workspace changed after rollback preview",
                        ),
                        next_status: RestorePlanStatus::Ready,
                    });
                }
                Err(error) => {
                    return Err(CommitAttemptFailure {
                        response: self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to validate workspace version: {error}"),
                        ),
                        next_status: RestorePlanStatus::Ready,
                    });
                }
            }
        }
        if let CommitAction::WorkspaceRestoreRetry {
            expected_workspace_version,
        } = &action
            && let Some(checkpoint) = plan.checkpoint.as_ref()
        {
            let workspace_root = PathBuf::from(
                checkpoint
                    .workspace_root
                    .as_deref()
                    .expect("Git checkpoint has workspace root"),
            );
            match crate::workspace_changes::git_workspace_matches_version(
                workspace_root,
                expected_workspace_version.clone(),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    return Err(CommitAttemptFailure {
                        response: self.error_response(
                            request_id,
                            ProtocolErrorCode::WorkspaceVersionConflict,
                            "workspace changed after the failed restore attempt",
                        ),
                        next_status: retry_status,
                    });
                }
                Err(error) => {
                    return Err(CommitAttemptFailure {
                        response: self.error_response(
                            request_id,
                            ProtocolErrorCode::InternalError,
                            format!("failed to validate recovery workspace version: {error}"),
                        ),
                        next_status: retry_status,
                    });
                }
            }
        }
        let mut rebuilt = self
            .build_runtime_session_from_user_turn_cut(
                &source,
                RuntimeSessionTurnCutOptions {
                    session_id: plan.session_id,
                    user_turn_index: Some(plan.user_turn_index),
                    rollback_mode: plan.rollback_mode.clone(),
                    cwd_override: None,
                    title_override: source.summary.title.clone(),
                    created_at: source.summary.created_at,
                },
            )
            .await
            .map_err(|message| CommitAttemptFailure {
                response: self.error_response(
                    request_id.clone(),
                    ProtocolErrorCode::InvalidParams,
                    message,
                ),
                next_status: retry_status.clone(),
            })?;
        let record = source.record.clone();
        let restored_file_count = match action {
            CommitAction::HistoryPending {
                restored_file_count,
            } => restored_file_count,
            CommitAction::WorkspaceCompletionPending {
                completed,
                restored_file_count,
            } => {
                let checkpoint = plan
                    .checkpoint
                    .as_ref()
                    .expect("workspace completion retry has checkpoint");
                self.persist_workspace_restore_completion(
                    &request_id,
                    plan.public_plan.restore_plan_id.as_str(),
                    checkpoint.turn_id,
                    &completed,
                    record.as_ref(),
                )
                .await
                .map_err(|response| CommitAttemptFailure {
                    response,
                    next_status: RestorePlanStatus::WorkspaceCompletionPending {
                        completed,
                        restored_file_count,
                    },
                })?;
                restored_file_count
            }
            CommitAction::Full | CommitAction::WorkspaceRestoreRetry { .. } => {
                if let Some(checkpoint) = plan.checkpoint.as_ref() {
                    self.commit_workspace_restore(
                        &request_id,
                        plan.public_plan.restore_plan_id.as_str(),
                        checkpoint,
                        &plan.public_plan.affected_files,
                        record.as_ref(),
                    )
                    .await
                    .map_err(|failure| CommitAttemptFailure {
                        response: failure.response,
                        next_status: if let Some((completed, restored_file_count)) =
                            failure.completion_pending
                        {
                            RestorePlanStatus::WorkspaceCompletionPending {
                                completed,
                                restored_file_count,
                            }
                        } else if let Some(expected_workspace_version) =
                            failure.retry_workspace_version
                        {
                            RestorePlanStatus::WorkspaceRestoreRetry {
                                expected_workspace_version,
                            }
                        } else {
                            retry_status
                        },
                    })?
                } else {
                    0
                }
            }
        };
        let (retained_turn_ids, retained_item_ids) = retained_ids(&rebuilt.persisted_turn_items);
        let latest_turn_id = rebuilt.latest_turn.as_ref().map(|turn| turn.turn_id);
        if let Some(record) = record.as_ref()
            && let Err(error) = self.rollout_store.append_session_rollback(
                record,
                retained_turn_ids,
                retained_item_ids,
                latest_turn_id,
            )
        {
            return Err(CommitAttemptFailure {
                response: self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist session rollback: {error}"),
                ),
                next_status: RestorePlanStatus::HistoryPending {
                    restored_file_count,
                },
            });
        }
        rebuilt.record = record;
        session_handle
            .replace_state(SessionActorState::from_runtime_session(rebuilt))
            .await;
        Ok(SessionRollbackCommitResult {
            restored_turn_count: plan.public_plan.dropped_turn_count,
            restored_file_count,
        })
    }
}
