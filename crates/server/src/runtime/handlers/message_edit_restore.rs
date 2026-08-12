use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use devo_core::{
    FileRestoreOutcome, RestoreFileStatus, ToolResultItem, TurnId, TurnItem,
    TurnWorkspaceRestoreCompletedRecord, TurnWorkspaceRestoreStartedRecord, WorkspaceRestorePolicy,
};

use crate::execution::PersistedTurnItem;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RestoreCandidate {
    file_path: String,
    state: RestoreCandidateState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RestoreCandidateState {
    Added {
        post_content: String,
    },
    Deleted {
        pre_content: String,
    },
    Modified {
        pre_content: String,
        post_content: String,
    },
    Moved {
        source_path: String,
        pre_content: String,
        post_content: String,
    },
    Unsupported,
}

pub(super) fn core_restore_policy(
    policy: devo_protocol::native::rpc_session::MessageEditWorkspaceRestore,
) -> WorkspaceRestorePolicy {
    match policy {
        devo_protocol::native::rpc_session::MessageEditWorkspaceRestore::Safe => {
            WorkspaceRestorePolicy::Safe
        }
        devo_protocol::native::rpc_session::MessageEditWorkspaceRestore::Skip => {
            WorkspaceRestorePolicy::Skip
        }
        devo_protocol::native::rpc_session::MessageEditWorkspaceRestore::ConfiguredRestore => {
            WorkspaceRestorePolicy::ConfiguredRestore
        }
    }
}

pub(super) fn discover_restore_candidates(
    persisted_items: &[PersistedTurnItem],
    turn_id: TurnId,
) -> Vec<RestoreCandidate> {
    let mut candidates = Vec::new();
    for item in persisted_items
        .iter()
        .filter(|item| item.turn_id == turn_id)
    {
        let TurnItem::ToolResult(ToolResultItem {
            tool_name: Some(tool_name),
            output,
            is_error: false,
            ..
        }) = &item.turn_item
        else {
            continue;
        };
        if !matches!(tool_name.as_str(), "write" | "apply_patch" | "edit") {
            continue;
        }
        collect_candidates_from_tool_output(output, &mut candidates);
    }
    candidates
}

pub(super) fn candidate_files(candidates: &[RestoreCandidate]) -> Vec<String> {
    let mut files = Vec::new();
    for candidate in candidates {
        if !files.contains(&candidate.file_path) {
            files.push(candidate.file_path.clone());
        }
    }
    files
}

pub(super) async fn apply_safe_workspace_restore(
    workspace_root: &Path,
    candidates: &[RestoreCandidate],
) -> Vec<FileRestoreOutcome> {
    let mut outcomes = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let status = restore_candidate(workspace_root, candidate).await;
        outcomes.push(FileRestoreOutcome {
            file_path: candidate.file_path.clone(),
            status,
        });
    }
    outcomes
}

pub(super) fn restore_started_payload(
    record: &TurnWorkspaceRestoreStartedRecord,
    edit_id: &str,
) -> crate::WorkspaceRestoreStartedPayload {
    crate::WorkspaceRestoreStartedPayload {
        session_id: record.session_id,
        edit_id: edit_id.to_string(),
        superseded_turn_id: record.turn_id,
        checkpoint_id: None,
        candidate_files: record.candidate_files.clone(),
        restore_policy: restore_policy_name(record.policy).to_string(),
        timestamp: record.started_at,
    }
}

pub(super) fn restore_completed_payload(
    record: &TurnWorkspaceRestoreCompletedRecord,
    edit_id: &str,
    superseded_turn_id: TurnId,
) -> crate::WorkspaceRestoreCompletedPayload {
    crate::WorkspaceRestoreCompletedPayload {
        session_id: record.session_id,
        edit_id: edit_id.to_string(),
        superseded_turn_id,
        restored_files: files_with_status(&record.outcomes, RestoreFileStatus::Restored),
        skipped_files: files_with_status(&record.outcomes, RestoreFileStatus::Skipped),
        unsupported_files: files_with_status(&record.outcomes, RestoreFileStatus::Unsupported),
        failed_files: files_with_status(&record.outcomes, RestoreFileStatus::Failed),
        current_state_kept: record
            .outcomes
            .iter()
            .any(|outcome| outcome.status != RestoreFileStatus::Restored),
        timestamp: record.completed_at,
    }
}

fn collect_candidates_from_tool_output(
    output: &serde_json::Value,
    candidates: &mut Vec<RestoreCandidate>,
) {
    let Some(files) = output.get("files").and_then(serde_json::Value::as_array) else {
        return;
    };
    let top_level_diff = output.get("diff").and_then(serde_json::Value::as_str);
    for file in files {
        let Some(object) = file.as_object() else {
            continue;
        };
        let Some(file_path) = string_field(object, &["path", "filePath", "relativePath"]) else {
            continue;
        };
        let kind = string_field(object, &["kind", "type"]).unwrap_or_else(|| "update".to_string());
        let (file_path, state) = match kind.as_str() {
            "add" => {
                let state = string_field(object, &["content", "postContent", "post_content"])
                    .map_or(RestoreCandidateState::Unsupported, |post_content| {
                        RestoreCandidateState::Added { post_content }
                    });
                (file_path, state)
            }
            "delete" | "remove" | "unlink" => {
                let state = string_field(
                    object,
                    &[
                        "content",
                        "preContent",
                        "pre_content",
                        "oldContent",
                        "old_content",
                    ],
                )
                .map_or(RestoreCandidateState::Unsupported, |pre_content| {
                    RestoreCandidateState::Deleted { pre_content }
                });
                (file_path, state)
            }
            "update" => {
                let diff = string_field(object, &["diff", "patch"])
                    .or_else(|| top_level_diff.map(ToOwned::to_owned));
                let pre_content = string_field(
                    object,
                    &[
                        "preContent",
                        "pre_content",
                        "oldContent",
                        "old_content",
                        "previousContent",
                        "previous_content",
                    ],
                );
                let post_content = string_field(
                    object,
                    &[
                        "postContent",
                        "post_content",
                        "newContent",
                        "new_content",
                        "content",
                    ],
                );
                let pre_content = pre_content.or_else(|| {
                    let post_content = post_content.as_deref()?;
                    let diff = diff.as_deref()?;
                    pre_content_from_reverse_patch(post_content, diff)
                });
                match (pre_content, post_content) {
                    (Some(pre_content), Some(post_content)) => (
                        file_path,
                        RestoreCandidateState::Modified {
                            pre_content,
                            post_content,
                        },
                    ),
                    _ => (file_path, RestoreCandidateState::Unsupported),
                }
            }
            "move" => {
                let target_path = string_field(
                    object,
                    &[
                        "path",
                        "relativePath",
                        "relative_path",
                        "targetPath",
                        "target_path",
                        "newPath",
                        "new_path",
                    ],
                )
                .or_else(|| string_field(object, &["movePath", "move_path"]))
                .unwrap_or_else(|| file_path.clone());
                let source_path = string_field(
                    object,
                    &[
                        "sourcePath",
                        "source_path",
                        "oldPath",
                        "old_path",
                        "fromPath",
                        "from_path",
                        "filePath",
                    ],
                );
                let diff = string_field(object, &["diff", "patch"])
                    .or_else(|| top_level_diff.map(ToOwned::to_owned));
                let pre_content = string_field(
                    object,
                    &[
                        "preContent",
                        "pre_content",
                        "oldContent",
                        "old_content",
                        "previousContent",
                        "previous_content",
                    ],
                );
                let post_content = string_field(
                    object,
                    &[
                        "postContent",
                        "post_content",
                        "newContent",
                        "new_content",
                        "content",
                    ],
                );
                let pre_content = pre_content.or_else(|| {
                    let post_content = post_content.as_deref()?;
                    let diff = diff.as_deref()?;
                    pre_content_from_reverse_patch(post_content, diff)
                });
                match (source_path, pre_content, post_content) {
                    (Some(source_path), Some(pre_content), Some(post_content)) => (
                        target_path,
                        RestoreCandidateState::Moved {
                            source_path,
                            pre_content,
                            post_content,
                        },
                    ),
                    _ => (target_path, RestoreCandidateState::Unsupported),
                }
            }
            _ => (file_path, RestoreCandidateState::Unsupported),
        };
        candidates.push(RestoreCandidate { file_path, state });
    }
}

async fn restore_candidate(
    workspace_root: &Path,
    candidate: &RestoreCandidate,
) -> RestoreFileStatus {
    let Some(path) = resolve_candidate_path(workspace_root, &candidate.file_path) else {
        return RestoreFileStatus::Unsupported;
    };
    match &candidate.state {
        RestoreCandidateState::Added { post_content } => {
            restore_added_file(&path, post_content).await
        }
        RestoreCandidateState::Deleted { pre_content } => {
            restore_deleted_file(&path, pre_content).await
        }
        RestoreCandidateState::Modified {
            pre_content,
            post_content,
        } => restore_modified_file(&path, pre_content, post_content).await,
        RestoreCandidateState::Moved {
            source_path,
            pre_content,
            post_content,
        } => {
            let Some(source_path) = resolve_candidate_path(workspace_root, source_path) else {
                return RestoreFileStatus::Unsupported;
            };
            if source_path == path {
                restore_modified_file(&path, pre_content, post_content).await
            } else {
                restore_moved_file(&source_path, &path, pre_content, post_content).await
            }
        }
        RestoreCandidateState::Unsupported => RestoreFileStatus::Unsupported,
    }
}

async fn restore_added_file(path: &Path, post_content: &str) -> RestoreFileStatus {
    match tokio::fs::read_to_string(path).await {
        Ok(current_content) if current_content == post_content => {
            match tokio::fs::remove_file(path).await {
                Ok(()) => RestoreFileStatus::Restored,
                Err(error) if error.kind() == ErrorKind::NotFound => RestoreFileStatus::Restored,
                Err(_) => RestoreFileStatus::Failed,
            }
        }
        Ok(_) => RestoreFileStatus::Skipped,
        Err(error) if error.kind() == ErrorKind::NotFound => RestoreFileStatus::Restored,
        Err(error) if error.kind() == ErrorKind::InvalidData => RestoreFileStatus::Skipped,
        Err(_) => RestoreFileStatus::Failed,
    }
}

async fn restore_deleted_file(path: &Path, pre_content: &str) -> RestoreFileStatus {
    match tokio::fs::read_to_string(path).await {
        Ok(_) => RestoreFileStatus::Skipped,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            write_text_file(path, pre_content).await
        }
        Err(error) if error.kind() == ErrorKind::InvalidData => RestoreFileStatus::Skipped,
        Err(_) => RestoreFileStatus::Failed,
    }
}

async fn restore_modified_file(
    path: &Path,
    pre_content: &str,
    post_content: &str,
) -> RestoreFileStatus {
    match tokio::fs::read_to_string(path).await {
        Ok(current_content) if current_content == post_content => {
            write_text_file(path, pre_content).await
        }
        Ok(_) => RestoreFileStatus::Skipped,
        Err(error) if error.kind() == ErrorKind::NotFound => RestoreFileStatus::Skipped,
        Err(error) if error.kind() == ErrorKind::InvalidData => RestoreFileStatus::Skipped,
        Err(_) => RestoreFileStatus::Failed,
    }
}

async fn restore_moved_file(
    source_path: &Path,
    target_path: &Path,
    pre_content: &str,
    post_content: &str,
) -> RestoreFileStatus {
    match tokio::fs::read_to_string(target_path).await {
        Ok(current_content) if current_content == post_content => {}
        Ok(_) => return RestoreFileStatus::Skipped,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return match tokio::fs::read_to_string(source_path).await {
                Ok(current_content) if current_content == pre_content => {
                    RestoreFileStatus::Restored
                }
                Ok(_) => RestoreFileStatus::Skipped,
                Err(error) if error.kind() == ErrorKind::NotFound => RestoreFileStatus::Skipped,
                Err(error) if error.kind() == ErrorKind::InvalidData => RestoreFileStatus::Skipped,
                Err(_) => RestoreFileStatus::Failed,
            };
        }
        Err(error) if error.kind() == ErrorKind::InvalidData => return RestoreFileStatus::Skipped,
        Err(_) => return RestoreFileStatus::Failed,
    }

    match tokio::fs::read_to_string(source_path).await {
        Ok(current_content) if current_content == pre_content => {
            match tokio::fs::remove_file(target_path).await {
                Ok(()) => RestoreFileStatus::Restored,
                Err(error) if error.kind() == ErrorKind::NotFound => RestoreFileStatus::Restored,
                Err(_) => RestoreFileStatus::Failed,
            }
        }
        Ok(_) => RestoreFileStatus::Skipped,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let write_status = write_text_file(source_path, pre_content).await;
            if write_status != RestoreFileStatus::Restored {
                return write_status;
            }
            match tokio::fs::remove_file(target_path).await {
                Ok(()) => RestoreFileStatus::Restored,
                Err(error) if error.kind() == ErrorKind::NotFound => RestoreFileStatus::Restored,
                Err(_) => RestoreFileStatus::Failed,
            }
        }
        Err(error) if error.kind() == ErrorKind::InvalidData => RestoreFileStatus::Skipped,
        Err(_) => RestoreFileStatus::Failed,
    }
}

async fn write_text_file(path: &Path, content: &str) -> RestoreFileStatus {
    if let Some(parent) = path.parent()
        && tokio::fs::create_dir_all(parent).await.is_err()
    {
        return RestoreFileStatus::Failed;
    }
    match tokio::fs::write(path, content).await {
        Ok(()) => RestoreFileStatus::Restored,
        Err(_) => RestoreFileStatus::Failed,
    }
}

fn resolve_candidate_path(workspace_root: &Path, file_path: &str) -> Option<PathBuf> {
    let root = normalize_components(workspace_root);
    let raw_path = PathBuf::from(file_path);
    let candidate = if raw_path.is_absolute() {
        raw_path
    } else {
        root.join(raw_path)
    };
    let candidate = normalize_components(&candidate);
    candidate.starts_with(&root).then_some(candidate)
}

fn normalize_components(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    names: &[&str],
) -> Option<String> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

fn pre_content_from_reverse_patch(post_content: &str, diff: &str) -> Option<String> {
    let patch = diffy::Patch::from_str(diff).ok()?;
    let reverse_patch = patch.reverse();
    diffy::apply(post_content, &reverse_patch).ok()
}

fn restore_policy_name(policy: WorkspaceRestorePolicy) -> &'static str {
    match policy {
        WorkspaceRestorePolicy::Safe => "safe",
        WorkspaceRestorePolicy::Skip => "skip",
        WorkspaceRestorePolicy::ConfiguredRestore => "configured_restore",
    }
}

fn files_with_status(outcomes: &[FileRestoreOutcome], status: RestoreFileStatus) -> Vec<String> {
    outcomes
        .iter()
        .filter(|outcome| outcome.status == status)
        .map(|outcome| outcome.file_path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    /// Trace: L2-DES-APP-003, L1-REQ-CONV-005
    /// Verifies: safe restore preserves diverged file content and reports a skipped outcome.
    #[tokio::test]
    async fn safe_restore_skips_diverged_added_file() {
        let workspace = TempDir::new().expect("tempdir");
        let path = workspace.path().join("generated.txt");
        tokio::fs::write(&path, "user changed\n")
            .await
            .expect("write diverged file");
        let candidates = vec![RestoreCandidate {
            file_path: "generated.txt".to_string(),
            state: RestoreCandidateState::Added {
                post_content: "generated\n".to_string(),
            },
        }];

        let outcomes = apply_safe_workspace_restore(workspace.path(), &candidates).await;

        assert_eq!(
            outcomes,
            vec![FileRestoreOutcome {
                file_path: "generated.txt".to_string(),
                status: RestoreFileStatus::Skipped,
            }]
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.expect("read file"),
            "user changed\n"
        );
    }

    /// Trace: L2-DES-APP-003, L1-REQ-CONV-005
    /// Verifies: safe restore removes an unchanged file that was added by the superseded turn.
    #[tokio::test]
    async fn safe_restore_removes_unchanged_added_file() {
        let workspace = TempDir::new().expect("tempdir");
        let path = workspace.path().join("generated.txt");
        tokio::fs::write(&path, "generated\n")
            .await
            .expect("write generated file");
        let candidates = vec![RestoreCandidate {
            file_path: "generated.txt".to_string(),
            state: RestoreCandidateState::Added {
                post_content: "generated\n".to_string(),
            },
        }];

        let outcomes = apply_safe_workspace_restore(workspace.path(), &candidates).await;

        assert_eq!(
            outcomes,
            vec![FileRestoreOutcome {
                file_path: "generated.txt".to_string(),
                status: RestoreFileStatus::Restored,
            }]
        );
        assert!(!path.exists());
    }

    /// Trace: L2-DES-APP-003, L1-REQ-CONV-005
    /// Verifies: safe restore restores an unchanged modified file to its pre-turn content.
    #[tokio::test]
    async fn safe_restore_restores_unchanged_modified_file() {
        let workspace = TempDir::new().expect("tempdir");
        let path = workspace.path().join("edited.txt");
        tokio::fs::write(&path, "before\n")
            .await
            .expect("write modified file");
        let candidates = vec![RestoreCandidate {
            file_path: "edited.txt".to_string(),
            state: RestoreCandidateState::Modified {
                pre_content: "old\n".to_string(),
                post_content: "before\n".to_string(),
            },
        }];

        let outcomes = apply_safe_workspace_restore(workspace.path(), &candidates).await;

        assert_eq!(
            outcomes,
            vec![FileRestoreOutcome {
                file_path: "edited.txt".to_string(),
                status: RestoreFileStatus::Restored,
            }]
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.expect("read file"),
            "old\n"
        );
    }

    /// Trace: L2-DES-APP-003
    /// Verifies: restore candidate discovery reads structured file metadata from edit tool results.
    #[test]
    fn discovers_structured_file_change_candidates() {
        let turn_id = TurnId::new();
        let item = PersistedTurnItem {
            turn_id,
            turn_kind: devo_core::TurnKind::Regular,
            item_id: devo_core::ItemId::new(),
            turn_item: TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "call-1".to_string(),
                tool_name: Some("apply_patch".to_string()),
                output: serde_json::json!({
                    "files": [{
                        "path": "generated.txt",
                        "kind": "add",
                        "content": "generated\n"
                    }]
                }),
                display_content: None,
                is_error: false,
            }),
        };

        let candidates = discover_restore_candidates(&[item], turn_id);

        assert_eq!(
            candidates,
            vec![RestoreCandidate {
                file_path: "generated.txt".to_string(),
                state: RestoreCandidateState::Added {
                    post_content: "generated\n".to_string(),
                },
            }]
        );
    }

    /// Trace: L2-DES-APP-003, L1-REQ-CONV-005
    /// Verifies: update metadata with post content plus a per-file diff is restorable.
    #[tokio::test]
    async fn discovers_and_restores_modified_file_from_reverse_diff() {
        let workspace = TempDir::new().expect("tempdir");
        let path = workspace.path().join("edited.txt");
        tokio::fs::write(&path, "start\nnew\nend\n")
            .await
            .expect("write modified file");
        let turn_id = TurnId::new();
        let item = PersistedTurnItem {
            turn_id,
            turn_kind: devo_core::TurnKind::Regular,
            item_id: devo_core::ItemId::new(),
            turn_item: TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "call-1".to_string(),
                tool_name: Some("apply_patch".to_string()),
                output: serde_json::json!({
                    "files": [{
                        "path": "edited.txt",
                        "kind": "update",
                        "content": "start\nnew\nend\n",
                        "diff": "diff --git a/edited.txt b/edited.txt\n--- a/edited.txt\n+++ b/edited.txt\n@@ -1,3 +1,3 @@\n start\n-old\n+new\n end\n"
                    }]
                }),
                display_content: None,
                is_error: false,
            }),
        };

        let candidates = discover_restore_candidates(&[item], turn_id);
        let outcomes = apply_safe_workspace_restore(workspace.path(), &candidates).await;

        assert_eq!(
            outcomes,
            vec![FileRestoreOutcome {
                file_path: "edited.txt".to_string(),
                status: RestoreFileStatus::Restored,
            }]
        );
        assert_eq!(
            tokio::fs::read_to_string(&path).await.expect("read file"),
            "start\nold\nend\n"
        );
    }

    /// Trace: L2-DES-APP-003, L1-REQ-CONV-005
    /// Verifies: move metadata with target content plus a diff can be reversed safely.
    #[tokio::test]
    async fn discovers_and_restores_moved_file_from_reverse_diff() {
        let workspace = TempDir::new().expect("tempdir");
        let source_path = workspace.path().join("from.txt");
        let target_path = workspace.path().join("moved").join("to.txt");
        tokio::fs::create_dir_all(target_path.parent().expect("target parent"))
            .await
            .expect("create target parent");
        tokio::fs::write(&target_path, "after\n")
            .await
            .expect("write moved file");
        let turn_id = TurnId::new();
        let item = PersistedTurnItem {
            turn_id,
            turn_kind: devo_core::TurnKind::Regular,
            item_id: devo_core::ItemId::new(),
            turn_item: TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "call-1".to_string(),
                tool_name: Some("apply_patch".to_string()),
                output: serde_json::json!({
                    "files": [{
                        "path": "moved/to.txt",
                        "filePath": source_path,
                        "kind": "move",
                        "content": "after\n",
                        "diff": "diff --git a/moved/to.txt b/moved/to.txt\n--- a/moved/to.txt\n+++ b/moved/to.txt\n@@ -1,1 +1,1 @@\n-before\n+after\n",
                        "movePath": target_path
                    }]
                }),
                display_content: None,
                is_error: false,
            }),
        };

        let candidates = discover_restore_candidates(&[item], turn_id);
        let outcomes = apply_safe_workspace_restore(workspace.path(), &candidates).await;

        assert_eq!(
            outcomes,
            vec![FileRestoreOutcome {
                file_path: "moved/to.txt".to_string(),
                status: RestoreFileStatus::Restored,
            }]
        );
        assert_eq!(
            tokio::fs::read_to_string(&source_path)
                .await
                .expect("read restored source"),
            "before\n"
        );
        assert!(!target_path.exists());
    }

    /// Trace: L2-DES-APP-003, L1-REQ-CONV-005
    /// Verifies: safe restore preserves a moved target that diverged after the superseded turn.
    #[tokio::test]
    async fn safe_restore_skips_diverged_moved_file() {
        let workspace = TempDir::new().expect("tempdir");
        let source_path = workspace.path().join("from.txt");
        let target_path = workspace.path().join("moved").join("to.txt");
        tokio::fs::create_dir_all(target_path.parent().expect("target parent"))
            .await
            .expect("create target parent");
        tokio::fs::write(&target_path, "user changed\n")
            .await
            .expect("write diverged target");
        let candidates = vec![RestoreCandidate {
            file_path: "moved/to.txt".to_string(),
            state: RestoreCandidateState::Moved {
                source_path: source_path.to_string_lossy().into_owned(),
                pre_content: "before\n".to_string(),
                post_content: "after\n".to_string(),
            },
        }];

        let outcomes = apply_safe_workspace_restore(workspace.path(), &candidates).await;

        assert_eq!(
            outcomes,
            vec![FileRestoreOutcome {
                file_path: "moved/to.txt".to_string(),
                status: RestoreFileStatus::Skipped,
            }]
        );
        assert_eq!(
            tokio::fs::read_to_string(&target_path)
                .await
                .expect("read diverged target"),
            "user changed\n"
        );
        assert!(!source_path.exists());
    }
}
