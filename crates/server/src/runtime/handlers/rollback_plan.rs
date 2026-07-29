//! In-memory state and pure helpers for P4d restore plans.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use devo_core::TurnWorkspaceRestoreCompletedRecord;
use devo_core::{ItemId, SessionId, TurnWorkspaceCheckpointRecordedRecord};
use devo_protocol::canonical::rpc_session::{
    RestorePlan, RollbackMode, SessionRollbackCommitResult,
};
use tokio::sync::Notify;

use super::super::*;

pub(super) const RESTORE_PLAN_TTL: Duration = Duration::minutes(10);
pub(super) const RESTORE_PLAN_TOMBSTONE_TTL: Duration = Duration::minutes(10);
pub(super) const RESTORE_PLAN_STORE_LIMIT: usize = 1024;
pub(super) const HISTORY_ONLY_WORKSPACE_VERSION: &str = "history-only";

pub(crate) type RestorePlanStore = HashMap<String, StoredRestorePlan>;

#[derive(Debug, Clone)]
pub(crate) struct StoredRestorePlan {
    pub(super) connection_id: u64,
    pub(super) owner_disconnected: bool,
    pub(super) session_id: SessionId,
    pub(super) user_turn_index: u32,
    pub(super) rollback_mode: SessionRollbackMode,
    pub(super) history_fingerprint: Vec<(TurnId, ItemId)>,
    pub(super) checkpoint: Option<TurnWorkspaceCheckpointRecordedRecord>,
    pub(super) public_plan: RestorePlan,
    pub(super) expires_at: DateTime<Utc>,
    pub(super) status: RestorePlanStatus,
    pub(super) notify: Arc<Notify>,
}

#[derive(Debug, Clone)]
pub(super) enum RestorePlanStatus {
    Ready,
    InFlight,
    // A restore may have changed files before an error was reported. These
    // recovery phases deliberately outlive the normal plan TTL: retry must
    // finish the same checkpoint restore/history cut instead of comparing
    // against the now-obsolete preview worktree.
    WorkspaceRestoreRetry {
        expected_workspace_version: String,
    },
    WorkspaceCompletionPending {
        completed: TurnWorkspaceRestoreCompletedRecord,
        restored_file_count: u32,
    },
    HistoryPending {
        restored_file_count: u32,
    },
    Completed(SessionRollbackCommitResult),
}

#[derive(Debug, Clone)]
pub(super) enum CommitAction {
    Full,
    WorkspaceRestoreRetry {
        expected_workspace_version: String,
    },
    WorkspaceCompletionPending {
        completed: TurnWorkspaceRestoreCompletedRecord,
        restored_file_count: u32,
    },
    HistoryPending {
        restored_file_count: u32,
    },
}

pub(super) struct CommitAttemptFailure {
    pub response: serde_json::Value,
    pub next_status: RestorePlanStatus,
}

pub(super) fn legacy_rollback_mode(mode: RollbackMode) -> SessionRollbackMode {
    match mode {
        RollbackMode::ThroughUserTurn => SessionRollbackMode::ThroughUserTurn,
        RollbackMode::BeforeUserTurn => SessionRollbackMode::BeforeUserTurn,
    }
}

pub(super) fn status_for_retry(action: &CommitAction) -> RestorePlanStatus {
    match action {
        CommitAction::Full => RestorePlanStatus::Ready,
        CommitAction::WorkspaceRestoreRetry {
            expected_workspace_version,
        } => RestorePlanStatus::WorkspaceRestoreRetry {
            expected_workspace_version: expected_workspace_version.clone(),
        },
        CommitAction::WorkspaceCompletionPending {
            completed,
            restored_file_count,
        } => RestorePlanStatus::WorkspaceCompletionPending {
            completed: completed.clone(),
            restored_file_count: *restored_file_count,
        },
        CommitAction::HistoryPending {
            restored_file_count,
        } => RestorePlanStatus::HistoryPending {
            restored_file_count: *restored_file_count,
        },
    }
}

pub(super) fn recovery_action(status: &RestorePlanStatus) -> Option<CommitAction> {
    match status {
        RestorePlanStatus::WorkspaceRestoreRetry {
            expected_workspace_version,
        } => Some(CommitAction::WorkspaceRestoreRetry {
            expected_workspace_version: expected_workspace_version.clone(),
        }),
        RestorePlanStatus::WorkspaceCompletionPending {
            completed,
            restored_file_count,
        } => Some(CommitAction::WorkspaceCompletionPending {
            completed: completed.clone(),
            restored_file_count: *restored_file_count,
        }),
        RestorePlanStatus::HistoryPending {
            restored_file_count,
        } => Some(CommitAction::HistoryPending {
            restored_file_count: *restored_file_count,
        }),
        RestorePlanStatus::Ready
        | RestorePlanStatus::InFlight
        | RestorePlanStatus::Completed(_) => None,
    }
}

pub(super) fn history_fingerprint(
    items: &[crate::execution::PersistedTurnItem],
) -> Vec<(TurnId, ItemId)> {
    items
        .iter()
        .map(|item| (item.turn_id, item.item_id))
        .collect()
}

pub(super) fn dropped_turn_ids(
    source: &[crate::execution::PersistedTurnItem],
    retained: &[crate::execution::PersistedTurnItem],
) -> Vec<TurnId> {
    let retained_ids: HashSet<TurnId> = retained.iter().map(|item| item.turn_id).collect();
    let mut dropped = Vec::new();
    for item in source {
        if !retained_ids.contains(&item.turn_id) && !dropped.contains(&item.turn_id) {
            dropped.push(item.turn_id);
        }
    }
    dropped
}

pub(super) fn retained_ids(
    items: &[crate::execution::PersistedTurnItem],
) -> (Vec<TurnId>, Vec<ItemId>) {
    let mut turn_ids = Vec::new();
    let mut item_ids = Vec::new();
    for item in items {
        if !turn_ids.contains(&item.turn_id) {
            turn_ids.push(item.turn_id);
        }
        if !item_ids.contains(&item.item_id) {
            item_ids.push(item.item_id);
        }
    }
    (turn_ids, item_ids)
}

pub(super) fn rollback_commit_response(
    request_id: serde_json::Value,
    result: SessionRollbackCommitResult,
) -> serde_json::Value {
    serde_json::to_value(SuccessResponse {
        id: request_id,
        result,
    })
    .expect("serialize session/rollback/commit response")
}
