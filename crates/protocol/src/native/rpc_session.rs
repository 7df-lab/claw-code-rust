//! Params/result types for session-domain methods (`session/*`,
//! `session/goal/*`). Truth source: `devo-api-design/01-native-api.md` §4.2/§4.5.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::goal::Goal;
use super::goal::GoalStatus;
use super::ids::GoalId;
use super::ids::ItemId;
use super::ids::RestorePlanId;
use super::ids::SessionId;
use super::ids::TurnId;
use super::item::ItemEnvelope;
use super::item::UserInput;
use super::model::ModelBinding;
use super::model::PermissionProfile;
use super::page::Page;
use super::page::PageParams;
use super::patch::PatchField;
use super::session::Session;

// ── session/interrupt ──

/// The unit of work addressed by Native `session/interrupt`.
///
/// `Command` intentionally does not require a session identifier: the TUI can
/// start a `!` command before a session exists, while the server still scopes
/// the process to the requesting connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(
    tag = "scope",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionInterruptScope {
    Session {
        #[schemars(rename = "sessionId")]
        #[ts(rename = "sessionId")]
        session_id: SessionId,
    },
    Task {
        #[schemars(rename = "itemId")]
        #[ts(rename = "itemId")]
        item_id: ItemId,
    },
    Command {
        #[schemars(rename = "processId")]
        #[ts(rename = "processId")]
        process_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionInterruptParams {
    pub scope: SessionInterruptScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionInterruptResult {
    pub interrupted: bool,
}

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
    /// Optimistic concurrency guard compared against `Session.version`;
    /// `0` skips the check (transitional escape for clients that do not
    /// track versions yet — L2-DES-CONV-002 DD-2).
    pub expected_version: u64,
    #[serde(default, skip_serializing_if = "PatchField::is_missing")]
    #[ts(type = "string | null")]
    pub title: PatchField<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelBinding>,
    /// Provider model binding id selecting the configured binding for future
    /// turns, when the session resolves models through provider bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<SessionSettingsPatch>,
}

/// Patch-shaped settings payload: every field is optional and only present
/// fields are changed (L2-DES-CONV-002 DD-10). Unlike `SessionSettings` —
/// the full current-state snapshot — `permission_profile` is optional here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile: Option<PermissionProfile>,
    /// Raw reasoning effort selection (e.g. `"high"`, `"xhigh"`), including
    /// the toggle keywords `"enabled"`/`"disabled"` for toggle-style models;
    /// the typed `ReasoningEffort` enum cannot express those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// ACP-style session mode id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_profile: Option<String>,
    /// Global compaction threshold preference (persisted to `config.toml`;
    /// applied per session clamped to the model's context window).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadataUpdateResult {
    pub session: Session,
    /// Whether the update was also delivered to the currently running turn's
    /// override channel (L2-DES-CONV-002 DD-2). `false` means the change
    /// takes effect from the next turn. The session `version` doubles as the
    /// settings epoch.
    #[serde(default)]
    pub applied_to_active_turn: bool,
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

// ── session/goal/update (ratified #3) ──

/// In-place edit patch for the session's current goal. Only present fields
/// change; `tokenBudget` uses `PatchField` (`Null` is rejected — clearing a
/// budget is not in the vocabulary, matching title clearing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct GoalPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    /// Direct status set for the edit flow; only `active`/`paused`/
    /// `completed` are accepted (system-computed statuses like
    /// `budgetLimited` are rejected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GoalStatus>,
    #[serde(default, skip_serializing_if = "PatchField::is_missing")]
    #[ts(type = "number | null")]
    pub token_budget: super::patch::PatchField<i64>,
}

/// In-place edit preserving the goal's id, usage stats, and continuation
/// linkage (ratified #3) — unlike `session/goal/set` replace semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalUpdateParams {
    pub session_id: SessionId,
    /// Precondition on the goal being edited; a mismatch (or no active goal)
    /// fails with `GOAL_NOT_FOUND`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_goal_id: Option<GoalId>,
    pub patch: GoalPatch,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionGoalUpdateResult {
    pub goal: Goal,
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

// ── session/message/edit (ratified #10; L1-REQ-CONV-005) ──

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum MessageEditMode {
    #[default]
    Normal,
    QueuedOnly,
}

/// Workspace restoration policy for the superseded turn's file changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum MessageEditWorkspaceRestore {
    #[default]
    Safe,
    Skip,
    ConfiguredRestore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum MessageEditState {
    Accepted,
    Queued,
}

/// Edits the immediately-previous eligible user message. The accepted edit
/// is a new revision of the same `UserMessage` item — never an in-place
/// mutation (L1-REQ-CONV-005).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageEditParams {
    pub session_id: SessionId,
    /// The user message item being edited.
    pub item_id: ItemId,
    /// Optimistic concurrency on the item revision; `0` skips the check
    /// (the L2-DES-CONV-002 DD-2 convention), mismatch fails
    /// `VERSION_CONFLICT`.
    pub expected_revision: u32,
    pub content: Vec<UserInput>,
    #[serde(default)]
    pub mode: MessageEditMode,
    #[serde(default)]
    pub workspace_restore: MessageEditWorkspaceRestore,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageEditResult {
    /// The superseding `UserMessage` item (revision + 1).
    pub item: ItemEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_turn_id: Option<TurnId>,
    pub edit_state: MessageEditState,
}
