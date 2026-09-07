use std::path::PathBuf;

use devo_core::ChangeSetCoverage;
use devo_protocol::{
    WorkspaceChangeAttribution, WorkspaceChangeCoverage, WorkspaceChangeScope,
    WorkspaceChangeSetStatus, WorkspaceChangeStats, WorkspaceChangeView, WorkspaceChangeViewStatus,
    WorkspaceChangedFile, WorkspaceChangedFileStatus, WorkspaceDiffDetail,
};

pub(super) struct DiffViewInput {
    pub scope: WorkspaceChangeScope,
    pub workspace_root: PathBuf,
    pub base: Option<devo_protocol::WorkspaceChangeBase>,
    pub attribution: WorkspaceChangeAttribution,
    pub coverage: WorkspaceChangeCoverage,
    pub change_set_status: WorkspaceChangeSetStatus,
    pub diff: String,
    pub warnings: Vec<String>,
    pub diff_detail: WorkspaceDiffDetail,
    pub max_diff_bytes: Option<u64>,
}

pub(super) fn view_from_diff(input: DiffViewInput) -> WorkspaceChangeView {
    let (files, stats) = files_from_diff(&input.diff);
    let status = if files.is_empty() {
        WorkspaceChangeViewStatus::Empty
    } else if matches!(input.coverage, WorkspaceChangeCoverage::Partial)
        || !input.warnings.is_empty()
    {
        WorkspaceChangeViewStatus::Partial
    } else {
        WorkspaceChangeViewStatus::Ready
    };
    let mut view = WorkspaceChangeView {
        scope: input.scope,
        status,
        workspace_root: input.workspace_root,
        base: input.base,
        coverage: input.coverage,
        attribution: input.attribution,
        change_set_status: input.change_set_status,
        files,
        stats,
        unified_diff: Some(input.diff),
        warnings: input.warnings,
        generated_at: chrono::Utc::now(),
    };
    apply_diff_detail(&mut view, input.diff_detail, input.max_diff_bytes);
    view
}

pub(super) fn apply_diff_detail(
    view: &mut WorkspaceChangeView,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) {
    if !matches!(diff_detail, WorkspaceDiffDetail::Full) {
        view.unified_diff = None;
        return;
    }
    let Some(diff) = view.unified_diff.as_mut() else {
        return;
    };
    let max = max_diff_bytes.unwrap_or(2 * 1024 * 1024) as usize;
    if diff.len() > max {
        diff.truncate(max);
        view.warnings.push("diff_truncated".to_string());
        for file in &mut view.files {
            file.diff_truncated = true;
        }
        if view.status == WorkspaceChangeViewStatus::Ready {
            view.status = WorkspaceChangeViewStatus::Partial;
        }
    }
}

pub(crate) fn unsupported_view(
    scope: WorkspaceChangeScope,
    workspace_root: PathBuf,
    attribution: WorkspaceChangeAttribution,
    reason: &str,
) -> WorkspaceChangeView {
    WorkspaceChangeView {
        scope,
        status: WorkspaceChangeViewStatus::Unsupported,
        workspace_root,
        base: None,
        coverage: WorkspaceChangeCoverage::None,
        attribution,
        change_set_status: WorkspaceChangeSetStatus::Finalized,
        files: Vec::new(),
        stats: WorkspaceChangeStats::default(),
        unified_diff: None,
        warnings: vec![reason.to_string()],
        generated_at: chrono::Utc::now(),
    }
}

pub(crate) fn error_view(
    scope: WorkspaceChangeScope,
    workspace_root: PathBuf,
    attribution: WorkspaceChangeAttribution,
    error: String,
) -> WorkspaceChangeView {
    WorkspaceChangeView {
        scope,
        status: WorkspaceChangeViewStatus::Error,
        workspace_root,
        base: None,
        coverage: WorkspaceChangeCoverage::None,
        attribution,
        change_set_status: WorkspaceChangeSetStatus::Finalized,
        files: Vec::new(),
        stats: WorkspaceChangeStats::default(),
        unified_diff: None,
        warnings: vec![error],
        generated_at: chrono::Utc::now(),
    }
}

pub(super) fn coverage_to_change_set(value: WorkspaceChangeCoverage) -> ChangeSetCoverage {
    match value {
        WorkspaceChangeCoverage::Full
        | WorkspaceChangeCoverage::GitVisible
        | WorkspaceChangeCoverage::BoundedFilesystem => ChangeSetCoverage::Full,
        WorkspaceChangeCoverage::Partial => ChangeSetCoverage::Partial,
        WorkspaceChangeCoverage::None => ChangeSetCoverage::None,
    }
}

fn files_from_diff(diff: &str) -> (Vec<WorkspaceChangedFile>, WorkspaceChangeStats) {
    let mut files = Vec::new();
    let mut current: Option<WorkspaceChangedFile> = None;
    let mut stats = WorkspaceChangeStats::default();
    for line in diff.lines() {
        if let Some(path) = line
            .strip_prefix("diff --git ")
            .and_then(parse_diff_git_path)
        {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(WorkspaceChangedFile {
                path,
                status: WorkspaceChangedFileStatus::Modified,
                additions: Some(0),
                deletions: Some(0),
                binary: false,
                diff_truncated: false,
                old_text: None,
                new_text: None,
            });
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if line.starts_with("new file mode") {
            file.status = WorkspaceChangedFileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            file.status = WorkspaceChangedFileStatus::Deleted;
        } else if line.starts_with("rename from ") || line.starts_with("rename to ") {
            file.status = WorkspaceChangedFileStatus::Renamed;
        } else if line.starts_with("Binary files ") {
            file.binary = true;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            let additions = file.additions.get_or_insert(0);
            *additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            let deletions = file.deletions.get_or_insert(0);
            *deletions += 1;
        }
    }
    if let Some(file) = current {
        files.push(file);
    }
    for file in &files {
        stats.files_changed += 1;
        stats.additions += file.additions.unwrap_or_default();
        stats.deletions += file.deletions.unwrap_or_default();
    }
    (files, stats)
}

fn parse_diff_git_path(rest: &str) -> Option<PathBuf> {
    let (_, b_path) = rest.rsplit_once(" b/")?;
    Some(PathBuf::from(unquote_git_path(b_path)))
}

fn unquote_git_path(path: &str) -> String {
    path.trim_matches('"').replace("\\\"", "\"")
}

pub(super) fn text_file_diff(path: &str, before: Option<&str>, after: Option<&str>) -> String {
    let before = before.unwrap_or_default();
    let after = after.unwrap_or_default();
    if before == after {
        return String::new();
    }
    let patch = diffy::create_patch(before, after);
    let patch_text = diffy::PatchFormatter::new().fmt_patch(&patch).to_string();
    let old_path = if before.is_empty() {
        "/dev/null".to_string()
    } else {
        format!("a/{path}")
    };
    let new_path = if after.is_empty() {
        "/dev/null".to_string()
    } else {
        format!("b/{path}")
    };
    format!("diff --git a/{path} b/{path}\n--- {old_path}\n+++ {new_path}\n{patch_text}")
}

pub(super) fn count_diff_lines(diff: &str) -> (u64, u64) {
    let mut additions = 0;
    let mut deletions = 0;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }
    (additions, deletions)
}
