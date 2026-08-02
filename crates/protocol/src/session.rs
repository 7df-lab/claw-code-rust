use std::collections::HashMap;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::ItemId;
use crate::ReasoningEffort;
use crate::SessionId;
use crate::SessionTitleState;
use crate::TurnId;
use crate::TurnUsage;
use crate::parse_command::ParsedCommand;
use crate::permissions::PermissionPreset;
use crate::protocol::FileChange;
use crate::turn::CollaborationMode;
use crate::turn::TurnMetadata;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionRuntimeStatus {
    Idle,
    ActiveTurn,
    WaitingClient,
    Archived,
    Unloaded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionMetadata {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub title: Option<String>,
    pub title_state: SessionTitleState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_nickname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    pub ephemeral: bool,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    #[serde(default)]
    pub total_tokens: usize,
    pub total_cache_creation_tokens: usize,
    pub total_cache_read_tokens: usize,
    pub prompt_token_estimate: usize,
    /// Structured usage for the latest completed model query.
    ///
    /// Context length in the UI is based on this latest-query snapshot (display
    /// total: provider `total_tokens` when available, otherwise
    /// `input_tokens + output_tokens`). It is **not** the session cumulative
    /// `total_input_tokens` / `total_output_tokens` / `total_tokens`.
    ///
    /// Refreshed on every completed model invoke so resume and clients can show
    /// the latest completed-query usage rather than rolling session totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_query_usage: Option<TurnUsage>,
    /// Compatibility scalar for current context-window occupancy.
    ///
    /// Prefer [`Self::last_query_usage`] when available for provider query
    /// accounting. Prefer [`Self::last_context_occupancy`] when available for
    /// window occupancy (including category breakdown).
    ///
    /// When `last_context_occupancy` is set, this should equal
    /// `last_context_occupancy.total_tokens` (a derived projection, not a
    /// second independent counter). When occupancy is absent (for example
    /// before the first completed query, or older resume payloads), this may
    /// still carry a legacy scalar from `last_query_usage.display_total_tokens()`
    /// or remain `0`.
    ///
    /// Kept for TUI status, resume, and existing `SessionMetadata` consumers
    /// until they migrate to reading `last_context_occupancy.total_tokens`.
    /// While a turn is in flight, the UI may temporarily fall back to the live
    /// prompt estimate instead.
    pub last_query_total_tokens: usize,
    /// Source of truth for context-window occupancy and category shares.
    ///
    /// Includes `total_tokens`, effective `context_window_tokens`, and
    /// category breakdown (`base`, `skills`, `toolsBuiltin`, `toolsMcp`,
    /// `conversation`). New clients should use this for occupancy reads
    /// (`context/usage/read`, `context/usageUpdated`) rather than inventing a
    /// second total.
    ///
    /// Refreshed on completed turn queries (categories scaled so they sum to
    /// the latest provider query display total) and immediately after
    /// successful compaction (conversation bucket replaced; other buckets
    /// reused). When present, [`Self::last_query_total_tokens`] should mirror
    /// `total_tokens` as a compatibility projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_context_occupancy: Option<crate::canonical::item::ContextOccupancy>,
    pub status: SessionRuntimeStatus,
    /// Collaboration mode restored from the latest completed turn context.
    ///
    /// Defaults to [`CollaborationMode::Build`] for older payloads that omit
    /// the field. Resume and session-switch clients use this to rehydrate the
    /// composer (Plan vs Build) without waiting for the next turn start.
    #[serde(default)]
    pub collaboration_mode: CollaborationMode,
    /// Session override for the absolute effective context window (tokens).
    ///
    /// When set, clients should treat this as the effective Settings Hub /
    /// compaction-threshold value on resume (clamped to the model
    /// `context_window`). Absent means use the model-derived
    /// `effective_context_window()`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_context_window: Option<u64>,
    /// Permission preset restored from session durable metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_preset: Option<PermissionPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionStartParams {
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    pub ephemeral: bool,
    pub title: Option<String>,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionStartResult {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionResumeParams {
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionResumeResult {
    pub session: SessionMetadata,
    pub latest_turn: Option<TurnMetadata>,
    pub loaded_item_count: u64,
    pub history_items: Vec<SessionHistoryItem>,
    /// Pending turn input texts queued for the next turn.
    pub pending_texts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionHistoryItemKind {
    User,
    Assistant,
    Reasoning,
    ToolCall,
    ToolResult,
    CommandExecution,
    Error,
    TurnSummary,
    ContextCompaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionPlanStepStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionPlanStep {
    pub text: String,
    pub status: SessionPlanStepStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionHistoryMetadata {
    Explored {
        actions: Vec<ParsedCommand>,
    },
    Edited {
        changes: HashMap<PathBuf, FileChange>,
    },
    PlanUpdate {
        explanation: Option<String>,
        steps: Vec<SessionPlanStep>,
    },
    /// Markdown Proposed Plan from Plan mode (not the `update_plan` checklist).
    ///
    /// Body text lives on [`SessionHistoryItem::body`]. Resume uses this to
    /// rebuild the Proposed Plan cell and reopen Implement/Revise actions.
    ProposedPlan,
    /// Collaboration mode for a restored turn-summary row (`▣ PLAN · …`).
    ///
    /// Defaults to [`CollaborationMode::Build`] when older payloads omit the
    /// field. Resume uses this so Plan turns do not render as Build.
    TurnSummary {
        #[serde(default)]
        collaboration_mode: CollaborationMode,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionHistoryToolIo {
    pub tool_name: String,
    pub input: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    /// Optional human-facing rendering of `output`.
    ///
    /// Session history keeps the canonical output for replay/debugging and this
    /// separate text for compact display surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionHistoryItem {
    pub tool_call_id: Option<String>,
    pub kind: SessionHistoryItemKind,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_io: Option<SessionHistoryToolIo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionHistoryMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl SessionHistoryItem {
    pub fn new(
        tool_call_id: Option<String>,
        kind: SessionHistoryItemKind,
        title: String,
        body: String,
    ) -> Self {
        Self {
            tool_call_id,
            kind,
            title,
            body,
            tool_io: None,
            metadata: None,
            duration_ms: None,
        }
    }

    pub fn with_tool_io(mut self, tool_io: SessionHistoryToolIo) -> Self {
        self.tool_io = Some(tool_io);
        self
    }

    pub fn with_metadata(mut self, metadata: SessionHistoryMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionTitleUpdateParams {
    pub session_id: SessionId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionTitleUpdateResult {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionMetadataUpdateParams {
    pub session_id: SessionId,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    #[serde(default, alias = "thinking", skip_serializing_if = "Option::is_none")]
    pub reasoning_effort_selection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<CollaborationMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionMetadataUpdateResult {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionCompactParams {
    pub session_id: SessionId,
}

/// Persists a global compaction token limit and hot-applies it to live sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactionUpdateParams {
    pub session_id: SessionId,
    /// Absolute global compaction threshold in tokens. Written to user
    /// `config.toml` as `compaction_token_limit`. Values above a session model's
    /// `context_window` are clamped when applied to that session.
    pub effective_context_window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactionUpdateResult {
    pub session_id: SessionId,
    /// Applied absolute window for this session after clamp.
    pub effective_context_window: u64,
    /// Model default (`effective_context_window()`, clamped) used as recommended.
    pub recommended_token_limit: u64,
    /// Raw model context window used for clamp.
    pub context_window_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionForkParams {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_turn_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionForkResult {
    pub session: SessionMetadata,
    pub forked_from_session_id: SessionId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionRollbackMode {
    #[default]
    ThroughUserTurn,
    BeforeUserTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionRollbackParams {
    pub session_id: SessionId,
    pub user_turn_index: u32,
    #[serde(default)]
    pub mode: SessionRollbackMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionRollbackResult {
    pub session: SessionMetadata,
    pub latest_turn: Option<TurnMetadata>,
    pub loaded_item_count: u64,
    pub history_items: Vec<SessionHistoryItem>,
    pub pending_texts: Vec<String>,
}

// ── Session Subscribe (L3-BEH-PROTOCOL-001 B3) ───────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSubscribeParams {
    pub session_id: SessionId,
    #[serde(default)]
    pub from_sequence: Option<u64>,
    #[serde(default)]
    pub event_filter: Option<Vec<String>>,
    #[serde(default)]
    pub projection: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct SessionSubscribeResult {
    pub subscription_id: String,
    pub session_id: SessionId,
    pub next_sequence: u64,
    pub session_snapshot: Option<serde_json::Value>,
}

// ── Message Edit Previous (L3-BEH-PROTOCOL-001 B11) ──────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MessageEditPreviousParams {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_message_id: Option<ItemId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_target_message_id: Option<ItemId>,
    #[serde(default, alias = "replacement_content_parts")]
    pub edited_content_parts: Vec<serde_json::Value>,
    #[serde(default, alias = "replacement_mentions")]
    pub edited_mentions: Vec<serde_json::Value>,
    #[serde(default)]
    pub edit_mode: EditMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_edit_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_restore_policy: Option<MessageEditWorkspaceRestorePolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS, Default)]
#[serde(rename_all = "snake_case")]
pub enum EditMode {
    #[default]
    Normal,
    QueuedOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum MessageEditWorkspaceRestorePolicy {
    Safe,
    Skip,
    ConfiguredRestore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct MessageEditPreviousResult {
    pub edit_id: String,
    pub replacement_message_id: ItemId,
    pub replacement_turn_id: Option<TurnId>,
    pub edit_state: String,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::SessionTitleState;

    #[test]
    fn session_metadata_roundtrips_with_model_and_reasoning_effort_selection() {
        let metadata = SessionMetadata {
            session_id: SessionId::new(),
            cwd: "/tmp".into(),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Test".to_string()),
            title_state: SessionTitleState::Unset,
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: false,
            model: Some("test-model".to_string()),
            model_binding_id: Some("test-binding".to_string()),
            reasoning_effort_selection: Some("medium".to_string()),
            reasoning_effort: Some(crate::ReasoningEffort::Medium),
            total_input_tokens: 12,
            total_output_tokens: 34,
            total_tokens: 46,
            total_cache_creation_tokens: 5,
            total_cache_read_tokens: 7,
            prompt_token_estimate: 21,
            last_query_usage: Some(TurnUsage {
                input_tokens: 10,
                output_tokens: 11,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_output_tokens: None,
                total_tokens: Some(21),
            }),
            last_query_total_tokens: 21,
            last_context_occupancy: None,
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: CollaborationMode::Plan,
            effective_context_window: None,
            permission_preset: None,
        };

        let json = serde_json::to_string(&metadata).expect("serialize");
        let restored: SessionMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, metadata);
    }

    #[test]
    fn session_metadata_deserializes_without_last_query_usage() {
        let session_id = SessionId::new();
        let created_at = Utc::now();
        let payload = serde_json::json!({
            "session_id": session_id,
            "cwd": "/tmp",
            "created_at": created_at,
            "updated_at": created_at,
            "last_activity_at": created_at,
            "title": null,
            "title_state": "Unset",
            "ephemeral": false,
            "model": null,
            "reasoning_effort": null,
            "total_input_tokens": 100,
            "total_output_tokens": 20,
            "total_tokens": 120,
            "total_cache_creation_tokens": 0,
            "total_cache_read_tokens": 0,
            "prompt_token_estimate": 50,
            "last_query_total_tokens": 30,
            "status": "idle"
        });

        let restored: SessionMetadata =
            serde_json::from_value(payload).expect("deserialize legacy session metadata");
        assert_eq!(restored.last_query_usage, None);
        assert_eq!(restored.last_query_total_tokens, 30);
        assert_eq!(restored.total_input_tokens, 100);
        assert_eq!(restored.collaboration_mode, CollaborationMode::Build);
    }

    #[test]
    fn message_edit_previous_params_accept_spec_shape() {
        let session_id = SessionId::new();
        let expected_target_message_id = ItemId::new();
        let payload = serde_json::json!({
            "session_id": session_id,
            "expected_target_message_id": expected_target_message_id,
            "edited_content_parts": [{ "type": "text", "text": "updated" }],
            "edited_mentions": [{ "type": "file", "path": "src/main.rs" }],
            "client_edit_id": "client-edit-1",
            "workspace_restore_policy": "skip"
        });

        let params: MessageEditPreviousParams =
            serde_json::from_value(payload).expect("deserialize message/editPrevious params");

        assert_eq!(
            params,
            MessageEditPreviousParams {
                session_id,
                target_message_id: None,
                expected_target_message_id: Some(expected_target_message_id),
                edited_content_parts: vec![serde_json::json!({
                    "type": "text",
                    "text": "updated"
                })],
                edited_mentions: vec![serde_json::json!({
                    "type": "file",
                    "path": "src/main.rs"
                })],
                edit_mode: EditMode::Normal,
                client_edit_id: Some("client-edit-1".to_string()),
                workspace_restore_policy: Some(MessageEditWorkspaceRestorePolicy::Skip),
            }
        );
    }

    #[test]
    fn message_edit_previous_params_accept_legacy_replacement_names() {
        let session_id = SessionId::new();
        let target_message_id = ItemId::new();
        let payload = serde_json::json!({
            "session_id": session_id,
            "target_message_id": target_message_id,
            "replacement_content_parts": [{ "type": "text", "text": "legacy" }],
            "replacement_mentions": [],
            "edit_mode": "queued_only"
        });

        let params: MessageEditPreviousParams =
            serde_json::from_value(payload).expect("deserialize legacy edit params");

        assert_eq!(
            params,
            MessageEditPreviousParams {
                session_id,
                target_message_id: Some(target_message_id),
                expected_target_message_id: None,
                edited_content_parts: vec![serde_json::json!({
                    "type": "text",
                    "text": "legacy"
                })],
                edited_mentions: Vec::new(),
                edit_mode: EditMode::QueuedOnly,
                client_edit_id: None,
                workspace_restore_policy: None,
            }
        );
    }

    #[test]
    fn session_history_tool_io_is_optional_and_roundtrips() {
        let legacy: SessionHistoryItem = serde_json::from_str(
            r#"{
                "tool_call_id": "call-1",
                "kind": "tool_call",
                "title": "read foo.txt",
                "body": ""
            }"#,
        )
        .expect("deserialize legacy history item");
        assert_eq!(legacy.tool_io, None);

        let item = SessionHistoryItem::new(
            Some("call-1".to_string()),
            SessionHistoryItemKind::ToolCall,
            "read foo.txt".to_string(),
            String::new(),
        )
        .with_tool_io(SessionHistoryToolIo {
            tool_name: "read".to_string(),
            input: serde_json::json!({"filePath": "foo.txt"}),
            output: None,
            display_content: None,
        });

        let json = serde_json::to_string(&item).expect("serialize history item");
        let restored: SessionHistoryItem =
            serde_json::from_str(&json).expect("deserialize history item");
        assert_eq!(restored, item);
    }

    #[test]
    fn session_rollback_params_default_to_through_user_turn_mode() {
        let session_id = SessionId::new();
        let params: SessionRollbackParams = serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "user_turn_index": 2,
        }))
        .expect("deserialize legacy rollback params");

        assert_eq!(
            params,
            SessionRollbackParams {
                session_id,
                user_turn_index: 2,
                mode: SessionRollbackMode::ThroughUserTurn,
            }
        );
    }
}
