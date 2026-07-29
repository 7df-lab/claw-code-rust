use std::path::{Path, PathBuf};
use std::process::Output;

use anyhow::{Context, Result};
use devo_core::ChangeSetCoverage;
use devo_core::TurnWorkspaceCheckpointRecordedRecord;
use devo_protocol::{
    SessionId, TurnId, WorkspaceChangeAttribution, WorkspaceChangeBase, WorkspaceChangeCoverage,
    WorkspaceChangeScope, WorkspaceChangeSetStatus, WorkspaceChangeView,
    WorkspaceCheckpointBackend, WorkspaceDiffDetail,
};
use devo_util_git::{
    CreateGhostCommitOptions, GhostCommit, GhostSnapshotReport, create_ghost_commit_with_report,
    default_branch_name, diff_ghost_commits, extract_paths_from_patch, get_git_repo_root,
    merge_base_with_head, restore_ghost_commit, restore_to_commit,
};
use tokio::process::Command;

use super::{ActiveWorkspaceBaseline, CapturedWorkspaceBaseline};
use super::{CheckpointRecordInput, artifact_ref, checkpoint_record, write_json};
use crate::workspace_changes::diff::{DiffViewInput, error_view, unsupported_view, view_from_diff};

#[derive(Debug, Clone)]
pub(crate) struct GitWorkspaceBaseline {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub workspace_root: PathBuf,
    pub checkpoint_id: String,
    ghost: GhostCommit,
    warnings: Vec<String>,
}

/// Rebuilds the restore-capable ghost snapshot from a durable checkpoint.
///
/// Checkpoints written before P4d have no untracked-path manifest. Returning
/// `None` for them makes callers use tracked-file-only restore rather than
/// guessing which user files may safely be deleted.
pub(crate) fn ghost_commit_from_checkpoint(
    checkpoint: &TurnWorkspaceCheckpointRecordedRecord,
) -> Option<GhostCommit> {
    let files = checkpoint.preexisting_untracked_files.as_ref()?;
    let dirs = checkpoint.preexisting_untracked_dirs.as_ref()?;
    Some(GhostCommit::new(
        checkpoint.checkpoint_id.clone(),
        /*parent*/ None,
        files.iter().map(PathBuf::from).collect(),
        dirs.iter().map(PathBuf::from).collect(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitRollbackPreview {
    pub workspace_version: String,
    pub affected_files: Vec<PathBuf>,
}

pub(crate) fn preview_git_rollback(
    workspace_root: &Path,
    checkpoint_id: &str,
) -> Result<GitRollbackPreview> {
    let (current, _) = create_ghost_commit_with_report(
        &CreateGhostCommitOptions::new(workspace_root)
            .message("devo rollback preview")
            .ignore_large_untracked_files(10 * 1024 * 1024),
    )
    .with_context(|| {
        format!(
            "capture rollback workspace version at {}",
            workspace_root.display()
        )
    })?;
    let checkpoint = GhostCommit::new(
        checkpoint_id.to_string(),
        /*parent*/ None,
        Vec::new(),
        Vec::new(),
    );
    let diff = diff_ghost_commits(workspace_root, &checkpoint, &current)
        .with_context(|| format!("diff rollback checkpoint {checkpoint_id}"))?;
    Ok(GitRollbackPreview {
        workspace_version: current.id().to_string(),
        affected_files: extract_paths_from_patch(&diff)
            .into_iter()
            .map(PathBuf::from)
            .collect(),
    })
}

pub(crate) fn git_workspace_matches_version(
    workspace_root: &Path,
    workspace_version: &str,
) -> Result<bool> {
    let (current, _) = create_ghost_commit_with_report(
        &CreateGhostCommitOptions::new(workspace_root)
            .message("devo rollback commit validation")
            .ignore_large_untracked_files(10 * 1024 * 1024),
    )
    .with_context(|| {
        format!(
            "capture rollback workspace version at {}",
            workspace_root.display()
        )
    })?;
    let preview = GhostCommit::new(
        workspace_version.to_string(),
        /*parent*/ None,
        Vec::new(),
        Vec::new(),
    );
    diff_ghost_commits(workspace_root, &preview, &current)
        .map(|diff| diff.is_empty())
        .with_context(|| format!("compare rollback workspace version {workspace_version}"))
}

pub(crate) fn current_git_workspace_version(workspace_root: &Path) -> Result<String> {
    create_ghost_commit_with_report(
        &CreateGhostCommitOptions::new(workspace_root)
            .message("devo rollback recovery checkpoint")
            .ignore_large_untracked_files(10 * 1024 * 1024),
    )
    .map(|(current, _)| current.id().to_string())
    .with_context(|| {
        format!(
            "capture rollback recovery version at {}",
            workspace_root.display()
        )
    })
}

pub(crate) fn restore_git_checkpoint(
    workspace_root: &Path,
    checkpoint: &TurnWorkspaceCheckpointRecordedRecord,
) -> Result<()> {
    if let Some(ghost) = ghost_commit_from_checkpoint(checkpoint) {
        restore_ghost_commit(workspace_root, &ghost)
            .with_context(|| format!("restore git checkpoint {}", checkpoint.checkpoint_id))
    } else {
        // Checkpoints written before P4d lack the original untracked manifest.
        // Restoring tracked files is safe; deleting untracked files by guess is not.
        restore_to_commit(workspace_root, &checkpoint.checkpoint_id).with_context(|| {
            format!(
                "restore legacy git checkpoint {} (tracked files only)",
                checkpoint.checkpoint_id
            )
        })
    }
}

pub(crate) fn capture_git_baseline(
    artifact_dir: &Path,
    session_id: SessionId,
    turn_id: TurnId,
    repo_root: &Path,
) -> Result<CapturedWorkspaceBaseline> {
    let (ghost, report) = create_ghost_commit_with_report(
        &CreateGhostCommitOptions::new(repo_root)
            .message("devo turn workspace baseline")
            .ignore_large_untracked_files(10 * 1024 * 1024),
    )
    .with_context(|| format!("create git ghost baseline at {}", repo_root.display()))?;
    let warnings = ghost_report_warnings(&report);
    let checkpoint_id = ghost.id().to_string();
    let artifact_ref = artifact_ref(session_id, turn_id, "checkpoint.json");
    let baseline = GitWorkspaceBaseline {
        session_id,
        turn_id,
        workspace_root: repo_root.to_path_buf(),
        checkpoint_id: checkpoint_id.clone(),
        ghost,
        warnings: warnings.clone(),
    };
    write_json(
        &artifact_dir.join("checkpoint.json"),
        &serde_json::json!({
            "schema_version": 1,
            "backend": "git_ghost_commit",
            "checkpoint_id": checkpoint_id,
            "workspace_root": repo_root,
            "warnings": warnings,
        }),
    )?;
    Ok(CapturedWorkspaceBaseline {
        record: checkpoint_record(CheckpointRecordInput {
            session_id,
            turn_id,
            checkpoint_id: &baseline.checkpoint_id,
            workspace_root: &baseline.workspace_root,
            backend: "git_ghost_commit",
            coverage: ChangeSetCoverage::Full,
            warnings: baseline.warnings.clone(),
            artifact_ref: Some(artifact_ref),
            preexisting_untracked_files: Some(
                baseline
                    .ghost
                    .preexisting_untracked_files()
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
            ),
            preexisting_untracked_dirs: Some(
                baseline
                    .ghost
                    .preexisting_untracked_dirs()
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
            ),
        }),
        baseline: ActiveWorkspaceBaseline::Git(baseline),
    })
}

pub(crate) fn diff_git_baseline(
    baseline: &GitWorkspaceBaseline,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
    change_set_status: WorkspaceChangeSetStatus,
) -> Result<WorkspaceChangeView> {
    let (current, report) = create_ghost_commit_with_report(
        &CreateGhostCommitOptions::new(baseline.workspace_root.as_path())
            .message("devo turn workspace current")
            .ignore_large_untracked_files(10 * 1024 * 1024),
    )?;
    let diff = diff_ghost_commits(baseline.workspace_root.as_path(), &baseline.ghost, &current)?;
    let mut warnings = baseline.warnings.clone();
    warnings.extend(ghost_report_warnings(&report));
    warnings.sort();
    warnings.dedup();
    Ok(view_from_diff(DiffViewInput {
        scope: WorkspaceChangeScope::Turn,
        workspace_root: baseline.workspace_root.clone(),
        base: Some(WorkspaceChangeBase::TurnCheckpoint {
            turn_id: baseline.turn_id,
            checkpoint_id: baseline.checkpoint_id.clone(),
            backend: WorkspaceCheckpointBackend::GitGhostCommit,
        }),
        attribution: WorkspaceChangeAttribution::WorkspaceNet,
        coverage: if warnings.is_empty() {
            WorkspaceChangeCoverage::GitVisible
        } else {
            WorkspaceChangeCoverage::Partial
        },
        change_set_status,
        diff,
        warnings,
        diff_detail,
        max_diff_bytes,
    }))
}

pub(crate) async fn branch_view(
    cwd: PathBuf,
    base_branch: Option<String>,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> WorkspaceChangeView {
    let Some(repo_root) = get_git_repo_root(&cwd) else {
        return unsupported_view(
            WorkspaceChangeScope::Branch,
            cwd,
            WorkspaceChangeAttribution::GitBranch,
            "not_git_repository",
        );
    };
    let base_branch = match base_branch {
        Some(branch) => branch,
        None => default_branch_name(repo_root.as_path())
            .await
            .unwrap_or_else(|| "main".to_string()),
    };
    let merge_base = match merge_base_with_head(repo_root.as_path(), &base_branch) {
        Ok(Some(merge_base)) => merge_base,
        Ok(None) => {
            return unsupported_view(
                WorkspaceChangeScope::Branch,
                repo_root,
                WorkspaceChangeAttribution::GitBranch,
                "base_branch_not_found_or_no_head",
            );
        }
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Branch,
                repo_root,
                WorkspaceChangeAttribution::GitBranch,
                error.to_string(),
            );
        }
    };
    let head = match git_stdout(&repo_root, &["rev-parse", "HEAD"]).await {
        Ok(head) => head,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Branch,
                repo_root,
                WorkspaceChangeAttribution::GitBranch,
                error,
            );
        }
    };
    let diff = match git_stdout(
        &repo_root,
        &[
            "diff",
            "--no-textconv",
            "--no-ext-diff",
            "--binary",
            &merge_base,
            "HEAD",
            "--",
        ],
    )
    .await
    {
        Ok(diff) => diff,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Branch,
                repo_root,
                WorkspaceChangeAttribution::GitBranch,
                error,
            );
        }
    };
    view_from_diff(DiffViewInput {
        scope: WorkspaceChangeScope::Branch,
        workspace_root: repo_root,
        base: Some(WorkspaceChangeBase::Branch {
            base_branch,
            merge_base,
            head,
        }),
        attribution: WorkspaceChangeAttribution::GitBranch,
        coverage: WorkspaceChangeCoverage::GitVisible,
        change_set_status: WorkspaceChangeSetStatus::Finalized,
        diff,
        warnings: Vec::new(),
        diff_detail,
        max_diff_bytes,
    })
}

pub(crate) async fn uncommitted_view(
    cwd: PathBuf,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> WorkspaceChangeView {
    let Some(repo_root) = get_git_repo_root(&cwd) else {
        return unsupported_view(
            WorkspaceChangeScope::Uncommitted,
            cwd,
            WorkspaceChangeAttribution::GitWorkingTree,
            "not_git_repository",
        );
    };
    let head = git_stdout(&repo_root, &["rev-parse", "--verify", "HEAD"])
        .await
        .ok();
    let Some(head_ref) = head.clone() else {
        return unsupported_view(
            WorkspaceChangeScope::Uncommitted,
            repo_root,
            WorkspaceChangeAttribution::GitWorkingTree,
            "no_head",
        );
    };
    let mut diff = match git_stdout(
        &repo_root,
        &[
            "diff",
            "--no-textconv",
            "--no-ext-diff",
            "--binary",
            "HEAD",
            "--",
        ],
    )
    .await
    {
        Ok(diff) => diff,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Uncommitted,
                repo_root,
                WorkspaceChangeAttribution::GitWorkingTree,
                error,
            );
        }
    };
    match git_stdout(&repo_root, &["ls-files", "--others", "--exclude-standard"]).await {
        Ok(paths) => {
            for path in paths.lines().filter(|line| !line.trim().is_empty()) {
                if let Ok(extra) = git_stdout_allow_diff_exit(
                    &repo_root,
                    &[
                        "diff",
                        "--no-textconv",
                        "--no-ext-diff",
                        "--binary",
                        "--no-index",
                        "--",
                        null_device(),
                        path,
                    ],
                )
                .await
                {
                    diff.push_str(&extra);
                }
            }
        }
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Uncommitted,
                repo_root,
                WorkspaceChangeAttribution::GitWorkingTree,
                error,
            );
        }
    }
    view_from_diff(DiffViewInput {
        scope: WorkspaceChangeScope::Uncommitted,
        workspace_root: repo_root,
        base: Some(WorkspaceChangeBase::Head {
            head: Some(head_ref),
        }),
        attribution: WorkspaceChangeAttribution::GitWorkingTree,
        coverage: WorkspaceChangeCoverage::GitVisible,
        change_set_status: WorkspaceChangeSetStatus::Accumulating,
        diff,
        warnings: Vec::new(),
        diff_detail,
        max_diff_bytes,
    })
}

async fn git_stdout(cwd: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = git_output(cwd, args).await?;
    if output.status.success() {
        String::from_utf8(output.stdout)
            .map(|value| value.trim().to_string())
            .map_err(|error| error.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

async fn git_stdout_allow_diff_exit(
    cwd: &Path,
    args: &[&str],
) -> std::result::Result<String, String> {
    let output = git_output(cwd, args).await?;
    if output.status.success() || output.status.code() == Some(1) {
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

async fn git_output(cwd: &Path, args: &[&str]) -> std::result::Result<Output, String> {
    Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| error.to_string())
}

fn ghost_report_warnings(report: &GhostSnapshotReport) -> Vec<String> {
    let mut warnings = Vec::new();
    for file in &report.ignored_untracked_files {
        warnings.push(format!(
            "large_untracked_file_excluded: {} ({} bytes)",
            file.path.display(),
            file.byte_size
        ));
    }
    for dir in &report.large_untracked_dirs {
        warnings.push(format!(
            "large_untracked_dir_excluded: {} ({} files)",
            dir.path.display(),
            dir.file_count
        ));
    }
    warnings
}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}
