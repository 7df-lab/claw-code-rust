//! Native `workspace/changes/read`.
//!
//! The desktop client consumes this read model (branch / uncommitted /
//! turn-scoped diffs). Types mirror the legacy `workspace_changes` shapes
//! with native camelCase field names and ids; the legacy enums are reused
//! directly (their wire values stay snake_case inside otherwise camelCase
//! payloads — an accepted inconsistency to keep one vocabulary).

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::SessionId;
use super::ids::TurnId;
use crate::WorkspaceChangeAttribution;
use crate::WorkspaceChangeBase;
use crate::WorkspaceChangeCoverage;
use crate::WorkspaceChangeScope;
use crate::WorkspaceChangeSetStatus;
use crate::WorkspaceChangeStats;
use crate::WorkspaceChangeViewStatus;
use crate::WorkspaceChangedFile;
use crate::WorkspaceDiffDetail;

// ── workspace/changes/read ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChangesReadResult {
    pub views: Vec<WorkspaceChangeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
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
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

impl From<crate::WorkspaceChangeView> for WorkspaceChangeView {
    fn from(view: crate::WorkspaceChangeView) -> Self {
        Self {
            scope: view.scope,
            status: view.status,
            workspace_root: view.workspace_root,
            base: view.base,
            coverage: view.coverage,
            attribution: view.attribution,
            change_set_status: view.change_set_status,
            files: view.files,
            stats: view.stats,
            unified_diff: view.unified_diff,
            warnings: view.warnings,
            generated_at: view.generated_at,
        }
    }
}

impl From<WorkspaceChangeView> for crate::WorkspaceChangeView {
    fn from(view: WorkspaceChangeView) -> Self {
        Self {
            scope: view.scope,
            status: view.status,
            workspace_root: view.workspace_root,
            base: view.base,
            coverage: view.coverage,
            attribution: view.attribution,
            change_set_status: view.change_set_status,
            files: view.files,
            stats: view.stats,
            unified_diff: view.unified_diff,
            warnings: view.warnings,
            generated_at: view.generated_at,
        }
    }
}
