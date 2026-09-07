use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{SessionId, TurnId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeScope {
    Branch,
    Staged,
    Unstaged,
    Uncommitted,
    Turn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDiffDetail {
    None,
    #[default]
    Summary,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeViewStatus {
    Ready,
    Empty,
    Unsupported,
    Partial,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeCoverage {
    Full,
    GitVisible,
    BoundedFilesystem,
    Partial,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeSetStatus {
    Accumulating,
    Finalized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangedFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
    Untracked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkspaceChangesReadParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub scopes: Vec<WorkspaceChangeScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default)]
    pub diff_detail: WorkspaceDiffDetail,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_diff_bytes: Option<u64>,
    /// Server-side `--ignore-all-space` for the git-backed scopes
    /// (branch/staged/unstaged/uncommitted). Ignored for the turn scope,
    /// whose finalized diffs are precomputed artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_whitespace: Option<bool>,
    /// Optional path filter for expand-on-demand Full reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<PathBuf>>,
    /// When true with path-scoped Full, attach `old_text`/`new_text` so
    /// clients can render expandable (non-partial) diffs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_file_sides: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkspaceChangesReadResult {
    pub views: Vec<WorkspaceChangeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkspaceChangeView {
    pub scope: WorkspaceChangeScope,
    pub status: WorkspaceChangeViewStatus,
    pub workspace_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<WorkspaceChangeBase>,
    pub coverage: WorkspaceChangeCoverage,
    pub attribution: WorkspaceChangeAttribution,
    pub change_set_status: WorkspaceChangeSetStatus,
    pub files: Vec<WorkspaceChangedFile>,
    pub stats: WorkspaceChangeStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_diff: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceChangeBase {
    Branch {
        base_branch: String,
        merge_base: String,
        head: String,
    },
    Head {
        head: Option<String>,
    },
    TurnCheckpoint {
        turn_id: TurnId,
        checkpoint_id: String,
        backend: WorkspaceCheckpointBackend,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceCheckpointBackend {
    GitGhostCommit,
    FileManifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeAttribution {
    GitBranch,
    GitWorkingTree,
    WorkspaceNet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkspaceChangedFile {
    pub path: PathBuf,
    pub status: WorkspaceChangedFileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u64>,
    #[serde(default)]
    pub binary: bool,
    #[serde(default)]
    pub diff_truncated: bool,
    /// Full previous-side text for MultiFileDiff expand-up/down.
    /// Absent for added/untracked, binary, oversize, or when sides were not requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    /// Full new-side text for MultiFileDiff expand-up/down.
    /// Absent for deleted, binary, oversize, or when sides were not requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkspaceChangeStats {
    pub files_changed: u64,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct WorkspaceChangesUpdatedPayload {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub scope: WorkspaceChangeScope,
    pub status: WorkspaceChangeViewStatus,
    pub coverage: WorkspaceChangeCoverage,
    pub change_set_status: WorkspaceChangeSetStatus,
    pub stats: WorkspaceChangeStats,
    pub version: u64,
    pub generated_at: DateTime<Utc>,
}
