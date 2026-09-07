//! Fast file-list path for `WorkspaceDiffDetail::Summary`.
//!
//! Cursor/Codex-style Changes UIs show the file list first. A full
//! `git diff --binary` (plus untracked `--no-index` spawns) is far too slow for
//! that. Summary uses `name-status` + `numstat` (+ `ls-files` for untracked)
//! and never materializes a unified patch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use devo_protocol::{
    WorkspaceChangeAttribution, WorkspaceChangeBase, WorkspaceChangeCoverage, WorkspaceChangeScope,
    WorkspaceChangeSetStatus, WorkspaceChangeStats, WorkspaceChangeView, WorkspaceChangeViewStatus,
    WorkspaceChangedFile, WorkspaceChangedFileStatus, WorkspaceDiffDetail,
};
use tokio::process::Command;

pub(super) struct GitListInput {
    pub scope: WorkspaceChangeScope,
    pub workspace_root: PathBuf,
    pub base: Option<WorkspaceChangeBase>,
    pub attribution: WorkspaceChangeAttribution,
    pub coverage: WorkspaceChangeCoverage,
    pub change_set_status: WorkspaceChangeSetStatus,
    /// Args after the common `git diff` flags, e.g. `["HEAD", "--"]` or
    /// `["--cached", "HEAD", "--"]` or `[merge_base, "HEAD", "--"]`.
    pub range_args: Vec<String>,
    pub ignore_whitespace: bool,
    pub include_untracked: bool,
}

pub(super) async fn view_from_git_list(input: GitListInput) -> WorkspaceChangeView {
    let mut name_args = vec![
        "diff".to_string(),
        "--name-status".to_string(),
        "--no-textconv".to_string(),
        "--no-ext-diff".to_string(),
    ];
    let mut num_args = vec![
        "diff".to_string(),
        "--numstat".to_string(),
        "--no-textconv".to_string(),
        "--no-ext-diff".to_string(),
    ];
    if input.ignore_whitespace {
        name_args.push("--ignore-all-space".to_string());
        num_args.push("--ignore-all-space".to_string());
    }
    name_args.extend(input.range_args.iter().cloned());
    num_args.extend(input.range_args.iter().cloned());

    let (name_status, numstat) = tokio::join!(
        git_stdout(&input.workspace_root, &name_args),
        git_stdout(&input.workspace_root, &num_args),
    );
    let name_status = match name_status {
        Ok(value) => value,
        Err(error) => {
            return super::diff::error_view(
                input.scope,
                input.workspace_root,
                input.attribution,
                error,
            );
        }
    };
    let numstat = match numstat {
        Ok(value) => value,
        Err(error) => {
            return super::diff::error_view(
                input.scope,
                input.workspace_root,
                input.attribution,
                error,
            );
        }
    };

    let mut files = files_from_name_status_and_numstat(&name_status, &numstat);
    let mut warnings = Vec::new();
    if input.include_untracked {
        match list_untracked(&input.workspace_root).await {
            Ok(untracked) => {
                const MAX_UNTRACKED: usize = 500;
                let total = untracked.len();
                for path in untracked.into_iter().take(MAX_UNTRACKED) {
                    if files.iter().any(|file| file.path == Path::new(&path)) {
                        continue;
                    }
                    let (additions, deletions, binary) =
                        untracked_line_stats(&input.workspace_root, &path).await;
                    files.push(WorkspaceChangedFile {
                        path: PathBuf::from(path),
                        status: WorkspaceChangedFileStatus::Untracked,
                        additions,
                        deletions,
                        binary,
                        diff_truncated: false,
                        old_text: None,
                        new_text: None,
                    });
                }
                if total > MAX_UNTRACKED {
                    warnings.push(format!(
                        "untracked_files_truncated: showing {MAX_UNTRACKED} of {total}"
                    ));
                }
            }
            Err(error) => {
                return super::diff::error_view(
                    input.scope,
                    input.workspace_root,
                    input.attribution,
                    error,
                );
            }
        }
    }

    let mut stats = WorkspaceChangeStats::default();
    for file in &files {
        stats.files_changed += 1;
        stats.additions += file.additions.unwrap_or_default();
        stats.deletions += file.deletions.unwrap_or_default();
    }

    let status = if files.is_empty() {
        WorkspaceChangeViewStatus::Empty
    } else if !warnings.is_empty() {
        WorkspaceChangeViewStatus::Partial
    } else {
        WorkspaceChangeViewStatus::Ready
    };

    // Summary never ships unified_diff; apply_diff_detail is a no-op for that.
    let _ = WorkspaceDiffDetail::Summary;
    WorkspaceChangeView {
        scope: input.scope,
        status,
        workspace_root: input.workspace_root,
        base: input.base,
        coverage: input.coverage,
        attribution: input.attribution,
        change_set_status: input.change_set_status,
        files,
        stats,
        unified_diff: None,
        warnings,
        generated_at: chrono::Utc::now(),
    }
}

pub(super) fn files_from_name_status_and_numstat(
    name_status: &str,
    numstat: &str,
) -> Vec<WorkspaceChangedFile> {
    let mut stats_by_path: BTreeMap<PathBuf, (Option<u64>, Option<u64>, bool)> = BTreeMap::new();
    for line in numstat.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let additions_raw = parts.next().unwrap_or("-");
        let deletions_raw = parts.next().unwrap_or("-");
        let path_raw = parts.next().unwrap_or("");
        if path_raw.is_empty() {
            continue;
        }
        // Renames in numstat: `add\tdel\told\tnew` — take the new path.
        let path = PathBuf::from(parts.next().unwrap_or(path_raw));
        let binary = additions_raw == "-" || deletions_raw == "-";
        let additions = if binary {
            None
        } else {
            additions_raw.parse().ok()
        };
        let deletions = if binary {
            None
        } else {
            deletions_raw.parse().ok()
        };
        stats_by_path.insert(path, (additions, deletions, binary));
    }

    let mut files = Vec::new();
    for line in name_status.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let status_raw = parts.next().unwrap_or("");
        let first_path = parts.next().unwrap_or("");
        if status_raw.is_empty() || first_path.is_empty() {
            continue;
        }
        let (path, status) = match status_raw.chars().next() {
            Some('A') => (PathBuf::from(first_path), WorkspaceChangedFileStatus::Added),
            Some('D') => (
                PathBuf::from(first_path),
                WorkspaceChangedFileStatus::Deleted,
            ),
            Some('M') => (
                PathBuf::from(first_path),
                WorkspaceChangedFileStatus::Modified,
            ),
            Some('T') => (
                PathBuf::from(first_path),
                WorkspaceChangedFileStatus::TypeChanged,
            ),
            Some('R') | Some('C') => {
                let new_path = parts.next().unwrap_or(first_path);
                (PathBuf::from(new_path), WorkspaceChangedFileStatus::Renamed)
            }
            Some('U') => (
                PathBuf::from(first_path),
                WorkspaceChangedFileStatus::Unknown,
            ),
            _ => (
                PathBuf::from(first_path),
                WorkspaceChangedFileStatus::Unknown,
            ),
        };
        let (additions, deletions, binary) =
            stats_by_path.remove(&path).unwrap_or((None, None, false));
        files.push(WorkspaceChangedFile {
            path,
            status,
            additions,
            deletions,
            binary,
            diff_truncated: false,
            old_text: None,
            new_text: None,
        });
    }
    files
}

async fn list_untracked(repo_root: &Path) -> Result<Vec<String>, String> {
    let raw = git_stdout(
        repo_root,
        &[
            "ls-files".to_string(),
            "--others".to_string(),
            "--exclude-standard".to_string(),
        ],
    )
    .await?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Line counts for an untracked path so Summary rows can show +/- without Full.
///
/// Caps read size so huge untracked blobs (e.g. accidental binaries) do not
/// stall the Changes open path. Oversized / binary files stay without counts.
async fn untracked_line_stats(repo_root: &Path, rel: &str) -> (Option<u64>, Option<u64>, bool) {
    const MAX_BYTES: u64 = 512 * 1024;
    let abs = repo_root.join(rel);
    let meta = match tokio::fs::metadata(&abs).await {
        Ok(meta) if meta.is_file() => meta,
        _ => return (None, None, false),
    };
    if meta.len() > MAX_BYTES {
        return (None, None, false);
    }
    let bytes = match tokio::fs::read(&abs).await {
        Ok(bytes) => bytes,
        Err(_) => return (None, None, false),
    };
    if bytes.contains(&0) {
        return (None, None, true);
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return (None, None, true);
    };
    let additions = if text.is_empty() {
        0
    } else {
        u64::try_from(text.lines().count()).unwrap_or(u64::MAX)
    };
    (Some(additions), Some(0), false)
}

async fn git_stdout(cwd: &Path, args: &[String]) -> Result<String, String> {
    let output = git_output(cwd, args).await?;
    // name-status/numstat exit 1 when differences exist.
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
        // CRLF advice on dirty Windows trees floods stderr and adds latency.
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| error.to_string())
}
