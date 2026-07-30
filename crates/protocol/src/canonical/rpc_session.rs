//! Params/result types for session-domain methods (`session/*`,
//! `session/goal/*`). Truth source: `devo-api-design/01-native-api.md` §4.2/§4.5.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::goal::Goal;
use super::ids::GoalId;
use super::ids::RestorePlanId;
use super::ids::SessionId;
use super::ids::TurnId;
use super::model::ModelBinding;
use super::page::Page;
use super::page::PageParams;
use super::patch::PatchField;
use super::session::Session;
use super::session::SessionSettings;

// ── session/new ──

/// Deliberately minimal: create binds a cwd, nothing else. Model/settings are
/// changed later via `session/metadata/update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewParams {
    pub cwd: PathBuf,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionNewResult {
    pub session: Session,
}

// ── session/list ──

/// No filtering by status/archive flags; only title search is supported.
/// Returned sessions use `turnsView = notLoaded` (no embedded history).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionListParams {
    /// Restrict to these cwds; empty means all known cwds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cwds: Vec<PathBuf>,
    /// Case-insensitive substring match on title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

pub type SessionListResult = Page<Session>;

// ── session/read ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionReadParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionReadResult {
    pub session: Session,
}

// ── session/resume ──

/// Addressed by session id only; never changes the session's cwd. The result
/// returns the real cwd so clients connecting from another directory can
/// surface it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeResult {
    pub session: Session,
}

// ── session/fork ──

/// Forks at a turn boundary into parallel history; the goal is copied by
/// value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionForkResult {
    pub session: Session,
}

// ── session/rollback/preview + commit ──

/// Which user turns to keep, counted by user turn index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum RollbackMode {
    /// Keep the selected user turn, drop everything after it.
    ThroughUserTurn,
    /// Drop the selected user turn as well.
    BeforeUserTurn,
}

/// Computes the history/workspace impact without changing any state. The
/// client must show the impact and get confirmation before `commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionRollbackPreviewParams {
    pub session_id: SessionId,
    pub user_turn_index: u32,
    pub mode: RollbackMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct RestorePlan {
    pub restore_plan_id: RestorePlanId,
    /// Files the workspace restore would touch (restore or delete).
    pub affected_files: Vec<PathBuf>,
    /// Turns/items the history truncation would drop, for display.
    pub dropped_turn_count: u32,
    /// Workspace version/hash captured at preview time; `commit` revalidates
    /// it and rejects with `WORKSPACE_VERSION_CONFLICT` on drift.
    pub workspace_version: String,
}

pub type SessionRollbackPreviewResult = RestorePlan;

/// Commits a previously previewed plan. Plans are short-lived, single-use,
/// and bound to the session, target checkpoint and caller identity; retrying
/// with the same `restorePlanId` returns the first commit's result instead of
/// restoring again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionRollbackCommitParams {
    pub restore_plan_id: RestorePlanId,
    pub expected_workspace_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionRollbackCommitResult {
    pub restored_turn_count: u32,
    pub restored_file_count: u32,
}

// ── session/metadata/update ──

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadataUpdateParams {
    pub session_id: SessionId,
    pub expected_version: u64,
    #[serde(default, skip_serializing_if = "PatchField::is_missing")]
    #[ts(type = "string | null")]
    pub title: PatchField<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<SessionSettings>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadataUpdateResult {
    pub session: Session,
}

// ── session/cwd/change ──

/// Explicitly migrates the session to another cwd; recomputes
/// permissions/skills/git/memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionCwdChangeParams {
    pub session_id: SessionId,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionCwdChangeResult {
    pub session: Session,
}

// ── session/archive / session/delete ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchiveParams {
    pub session_id: SessionId,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionArchiveResult {
    pub session: Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionDeleteParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionDeleteResult {}

// ── session/turns/list / session/items/list ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionTurnsListParams {
    pub session_id: SessionId,
    #[serde(flatten)]
    pub page: PageParams,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionItemsListParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(flatten)]
    pub page: PageParams,
}

// ── session/compact/start ──

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactStartParams {
    pub session_id: SessionId,
}

// ── session/goal/* ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum GoalIfExists {
    Replace,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalSetParams {
    pub session_id: SessionId,
    pub objective: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    pub if_exists: GoalIfExists,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalSetResult {
    pub goal: Goal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalReadParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalReadResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Goal>,
}

/// Shared params for goal lifecycle transitions; `expectedGoalId` prevents
/// acting on a goal that was replaced concurrently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalTransitionParams {
    pub session_id: SessionId,
    pub expected_goal_id: GoalId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalTransitionResult {
    pub goal: Goal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalClearResult {}
