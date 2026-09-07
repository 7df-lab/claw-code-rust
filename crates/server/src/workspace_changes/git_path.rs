//! Path-scoped Full diffs for expand-on-demand.
//!
//! Whole-tree Full over stdio is ~0.5s and ~400KiB on a dirty `devo` tree; expanding
//! one row should only pay for that path. Callers pass `paths` on
//! `workspace/changes/read` with `diffDetail=full`.

use std::path::{Path, PathBuf};
use std::process::Output;

use devo_protocol::{
    WorkspaceChangeAttribution, WorkspaceChangeBase, WorkspaceChangeCoverage, WorkspaceChangeScope,
    WorkspaceChangeSetStatus, WorkspaceChangeView, WorkspaceDiffDetail,
};
use devo_util_git::{default_branch_name, get_git_repo_root, merge_base_with_head};
use tokio::process::Command;

use crate::workspace_changes::diff::{DiffViewInput, error_view, unsupported_view, view_from_diff};
use crate::workspace_changes::view_cache;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn path_scoped_full_view(
    cwd: PathBuf,
    scope: WorkspaceChangeScope,
    paths: Vec<PathBuf>,
    base_branch: Option<String>,
    ignore_whitespace: bool,
    max_diff_bytes: Option<u64>,
    turn_checkpoint_id: Option<String>,
    include_file_sides: bool,
) -> WorkspaceChangeView {
    let normalized: Vec<String> = paths
        .into_iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .collect();
    if normalized.is_empty() {
        return unsupported_view(scope, cwd, attribution_for(scope), "paths_empty");
    }

    match scope {
        WorkspaceChangeScope::Turn => {
            let Some(checkpoint) = turn_checkpoint_id.filter(|id| !id.trim().is_empty()) else {
                return unsupported_view(
                    scope,
                    cwd,
                    WorkspaceChangeAttribution::WorkspaceNet,
                    "turn_checkpoint_unavailable",
                );
            };
            let Some(repo_root) = get_git_repo_root(&cwd) else {
                return unsupported_view(
                    scope,
                    cwd,
                    WorkspaceChangeAttribution::WorkspaceNet,
                    "not_git_repository",
                );
            };
            let mut args = vec![
                "diff".to_string(),
                "--no-textconv".to_string(),
                "--no-ext-diff".to_string(),
            ];
            if ignore_whitespace {
                args.push("--ignore-all-space".to_string());
            }
            args.push(checkpoint.clone());
            args.push("--".to_string());
            args.extend(normalized.iter().cloned());
            let view = finish_diff(
                scope,
                repo_root,
                None,
                WorkspaceChangeAttribution::WorkspaceNet,
                WorkspaceChangeCoverage::GitVisible,
                WorkspaceChangeSetStatus::Accumulating,
                args,
                &normalized,
                /*include_untracked*/ true,
                ignore_whitespace,
                max_diff_bytes,
            )
            .await;
            return maybe_attach_sides(view, include_file_sides, Some(checkpoint.as_str())).await;
        }
        WorkspaceChangeScope::Branch
        | WorkspaceChangeScope::Staged
        | WorkspaceChangeScope::Unstaged
        | WorkspaceChangeScope::Uncommitted => {}
    }

    let Some(repo_root) = get_git_repo_root(&cwd) else {
        return unsupported_view(scope, cwd, attribution_for(scope), "not_git_repository");
    };
    let head = match git_stdout(&repo_root, &["rev-parse", "--verify", "HEAD"]).await {
        Ok(head) => head,
        Err(_) => {
            return unsupported_view(scope, repo_root, attribution_for(scope), "no_head");
        }
    };

    let (base, mut args, include_untracked) = match scope {
        WorkspaceChangeScope::Uncommitted => (
            Some(WorkspaceChangeBase::Head {
                head: Some(head.clone()),
            }),
            diff_prefix(ignore_whitespace, &["HEAD"]),
            true,
        ),
        WorkspaceChangeScope::Staged => (
            Some(WorkspaceChangeBase::Head {
                head: Some(head.clone()),
            }),
            diff_prefix(ignore_whitespace, &["--cached", "HEAD"]),
            false,
        ),
        WorkspaceChangeScope::Unstaged => (
            Some(WorkspaceChangeBase::Head {
                head: Some(head.clone()),
            }),
            diff_prefix(ignore_whitespace, &[]),
            true,
        ),
        WorkspaceChangeScope::Branch => {
            let branch = match base_branch {
                Some(branch) => branch,
                None => default_branch_name(repo_root.as_path())
                    .await
                    .unwrap_or_else(|| "main".to_string()),
            };
            let Some(merge_base) =
                view_cache::cached_merge_base(&repo_root, &branch, &head, || {
                    merge_base_with_head(repo_root.as_path(), &branch).unwrap_or_default()
                })
            else {
                return unsupported_view(
                    scope,
                    repo_root,
                    WorkspaceChangeAttribution::GitBranch,
                    "base_branch_not_found_or_no_head",
                );
            };
            (
                Some(WorkspaceChangeBase::Branch {
                    base_branch: branch,
                    merge_base: merge_base.clone(),
                    head: head.clone(),
                }),
                diff_prefix(ignore_whitespace, &[merge_base.as_str(), "HEAD"]),
                false,
            )
        }
        WorkspaceChangeScope::Turn => unreachable!(),
    };

    args.push("--".to_string());
    args.extend(normalized.iter().cloned());

    let view = finish_diff(
        scope,
        repo_root,
        base,
        attribution_for(scope),
        WorkspaceChangeCoverage::GitVisible,
        if matches!(scope, WorkspaceChangeScope::Branch) {
            WorkspaceChangeSetStatus::Finalized
        } else {
            WorkspaceChangeSetStatus::Accumulating
        },
        args,
        &normalized,
        include_untracked,
        ignore_whitespace,
        max_diff_bytes,
    )
    .await;
    maybe_attach_sides(view, include_file_sides, None).await
}

async fn maybe_attach_sides(
    view: WorkspaceChangeView,
    include_file_sides: bool,
    turn_checkpoint: Option<&str>,
) -> WorkspaceChangeView {
    if !include_file_sides {
        return view;
    }
    crate::workspace_changes::git_sides::attach_file_sides(view, turn_checkpoint).await
}

fn attribution_for(scope: WorkspaceChangeScope) -> WorkspaceChangeAttribution {
    match scope {
        WorkspaceChangeScope::Branch => WorkspaceChangeAttribution::GitBranch,
        WorkspaceChangeScope::Turn => WorkspaceChangeAttribution::WorkspaceNet,
        WorkspaceChangeScope::Staged
        | WorkspaceChangeScope::Unstaged
        | WorkspaceChangeScope::Uncommitted => WorkspaceChangeAttribution::GitWorkingTree,
    }
}

fn diff_prefix(ignore_whitespace: bool, range: &[&str]) -> Vec<String> {
    let mut args = vec![
        "diff".to_string(),
        "--no-textconv".to_string(),
        "--no-ext-diff".to_string(),
    ];
    if ignore_whitespace {
        args.push("--ignore-all-space".to_string());
    }
    for part in range {
        args.push((*part).to_string());
    }
    args
}

#[allow(clippy::too_many_arguments)]
async fn finish_diff(
    scope: WorkspaceChangeScope,
    repo_root: PathBuf,
    base: Option<WorkspaceChangeBase>,
    attribution: WorkspaceChangeAttribution,
    coverage: WorkspaceChangeCoverage,
    change_set_status: WorkspaceChangeSetStatus,
    args: Vec<String>,
    normalized_paths: &[String],
    include_untracked: bool,
    _ignore_whitespace: bool,
    max_diff_bytes: Option<u64>,
) -> WorkspaceChangeView {
    let mut diff = match git_stdout_owned(&repo_root, &args).await {
        Ok(diff) => diff,
        Err(error) => return error_view(scope, repo_root, attribution, error),
    };

    if include_untracked {
        for path in normalized_paths {
            if patch_mentions_path(&diff, path) {
                continue;
            }
            match append_untracked_content_patch(&repo_root, path).await {
                Ok(Some(stub)) => {
                    if !diff.is_empty() && !diff.ends_with('\n') {
                        diff.push('\n');
                    }
                    diff.push_str(&stub);
                }
                Ok(None) => {}
                Err(error) => return error_view(scope, repo_root, attribution, error),
            }
        }
    }

    view_from_diff(DiffViewInput {
        scope,
        workspace_root: repo_root,
        base,
        attribution,
        coverage,
        change_set_status,
        diff,
        warnings: Vec::new(),
        diff_detail: WorkspaceDiffDetail::Full,
        max_diff_bytes,
    })
}

fn patch_mentions_path(diff: &str, path: &str) -> bool {
    diff.contains(&format!("b/{path}\n"))
        || diff.contains(&format!("b/{path}\r\n"))
        || diff.ends_with(&format!("b/{path}"))
}

/// Real content for a single untracked path (not header-only stub).
async fn append_untracked_content_patch(
    repo_root: &Path,
    path: &str,
) -> Result<Option<String>, String> {
    let listed = git_stdout(
        repo_root,
        &["ls-files", "--others", "--exclude-standard", "--", path],
    )
    .await?;
    if listed.trim().is_empty() {
        return Ok(None);
    }
    let abs = repo_root.join(path);
    let content = match tokio::fs::read(&abs).await {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                return Ok(Some(format!(
                    "diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n"
                )));
            }
        },
        Err(_) => {
            return Ok(Some(format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n"
            )));
        }
    };
    const MAX: usize = 400_000;
    let truncated = content.len() > MAX;
    let slice = if truncated { &content[..MAX] } else { &content };
    let lines: Vec<&str> = slice.split('\n').collect();
    let count = lines.len();
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    out.push_str("new file mode 100644\n");
    out.push_str("--- /dev/null\n");
    out.push_str(&format!("+++ b/{path}\n"));
    out.push_str(&format!("@@ -0,0 +1,{count} @@\n"));
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if truncated {
        out.push_str("+/* truncated */\n");
    }
    Ok(Some(out))
}

async fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    git_stdout_owned(cwd, &owned).await
}

async fn git_stdout_owned(cwd: &Path, args: &[String]) -> Result<String, String> {
    let output = git_output(cwd, args).await?;
    if output.status.success() || output.status.code() == Some(1) {
        String::from_utf8(output.stdout).map_err(|error| error.to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

async fn git_output(cwd: &Path, args: &[String]) -> Result<Output, String> {
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
