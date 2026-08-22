use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::conversation::{ItemId, SessionId, SessionTitleState, TurnId, TurnStatus, TurnUsage};
use crate::{
    MessageEditRecordedRecord, SessionContext, TurnContext, TurnKind, TurnSupersededRecord,
    TurnWorkspaceChangeRecordedRecord, TurnWorkspaceCheckpointRecordedRecord,
    TurnWorkspaceRestoreCompletedRecord, TurnWorkspaceRestoreStartedRecord,
};
use devo_protocol::{CollaborationMode, PermissionPreset, StopReason, TurnFailureReason};

/// Stores persistent metadata for one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// The stable session identifier.
    pub id: SessionId,
    /// The absolute rollout path used for canonical JSONL persistence.
    pub rollout_path: PathBuf,
    /// The timestamp when the session was created.
    pub created_at: DateTime<Utc>,
    /// The timestamp of the most recent update to session metadata.
    pub updated_at: DateTime<Utc>,
    /// The timestamp of the most recent user-visible session activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<DateTime<Utc>>,
    /// The session source kind, such as CLI or API.
    pub source: String,
    /// Optional nickname assigned to a spawned sub-agent session.
    pub agent_nickname: Option<String>,
    /// Optional role assigned to a spawned sub-agent session.
    pub agent_role: Option<String>,
    /// Optional canonical agent path associated with a spawned sub-agent.
    pub agent_path: Option<String>,
    /// The model provider last observed for this session.
    pub model_provider: String,
    /// The latest resolved model slug for the session.
    pub model: Option<String>,
    /// The latest selected provider model binding id for the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    /// The logical reasoning effort selection used as the default for the next turn.
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    /// The working directory associated with the session.
    pub cwd: PathBuf,
    /// Additional absolute workspace roots associated with the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    /// The CLI version that created the session.
    pub cli_version: String,
    /// The current best-known title for the session.
    pub title: Option<String>,
    /// The lifecycle state for the current session title.
    pub title_state: SessionTitleState,
    /// The active sandbox policy description for the session.
    pub sandbox_policy: String,
    /// The active approval mode description for the session.
    pub approval_mode: String,
    /// Session override for the absolute effective context window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_window: Option<u64>,
    /// The last observed aggregate token count for the session.
    pub tokens_used: i64,
    /// The first user message stored for preview or title derivation.
    pub first_user_message: Option<String>,
    /// The time when the session was archived, if it has been archived.
    pub archived_at: Option<DateTime<Utc>>,
    /// The git commit SHA associated with the session workspace, if known.
    pub git_sha: Option<String>,
    /// The git branch associated with the session workspace, if known.
    pub git_branch: Option<String>,
    /// The git origin URL associated with the session workspace, if known.
    pub git_origin_url: Option<String>,
    /// The parent session identifier when this session was created by forking.
    pub parent_session_id: Option<SessionId>,
    /// The latest locked session context known for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context: Option<SessionContext>,
    /// The latest turn context snapshot known for this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_turn_context: Option<TurnContext>,
    /// Session-level collaboration mode override, persisted without requiring a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<CollaborationMode>,
    /// Session-level permission preset override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_preset: Option<PermissionPreset>,
    /// The schema version for persisted session metadata.
    pub schema_version: u32,
}

/// Stores persistent metadata for one turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// The stable turn identifier.
    pub id: TurnId,
    /// The session that owns this turn.
    pub session_id: SessionId,
    /// The strictly increasing sequence number within the session.
    pub sequence: u32,
    /// The time when the turn started.
    pub started_at: DateTime<Utc>,
    /// The time when the turn reached a terminal state, if it has completed.
    pub completed_at: Option<DateTime<Utc>>,
    /// The current lifecycle status of the turn.
    pub status: TurnStatus,
    /// The kind of turn (Regular, Review, ManualCompaction, etc.).
    #[serde(default)]
    pub kind: TurnKind,
    /// The logical model selection used for the turn.
    pub model: String,
    /// The selected provider model binding id used for the turn, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    /// The logical reasoning effort selection used for the turn.
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    /// The concrete request model used to execute the turn.
    pub request_model: String,
    /// The concrete request thinking parameter used to execute the turn.
    pub request_thinking: Option<String>,
    /// The estimated input-token count at turn start, when available.
    pub input_token_estimate: Option<u32>,
    /// The authoritative provider token usage, when available.
    pub usage: Option<TurnUsage>,
    /// The provider token usage from the latest model query in this turn.
    ///
    /// Unlike [`Self::usage`], this excludes earlier model queries that may
    /// have been performed for tools, retries, or delegated work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_query_usage: Option<TurnUsage>,
    /// Context-window occupancy snapshot after this turn's latest model query.
    ///
    /// Used by resume/fork/rollback to restore category occupancy at a cut
    /// turn rather than copying the session tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    /// The terminal provider/model stop reason, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// The typed terminal failure reason, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<TurnFailureReason>,
    /// The normalized terminal error for a failed turn, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnError>,
    /// The locked session context used to build the stable request prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_context: Option<SessionContext>,
    /// The turn context used for this user-visible turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_context: Option<TurnContext>,
    /// The schema version for persisted turn metadata.
    pub schema_version: u32,
}

/// Carries a simple text payload for lightweight item kinds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextItem {
    /// The textual payload for the item.
    pub text: String,
}

/// Stores one tool-call request as a persisted item payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallItem {
    /// The stable identifier of the tool call.
    pub tool_call_id: String,
    /// The stable runtime name of the requested tool.
    pub tool_name: String,
    /// The validated JSON input passed to the tool.
    pub input: serde_json::Value,
}

/// Stores one tool-progress update as a persisted item payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolProgressItem {
    /// The tool call this progress record belongs to.
    pub tool_call_id: String,
    /// The human-readable progress message.
    pub message: String,
}

/// Stores one tool result as a persisted item payload.
///
/// This is the generic result type used for all tools *except* `exec_command` and
/// `write_stdin`.  Those two tools use [`CommandExecutionItem`] instead, because
/// they carry extra data (the display command, the original model input for
/// prompt replay) that does not apply to other tools.
///
/// The two types exist side by side — rather than a single type with optional
/// command fields — so that downstream code can match on the [`TurnItem`] enum
/// and immediately know whether it is dealing with a terminal command or a
/// generic tool result, without inspecting optional fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultItem {
    /// The tool call this result belongs to.
    pub tool_call_id: String,
    /// The runtime tool name when it is available at result time.
    pub tool_name: Option<String>,
    /// The normalized structured output returned by the tool.
    pub output: serde_json::Value,
    /// Optional UI-only text for displaying the result without changing replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_content: Option<String>,
    /// Whether the result represents an error outcome.
    pub is_error: bool,
}

/// Stores one unified command execution as a persisted item payload.
///
/// This is a specialised result type for `exec_command` and `write_stdin`.
/// It exists as a separate [`TurnItem`] variant (rather than reusing
/// [`ToolResultItem`]) for two reasons:
///
/// 1. **Display** — the `command` field holds the human-readable shell command
///    (e.g. `"nc 127.0.0.1 4444"`) so the UI can render it as a terminal
///    interaction instead of a generic tool result.
///
/// 2. **Prompt replay** — the `input` field preserves the exact model-supplied
///    JSON input that triggered the command.  During compaction and context
///    reconstruction the runtime needs this to rebuild a faithful prompt,
///    and it is not stored on [`ToolCallItem`] in a form that survives
///    compaction cleanly.
///
/// Every other tool uses [`ToolResultItem`] instead.  See the doc comment on
/// that struct for the trade-off rationale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExecutionItem {
    /// The tool call this command execution belongs to.
    pub tool_call_id: String,
    /// The runtime tool name, usually `exec_command` or `write_stdin`.
    pub tool_name: String,
    /// The display command or terminal interaction text.
    pub command: String,
    /// Original model input for prompt replay.
    pub input: serde_json::Value,
    /// Normalized tool output returned by the command.
    pub output: serde_json::Value,
    /// Whether the result represents an error outcome.
    pub is_error: bool,
}

/// Stores one approval request as a persisted item payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestItem {
    /// The stable approval request identifier.
    pub approval_id: String,
    /// A concise summary of the action awaiting approval.
    pub action_summary: String,
    /// The justification shown to the user.
    pub justification: String,
    /// The resource kind this approval gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// Scope choices offered to the user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_scopes: Vec<String>,
    /// Normalized command pattern used for session-scoped approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_pattern: Option<Vec<String>>,
    /// Suggested command prefix for persistent exec-policy approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_prefix: Option<Vec<String>>,
    /// Optional path related to the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional host related to the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Optional command, URL, query, or other target string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// Stores one approval decision as a persisted item payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecisionItem {
    /// The approval request identifier this decision answers.
    pub approval_id: String,
    /// The decision taken by the user or policy.
    pub decision: String,
    /// The scope attached to the decision.
    pub scope: String,
    /// Authority that produced the decision. Absent on legacy records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_source: Option<devo_protocol::native::item::ApprovalDecisionSource>,
}

/// Enumerates the canonical persisted item kinds used by the conversation model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnItem {
    /// A normal user message.
    UserMessage(TextItem),
    /// Same-turn steering input appended while a turn is active.
    SteerInput(TextItem),
    /// A hook or system-generated prompt fragment.
    HookPrompt(TextItem),
    /// A user-visible assistant message.
    AgentMessage(TextItem),
    /// A planning item emitted by the runtime or model.
    Plan(TextItem),
    /// A reasoning summary or reasoning payload item.
    Reasoning(TextItem),
    /// A tool-call item.
    ToolCall(ToolCallItem),
    /// A tool-progress item.
    ToolProgress(ToolProgressItem),
    /// A terminal tool-result item (every tool *except* exec_command / write_stdin).
    ToolResult(ToolResultItem),
    /// A unified command execution item (only exec_command and write_stdin).
    /// Carries extra fields for display (the human-readable command) and prompt
    /// replay (the original model input).  See [`CommandExecutionItem`].
    CommandExecution(CommandExecutionItem),
    /// An approval-request item.
    ApprovalRequest(ApprovalRequestItem),
    /// An approval-decision item.
    ApprovalDecision(ApprovalDecisionItem),
    /// A web-search item.
    WebSearch(TextItem),
    /// An image-generation item.
    ImageGeneration(TextItem),
    /// A context-compaction summary item.
    ContextCompaction(TextItem),
    /// A turn boundary summary with model name and duration.
    /// title = model name, body = duration_secs:u64 as string
    TurnSummary(TextItem),
}

/// Stores optional high-level progress notes for an item append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worklog {
    /// A human-readable summary of the work performed.
    pub summary: String,
}

/// Stores a normalized turn-scoped error payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnError {
    /// The stable machine-readable error code.
    pub code: String,
    /// The human-readable error message.
    pub message: String,
    /// Optional user-facing next step for recovering from this failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_hint: Option<String>,
}

/// Stores one persisted item record in the canonical journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRecord {
    /// The stable item identifier.
    pub id: ItemId,
    /// The owning session identifier.
    pub session_id: SessionId,
    /// The owning turn identifier.
    pub turn_id: TurnId,
    /// The strictly increasing per-session item sequence number.
    pub seq: u64,
    /// The timestamp when the item record was persisted.
    pub timestamp: DateTime<Utc>,
    /// Optional attempt-placement metadata used by higher-level orchestration.
    pub attempt_placement: Option<i64>,
    /// The turn status observed when this item was appended.
    pub turn_status: Option<TurnStatus>,
    /// Additional related turn identifiers when an item spans turn boundaries.
    pub sibling_turn_ids: Vec<TurnId>,
    /// Input-side item payloads captured for this record.
    pub input_items: Vec<TurnItem>,
    /// Output-side item payloads captured for this record.
    pub output_items: Vec<TurnItem>,
    /// Optional worklog metadata associated with the item.
    pub worklog: Option<Worklog>,
    /// Optional terminal error metadata associated with the item.
    pub error: Option<TurnError>,
    /// The schema version for persisted item records.
    pub schema_version: u32,
}

/// Stores the first canonical line written for a session rollout file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMetaLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The session metadata payload carried by the line.
    pub session: SessionRecord,
}

/// Stores one turn metadata line in the rollout file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The turn metadata payload carried by the line.
    pub turn: TurnRecord,
}

/// Stores one item-record line in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The item-record payload carried by the line.
    pub item: ItemRecord,
}

/// Stores the locked session context once per durable rollout file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionContextUpdatedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The session whose stable context was recorded.
    pub session_id: SessionId,
    /// The locked session context captured for replay.
    pub session_context: SessionContext,
    /// The schema version for persisted session-context metadata.
    pub schema_version: u32,
}

/// Stores one session-title update line in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTitleUpdatedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The session whose title changed.
    pub session_id: SessionId,
    /// The new title value.
    pub title: String,
    /// The new title lifecycle state.
    pub title_state: SessionTitleState,
    /// The previous title value, when there was one.
    pub previous_title: Option<String>,
}

/// Stores one compaction snapshot reference in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSnapshotLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The session that owns the compaction snapshot.
    pub session_id: SessionId,
    /// The turn during which compaction occurred.
    pub turn_id: TurnId,
    /// The summary item that represents the compacted history.
    pub summary_item_id: ItemId,
    /// The pre-existing item ids that remain after compaction, in prompt order.
    pub preserved_item_ids: Vec<ItemId>,
    /// Post-compaction context occupancy, when computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
}

/// Identifies one field of the field-level session settings log
/// (L2-DES-CONV-002 DD-4). Field lines supersede the corresponding
/// `SessionMeta` record fields during replay: the last line per field wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionSettingsField {
    /// Session permission preset override (`SessionRecord::permission_preset`).
    PermissionPreset,
    /// Explicit sandbox profile override. A `PermissionPreset` line clears
    /// this override (the preset re-implies the sandbox); a later
    /// `SandboxProfile` line sets it again.
    SandboxProfile,
    /// Logical model selection (`SessionRecord::model`).
    Model,
    /// Provider model binding id (`SessionRecord::model_binding_id`).
    ModelBindingId,
    /// Reasoning effort selection (`SessionRecord::reasoning_effort_selection`).
    ReasoningEffortSelection,
    /// Session collaboration mode (`SessionRecord::collaboration_mode`).
    CollaborationMode,
}

/// Stores one field-level session settings change in the rollout file.
/// Carried on disk as `InternalRecordV2::SessionSettings`; the legacy line
/// exists so replay can consume it through the same `RolloutLine` channel as
/// every other record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSettingsLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The session whose setting changed.
    pub session_id: SessionId,
    /// The settings field that changed.
    pub field: SessionSettingsField,
    /// The new value, encoded with the field's natural JSON shape (e.g. the
    /// `PermissionPreset` string, a sandbox profile name, a model slug).
    pub value: serde_json::Value,
    /// Per-session logical sequence number ordering settings writes. Assigned
    /// authoritatively by the per-file projector at write time (callers pass
    /// `0` as a placeholder); replay uses line order and treats the epoch as
    /// an ordering/trace annotation for cross-path writes.
    #[serde(default)]
    pub epoch: u64,
}

/// Stores an append-only rollback marker for a session rollout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRollbackLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The session whose in-memory history was rebuilt.
    pub session_id: SessionId,
    /// The retained turn identifiers in prompt/replay order.
    pub retained_turn_ids: Vec<TurnId>,
    /// The retained item identifiers in prompt/replay order.
    pub retained_item_ids: Vec<ItemId>,
    /// The latest retained turn after rollback, if any.
    pub latest_turn_id: Option<TurnId>,
    /// The schema version for persisted rollback metadata.
    pub schema_version: u32,
}

/// Stores one accepted message-edit record in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEditRecordedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The accepted edit payload carried by the line.
    pub record: MessageEditRecordedRecord,
}

/// Stores one turn-superseded marker in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSupersededLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The superseded-turn payload carried by the line.
    pub record: TurnSupersededRecord,
}

/// Stores one workspace-checkpoint record in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkspaceCheckpointRecordedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The workspace-checkpoint payload carried by the line.
    pub record: TurnWorkspaceCheckpointRecordedRecord,
}

/// Stores one workspace-change record in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkspaceChangeRecordedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The workspace-change payload carried by the line.
    pub record: TurnWorkspaceChangeRecordedRecord,
}

/// Stores one workspace-restore-start record in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkspaceRestoreStartedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The restore-start payload carried by the line.
    pub record: TurnWorkspaceRestoreStartedRecord,
}

/// Stores one workspace-restore-completed record in the rollout file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnWorkspaceRestoreCompletedLine {
    /// The time when this rollout line was persisted.
    pub timestamp: DateTime<Utc>,
    /// The restore-completed payload carried by the line.
    pub record: TurnWorkspaceRestoreCompletedRecord,
}

/// Enumerates every canonical line type written to the rollout journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RolloutLine {
    /// Session metadata line.
    SessionMeta(Box<SessionMetaLine>),
    /// Turn metadata line.
    Turn(Box<TurnLine>),
    /// Item record line.
    Item(ItemLine),
    /// Session-title update line.
    SessionTitleUpdated(SessionTitleUpdatedLine),
    /// Locked session-context update line.
    SessionContextUpdated(Box<SessionContextUpdatedLine>),
    /// Compaction snapshot line.
    CompactionSnapshot(Box<CompactionSnapshotLine>),
    /// Accepted message-edit record line.
    MessageEditRecorded(Box<MessageEditRecordedLine>),
    /// Turn-superseded marker line.
    TurnSuperseded(Box<TurnSupersededLine>),
    /// Workspace-checkpoint record line.
    TurnWorkspaceCheckpointRecorded(Box<TurnWorkspaceCheckpointRecordedLine>),
    /// Workspace-change record line.
    TurnWorkspaceChangeRecorded(Box<TurnWorkspaceChangeRecordedLine>),
    /// Workspace-restore-start record line.
    TurnWorkspaceRestoreStarted(Box<TurnWorkspaceRestoreStartedLine>),
    /// Workspace-restore-completed record line.
    TurnWorkspaceRestoreCompleted(Box<TurnWorkspaceRestoreCompletedLine>),
    /// Session rollback marker line.
    SessionRollback(Box<SessionRollbackLine>),
    /// Field-level session settings change line.
    SessionSettings(SessionSettingsLine),
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::Model;
    use crate::conversation::{ItemId, SessionId, SessionTitleState, TurnId, TurnStatus};

    // ── SessionRecord ──────────────────────────────────────────

    #[test]
    fn session_record_supports_unset_title() {
        let session = SessionRecord {
            id: SessionId::new(),
            rollout_path: "rollout.jsonl".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Some(Utc::now()),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test".into(),
            model: None,
            model_binding_id: None,
            reasoning_effort_selection: None,
            cwd: ".".into(),
            additional_directories: Vec::new(),
            cli_version: "0.1.0".into(),
            title: None,
            title_state: SessionTitleState::Unset,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        };

        assert!(session.title.is_none());
        assert_eq!(session.title_state, SessionTitleState::Unset);
    }

    #[test]
    fn session_record_with_fork_parent() {
        let parent_id = SessionId::new();
        let session = SessionRecord {
            parent_session_id: Some(parent_id),
            ..make_test_session()
        };
        let json = serde_json::to_string(&session).expect("serialize");
        let restored: SessionRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.parent_session_id, Some(parent_id));
    }

    #[test]
    fn session_record_serde_roundtrip() {
        let session = make_test_session();
        let json = serde_json::to_string(&session).expect("serialize");
        let restored: SessionRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(session, restored);
    }

    #[test]
    fn session_record_reads_legacy_thinking_field() {
        let mut expected = make_test_session();
        expected.reasoning_effort_selection = Some("high".into());
        let mut value = serde_json::to_value(&expected).expect("serialize value");
        let object = value.as_object_mut().expect("session json object");
        object.remove("reasoning_effort_selection");
        object.insert("thinking".to_string(), serde_json::json!("high"));

        let restored: SessionRecord = serde_json::from_value(value).expect("deserialize legacy");
        assert_eq!(restored, expected);

        let serialized = serde_json::to_value(&restored).expect("serialize restored");
        assert_eq!(serialized["reasoning_effort_selection"], "high");
        assert_eq!(serialized.get("thinking"), None);
    }

    // ── TurnRecord ────────────────────────────────────────────

    #[test]
    fn turn_record_starts_in_pending_or_running() {
        let turn = make_test_turn(TurnStatus::Running);
        assert!(matches!(turn.status, TurnStatus::Running));
    }

    #[test]
    fn turn_record_serde_roundtrip() {
        let turn = TurnRecord {
            status: TurnStatus::Failed,
            error: Some(TurnError {
                code: "PROVIDER_SERVER_ERROR".into(),
                message: "provider request failed".into(),
                recovery_hint: None,
            }),
            schema_version: 4,
            ..make_test_turn(TurnStatus::Running)
        };
        let json = serde_json::to_string(&turn).expect("serialize");
        let restored: TurnRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(turn, restored);
    }

    #[test]
    fn turn_record_reads_legacy_record_without_terminal_error() {
        let expected = make_test_turn(TurnStatus::Completed);
        let mut value = serde_json::to_value(&expected).expect("serialize value");
        value
            .as_object_mut()
            .expect("turn json object")
            .remove("error");

        let restored: TurnRecord = serde_json::from_value(value).expect("deserialize legacy");
        assert_eq!(restored, expected);
    }

    #[test]
    fn turn_record_reads_legacy_thinking_field() {
        let mut expected = make_test_turn(TurnStatus::Running);
        expected.reasoning_effort_selection = Some("high".into());
        let mut value = serde_json::to_value(&expected).expect("serialize value");
        let object = value.as_object_mut().expect("turn json object");
        object.remove("reasoning_effort_selection");
        object.insert("thinking".to_string(), serde_json::json!("high"));

        let restored: TurnRecord = serde_json::from_value(value).expect("deserialize legacy");
        assert_eq!(restored, expected);

        let serialized = serde_json::to_value(&restored).expect("serialize restored");
        assert_eq!(serialized["reasoning_effort_selection"], "high");
        assert_eq!(serialized.get("thinking"), None);
    }

    #[test]
    fn turn_record_roundtrips_context_occupancy() {
        use pretty_assertions::assert_eq;

        let occupancy = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 5_000,
            /*tools_builtin*/ 20_000, /*tools_mcp*/ 15_000, /*conversation*/ 50_000,
        );
        let mut turn = make_test_turn(TurnStatus::Completed);
        turn.context_occupancy = Some(occupancy.clone());
        let json = serde_json::to_string(&turn).expect("serialize");
        let restored: TurnRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.context_occupancy, Some(occupancy));
    }

    #[test]
    fn turn_record_reads_legacy_record_without_context_occupancy() {
        let expected = make_test_turn(TurnStatus::Completed);
        let mut value = serde_json::to_value(&expected).expect("serialize value");
        value
            .as_object_mut()
            .expect("turn json object")
            .remove("context_occupancy");

        let restored: TurnRecord = serde_json::from_value(value).expect("deserialize legacy");
        assert_eq!(restored.context_occupancy, None);
        assert_eq!(restored, expected);
    }

    #[test]
    fn turn_cannot_transition_from_completed_to_running() {
        // Per L3-BEH-CORE-001 §4: Completed → Running is ILLEGAL
        let completed = TurnStatus::Completed;
        assert!(!matches!(completed, TurnStatus::Running));
        // Terminal states are final
        assert!(is_terminal_turn_status(TurnStatus::Completed));
        assert!(is_terminal_turn_status(TurnStatus::Failed));
        assert!(is_terminal_turn_status(TurnStatus::Interrupted));
    }

    #[test]
    fn turn_terminal_states_are_distinct() {
        let terminal = [
            TurnStatus::Completed,
            TurnStatus::Failed,
            TurnStatus::Interrupted,
        ];
        // All terminal states are different
        for i in 0..terminal.len() {
            for j in (i + 1)..terminal.len() {
                assert_ne!(terminal[i], terminal[j]);
            }
        }
    }

    // ── ItemRecord ────────────────────────────────────────────

    #[test]
    fn item_record_carries_turn_and_session_identity() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item = ItemRecord {
            id: ItemId::new(),
            session_id,
            turn_id,
            seq: 1,
            timestamp: Utc::now(),
            attempt_placement: None,
            turn_status: Some(TurnStatus::Running),
            sibling_turn_ids: Vec::new(),
            input_items: vec![TurnItem::ToolCall(ToolCallItem {
                tool_call_id: "call-1".into(),
                tool_name: "shell_command".into(),
                input: serde_json::json!({"command":"pwd"}),
            })],
            output_items: vec![TurnItem::AgentMessage(TextItem {
                text: "running".into(),
            })],
            worklog: None,
            error: None,
            schema_version: 1,
        };

        assert_eq!(item.session_id, session_id);
        assert_eq!(item.turn_id, turn_id);
    }

    #[test]
    fn item_record_with_error_preserves_error_details() {
        let item = ItemRecord {
            error: Some(TurnError {
                code: "TOOL_EXECUTION_FAILED".into(),
                message: "command exited with code 1".into(),
                recovery_hint: None,
            }),
            ..make_test_item()
        };
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: ItemRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(
            restored.error.as_ref().unwrap().code,
            "TOOL_EXECUTION_FAILED"
        );
    }

    #[test]
    fn item_record_with_sibling_turns() {
        let sibling = TurnId::new();
        let item = ItemRecord {
            sibling_turn_ids: vec![sibling],
            ..make_test_item()
        };
        assert_eq!(item.sibling_turn_ids.len(), 1);
        assert_eq!(item.sibling_turn_ids[0], sibling);
    }

    #[test]
    fn item_record_serde_roundtrip() {
        let item = make_test_item();
        let json = serde_json::to_string(&item).expect("serialize");
        let restored: ItemRecord = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(item, restored);
    }

    // ── TurnItem enum ─────────────────────────────────────────

    #[test]
    fn turn_item_all_variants_roundtrip() {
        let variants = vec![
            TurnItem::UserMessage(TextItem {
                text: "hello".into(),
            }),
            TurnItem::SteerInput(TextItem {
                text: "steer".into(),
            }),
            TurnItem::AgentMessage(TextItem {
                text: "response".into(),
            }),
            TurnItem::Reasoning(TextItem {
                text: "think".into(),
            }),
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id: "t1".into(),
                tool_name: "read".into(),
                input: serde_json::json!({"path": "a.rs"}),
            }),
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "t1".into(),
                tool_name: Some("read".into()),
                output: serde_json::json!({"content": "fn main()"}),
                display_content: None,
                is_error: false,
            }),
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id: "t2".into(),
                tool_name: "exec_command".into(),
                command: "ls".into(),
                input: serde_json::json!({"command": "ls"}),
                output: serde_json::json!({"stdout": "src"}),
                is_error: false,
            }),
            TurnItem::ApprovalRequest(ApprovalRequestItem {
                approval_id: "a1".into(),
                action_summary: "Run command".into(),
                justification: "need to".into(),
                resource: Some("ShellExec".into()),
                available_scopes: vec!["Once".into(), "Session".into()],
                command_pattern: None,
                command_prefix: None,
                path: None,
                host: None,
                target: Some("npm install".into()),
            }),
            TurnItem::ApprovalDecision(ApprovalDecisionItem {
                approval_id: "a1".into(),
                decision: "Allow".into(),
                scope: "Once".into(),
                decision_source: None,
            }),
            TurnItem::Plan(TextItem { text: "[]".into() }),
            TurnItem::ContextCompaction(TextItem {
                text: "summary".into(),
            }),
            TurnItem::TurnSummary(TextItem { text: "0".into() }),
            TurnItem::WebSearch(TextItem {
                text: "results".into(),
            }),
            TurnItem::HookPrompt(TextItem {
                text: "hook".into(),
            }),
        ];

        for variant in variants {
            let json = serde_json::to_string(&variant).expect("serialize");
            let restored: TurnItem = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(variant, restored, "roundtrip failed for variant");
        }
    }

    // ── RolloutLine enum ──────────────────────────────────────

    #[test]
    fn rollout_line_all_variants_roundtrip() {
        let session = make_test_session();
        let turn = make_test_turn(TurnStatus::Running);
        let item = make_test_item();

        let variants: Vec<RolloutLine> = vec![
            RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: Utc::now(),
                session: session.clone(),
            })),
            RolloutLine::Turn(Box::new(TurnLine {
                timestamp: Utc::now(),
                turn: turn.clone(),
            })),
            RolloutLine::Item(ItemLine {
                timestamp: Utc::now(),
                item: item.clone(),
            }),
            RolloutLine::SessionTitleUpdated(SessionTitleUpdatedLine {
                timestamp: Utc::now(),
                session_id: session.id,
                title: "New Title".into(),
                title_state: SessionTitleState::Provisional,
                previous_title: Some("Old Title".into()),
            }),
            RolloutLine::SessionContextUpdated(Box::new(SessionContextUpdatedLine {
                timestamp: Utc::now(),
                session_id: session.id,
                session_context: SessionContext {
                    base_instructions: "base".into(),
                    available_skills: None,
                    workspace_instructions: None,
                    locked_agents_snapshot: None,
                    environment: crate::EnvironmentContext {
                        cwd: ".".into(),
                        shell: "bash".into(),
                        current_date: "2026-07-08".into(),
                        timezone: "UTC".into(),
                    },
                    language: crate::LanguageContext::default(),
                    persona: crate::Persona::Default,
                    model: Model {
                        slug: "test-model".into(),
                        ..Model::default()
                    },
                    reasoning_effort_selection: None,
                    reasoning_effort: None,
                    system_prompt_mode: crate::SystemPromptMode::CodingAgent,
                },
                schema_version: 1,
            })),
            RolloutLine::CompactionSnapshot(Box::new(CompactionSnapshotLine {
                timestamp: Utc::now(),
                session_id: session.id,
                turn_id: turn.id,
                summary_item_id: item.id,
                preserved_item_ids: vec![item.id],
                context_occupancy: None,
            })),
            RolloutLine::SessionRollback(Box::new(SessionRollbackLine {
                timestamp: Utc::now(),
                session_id: session.id,
                retained_turn_ids: vec![turn.id],
                retained_item_ids: vec![item.id],
                latest_turn_id: Some(turn.id),
                schema_version: 1,
            })),
        ];

        for variant in variants {
            let json = serde_json::to_string(&variant).expect("serialize");
            let restored: RolloutLine = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                variant, restored,
                "roundtrip failed for RolloutLine variant"
            );
        }
    }

    #[test]
    fn rollout_line_session_meta_carries_full_session_record() {
        let session = make_test_session();
        let line = RolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: Utc::now(),
            session: session.clone(),
        }));
        let json = serde_json::to_string(&line).expect("serialize");
        let restored: RolloutLine = serde_json::from_str(&json).expect("deserialize");
        if let RolloutLine::SessionMeta(meta) = restored {
            assert_eq!(meta.session.title, session.title);
            assert_eq!(meta.session.schema_version, session.schema_version);
        } else {
            panic!("expected SessionMeta");
        }
    }

    // ── Tool record coverage ──────────────────────────────────

    #[test]
    fn tool_call_item_preserves_input_schema() {
        let call = ToolCallItem {
            tool_call_id: "call-x".into(),
            tool_name: "apply_patch".into(),
            input: serde_json::json!({"patch": "@@ -1 +1 @@"}),
        };
        let json = serde_json::to_string(&call).expect("serialize");
        let restored: ToolCallItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.tool_call_id, "call-x");
        assert_eq!(restored.tool_name, "apply_patch");
    }

    #[test]
    fn tool_result_item_marks_is_error() {
        let err_result = ToolResultItem {
            tool_call_id: "c1".into(),
            tool_name: Some("shell".into()),
            output: serde_json::json!({"stderr": "not found"}),
            display_content: Some("Error: not found".into()),
            is_error: true,
        };
        assert!(err_result.is_error);

        let ok_result = ToolResultItem {
            tool_call_id: "c2".into(),
            tool_name: Some("read".into()),
            output: serde_json::json!({"content": "ok"}),
            display_content: None,
            is_error: false,
        };
        assert!(!ok_result.is_error);
    }

    #[test]
    fn command_execution_item_preserves_display_command() {
        let cmd = CommandExecutionItem {
            tool_call_id: "c3".into(),
            tool_name: "exec_command".into(),
            command: "curl -s http://localhost:3000".into(),
            input: serde_json::json!({"command": "curl -s http://localhost:3000"}),
            output: serde_json::json!({"stdout": "OK"}),
            is_error: false,
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        let restored: CommandExecutionItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.command, "curl -s http://localhost:3000");
    }

    // ── Approval records ──────────────────────────────────────

    #[test]
    fn approval_request_item_with_all_scopes() {
        let req = ApprovalRequestItem {
            approval_id: "apr-1".into(),
            action_summary: "Execute: npm install".into(),
            justification: "Need to install dependencies".into(),
            resource: Some("ShellExec".into()),
            available_scopes: vec![
                "Once".into(),
                "Turn".into(),
                "Session".into(),
                "PathPrefix".into(),
                "CommandPrefix".into(),
            ],
            command_pattern: Some(vec!["npm".into(), "install".into()]),
            command_prefix: Some(vec!["npm".into(), "install".into()]),
            path: Some("/workspace".into()),
            host: None,
            target: Some("npm install".into()),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let restored: ApprovalRequestItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.available_scopes.len(), 5);
    }

    #[test]
    fn approval_decision_covers_allow_and_deny() {
        for (decision, scope) in [("Allow", "Once"), ("Allow", "Session"), ("Deny", "Once")] {
            let dec = ApprovalDecisionItem {
                approval_id: "apr-1".into(),
                decision: decision.into(),
                scope: scope.into(),
                decision_source: None,
            };
            assert_eq!(dec.decision, decision);
            assert_eq!(dec.scope, scope);
        }
    }

    // ── Worklog and TurnError ─────────────────────────────────

    #[test]
    fn worklog_serde_roundtrip() {
        let wl = Worklog {
            summary: "Fixed 3 files".into(),
        };
        let json = serde_json::to_string(&wl).expect("serialize");
        let restored: Worklog = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.summary, "Fixed 3 files");
    }

    #[test]
    fn turn_error_codes_cover_recovery_context() {
        let errors = vec![
            TurnError {
                code: "CONTEXT_LIMIT_EXCEEDED".into(),
                message: "Too many tokens".into(),
                recovery_hint: None,
            },
            TurnError {
                code: "MODEL_RESOLUTION_FAILED".into(),
                message: "No valid binding".into(),
                recovery_hint: None,
            },
            TurnError {
                code: "PROVIDER_RATE_LIMITED".into(),
                message: "Retry after 30s".into(),
                recovery_hint: None,
            },
            TurnError {
                code: "PERSISTENCE_FAILURE".into(),
                message: "Disk full".into(),
                recovery_hint: None,
            },
            TurnError {
                code: "TOOL_EXECUTION_FAILED".into(),
                message: "exit code 1".into(),
                recovery_hint: None,
            },
            TurnError {
                code: "APPROVAL_TIMEOUT".into(),
                message: "User did not respond".into(),
                recovery_hint: None,
            },
        ];
        for err in &errors {
            let json = serde_json::to_string(err).expect("serialize");
            let restored: TurnError = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(err, &restored);
        }
    }

    // ── CompactionSnapshotLine ────────────────────────────────

    #[test]
    fn compaction_snapshot_preserves_preserved_items() {
        let snapshot = CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            summary_item_id: ItemId::new(),
            preserved_item_ids: vec![ItemId::new(), ItemId::new(), ItemId::new()],
            context_occupancy: Some(
                devo_protocol::native::item::ContextOccupancy::from_category_tokens(
                    /*context_window_tokens*/ 80_000, /*base*/ 8_000,
                    /*skills*/ 2_000, /*tools_builtin*/ 10_000, /*tools_mcp*/ 0,
                    /*conversation*/ 20_000,
                ),
            ),
        };
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let restored: CompactionSnapshotLine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.preserved_item_ids.len(), 3);
        assert_eq!(restored.context_occupancy, snapshot.context_occupancy);
    }

    #[test]
    fn compaction_snapshot_reads_legacy_without_context_occupancy() {
        let snapshot = CompactionSnapshotLine {
            timestamp: Utc::now(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            summary_item_id: ItemId::new(),
            preserved_item_ids: vec![ItemId::new()],
            context_occupancy: None,
        };
        let mut value = serde_json::to_value(&snapshot).expect("serialize");
        value
            .as_object_mut()
            .expect("snapshot object")
            .remove("context_occupancy");
        let restored: CompactionSnapshotLine =
            serde_json::from_value(value).expect("deserialize legacy");
        assert_eq!(restored.context_occupancy, None);
    }

    // ── Helpers ───────────────────────────────────────────────

    fn make_test_session() -> SessionRecord {
        SessionRecord {
            id: SessionId::new(),
            rollout_path: "test.jsonl".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Some(Utc::now()),
            source: "test".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test-provider".into(),
            model: Some("test-model".into()),
            model_binding_id: Some("test-binding".into()),
            reasoning_effort_selection: None,
            cwd: "/tmp/test".into(),
            additional_directories: Vec::new(),
            cli_version: "0.1.0".into(),
            title: Some("Test Session".into()),
            title_state: SessionTitleState::Provisional,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 100,
            first_user_message: Some("hello".into()),
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        }
    }

    fn make_test_turn(status: TurnStatus) -> TurnRecord {
        TurnRecord {
            id: TurnId::new(),
            session_id: SessionId::new(),
            sequence: 0,
            started_at: Utc::now(),
            completed_at: None,
            status,
            kind: crate::TurnKind::Regular,
            model: "test-model".into(),
            model_binding_id: Some("test-binding".into()),
            reasoning_effort_selection: None,
            request_model: "test-model".into(),
            request_thinking: None,
            input_token_estimate: Some(100),
            usage: None,
            latest_query_usage: None,
            context_occupancy: None,
            stop_reason: None,
            failure_reason: None,
            error: None,
            session_context: None,
            turn_context: None,
            schema_version: 2,
        }
    }

    fn make_test_item() -> ItemRecord {
        ItemRecord {
            id: ItemId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            seq: 0,
            timestamp: Utc::now(),
            attempt_placement: None,
            turn_status: Some(TurnStatus::Running),
            sibling_turn_ids: Vec::new(),
            input_items: vec![TurnItem::UserMessage(TextItem {
                text: "test".into(),
            })],
            output_items: vec![TurnItem::AgentMessage(TextItem { text: "ok".into() })],
            worklog: None,
            error: None,
            schema_version: 1,
        }
    }

    fn is_terminal_turn_status(s: TurnStatus) -> bool {
        matches!(
            s,
            TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Interrupted
        )
    }
}
