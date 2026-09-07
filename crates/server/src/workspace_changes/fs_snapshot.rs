use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use devo_core::ChangeSetCoverage;
use devo_protocol::{
    SessionId, TurnId, WorkspaceChangeAttribution, WorkspaceChangeBase, WorkspaceChangeCoverage,
    WorkspaceChangeScope, WorkspaceChangeSetStatus, WorkspaceChangeStats, WorkspaceChangeView,
    WorkspaceChangeViewStatus, WorkspaceChangedFile, WorkspaceChangedFileStatus,
    WorkspaceCheckpointBackend, WorkspaceDiffDetail,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{ActiveWorkspaceBaseline, CapturedWorkspaceBaseline};
use super::{CheckpointRecordInput, artifact_ref, checkpoint_record, write_json};
use crate::workspace_changes::diff::{apply_diff_detail, count_diff_lines, text_file_diff};

const FS_MAX_FILES: usize = 10_000;
const FS_MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
const FS_MAX_TEXT_FILE_BYTES: u64 = 2 * 1024 * 1024;
const FS_MAX_HASH_FILE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct FileWorkspaceBaseline {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub workspace_root: PathBuf,
    pub checkpoint_id: String,
    manifest: FileManifest,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileManifest {
    root: PathBuf,
    entries: BTreeMap<String, FileManifestEntry>,
    warnings: Vec<String>,
    scanned_files: usize,
    captured_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FileManifestEntry {
    kind: FileEntryKind,
    size: u64,
    modified_ms: Option<i64>,
    hash: Option<String>,
    text_content: Option<String>,
    link_target: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FileEntryKind {
    File,
    Directory,
    Symlink,
    Other,
    Unreadable,
}

pub(crate) fn capture_file_baseline(
    artifact_dir: &Path,
    session_id: SessionId,
    turn_id: TurnId,
    cwd: &Path,
) -> Result<CapturedWorkspaceBaseline> {
    let workspace_root = cwd.to_path_buf();
    let manifest = scan_file_manifest(cwd);
    let checkpoint_id = format!("fs-{}", Uuid::new_v4());
    let artifact_ref = artifact_ref(session_id, turn_id, "baseline.json");
    write_json(&artifact_dir.join("baseline.json"), &manifest)?;
    let mut warnings = manifest.warnings.clone();
    warnings.sort();
    warnings.dedup();
    let coverage = if warnings.is_empty() {
        ChangeSetCoverage::Full
    } else {
        ChangeSetCoverage::Partial
    };
    let baseline = FileWorkspaceBaseline {
        session_id,
        turn_id,
        workspace_root,
        checkpoint_id,
        manifest,
        warnings,
    };
    Ok(CapturedWorkspaceBaseline {
        record: checkpoint_record(CheckpointRecordInput {
            session_id,
            turn_id,
            checkpoint_id: &baseline.checkpoint_id,
            workspace_root: &baseline.workspace_root,
            backend: "file_manifest",
            coverage,
            warnings: baseline.warnings.clone(),
            artifact_ref: Some(artifact_ref),
            preexisting_untracked_files: None,
            preexisting_untracked_dirs: None,
        }),
        baseline: ActiveWorkspaceBaseline::File(baseline),
    })
}

pub(crate) fn diff_file_baseline(
    baseline: &FileWorkspaceBaseline,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
    change_set_status: WorkspaceChangeSetStatus,
) -> WorkspaceChangeView {
    let current = scan_file_manifest(&baseline.workspace_root);
    let mut diff = String::new();
    let mut files = Vec::new();
    let mut stats = WorkspaceChangeStats::default();
    let mut paths = BTreeSet::new();
    paths.extend(baseline.manifest.entries.keys().cloned());
    paths.extend(current.entries.keys().cloned());

    for path in paths {
        let before = baseline.manifest.entries.get(&path);
        let after = current.entries.get(&path);
        let status = match (before, after) {
            (None, Some(_)) => WorkspaceChangedFileStatus::Added,
            (Some(_), None) => WorkspaceChangedFileStatus::Deleted,
            (Some(before), Some(after)) if before.kind != after.kind => {
                WorkspaceChangedFileStatus::TypeChanged
            }
            (Some(before), Some(after))
                if before.hash != after.hash || before.size != after.size =>
            {
                WorkspaceChangedFileStatus::Modified
            }
            _ => continue,
        };
        let before_text = before.and_then(|entry| entry.text_content.as_deref());
        let after_text = after.and_then(|entry| entry.text_content.as_deref());
        let file_diff = match (before_text, after_text) {
            (Some(before), Some(after)) => text_file_diff(&path, Some(before), Some(after)),
            (None, Some(after)) if before.is_none() => text_file_diff(&path, None, Some(after)),
            (Some(before), None) if after.is_none() => text_file_diff(&path, Some(before), None),
            _ => String::new(),
        };
        let (additions, deletions) = count_diff_lines(&file_diff);
        stats.files_changed += 1;
        stats.additions += additions;
        stats.deletions += deletions;
        let binary = file_diff.is_empty()
            && before
                .or(after)
                .is_some_and(|entry| entry.kind == FileEntryKind::File);
        files.push(WorkspaceChangedFile {
            path: PathBuf::from(&path),
            status,
            additions: Some(additions),
            deletions: Some(deletions),
            binary,
            diff_truncated: false,
            old_text: None,
            new_text: None,
        });
        diff.push_str(&file_diff);
    }

    let mut warnings = baseline.warnings.clone();
    warnings.extend(current.warnings.clone());
    warnings.sort();
    warnings.dedup();
    let coverage = if warnings.is_empty() {
        WorkspaceChangeCoverage::BoundedFilesystem
    } else {
        WorkspaceChangeCoverage::Partial
    };
    let mut view = WorkspaceChangeView {
        scope: WorkspaceChangeScope::Turn,
        status: if files.is_empty() {
            WorkspaceChangeViewStatus::Empty
        } else if warnings.is_empty() {
            WorkspaceChangeViewStatus::Ready
        } else {
            WorkspaceChangeViewStatus::Partial
        },
        workspace_root: baseline.workspace_root.clone(),
        base: Some(WorkspaceChangeBase::TurnCheckpoint {
            turn_id: baseline.turn_id,
            checkpoint_id: baseline.checkpoint_id.clone(),
            backend: WorkspaceCheckpointBackend::FileManifest,
        }),
        coverage,
        attribution: WorkspaceChangeAttribution::WorkspaceNet,
        change_set_status,
        files,
        stats,
        unified_diff: Some(diff),
        warnings,
        generated_at: chrono::Utc::now(),
    };
    apply_diff_detail(&mut view, diff_detail, max_diff_bytes);
    view
}

fn scan_file_manifest(root: &Path) -> FileManifest {
    let mut manifest = FileManifest {
        root: root.to_path_buf(),
        entries: BTreeMap::new(),
        warnings: Vec::new(),
        scanned_files: 0,
        captured_bytes: 0,
    };
    scan_dir(root, root, &mut manifest);
    manifest.warnings.sort();
    manifest.warnings.dedup();
    manifest
}

fn scan_dir(root: &Path, dir: &Path, manifest: &mut FileManifest) {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            manifest.warnings.push(format!(
                "read_dir_failed: {}: {error}",
                relative_path(root, dir)
            ));
            return;
        }
    };
    for entry in read_dir {
        if manifest.scanned_files >= FS_MAX_FILES {
            manifest.warnings.push("max_files_exceeded".to_string());
            return;
        }
        let Ok(entry) = entry else {
            manifest.warnings.push("read_dir_entry_failed".to_string());
            continue;
        };
        scan_path(root, entry.path(), manifest);
    }
}

fn scan_path(root: &Path, path: PathBuf, manifest: &mut FileManifest) {
    let rel = relative_path(root, &path);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            manifest.entries.insert(
                rel.clone(),
                FileManifestEntry {
                    kind: FileEntryKind::Unreadable,
                    size: 0,
                    modified_ms: None,
                    hash: None,
                    text_content: None,
                    link_target: None,
                },
            );
            manifest
                .warnings
                .push(format!("metadata_failed: {rel}: {error}"));
            return;
        }
    };
    let modified_ms = metadata.modified().ok().and_then(|modified| {
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_millis() as i64)
    });
    if metadata.is_dir() {
        manifest.entries.insert(
            rel,
            FileManifestEntry {
                kind: FileEntryKind::Directory,
                size: 0,
                modified_ms,
                hash: None,
                text_content: None,
                link_target: None,
            },
        );
        scan_dir(root, &path, manifest);
    } else if metadata.file_type().is_symlink() {
        manifest.entries.insert(
            rel,
            FileManifestEntry {
                kind: FileEntryKind::Symlink,
                size: 0,
                modified_ms,
                hash: None,
                text_content: None,
                link_target: fs::read_link(&path)
                    .ok()
                    .map(|target| target.display().to_string()),
            },
        );
    } else if metadata.is_file() {
        manifest.scanned_files += 1;
        let entry = file_manifest_entry(&path, &rel, metadata.len(), modified_ms, manifest);
        manifest.entries.insert(rel, entry);
    } else {
        manifest.entries.insert(
            rel,
            FileManifestEntry {
                kind: FileEntryKind::Other,
                size: metadata.len(),
                modified_ms,
                hash: None,
                text_content: None,
                link_target: None,
            },
        );
    }
}

fn file_manifest_entry(
    path: &Path,
    rel: &str,
    size: u64,
    modified_ms: Option<i64>,
    manifest: &mut FileManifest,
) -> FileManifestEntry {
    if size > FS_MAX_HASH_FILE_BYTES {
        manifest
            .warnings
            .push(format!("large_file_without_hash: {rel}"));
        return metadata_only_file(size, modified_ms);
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            manifest
                .warnings
                .push(format!("read_file_failed: {rel}: {error}"));
            return FileManifestEntry {
                kind: FileEntryKind::Unreadable,
                size,
                modified_ms,
                hash: None,
                text_content: None,
                link_target: None,
            };
        }
    };
    manifest.captured_bytes += bytes.len() as u64;
    if manifest.captured_bytes > FS_MAX_SNAPSHOT_BYTES {
        manifest
            .warnings
            .push("snapshot_bytes_exceeded".to_string());
    }
    let text_content = if size <= FS_MAX_TEXT_FILE_BYTES
        && !bytes.contains(&0)
        && manifest.captured_bytes <= FS_MAX_SNAPSHOT_BYTES
    {
        String::from_utf8(bytes.clone()).ok()
    } else {
        if size > FS_MAX_TEXT_FILE_BYTES {
            manifest
                .warnings
                .push(format!("large_file_without_text_diff: {rel}"));
        }
        None
    };
    FileManifestEntry {
        kind: FileEntryKind::File,
        size,
        modified_ms,
        hash: Some(hash_bytes(&bytes)),
        text_content,
        link_target: None,
    }
}

fn metadata_only_file(size: u64, modified_ms: Option<i64>) -> FileManifestEntry {
    FileManifestEntry {
        kind: FileEntryKind::File,
        size,
        modified_ms,
        hash: None,
        text_content: None,
        link_target: None,
    }
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}
