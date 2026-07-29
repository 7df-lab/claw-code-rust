//! Git workspace side effects for two-phase session rollback.

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::Utc;
use devo_core::{
    FileRestoreOutcome, RestoreFileStatus, RestoreId, SessionId,
    TurnWorkspaceCheckpointRecordedRecord, TurnWorkspaceRestoreCompletedRecord,
    TurnWorkspaceRestoreStartedRecord, WorkspaceRestorePolicy,
};

use super::super::*;

pub(super) struct WorkspaceRestoreFailure {
    pub response: serde_json::Value,
    pub retry_workspace_version: Option<String>,
    pub completion_pending: Option<(TurnWorkspaceRestoreCompletedRecord, u32)>,
}

impl ServerRuntime {
    pub(super) async fn commit_workspace_restore(
        &self,
        request_id: &serde_json::Value,
        restore_plan_id: &str,
        checkpoint: &TurnWorkspaceCheckpointRecordedRecord,
        affected_files: &[PathBuf],
        record: Option<&devo_core::SessionRecord>,
    ) -> Result<u32, WorkspaceRestoreFailure> {
        let workspace_root = PathBuf::from(
            checkpoint
                .workspace_root
                .as_deref()
                .expect("Git checkpoint has workspace root"),
        );
        let restore_id = RestoreId::new();
        let started = TurnWorkspaceRestoreStartedRecord {
            schema_version: 1,
            session_id: checkpoint.session_id,
            turn_id: checkpoint.turn_id,
            restore_id,
            candidate_files: affected_files
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            policy: WorkspaceRestorePolicy::Safe,
            started_at: Utc::now(),
        };
        if let Some(record) = record
            && let Err(error) = self
                .rollout_store
                .append_workspace_restore_started(record, started.clone())
        {
            return Err(WorkspaceRestoreFailure {
                response: self.error_response(
                    request_id.clone(),
                    ProtocolErrorCode::WorkspaceRestoreFailedToStart,
                    format!("failed to persist workspace restore start: {error}"),
                ),
                retry_workspace_version: None,
                completion_pending: None,
            });
        }
        self.broadcast_event(ServerEvent::WorkspaceRestoreStarted(
            super::message_edit_restore::restore_started_payload(&started, restore_plan_id),
        ))
        .await;
        if let Err(error) = crate::workspace_changes::restore_git_checkpoint(
            workspace_root.clone(),
            checkpoint.clone(),
        )
        .await
        {
            let completed = restore_completed_record(
                checkpoint.session_id,
                started.restore_id,
                affected_files,
                &HashSet::new(),
                RestoreFileStatus::Failed,
            );
            if let Some(record) = record {
                let _ = self
                    .rollout_store
                    .append_workspace_restore_completed(record, completed.clone());
            }
            self.broadcast_event(ServerEvent::WorkspaceRestoreCompleted(
                super::message_edit_restore::restore_completed_payload(
                    &completed,
                    restore_plan_id,
                    checkpoint.turn_id,
                ),
            ))
            .await;
            let retry_workspace_version =
                crate::workspace_changes::current_git_workspace_version(workspace_root).await.ok();
            return Err(WorkspaceRestoreFailure {
                response: self.error_response(
                    request_id.clone(),
                    ProtocolErrorCode::InternalError,
                    format!("failed to restore workspace checkpoint: {error}"),
                ),
                retry_workspace_version,
                completion_pending: None,
            });
        }
        let remaining = match crate::workspace_changes::preview_git_rollback(
            workspace_root,
            checkpoint.checkpoint_id.clone(),
        )
        .await
        {
            Ok(preview) => preview.affected_files.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                let completed = restore_completed_record(
                    checkpoint.session_id,
                    started.restore_id,
                    affected_files,
                    &HashSet::new(),
                    RestoreFileStatus::Failed,
                );
                if let Some(record) = record {
                    let _ = self
                        .rollout_store
                        .append_workspace_restore_completed(record, completed.clone());
                }
                self.broadcast_event(ServerEvent::WorkspaceRestoreCompleted(
                    super::message_edit_restore::restore_completed_payload(
                        &completed,
                        restore_plan_id,
                        checkpoint.turn_id,
                    ),
                ))
                .await;
                return Err(WorkspaceRestoreFailure {
                    response: self.error_response(
                        request_id.clone(),
                        ProtocolErrorCode::InternalError,
                        format!("failed to verify workspace restore: {error}"),
                    ),
                    retry_workspace_version: crate::workspace_changes::current_git_workspace_version(
                        PathBuf::from(
                            checkpoint
                                .workspace_root
                                .as_deref()
                                .expect("Git checkpoint has workspace root"),
                        ),
                    )
                    .await
                    .ok(),
                    completion_pending: None,
                });
            }
        };
        let completed = restore_completed_record(
            checkpoint.session_id,
            started.restore_id,
            affected_files,
            &remaining,
            RestoreFileStatus::Restored,
        );
        let restored_file_count = restored_file_count(&completed);
        if let Some(record) = record
            && let Err(error) = self
                .rollout_store
                .append_workspace_restore_completed(record, completed.clone())
        {
            let retry_workspace_version =
                crate::workspace_changes::current_git_workspace_version(PathBuf::from(
                    checkpoint
                        .workspace_root
                        .as_deref()
                        .expect("Git checkpoint has workspace root"),
                ))
                .await
                .ok();
            return Err(WorkspaceRestoreFailure {
                response: self.error_response(
                    request_id.clone(),
                    ProtocolErrorCode::InternalError,
                    format!("failed to persist workspace restore completion: {error}"),
                ),
                retry_workspace_version,
                completion_pending: Some((completed, restored_file_count)),
            });
        }
        self.broadcast_event(ServerEvent::WorkspaceRestoreCompleted(
            super::message_edit_restore::restore_completed_payload(
                &completed,
                restore_plan_id,
                checkpoint.turn_id,
            ),
        ))
        .await;
        Ok(restored_file_count)
    }

    pub(super) async fn persist_workspace_restore_completion(
        &self,
        request_id: &serde_json::Value,
        restore_plan_id: &str,
        checkpoint_turn_id: devo_core::TurnId,
        completed: &TurnWorkspaceRestoreCompletedRecord,
        record: Option<&devo_core::SessionRecord>,
    ) -> Result<(), serde_json::Value> {
        if let Some(record) = record
            && let Err(error) = self
                .rollout_store
                .append_workspace_restore_completed(record, completed.clone())
        {
            return Err(self.error_response(
                request_id.clone(),
                ProtocolErrorCode::InternalError,
                format!("failed to persist workspace restore completion: {error}"),
            ));
        }
        self.broadcast_event(ServerEvent::WorkspaceRestoreCompleted(
            super::message_edit_restore::restore_completed_payload(
                completed,
                restore_plan_id,
                checkpoint_turn_id,
            ),
        ))
        .await;
        Ok(())
    }
}

fn restored_file_count(completed: &TurnWorkspaceRestoreCompletedRecord) -> u32 {
    u32::try_from(
        completed
            .outcomes
            .iter()
            .filter(|outcome| outcome.status == RestoreFileStatus::Restored)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn restore_completed_record(
    session_id: SessionId,
    restore_id: RestoreId,
    affected_files: &[PathBuf],
    remaining: &HashSet<PathBuf>,
    default_status: RestoreFileStatus,
) -> TurnWorkspaceRestoreCompletedRecord {
    TurnWorkspaceRestoreCompletedRecord {
        schema_version: 1,
        session_id,
        restore_id,
        outcomes: affected_files
            .iter()
            .map(|path| FileRestoreOutcome {
                file_path: path.display().to_string(),
                status: if remaining.contains(path) {
                    RestoreFileStatus::Skipped
                } else {
                    default_status
                },
            })
            .collect(),
        completed_at: Utc::now(),
    }
}
