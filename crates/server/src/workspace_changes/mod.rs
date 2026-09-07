use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use devo_core::{
    ChangeSetCoverage, ChangeSetStatus, TurnWorkspaceChangeRecordedRecord,
    TurnWorkspaceCheckpointRecordedRecord,
};
use devo_protocol::{
    SessionId, TurnId, WorkspaceChangeSetStatus, WorkspaceChangeView, WorkspaceDiffDetail,
};
use devo_util_git::get_git_repo_root;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

mod diff;
mod fs_snapshot;
mod git;
mod git_list;
mod git_path;
mod git_sides;
mod view_cache;

pub(crate) use diff::{error_view, unsupported_view};
pub(crate) use git::{branch_view, staged_view, uncommitted_view, unstaged_view};
pub(crate) use git_path::path_scoped_full_view;

const DEFAULT_MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) enum ActiveWorkspaceBaseline {
    Git(git::GitWorkspaceBaseline),
    File(fs_snapshot::FileWorkspaceBaseline),
}

#[derive(Debug, Clone)]
pub(crate) struct CapturedWorkspaceBaseline {
    pub baseline: ActiveWorkspaceBaseline,
    pub record: TurnWorkspaceCheckpointRecordedRecord,
}

#[derive(Debug, Clone)]
pub(crate) struct FinalizedWorkspaceChanges {
    pub view: WorkspaceChangeView,
    pub record: TurnWorkspaceChangeRecordedRecord,
}

pub(crate) async fn preview_git_rollback(
    workspace_root: PathBuf,
    checkpoint_id: String,
) -> Result<git::GitRollbackPreview> {
    tokio::task::spawn_blocking(move || git::preview_git_rollback(&workspace_root, &checkpoint_id))
        .await
        .context("preview Git rollback task failed")?
}

pub(crate) async fn git_workspace_matches_version(
    workspace_root: PathBuf,
    workspace_version: String,
) -> Result<bool> {
    tokio::task::spawn_blocking(move || {
        git::git_workspace_matches_version(&workspace_root, &workspace_version)
    })
    .await
    .context("validate Git rollback workspace task failed")?
}

pub(crate) async fn current_git_workspace_version(workspace_root: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || git::current_git_workspace_version(&workspace_root))
        .await
        .context("capture Git rollback recovery version task failed")?
}

pub(crate) async fn restore_git_checkpoint(
    workspace_root: PathBuf,
    checkpoint: TurnWorkspaceCheckpointRecordedRecord,
) -> Result<()> {
    tokio::task::spawn_blocking(move || git::restore_git_checkpoint(&workspace_root, &checkpoint))
        .await
        .context("restore Git checkpoint task failed")?
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FinalizedWorkspaceChangeArtifact {
    schema_version: u32,
    view: WorkspaceChangeView,
}

pub(crate) async fn capture_baseline(
    data_root: PathBuf,
    session_id: SessionId,
    turn_id: TurnId,
    cwd: PathBuf,
) -> Result<CapturedWorkspaceBaseline> {
    tokio::task::spawn_blocking(move || {
        capture_baseline_blocking(data_root.as_path(), session_id, turn_id, cwd.as_path())
    })
    .await
    .context("capture workspace baseline task failed")?
}

fn capture_baseline_blocking(
    data_root: &Path,
    session_id: SessionId,
    turn_id: TurnId,
    cwd: &Path,
) -> Result<CapturedWorkspaceBaseline> {
    let artifact_dir = artifact_dir(data_root, session_id, turn_id);
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("create workspace snapshot dir {}", artifact_dir.display()))?;

    if let Some(repo_root) = get_git_repo_root(cwd) {
        match git::capture_git_baseline(&artifact_dir, session_id, turn_id, repo_root.as_path()) {
            Ok(captured) => return Ok(captured),
            Err(error) => {
                let mut captured =
                    fs_snapshot::capture_file_baseline(&artifact_dir, session_id, turn_id, cwd)?;
                if let ActiveWorkspaceBaseline::File(baseline) = &mut captured.baseline {
                    baseline
                        .warnings
                        .push(format!("git_snapshot_unavailable: {error}"));
                    captured.record.warnings = baseline.warnings.clone();
                }
                return Ok(captured);
            }
        }
    }

    fs_snapshot::capture_file_baseline(&artifact_dir, session_id, turn_id, cwd)
}

pub(crate) async fn finalize_baseline(
    data_root: PathBuf,
    baseline: ActiveWorkspaceBaseline,
) -> Result<FinalizedWorkspaceChanges> {
    tokio::task::spawn_blocking(move || {
        let session_id = baseline.session_id();
        let turn_id = baseline.turn_id();
        let artifact_dir = artifact_dir(data_root.as_path(), session_id, turn_id);
        fs::create_dir_all(&artifact_dir)?;
        let view = diff_baseline_blocking(
            &baseline,
            WorkspaceDiffDetail::Full,
            Some(DEFAULT_MAX_DIFF_BYTES),
            WorkspaceChangeSetStatus::Finalized,
        )?;
        let final_ref = artifact_ref(session_id, turn_id, "final.json");
        write_json(
            &artifact_dir.join("final.json"),
            &FinalizedWorkspaceChangeArtifact {
                schema_version: 1,
                view: view.clone(),
            },
        )?;
        let record = TurnWorkspaceChangeRecordedRecord {
            schema_version: 1,
            session_id,
            turn_id,
            change_id: Uuid::new_v4().to_string(),
            file_path: ".".to_string(),
            pre_hash: baseline.checkpoint_id().to_string(),
            post_hash: hash_text(view.unified_diff.as_deref().unwrap_or_default()),
            inverse_ref: None,
            display_diff_ref: Some(final_ref.clone()),
            workspace_root: Some(view.workspace_root.display().to_string()),
            backend: Some(baseline.backend_name().to_string()),
            coverage: Some(diff::coverage_to_change_set(view.coverage)),
            warnings: view.warnings.clone(),
            changed_files: view
                .files
                .iter()
                .map(|file| file.path.display().to_string())
                .collect(),
            artifact_ref: Some(final_ref),
            change_set_status: Some(ChangeSetStatus::Finalized),
            recorded_at: Utc::now(),
        };
        Ok(FinalizedWorkspaceChanges { view, record })
    })
    .await
    .context("finalize workspace baseline task failed")?
}

pub(crate) async fn read_active_turn_view(
    baseline: ActiveWorkspaceBaseline,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> Result<WorkspaceChangeView> {
    tokio::task::spawn_blocking(move || {
        diff_baseline_blocking(
            &baseline,
            diff_detail,
            max_diff_bytes,
            WorkspaceChangeSetStatus::Accumulating,
        )
    })
    .await
    .context("read active workspace changes task failed")?
}

pub(crate) fn read_finalized_turn_view(
    data_root: &Path,
    session_id: SessionId,
    turn_id: TurnId,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> Result<Option<WorkspaceChangeView>> {
    let path = artifact_dir(data_root, session_id, turn_id).join("final.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read workspace changes artifact {}", path.display()))?;
    let artifact: FinalizedWorkspaceChangeArtifact = serde_json::from_str(&text)
        .with_context(|| format!("parse workspace changes artifact {}", path.display()))?;
    let mut view = artifact.view;
    diff::apply_diff_detail(&mut view, diff_detail, max_diff_bytes);
    Ok(Some(view))
}

fn diff_baseline_blocking(
    baseline: &ActiveWorkspaceBaseline,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
    change_set_status: WorkspaceChangeSetStatus,
) -> Result<WorkspaceChangeView> {
    match baseline {
        ActiveWorkspaceBaseline::Git(baseline) => {
            git::diff_git_baseline(baseline, diff_detail, max_diff_bytes, change_set_status)
        }
        ActiveWorkspaceBaseline::File(baseline) => Ok(fs_snapshot::diff_file_baseline(
            baseline,
            diff_detail,
            max_diff_bytes,
            change_set_status,
        )),
    }
}

pub(super) struct CheckpointRecordInput<'a> {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub checkpoint_id: &'a str,
    pub workspace_root: &'a Path,
    pub backend: &'a str,
    pub coverage: ChangeSetCoverage,
    pub warnings: Vec<String>,
    pub artifact_ref: Option<String>,
    pub preexisting_untracked_files: Option<Vec<String>>,
    pub preexisting_untracked_dirs: Option<Vec<String>>,
}

fn checkpoint_record(input: CheckpointRecordInput<'_>) -> TurnWorkspaceCheckpointRecordedRecord {
    TurnWorkspaceCheckpointRecordedRecord {
        schema_version: 1,
        session_id: input.session_id,
        turn_id: input.turn_id,
        checkpoint_id: input.checkpoint_id.to_string(),
        pre_turn_hash: input.checkpoint_id.to_string(),
        files: Vec::new(),
        workspace_root: Some(input.workspace_root.display().to_string()),
        backend: Some(input.backend.to_string()),
        coverage: Some(input.coverage),
        warnings: input.warnings,
        artifact_ref: input.artifact_ref,
        preexisting_untracked_files: input.preexisting_untracked_files,
        preexisting_untracked_dirs: input.preexisting_untracked_dirs,
        created_at: Utc::now(),
    }
}

fn artifact_dir(data_root: &Path, session_id: SessionId, turn_id: TurnId) -> PathBuf {
    data_root
        .join("workspace-snapshots")
        .join(session_id.to_string())
        .join(turn_id.to_string())
}

fn artifact_ref(session_id: SessionId, turn_id: TurnId, file_name: &str) -> String {
    format!("workspace-snapshots/{session_id}/{turn_id}/{file_name}")
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

impl ActiveWorkspaceBaseline {
    fn session_id(&self) -> SessionId {
        match self {
            Self::Git(baseline) => baseline.session_id,
            Self::File(baseline) => baseline.session_id,
        }
    }

    fn turn_id(&self) -> TurnId {
        match self {
            Self::Git(baseline) => baseline.turn_id,
            Self::File(baseline) => baseline.turn_id,
        }
    }

    pub(crate) fn checkpoint_id(&self) -> &str {
        match self {
            Self::Git(baseline) => &baseline.checkpoint_id,
            Self::File(baseline) => &baseline.checkpoint_id,
        }
    }

    fn backend_name(&self) -> &'static str {
        match self {
            Self::Git(_) => "git_ghost_commit",
            Self::File(_) => "file_manifest",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use devo_protocol::{
        WorkspaceChangeBase, WorkspaceChangeCoverage, WorkspaceChangeScope,
        WorkspaceChangeSetStatus, WorkspaceChangeViewStatus, WorkspaceChangedFileStatus,
        WorkspaceDiffDetail,
    };
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn non_git_finalized_turn_view_is_stable_after_later_changes() -> Result<()> {
        let data_root = tempdir()?;
        let workspace = tempdir()?;
        fs::write(workspace.path().join("a.txt"), "old\n")?;
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let captured = capture_baseline(
            data_root.path().to_path_buf(),
            session_id,
            turn_id,
            workspace.path().to_path_buf(),
        )
        .await?;

        fs::write(workspace.path().join("a.txt"), "new\n")?;
        fs::write(workspace.path().join("b.txt"), "added\n")?;
        let finalized =
            finalize_baseline(data_root.path().to_path_buf(), captured.baseline).await?;
        assert_eq!(finalized.view.status, WorkspaceChangeViewStatus::Ready);
        assert_eq!(
            finalized.view.change_set_status,
            WorkspaceChangeSetStatus::Finalized
        );
        let statuses = file_statuses(&finalized.view);
        assert_eq!(
            statuses,
            BTreeMap::from([
                ("a.txt".to_string(), WorkspaceChangedFileStatus::Modified),
                ("b.txt".to_string(), WorkspaceChangedFileStatus::Added),
            ])
        );

        fs::write(workspace.path().join("a.txt"), "later\n")?;
        let reread = read_finalized_turn_view(
            data_root.path(),
            session_id,
            turn_id,
            WorkspaceDiffDetail::Full,
            None,
        )?
        .expect("finalized view");
        let diff = reread.unified_diff.expect("full diff");
        assert!(diff.contains("+new"));
        assert!(!diff.contains("+later"));
        Ok(())
    }

    #[tokio::test]
    async fn git_turn_baseline_reports_tracked_and_untracked_net_changes() -> Result<()> {
        let data_root = tempdir()?;
        let repo = tempdir()?;
        run_git(repo.path(), &["init"]);
        run_git(
            repo.path(),
            &["config", "user.email", "snapshot@example.com"],
        );
        run_git(repo.path(), &["config", "user.name", "Snapshot Test"]);
        fs::write(repo.path().join("tracked.txt"), "before\n")?;
        run_git(repo.path(), &["add", "tracked.txt"]);
        run_git(repo.path(), &["commit", "-m", "initial"]);
        fs::write(repo.path().join("note.txt"), "preexisting\n")?;

        let captured = capture_baseline(
            data_root.path().to_path_buf(),
            SessionId::new(),
            TurnId::new(),
            repo.path().to_path_buf(),
        )
        .await?;

        fs::write(repo.path().join("tracked.txt"), "after\n")?;
        fs::remove_file(repo.path().join("note.txt"))?;
        fs::write(repo.path().join("later.txt"), "later\n")?;
        let view =
            read_active_turn_view(captured.baseline, WorkspaceDiffDetail::Full, None).await?;

        assert_eq!(view.coverage, WorkspaceChangeCoverage::GitVisible);
        let statuses = file_statuses(&view);
        assert_eq!(
            statuses,
            BTreeMap::from([
                ("later.txt".to_string(), WorkspaceChangedFileStatus::Added),
                ("note.txt".to_string(), WorkspaceChangedFileStatus::Deleted),
                (
                    "tracked.txt".to_string(),
                    WorkspaceChangedFileStatus::Modified,
                ),
            ])
        );
        let diff = view.unified_diff.expect("full diff");
        assert!(diff.contains("tracked.txt"));
        assert!(diff.contains("note.txt"));
        assert!(diff.contains("later.txt"));
        Ok(())
    }

    #[tokio::test]
    async fn git_rollback_deletes_only_new_untracked_paths_with_manifest() -> Result<()> {
        let data_root = tempdir()?;
        let repo = tempdir()?;
        let normalize_line_endings = |s: String| s.replace("\r\n", "\n");
        run_git(repo.path(), &["init"]);
        run_git(
            repo.path(),
            &["config", "user.email", "rollback@example.com"],
        );
        run_git(repo.path(), &["config", "user.name", "Rollback Test"]);
        fs::write(repo.path().join("tracked.txt"), "before\n")?;
        run_git(repo.path(), &["add", "tracked.txt"]);
        run_git(repo.path(), &["commit", "-m", "initial"]);
        fs::write(repo.path().join("preexisting.txt"), "keep\n")?;

        let captured = capture_baseline(
            data_root.path().to_path_buf(),
            SessionId::new(),
            TurnId::new(),
            repo.path().to_path_buf(),
        )
        .await?;
        assert_eq!(
            captured.record.preexisting_untracked_files,
            Some(vec!["preexisting.txt".to_string()])
        );
        assert_eq!(captured.record.preexisting_untracked_dirs, Some(Vec::new()));

        fs::write(repo.path().join("tracked.txt"), "after\n")?;
        fs::write(repo.path().join("created-after.txt"), "remove\n")?;
        let preview = git::preview_git_rollback(repo.path(), &captured.record.checkpoint_id)?;
        assert_eq!(
            preview.affected_files,
            vec![
                PathBuf::from("created-after.txt"),
                PathBuf::from("tracked.txt"),
            ]
        );
        assert!(git::git_workspace_matches_version(
            repo.path(),
            &preview.workspace_version
        )?);
        fs::write(repo.path().join("drift.txt"), "drift\n")?;
        assert!(!git::git_workspace_matches_version(
            repo.path(),
            &preview.workspace_version
        )?);
        fs::remove_file(repo.path().join("drift.txt"))?;
        git::restore_git_checkpoint(repo.path(), &captured.record)?;
        assert_eq!(
            normalize_line_endings(fs::read_to_string(repo.path().join("tracked.txt"))?),
            "before\n"
        );
        assert!(repo.path().join("preexisting.txt").exists());
        assert!(!repo.path().join("created-after.txt").exists());

        let mut legacy_checkpoint = captured.record;
        legacy_checkpoint.preexisting_untracked_files = None;
        legacy_checkpoint.preexisting_untracked_dirs = None;
        fs::write(repo.path().join("tracked.txt"), "changed-again\n")?;
        fs::write(repo.path().join("legacy-new.txt"), "must survive\n")?;
        git::restore_git_checkpoint(repo.path(), &legacy_checkpoint)?;
        assert_eq!(
            normalize_line_endings(fs::read_to_string(repo.path().join("tracked.txt"))?),
            "before\n"
        );
        assert_eq!(
            normalize_line_endings(fs::read_to_string(repo.path().join("legacy-new.txt"))?),
            "must survive\n"
        );
        assert_eq!(
            git::preview_git_rollback(repo.path(), &legacy_checkpoint.checkpoint_id)?
                .affected_files,
            vec![PathBuf::from("legacy-new.txt")]
        );
        Ok(())
    }

    fn file_statuses(view: &WorkspaceChangeView) -> BTreeMap<String, WorkspaceChangedFileStatus> {
        view.files
            .iter()
            .map(|file| (file.path.display().to_string(), file.status))
            .collect()
    }

    fn run_git(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git command failed: {args:?}");
    }

    /// Repo with a committed baseline of `staged.txt` + `unstaged.txt`, then:
    /// `staged.txt` modified and staged, `unstaged.txt` modified in the
    /// worktree only, `untracked.txt` left untracked.
    fn init_split_repo() -> Result<tempfile::TempDir> {
        let repo = tempdir()?;
        run_git(repo.path(), &["init"]);
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);
        run_git(
            repo.path(),
            &["config", "user.email", "changes@example.com"],
        );
        run_git(repo.path(), &["config", "user.name", "Changes Test"]);
        fs::write(repo.path().join("staged.txt"), "base\n")?;
        fs::write(repo.path().join("unstaged.txt"), "base\n")?;
        run_git(repo.path(), &["add", "."]);
        run_git(repo.path(), &["commit", "-m", "initial"]);
        fs::write(repo.path().join("staged.txt"), "staged change\n")?;
        run_git(repo.path(), &["add", "staged.txt"]);
        fs::write(repo.path().join("unstaged.txt"), "unstaged change\n")?;
        fs::write(repo.path().join("untracked.txt"), "untracked\n")?;
        Ok(repo)
    }

    #[tokio::test]
    async fn staged_view_reports_index_vs_head() -> Result<()> {
        let repo = init_split_repo()?;
        let view = git::staged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(view.status, WorkspaceChangeViewStatus::Ready);
        assert_eq!(
            file_statuses(&view),
            BTreeMap::from([(
                "staged.txt".to_string(),
                WorkspaceChangedFileStatus::Modified
            )])
        );
        assert!(matches!(
            view.base,
            Some(WorkspaceChangeBase::Head { head: Some(_) })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn unstaged_view_excludes_staged_and_includes_untracked() -> Result<()> {
        let repo = init_split_repo()?;
        let summary = git::unstaged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Summary,
            None,
        )
        .await;
        assert_eq!(summary.status, WorkspaceChangeViewStatus::Ready);
        assert_eq!(
            file_statuses(&summary),
            BTreeMap::from([
                (
                    "untracked.txt".to_string(),
                    WorkspaceChangedFileStatus::Untracked
                ),
                (
                    "unstaged.txt".to_string(),
                    WorkspaceChangedFileStatus::Modified
                ),
            ])
        );
        // Full lists tracked diffs only; untracked content is path-scoped on expand.
        let full = git::unstaged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            file_statuses(&full),
            BTreeMap::from([(
                "unstaged.txt".to_string(),
                WorkspaceChangedFileStatus::Modified
            )])
        );
        Ok(())
    }

    #[tokio::test]
    async fn staged_and_unstaged_ignore_whitespace_when_requested() -> Result<()> {
        let repo = tempdir()?;
        run_git(repo.path(), &["init"]);
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);
        run_git(
            repo.path(),
            &["config", "user.email", "changes@example.com"],
        );
        run_git(repo.path(), &["config", "user.name", "Changes Test"]);
        fs::write(repo.path().join("only.txt"), "content\n")?;
        run_git(repo.path(), &["add", "only.txt"]);
        run_git(repo.path(), &["commit", "-m", "initial"]);

        // Staged: trailing-whitespace-only change.
        fs::write(repo.path().join("only.txt"), "content  \n")?;
        run_git(repo.path(), &["add", "only.txt"]);
        // Unstaged: another trailing-whitespace-only change on top.
        fs::write(repo.path().join("only.txt"), "content    \n")?;

        let staged_without_flag = git::staged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            file_statuses(&staged_without_flag),
            BTreeMap::from([("only.txt".to_string(), WorkspaceChangedFileStatus::Modified)]),
            "staged should report the whitespace-only change without the flag"
        );
        let unstaged_without_flag = git::unstaged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            file_statuses(&unstaged_without_flag),
            BTreeMap::from([("only.txt".to_string(), WorkspaceChangedFileStatus::Modified)]),
            "unstaged should report the whitespace-only change without the flag"
        );

        let staged_with_flag = git::staged_view(
            repo.path().to_path_buf(),
            true,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            staged_with_flag.status,
            WorkspaceChangeViewStatus::Empty,
            "staged should hide whitespace-only changes with the flag"
        );
        assert!(staged_with_flag.files.is_empty(), "staged files");
        let unstaged_with_flag = git::unstaged_view(
            repo.path().to_path_buf(),
            true,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            unstaged_with_flag.status,
            WorkspaceChangeViewStatus::Empty,
            "unstaged should hide whitespace-only changes with the flag"
        );
        assert!(unstaged_with_flag.files.is_empty(), "unstaged files");
        Ok(())
    }

    #[tokio::test]
    async fn staged_and_unstaged_unsupported_without_head_and_outside_git() -> Result<()> {
        let unborn = tempdir()?;
        run_git(unborn.path(), &["init"]);
        let unborn_staged = git::staged_view(
            unborn.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(unborn_staged.status, WorkspaceChangeViewStatus::Unsupported);
        assert_eq!(
            unborn_staged.warnings,
            vec!["no_head".to_string()],
            "unborn branch should report no_head"
        );
        let unborn_unstaged = git::unstaged_view(
            unborn.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            unborn_unstaged.status,
            WorkspaceChangeViewStatus::Unsupported
        );
        assert_eq!(unborn_unstaged.warnings, vec!["no_head".to_string()]);

        let plain = tempdir()?;
        let plain_staged = git::staged_view(
            plain.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(plain_staged.status, WorkspaceChangeViewStatus::Unsupported);
        assert_eq!(
            plain_staged.warnings,
            vec!["not_git_repository".to_string()],
            "plain directory should report not_git_repository"
        );
        let plain_unstaged = git::unstaged_view(
            plain.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(
            plain_unstaged.status,
            WorkspaceChangeViewStatus::Unsupported
        );
        assert_eq!(
            plain_unstaged.warnings,
            vec!["not_git_repository".to_string()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn git_view_cache_returns_same_payload_without_rediff() -> Result<()> {
        view_cache::clear_for_tests();
        let repo = init_split_repo()?;
        let first = git::unstaged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        let second = git::unstaged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Full,
            None,
        )
        .await;
        assert_eq!(first.files, second.files);
        assert_eq!(first.unified_diff, second.unified_diff);
        assert_eq!(first.stats, second.stats);
        Ok(())
    }

    #[tokio::test]
    async fn summary_detail_lists_files_without_unified_diff() -> Result<()> {
        view_cache::clear_for_tests();
        let repo = init_split_repo()?;
        let summary = git::unstaged_view(
            repo.path().to_path_buf(),
            false,
            WorkspaceDiffDetail::Summary,
            None,
        )
        .await;
        assert_eq!(summary.status, WorkspaceChangeViewStatus::Ready);
        assert!(
            summary.unified_diff.is_none(),
            "summary must omit patch text"
        );
        assert_eq!(
            file_statuses(&summary),
            BTreeMap::from([
                (
                    "untracked.txt".to_string(),
                    WorkspaceChangedFileStatus::Untracked
                ),
                (
                    "unstaged.txt".to_string(),
                    WorkspaceChangedFileStatus::Modified
                ),
            ])
        );
        let untracked = summary
            .files
            .iter()
            .find(|file| file.path.ends_with("untracked.txt"))
            .expect("untracked.txt");
        assert_eq!(
            untracked.additions,
            Some(1),
            "Summary should count untracked lines"
        );
        assert_eq!(untracked.deletions, Some(0));
        Ok(())
    }

    /// Manual perf probe against the live `devo` checkout. Run with:
    /// `cargo test -p devo-server --lib bench_devo_workspace_changes_summary_vs_full -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "manual perf probe against the live checkout"]
    async fn bench_devo_workspace_changes_summary_vs_full() {
        view_cache::clear_for_tests();
        let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        eprintln!("bench cwd={}", cwd.display());

        // Warm git's own pack/index once so the first timed call isn't dominated by cold OS cache.
        let _ = git::unstaged_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None).await;

        for scope_label in ["unstaged", "uncommitted", "branch", "staged"] {
            view_cache::clear_for_tests();
            let started = std::time::Instant::now();
            let summary = match scope_label {
                "unstaged" => {
                    git::unstaged_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None).await
                }
                "uncommitted" => {
                    git::uncommitted_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None)
                        .await
                }
                "branch" => {
                    git::branch_view(cwd.clone(), None, false, WorkspaceDiffDetail::Summary, None)
                        .await
                }
                "staged" => {
                    git::staged_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None).await
                }
                _ => unreachable!(),
            };
            let summary_ms = started.elapsed().as_millis();

            // Same Summary again should hit the in-process view cache (list switch).
            let started = std::time::Instant::now();
            let _warm = match scope_label {
                "unstaged" => {
                    git::unstaged_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None).await
                }
                "uncommitted" => {
                    git::uncommitted_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None)
                        .await
                }
                "branch" => {
                    git::branch_view(cwd.clone(), None, false, WorkspaceDiffDetail::Summary, None)
                        .await
                }
                "staged" => {
                    git::staged_view(cwd.clone(), false, WorkspaceDiffDetail::Summary, None).await
                }
                _ => unreachable!(),
            };
            let warm_summary_ms = started.elapsed().as_millis();

            view_cache::clear_for_tests();
            let started = std::time::Instant::now();
            let full = match scope_label {
                "unstaged" => {
                    git::unstaged_view(cwd.clone(), false, WorkspaceDiffDetail::Full, None).await
                }
                "uncommitted" => {
                    git::uncommitted_view(cwd.clone(), false, WorkspaceDiffDetail::Full, None).await
                }
                "branch" => {
                    git::branch_view(cwd.clone(), None, false, WorkspaceDiffDetail::Full, None)
                        .await
                }
                "staged" => {
                    git::staged_view(cwd.clone(), false, WorkspaceDiffDetail::Full, None).await
                }
                _ => unreachable!(),
            };
            let full_ms = started.elapsed().as_millis();

            // Warm cache hit for Full.
            let started = std::time::Instant::now();
            let _cached = match scope_label {
                "unstaged" => {
                    git::unstaged_view(cwd.clone(), false, WorkspaceDiffDetail::Full, None).await
                }
                "uncommitted" => {
                    git::uncommitted_view(cwd.clone(), false, WorkspaceDiffDetail::Full, None).await
                }
                "branch" => {
                    git::branch_view(cwd.clone(), None, false, WorkspaceDiffDetail::Full, None)
                        .await
                }
                "staged" => {
                    git::staged_view(cwd.clone(), false, WorkspaceDiffDetail::Full, None).await
                }
                _ => unreachable!(),
            };
            let cached_ms = started.elapsed().as_millis();

            eprintln!(
                "{scope_label}: summary={}ms warm_summary={}ms files={} full={}ms patch_bytes={} cached_full={}ms",
                summary_ms,
                warm_summary_ms,
                summary.files.len(),
                full_ms,
                full.unified_diff.as_ref().map(|d| d.len()).unwrap_or(0),
                cached_ms,
            );
        }
    }

    #[tokio::test]
    async fn path_scoped_include_file_sides_attaches_old_and_new() -> Result<()> {
        let repo = tempdir()?;
        run_git(repo.path(), &["init"]);
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);
        run_git(repo.path(), &["config", "user.email", "sides@example.com"]);
        run_git(repo.path(), &["config", "user.name", "Sides Test"]);
        fs::write(repo.path().join("mod.txt"), "line1\nold\nline3\n")?;
        run_git(repo.path(), &["add", "mod.txt"]);
        run_git(repo.path(), &["commit", "-m", "initial"]);
        fs::write(repo.path().join("mod.txt"), "line1\nnew\nline3\n")?;
        fs::write(repo.path().join("added.txt"), "brand new\n")?;
        fs::write(repo.path().join("gone.txt"), "delete me\n")?;
        run_git(repo.path(), &["add", "gone.txt"]);
        run_git(repo.path(), &["commit", "-m", "add gone"]);
        run_git(repo.path(), &["rm", "gone.txt"]);

        let modified = path_scoped_full_view(
            repo.path().to_path_buf(),
            WorkspaceChangeScope::Uncommitted,
            vec![PathBuf::from("mod.txt")],
            None,
            false,
            None,
            None,
            /*include_file_sides*/ true,
        )
        .await;
        let mod_file = modified
            .files
            .iter()
            .find(|file| file.path.ends_with("mod.txt"))
            .expect("mod.txt");
        assert_eq!(mod_file.old_text.as_deref(), Some("line1\nold\nline3\n"));
        assert_eq!(mod_file.new_text.as_deref(), Some("line1\nnew\nline3\n"));

        let added = path_scoped_full_view(
            repo.path().to_path_buf(),
            WorkspaceChangeScope::Uncommitted,
            vec![PathBuf::from("added.txt")],
            None,
            false,
            None,
            None,
            true,
        )
        .await;
        let added_file = added
            .files
            .iter()
            .find(|file| file.path.ends_with("added.txt"))
            .expect("added.txt");
        assert!(added_file.old_text.is_none());
        assert_eq!(added_file.new_text.as_deref(), Some("brand new\n"));

        let deleted = path_scoped_full_view(
            repo.path().to_path_buf(),
            WorkspaceChangeScope::Uncommitted,
            vec![PathBuf::from("gone.txt")],
            None,
            false,
            None,
            None,
            true,
        )
        .await;
        let gone = deleted
            .files
            .iter()
            .find(|file| file.path.ends_with("gone.txt"))
            .expect("gone.txt");
        assert_eq!(gone.old_text.as_deref(), Some("delete me\n"));
        assert!(gone.new_text.is_none());

        let without_flag = path_scoped_full_view(
            repo.path().to_path_buf(),
            WorkspaceChangeScope::Uncommitted,
            vec![PathBuf::from("mod.txt")],
            None,
            false,
            None,
            None,
            /*include_file_sides*/ false,
        )
        .await;
        let bare = without_flag
            .files
            .iter()
            .find(|file| file.path.ends_with("mod.txt"))
            .expect("mod.txt");
        assert!(bare.old_text.is_none());
        assert!(bare.new_text.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn path_scoped_branch_sides_use_merge_base_and_head() -> Result<()> {
        let repo = tempdir()?;
        run_git(repo.path(), &["init"]);
        run_git(repo.path(), &["config", "core.autocrlf", "false"]);
        run_git(repo.path(), &["config", "user.email", "sides@example.com"]);
        run_git(repo.path(), &["config", "user.name", "Sides Test"]);
        // Ensure default branch is main for merge-base discovery.
        let _ = Command::new("git")
            .current_dir(repo.path())
            .args(["branch", "-M", "main"])
            .status();
        fs::write(repo.path().join("file.txt"), "base\n")?;
        run_git(repo.path(), &["add", "file.txt"]);
        run_git(repo.path(), &["commit", "-m", "main"]);
        run_git(repo.path(), &["checkout", "-b", "feature"]);
        fs::write(repo.path().join("file.txt"), "feature\n")?;
        run_git(repo.path(), &["add", "file.txt"]);
        run_git(repo.path(), &["commit", "-m", "feature change"]);

        let view = path_scoped_full_view(
            repo.path().to_path_buf(),
            WorkspaceChangeScope::Branch,
            vec![PathBuf::from("file.txt")],
            Some("main".to_string()),
            false,
            None,
            None,
            true,
        )
        .await;
        let file = view
            .files
            .iter()
            .find(|file| file.path.ends_with("file.txt"))
            .expect("file.txt");
        assert_eq!(file.old_text.as_deref(), Some("base\n"));
        assert_eq!(file.new_text.as_deref(), Some("feature\n"));
        Ok(())
    }
}
