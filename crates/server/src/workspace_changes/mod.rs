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

pub(crate) use diff::{error_view, unsupported_view};
pub(crate) use git::{branch_view, uncommitted_view};

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

    fn checkpoint_id(&self) -> &str {
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
        WorkspaceChangeCoverage, WorkspaceChangeSetStatus, WorkspaceChangeViewStatus,
        WorkspaceChangedFileStatus, WorkspaceDiffDetail,
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
}
