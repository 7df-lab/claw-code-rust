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
use crate::workspace_changes::git_list::{self, GitListInput};
use crate::workspace_changes::view_cache::{self, ViewCacheKey};

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
    // Never create a ghost commit on read. Ghosts are captured when a turn
    // starts; Changes panel reads must stay cheap (name-status / plain diff).
    if !matches!(diff_detail, WorkspaceDiffDetail::Full) {
        return Ok(summary_git_baseline(baseline, change_set_status));
    }
    let checkpoint = baseline.checkpoint_id.as_str();
    let mut diff = match sync_git_stdout(
        &baseline.workspace_root,
        &["diff", "--no-textconv", "--no-ext-diff", checkpoint, "--"],
    ) {
        Ok(value) => value,
        Err(error) => {
            return Err(anyhow::anyhow!(
                "diff turn checkpoint {checkpoint} at {}: {error}",
                baseline.workspace_root.display()
            ));
        }
    };
    let _ = append_untracked_stubs_sync(baseline, &mut diff);
    let mut warnings = baseline.warnings.clone();
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

/// List-only turn view: `git diff --name-status <checkpoint>` + untracked paths.
/// Avoids `create_ghost_commit` so Changes can paint while a turn is active.
fn summary_git_baseline(
    baseline: &GitWorkspaceBaseline,
    change_set_status: WorkspaceChangeSetStatus,
) -> WorkspaceChangeView {
    let checkpoint = baseline.checkpoint_id.as_str();
    let name_status = match sync_git_stdout(
        &baseline.workspace_root,
        &[
            "diff",
            "--name-status",
            "--no-textconv",
            "--no-ext-diff",
            checkpoint,
            "--",
        ],
    ) {
        Ok(value) => value,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Turn,
                baseline.workspace_root.clone(),
                WorkspaceChangeAttribution::WorkspaceNet,
                error,
            );
        }
    };
    let numstat = match sync_git_stdout(
        &baseline.workspace_root,
        &[
            "diff",
            "--numstat",
            "--no-textconv",
            "--no-ext-diff",
            checkpoint,
            "--",
        ],
    ) {
        Ok(value) => value,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Turn,
                baseline.workspace_root.clone(),
                WorkspaceChangeAttribution::WorkspaceNet,
                error,
            );
        }
    };
    let mut files = git_list::files_from_name_status_and_numstat(&name_status, &numstat);
    let preexisting: std::collections::HashSet<PathBuf> = baseline
        .ghost
        .preexisting_untracked_files()
        .iter()
        .cloned()
        .collect();
    if let Ok(untracked) = sync_git_stdout(
        &baseline.workspace_root,
        &["ls-files", "--others", "--exclude-standard"],
    ) {
        for path in untracked
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let path_buf = PathBuf::from(path);
            if preexisting.contains(&path_buf) {
                continue;
            }
            if files.iter().any(|file| file.path == path_buf) {
                continue;
            }
            files.push(devo_protocol::WorkspaceChangedFile {
                path: path_buf,
                status: devo_protocol::WorkspaceChangedFileStatus::Untracked,
                additions: None,
                deletions: None,
                binary: false,
                diff_truncated: false,
                old_text: None,
                new_text: None,
            });
        }
    }
    let mut stats = devo_protocol::WorkspaceChangeStats::default();
    for file in &files {
        stats.files_changed += 1;
        stats.additions += file.additions.unwrap_or_default();
        stats.deletions += file.deletions.unwrap_or_default();
    }
    let status = if files.is_empty() {
        devo_protocol::WorkspaceChangeViewStatus::Empty
    } else {
        devo_protocol::WorkspaceChangeViewStatus::Ready
    };
    let mut warnings = baseline.warnings.clone();
    warnings.sort();
    warnings.dedup();
    WorkspaceChangeView {
        scope: WorkspaceChangeScope::Turn,
        status,
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
        files,
        stats,
        unified_diff: None,
        warnings,
        generated_at: chrono::Utc::now(),
    }
}

fn sync_git_stdout(cwd: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(cwd)
        // Never inherit the server's JSON-RPC stdin pipe — git would block the
        // stdio transport read loop (and itself) on the same fd.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() || output.status.code() == Some(1) {
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Header-only stubs for turn-scoped untracked paths (excludes preexisting).
fn append_untracked_stubs_sync(
    baseline: &GitWorkspaceBaseline,
    diff: &mut String,
) -> std::result::Result<usize, String> {
    let paths_raw = sync_git_stdout(
        &baseline.workspace_root,
        &["ls-files", "--others", "--exclude-standard"],
    )?;
    let preexisting: std::collections::HashSet<PathBuf> = baseline
        .ghost
        .preexisting_untracked_files()
        .iter()
        .cloned()
        .collect();
    let paths: Vec<String> = paths_raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|path| !preexisting.contains(&PathBuf::from(path)))
        .map(str::to_string)
        .collect();
    if paths.is_empty() {
        return Ok(0);
    }
    for path in &paths {
        if !diff.is_empty() && !diff.ends_with('\n') {
            diff.push('\n');
        }
        diff.push_str(&format!(
            "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"
        ));
    }
    Ok(paths.len())
}

pub(crate) async fn branch_view(
    cwd: PathBuf,
    base_branch: Option<String>,
    ignore_whitespace: bool,
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
    let Some(merge_base) = view_cache::cached_merge_base(&repo_root, &base_branch, &head, || {
        merge_base_with_head(repo_root.as_path(), &base_branch).unwrap_or_default()
    }) else {
        return unsupported_view(
            WorkspaceChangeScope::Branch,
            repo_root,
            WorkspaceChangeAttribution::GitBranch,
            "base_branch_not_found_or_no_head",
        );
    };
    let cache_key = ViewCacheKey {
        repo_root: repo_root.clone(),
        scope: WorkspaceChangeScope::Branch,
        base_branch: Some(base_branch.clone()),
        ignore_whitespace,
        fingerprint: view_cache::branch_fingerprint(&merge_base, &head),
    };
    if let Some(cached) = view_cache::get(&cache_key, diff_detail, max_diff_bytes) {
        return cached;
    }
    if !matches!(diff_detail, WorkspaceDiffDetail::Full) {
        let view = git_list::view_from_git_list(GitListInput {
            scope: WorkspaceChangeScope::Branch,
            workspace_root: repo_root,
            base: Some(WorkspaceChangeBase::Branch {
                base_branch: base_branch.clone(),
                merge_base: merge_base.clone(),
                head: head.clone(),
            }),
            attribution: WorkspaceChangeAttribution::GitBranch,
            coverage: WorkspaceChangeCoverage::GitVisible,
            change_set_status: WorkspaceChangeSetStatus::Finalized,
            range_args: vec![merge_base, "HEAD".to_string(), "--".to_string()],
            ignore_whitespace,
            include_untracked: false,
        })
        .await;
        view_cache::put(cache_key, view.clone());
        return view;
    }
    // Full patches: skip `--binary` so we don't ship multi-MB binary blobs.
    let mut diff_args = vec!["diff", "--no-textconv", "--no-ext-diff"];
    if ignore_whitespace {
        diff_args.push("--ignore-all-space");
    }
    diff_args.extend_from_slice(&[merge_base.as_str(), "HEAD", "--"]);
    let diff = match git_stdout(&repo_root, &diff_args).await {
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
    let view = view_from_diff(DiffViewInput {
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
    });
    view_cache::put(cache_key, view.clone());
    view
}

pub(crate) async fn uncommitted_view(
    cwd: PathBuf,
    ignore_whitespace: bool,
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
    let cache_key = view_cache::porcelain_fingerprint(&repo_root)
        .await
        .map(|fingerprint| ViewCacheKey {
            repo_root: repo_root.clone(),
            scope: WorkspaceChangeScope::Uncommitted,
            base_branch: None,
            ignore_whitespace,
            fingerprint,
        });
    if let Some(key) = cache_key.as_ref()
        && let Some(cached) = view_cache::get(key, diff_detail, max_diff_bytes)
    {
        return cached;
    }
    if !matches!(diff_detail, WorkspaceDiffDetail::Full) {
        let view = git_list::view_from_git_list(GitListInput {
            scope: WorkspaceChangeScope::Uncommitted,
            workspace_root: repo_root,
            base: Some(WorkspaceChangeBase::Head {
                head: Some(head_ref),
            }),
            attribution: WorkspaceChangeAttribution::GitWorkingTree,
            coverage: WorkspaceChangeCoverage::GitVisible,
            change_set_status: WorkspaceChangeSetStatus::Accumulating,
            range_args: vec!["HEAD".to_string(), "--".to_string()],
            ignore_whitespace,
            include_untracked: true,
        })
        .await;
        if let Some(key) = cache_key {
            view_cache::put(key, view.clone());
        }
        return view;
    }
    let mut diff_args = vec!["diff", "--no-textconv", "--no-ext-diff"];
    if ignore_whitespace {
        diff_args.push("--ignore-all-space");
    }
    diff_args.extend_from_slice(&["HEAD", "--"]);
    let diff = match git_stdout(&repo_root, &diff_args).await {
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
    // Intentionally omit header-only untracked stubs from Full unified_diff.
    // Summary already lists untracked files; expand loads content path-scoped.
    let view = view_from_diff(DiffViewInput {
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
    });
    if let Some(key) = cache_key {
        let still_valid = view_cache::porcelain_fingerprint(&key.repo_root)
            .await
            .as_ref()
            == Some(&key.fingerprint);
        if still_valid {
            view_cache::put(key, view.clone());
        }
    }
    view
}

/// Staged changes: index vs HEAD. Untracked files are not in the index
/// until `git add`, so they never appear in this scope.
pub(crate) async fn staged_view(
    cwd: PathBuf,
    ignore_whitespace: bool,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> WorkspaceChangeView {
    let Some(repo_root) = get_git_repo_root(&cwd) else {
        return unsupported_view(
            WorkspaceChangeScope::Staged,
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
            WorkspaceChangeScope::Staged,
            repo_root,
            WorkspaceChangeAttribution::GitWorkingTree,
            "no_head",
        );
    };
    let cache_key = view_cache::porcelain_fingerprint(&repo_root)
        .await
        .map(|fingerprint| ViewCacheKey {
            repo_root: repo_root.clone(),
            scope: WorkspaceChangeScope::Staged,
            base_branch: None,
            ignore_whitespace,
            fingerprint,
        });
    if let Some(key) = cache_key.as_ref()
        && let Some(cached) = view_cache::get(key, diff_detail, max_diff_bytes)
    {
        return cached;
    }
    if !matches!(diff_detail, WorkspaceDiffDetail::Full) {
        let view = git_list::view_from_git_list(GitListInput {
            scope: WorkspaceChangeScope::Staged,
            workspace_root: repo_root,
            base: Some(WorkspaceChangeBase::Head {
                head: Some(head_ref),
            }),
            attribution: WorkspaceChangeAttribution::GitWorkingTree,
            coverage: WorkspaceChangeCoverage::GitVisible,
            change_set_status: WorkspaceChangeSetStatus::Accumulating,
            range_args: vec!["--cached".to_string(), "HEAD".to_string(), "--".to_string()],
            ignore_whitespace,
            include_untracked: false,
        })
        .await;
        if let Some(key) = cache_key {
            view_cache::put(key, view.clone());
        }
        return view;
    }
    let mut diff_args = vec!["diff", "--no-textconv", "--no-ext-diff"];
    if ignore_whitespace {
        diff_args.push("--ignore-all-space");
    }
    diff_args.extend_from_slice(&["--cached", "HEAD", "--"]);
    let diff = match git_stdout(&repo_root, &diff_args).await {
        Ok(diff) => diff,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Staged,
                repo_root,
                WorkspaceChangeAttribution::GitWorkingTree,
                error,
            );
        }
    };
    let view = view_from_diff(DiffViewInput {
        scope: WorkspaceChangeScope::Staged,
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
    });
    if let Some(key) = cache_key {
        let still_valid = view_cache::porcelain_fingerprint(&key.repo_root)
            .await
            .as_ref()
            == Some(&key.fingerprint);
        if still_valid {
            view_cache::put(key, view.clone());
        }
    }
    view
}

/// Unstaged changes: worktree vs index. Untracked files are listed by Summary;
/// Full omits header-only stubs so expand can load real content path-scoped.
pub(crate) async fn unstaged_view(
    cwd: PathBuf,
    ignore_whitespace: bool,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> WorkspaceChangeView {
    let Some(repo_root) = get_git_repo_root(&cwd) else {
        return unsupported_view(
            WorkspaceChangeScope::Unstaged,
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
            WorkspaceChangeScope::Unstaged,
            repo_root,
            WorkspaceChangeAttribution::GitWorkingTree,
            "no_head",
        );
    };
    let cache_key = view_cache::porcelain_fingerprint(&repo_root)
        .await
        .map(|fingerprint| ViewCacheKey {
            repo_root: repo_root.clone(),
            scope: WorkspaceChangeScope::Unstaged,
            base_branch: None,
            ignore_whitespace,
            fingerprint,
        });
    if let Some(key) = cache_key.as_ref()
        && let Some(cached) = view_cache::get(key, diff_detail, max_diff_bytes)
    {
        return cached;
    }
    if !matches!(diff_detail, WorkspaceDiffDetail::Full) {
        let view = git_list::view_from_git_list(GitListInput {
            scope: WorkspaceChangeScope::Unstaged,
            workspace_root: repo_root,
            base: Some(WorkspaceChangeBase::Head {
                head: Some(head_ref),
            }),
            attribution: WorkspaceChangeAttribution::GitWorkingTree,
            coverage: WorkspaceChangeCoverage::GitVisible,
            change_set_status: WorkspaceChangeSetStatus::Accumulating,
            range_args: vec!["--".to_string()],
            ignore_whitespace,
            include_untracked: true,
        })
        .await;
        if let Some(key) = cache_key {
            view_cache::put(key, view.clone());
        }
        return view;
    }
    let mut diff_args = vec!["diff", "--no-textconv", "--no-ext-diff"];
    if ignore_whitespace {
        diff_args.push("--ignore-all-space");
    }
    diff_args.push("--");
    let diff = match git_stdout(&repo_root, &diff_args).await {
        Ok(diff) => diff,
        Err(error) => {
            return error_view(
                WorkspaceChangeScope::Unstaged,
                repo_root,
                WorkspaceChangeAttribution::GitWorkingTree,
                error,
            );
        }
    };
    // Same as uncommitted Full: no header-only untracked stubs in unified_diff.
    let view = view_from_diff(DiffViewInput {
        scope: WorkspaceChangeScope::Unstaged,
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
    });
    if let Some(key) = cache_key {
        let still_valid = view_cache::porcelain_fingerprint(&key.repo_root)
            .await
            .as_ref()
            == Some(&key.fingerprint);
        if still_valid {
            view_cache::put(key, view.clone());
        }
    }
    view
}

async fn git_stdout(cwd: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = git_output(cwd, args).await?;
    // `git diff` / `diff-index` / `diff-files` exit 1 when differences exist.
    if output.status.success() || output.status.code() == Some(1) {
        String::from_utf8(output.stdout)
            .map(|value| value.trim_end().to_string())
            .map_err(|error| error.to_string())
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
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
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
