use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use ts_rs::TS;

use crate::command_exec::{CommandExecExitedPayload, CommandExecOutputDeltaPayload};
use crate::parse_command::ParsedCommand;
use crate::protocol::{ExecCommandSource, FileChange};
use crate::reference_search::{ReferenceSearchFailedPayload, ReferenceSearchSnapshot};
use crate::request_user_input::RequestUserInputQuestion;
use crate::session::{SessionMetadata, SessionRuntimeStatus};
use crate::turn::TurnMetadata;
use crate::workspace_changes::WorkspaceChangesUpdatedPayload;
use crate::{ItemId, SessionId, TurnId, TurnUsage};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventContext {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub seq: u64,
    /// The item's own sequence number within the session, when the emitter
    /// allocated one. Additive (P2): absent for older emitters and for
    /// events without an allocated item sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEnvelope {
    pub item_id: ItemId,
    pub item_kind: ItemKind,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallPayload {
    pub tool_call_id: String,
    pub tool_name: String,
    pub parameters: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_actions: Vec<ParsedCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultPayload {
    pub tool_call_id: String,
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    pub content: serde_json::Value,
    /// Optional UI-facing rendering of `content`.
    ///
    /// `content` remains the canonical protocol payload; this field lets clients
    /// show a compact version without losing the original result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_content: Option<String>,
    pub is_error: bool,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandExecutionPayload {
    pub tool_call_id: String,
    pub tool_name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(default)]
    pub source: ExecCommandSource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_actions: Vec<ParsedCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChangePayload {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    pub changes: Vec<(std::path::PathBuf, FileChange)>,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEventPayload {
    pub context: EventContext,
    pub item: ItemEnvelope,
}

/// Opt-in typed item event payload (P2, 06-item-model migration step 2):
/// the same context as [`ItemEventPayload`], but the item is the canonical
/// typed envelope — a typed `Item` instead of the legacy `ItemKind` +
/// `serde_json::Value` payload bag. Only emitted to connections that set
/// `_meta.devo.typedItems` on initialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedItemEventPayload {
    pub context: EventContext,
    pub item: crate::canonical::item::ItemEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDeltaPayload {
    pub context: EventContext,
    pub delta: String,
    pub stream_index: Option<u32>,
    pub channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEventPayload {
    pub session_id: SessionId,
    pub turn: TurnMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnFailedPayload {
    pub session_id: SessionId,
    pub turn: TurnMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<TurnErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub struct TurnErrorPayload {
    pub code: String,
    pub message: String,
    /// Optional user-facing next step for recovering from this failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub recovery_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnPlanStepPayload {
    pub step: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnPlanUpdatedPayload {
    pub session_id: SessionId,
    pub turn: TurnMetadata,
    pub explanation: Option<String>,
    pub plan: Vec<TurnPlanStepPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ProviderRetryPhase {
    Scheduled,
    Resumed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(export)]
pub struct TurnProviderRetryStatusPayload {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub attempt: usize,
    pub backoff_ms: u64,
    pub provider: String,
    pub model: String,
    pub phase: ProviderRetryPhase,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnUsageUpdatedPayload {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub usage: TurnUsage,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    #[serde(default)]
    pub total_tokens: usize,
    pub total_cache_read_tokens: usize,
    pub last_query_input_tokens: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallStatusUpdatedPayload {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub tool_call_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventPayload {
    pub session: SessionMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDeletedPayload {
    pub session_id: SessionId,
    pub deleted_session_ids: Vec<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusChangedPayload {
    pub session_id: SessionId,
    pub status: SessionRuntimeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCompactionFailedPayload {
    pub session_id: SessionId,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRequestResolvedPayload {
    pub session_id: SessionId,
    pub request_id: SmolStr,
    pub turn_id: Option<TurnId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputQueueUpdatedPayload {
    pub session_id: SessionId,
    pub pending_count: usize,
    pub pending_texts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerAcceptedPayload {
    pub session_id: SessionId,
    pub turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEditRecordedPayload {
    pub session_id: SessionId,
    pub edit_id: String,
    pub target_message_id: ItemId,
    pub replacement_message_id: ItemId,
    pub edit_state: String,
    pub content_preview: String,
    #[serde(default)]
    pub mentions: Vec<serde_json::Value>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSupersededPayload {
    pub session_id: SessionId,
    pub superseded_turn_id: TurnId,
    pub replacement_turn_id: TurnId,
    pub edit_id: String,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRestoreStartedPayload {
    pub session_id: SessionId,
    pub edit_id: String,
    pub superseded_turn_id: TurnId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub candidate_files: Vec<String>,
    pub restore_policy: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRestoreCompletedPayload {
    pub session_id: SessionId,
    pub edit_id: String,
    pub superseded_turn_id: TurnId,
    #[serde(default)]
    pub restored_files: Vec<String>,
    #[serde(default)]
    pub skipped_files: Vec<String>,
    #[serde(default)]
    pub unsupported_files: Vec<String>,
    #[serde(default)]
    pub failed_files: Vec<String>,
    pub current_state_kept: bool,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    UserMessage,
    AgentMessage,
    Reasoning,
    Plan,
    ToolCall,
    ToolResult,
    CommandExecution,
    FileChange,
    McpToolCall,
    WebSearch,
    ImageView,
    ContextCompaction,
    ApprovalRequest,
    ApprovalDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemDeltaKind {
    AgentMessageDelta,
    ReasoningSummaryTextDelta,
    ReasoningTextDelta,
    CommandExecutionOutputDelta,
    FileChangeOutputDelta,
    PlanDelta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerRequestKind {
    ItemCommandExecutionRequestApproval,
    ItemFileChangeRequestApproval,
    ItemPermissionsRequestApproval,
    ItemToolRequestUserInput,
    McpServerElicitationRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingServerRequestContext {
    pub request_id: SmolStr,
    pub request_kind: ServerRequestKind,
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestPayload {
    pub request: PendingServerRequestContext,
    pub approval_id: SmolStr,
    pub action_summary: String,
    pub justification: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_pattern: Option<Vec<String>>,
    /// Suggested command prefix for "always allow commands that start with …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_prefix: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalDecisionPayload {
    pub approval_id: SmolStr,
    pub decision: String,
    pub scope: String,
    /// Authority that produced the decision. Missing on legacy events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_source: Option<crate::canonical::item::ApprovalDecisionSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestUserInputPayload {
    pub request: PendingServerRequestContext,
    pub questions: Vec<RequestUserInputQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    SessionStarted(SessionEventPayload),
    SessionTitleUpdated(SessionEventPayload),
    SessionCompactionStarted(SessionEventPayload),
    SessionCompactionCompleted(SessionEventPayload),
    SessionCompactionFailed(SessionCompactionFailedPayload),
    SessionStatusChanged(SessionStatusChangedPayload),
    SessionArchived(SessionEventPayload),
    SessionUnarchived(SessionEventPayload),
    SessionClosed(SessionEventPayload),
    SessionDeleted(SessionDeletedPayload),
    TurnStarted(TurnEventPayload),
    TurnCompleted(TurnEventPayload),
    TurnInterrupted(TurnEventPayload),
    TurnFailed(TurnFailedPayload),
    TurnPlanUpdated(TurnPlanUpdatedPayload),
    TurnDiffUpdated(TurnEventPayload),
    TurnUsageUpdated(TurnUsageUpdatedPayload),
    TurnProviderRetryStatus(TurnProviderRetryStatusPayload),
    WorkspaceChangesUpdated(WorkspaceChangesUpdatedPayload),
    ToolCallStatusUpdated(ToolCallStatusUpdatedPayload),
    RequestUserInput(RequestUserInputPayload),
    InputQueueUpdated(InputQueueUpdatedPayload),
    SteerAccepted(SteerAcceptedPayload),
    MessageEditRecorded(MessageEditRecordedPayload),
    TurnSuperseded(TurnSupersededPayload),
    WorkspaceRestoreStarted(WorkspaceRestoreStartedPayload),
    WorkspaceRestoreCompleted(WorkspaceRestoreCompletedPayload),
    ItemStarted(ItemEventPayload),
    ItemCompleted(ItemEventPayload),
    ItemDelta {
        delta_kind: ItemDeltaKind,
        payload: ItemDeltaPayload,
    },
    ServerRequestResolved(ServerRequestResolvedPayload),
    ReferenceSearchUpdated(ReferenceSearchSnapshot),
    ReferenceSearchCompleted(ReferenceSearchSnapshot),
    ReferenceSearchFailed(ReferenceSearchFailedPayload),
    CommandExecOutputDelta(CommandExecOutputDeltaPayload),
    CommandExecExited(CommandExecExitedPayload),
}

impl ServerEvent {
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionStarted(payload)
            | Self::SessionTitleUpdated(payload)
            | Self::SessionCompactionStarted(payload)
            | Self::SessionCompactionCompleted(payload)
            | Self::SessionArchived(payload)
            | Self::SessionUnarchived(payload)
            | Self::SessionClosed(payload) => Some(payload.session.session_id),
            Self::SessionDeleted(payload) => Some(payload.session_id),
            Self::SessionCompactionFailed(payload) => Some(payload.session_id),
            Self::SessionStatusChanged(payload) => Some(payload.session_id),
            Self::TurnStarted(payload)
            | Self::TurnCompleted(payload)
            | Self::TurnInterrupted(payload)
            | Self::TurnDiffUpdated(payload) => Some(payload.session_id),
            Self::TurnFailed(payload) => Some(payload.session_id),
            Self::TurnPlanUpdated(payload) => Some(payload.session_id),
            Self::TurnUsageUpdated(payload) => Some(payload.session_id),
            Self::TurnProviderRetryStatus(payload) => Some(payload.session_id),
            Self::WorkspaceChangesUpdated(payload) => Some(payload.session_id),
            Self::ToolCallStatusUpdated(payload) => Some(payload.session_id),
            Self::RequestUserInput(payload) => Some(payload.request.session_id),
            Self::InputQueueUpdated(payload) => Some(payload.session_id),
            Self::SteerAccepted(payload) => Some(payload.session_id),
            Self::MessageEditRecorded(payload) => Some(payload.session_id),
            Self::TurnSuperseded(payload) => Some(payload.session_id),
            Self::WorkspaceRestoreStarted(payload) => Some(payload.session_id),
            Self::WorkspaceRestoreCompleted(payload) => Some(payload.session_id),
            Self::ItemStarted(payload) | Self::ItemCompleted(payload) => {
                Some(payload.context.session_id)
            }
            Self::ItemDelta { payload, .. } => Some(payload.context.session_id),
            Self::ServerRequestResolved(payload) => Some(payload.session_id),
            Self::ReferenceSearchUpdated(_)
            | Self::ReferenceSearchCompleted(_)
            | Self::ReferenceSearchFailed(_) => None,
            Self::CommandExecOutputDelta(payload) => payload.session_id,
            Self::CommandExecExited(payload) => payload.session_id,
        }
    }

    pub fn method_name(&self) -> &'static str {
        match self {
            Self::SessionStarted(_) => "session/started",
            Self::SessionTitleUpdated(_) => "session/title/updated",
            Self::SessionCompactionStarted(_) => "session/compaction/started",
            Self::SessionCompactionCompleted(_) => "session/compaction/completed",
            Self::SessionCompactionFailed(_) => "session/compaction/failed",
            Self::SessionStatusChanged(_) => "session/status/changed",
            Self::SessionArchived(_) => "session/archived",
            Self::SessionUnarchived(_) => "session/unarchived",
            Self::SessionClosed(_) => "session/closed",
            Self::SessionDeleted(_) => "session/deleted",
            Self::TurnStarted(_) => "turn/started",
            Self::TurnCompleted(_) => "turn/completed",
            Self::TurnInterrupted(_) => "turn/interrupted",
            Self::TurnFailed(_) => "turn/failed",
            Self::TurnPlanUpdated(_) => "turn/plan/updated",
            Self::TurnDiffUpdated(_) => "turn/diff/updated",
            Self::TurnUsageUpdated(_) => "turn/usage/updated",
            Self::TurnProviderRetryStatus(_) => "turn/provider_retry_status",
            Self::WorkspaceChangesUpdated(_) => "workspace/changes/updated",
            Self::ToolCallStatusUpdated(_) => "tool_call/status_updated",
            Self::RequestUserInput(_) => "item/tool/requestUserInput",
            Self::InputQueueUpdated(_) => "inputQueue/updated",
            Self::SteerAccepted(_) => "steer/accepted",
            Self::MessageEditRecorded(_) => "message/edit/recorded",
            Self::TurnSuperseded(_) => "turn/superseded",
            Self::WorkspaceRestoreStarted(_) => "workspace_restore_started",
            Self::WorkspaceRestoreCompleted(_) => "workspace_restore_completed",
            Self::ItemStarted(_) => "item/started",
            Self::ItemCompleted(_) => "item/completed",
            Self::ItemDelta { delta_kind, .. } => match delta_kind {
                ItemDeltaKind::AgentMessageDelta => "item/agentMessage/delta",
                ItemDeltaKind::ReasoningSummaryTextDelta => "item/reasoning/summaryTextDelta",
                ItemDeltaKind::ReasoningTextDelta => "item/reasoning/textDelta",
                ItemDeltaKind::CommandExecutionOutputDelta => "item/commandExecution/outputDelta",
                ItemDeltaKind::FileChangeOutputDelta => "item/fileChange/outputDelta",
                ItemDeltaKind::PlanDelta => "item/plan/delta",
            },
            Self::ServerRequestResolved(_) => "serverRequest/resolved",
            Self::ReferenceSearchUpdated(_) => "search/updated",
            Self::ReferenceSearchCompleted(_) => "search/completed",
            Self::ReferenceSearchFailed(_) => "search/failed",
            Self::CommandExecOutputDelta(_) => "command/exec/outputDelta",
            Self::CommandExecExited(_) => "command/exec/exited",
        }
    }

    pub fn with_seq(mut self, seq: u64) -> Self {
        match &mut self {
            Self::ItemStarted(payload) | Self::ItemCompleted(payload) => {
                payload.context.seq = seq;
            }
            Self::ItemDelta { payload, .. } => payload.context.seq = seq,
            Self::TurnUsageUpdated(_)
            | Self::TurnProviderRetryStatus(_)
            | Self::WorkspaceChangesUpdated(_)
            | Self::ToolCallStatusUpdated(_)
            | Self::RequestUserInput(_)
            | Self::InputQueueUpdated(_)
            | Self::SteerAccepted(_)
            | Self::MessageEditRecorded(_)
            | Self::TurnSuperseded(_)
            | Self::WorkspaceRestoreStarted(_)
            | Self::WorkspaceRestoreCompleted(_)
            | Self::ReferenceSearchUpdated(_)
            | Self::ReferenceSearchCompleted(_)
            | Self::ReferenceSearchFailed(_)
            | Self::CommandExecOutputDelta(_)
            | Self::CommandExecExited(_) => {}
            _ => {}
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::workspace_changes::{
        WorkspaceChangeCoverage, WorkspaceChangeScope, WorkspaceChangeSetStatus,
        WorkspaceChangeStats, WorkspaceChangeViewStatus,
    };

    #[test]
    fn input_queue_updated_event_roundtrips() {
        let payload = InputQueueUpdatedPayload {
            session_id: SessionId::new(),
            pending_count: 3,
            pending_texts: vec!["first".into(), "second".into()],
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: InputQueueUpdatedPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.pending_count, 3);
        assert_eq!(restored.pending_texts, vec!["first", "second"]);
    }

    #[test]
    fn turn_failed_payload_serializes_error_and_accepts_legacy_shape() {
        let session_id = SessionId::new();
        let turn = TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence: 1,
            status: crate::TurnStatus::Failed,
            kind: crate::TurnKind::Regular,
            model: "catalog-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "provider-model".to_string(),
            request_thinking: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        let payload = TurnFailedPayload {
            session_id,
            turn,
            error: Some(TurnErrorPayload {
                code: "PROVIDER_SERVER_ERROR".to_string(),
                message: "Internal server error".to_string(),
                recovery_hint: None,
            }),
        };

        let mut value = serde_json::to_value(&payload).expect("serialize turn failure");
        assert_eq!(
            value["error"],
            serde_json::json!({
                "code": "PROVIDER_SERVER_ERROR",
                "message": "Internal server error"
            })
        );
        value
            .as_object_mut()
            .expect("turn failure object")
            .remove("error");
        let legacy = serde_json::from_value::<TurnFailedPayload>(value)
            .expect("deserialize legacy turn failure");

        assert_eq!(
            legacy,
            TurnFailedPayload {
                error: None,
                ..payload
            }
        );
    }

    #[test]
    fn tool_result_payload_display_content_is_optional() {
        let payload: ToolResultPayload = serde_json::from_str(
            r#"{
                "tool_call_id": "call-1",
                "tool_name": "read",
                "content": "canonical",
                "is_error": false
            }"#,
        )
        .expect("deserialize legacy payload");
        assert_eq!(payload.display_content, None);
        assert_eq!(payload.input, None);
        assert_eq!(payload.summary, "");

        let payload = ToolResultPayload {
            tool_call_id: "call-1".to_string(),
            tool_name: Some("read".to_string()),
            input: Some(serde_json::json!({"filePath": "foo.txt"})),
            content: serde_json::Value::String("canonical".to_string()),
            display_content: Some("display".to_string()),
            is_error: false,
            summary: "read output".to_string(),
        };
        let json = serde_json::to_value(&payload).expect("serialize payload");
        assert_eq!(
            json.get("display_content"),
            Some(&serde_json::Value::String("display".to_string()))
        );
        assert_eq!(
            json.get("input"),
            Some(&serde_json::json!({"filePath": "foo.txt"}))
        );
    }

    #[test]
    fn steer_accepted_event_roundtrips() {
        let turn_id = TurnId::new();
        let payload = SteerAcceptedPayload {
            session_id: SessionId::new(),
            turn_id,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: SteerAcceptedPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.turn_id, turn_id);
    }

    #[test]
    fn message_edit_events_roundtrip_and_report_methods() {
        let session_id = SessionId::new();
        let target_message_id = ItemId::new();
        let replacement_message_id = ItemId::new();
        let superseded_turn_id = TurnId::new();
        let replacement_turn_id = TurnId::new();
        let timestamp = Utc::now();
        let edit_payload = MessageEditRecordedPayload {
            session_id,
            edit_id: "edit-1".to_string(),
            target_message_id,
            replacement_message_id,
            edit_state: "accepted".to_string(),
            content_preview: "edited".to_string(),
            mentions: vec![],
            timestamp,
        };
        let superseded_payload = TurnSupersededPayload {
            session_id,
            superseded_turn_id,
            replacement_turn_id,
            edit_id: "edit-1".to_string(),
            reason: "message_edit_previous".to_string(),
            timestamp,
        };
        let restore_started_payload = WorkspaceRestoreStartedPayload {
            session_id,
            edit_id: "edit-1".to_string(),
            superseded_turn_id,
            checkpoint_id: None,
            candidate_files: vec!["src/main.rs".to_string()],
            restore_policy: "safe".to_string(),
            timestamp,
        };
        let restore_completed_payload = WorkspaceRestoreCompletedPayload {
            session_id,
            edit_id: "edit-1".to_string(),
            superseded_turn_id,
            restored_files: vec![],
            skipped_files: vec!["src/main.rs".to_string()],
            unsupported_files: vec![],
            failed_files: vec![],
            current_state_kept: true,
            timestamp,
        };

        let restored: MessageEditRecordedPayload =
            serde_json::from_str(&serde_json::to_string(&edit_payload).expect("serialize"))
                .expect("deserialize");
        assert_eq!(restored, edit_payload);
        let restored: WorkspaceRestoreCompletedPayload = serde_json::from_str(
            &serde_json::to_string(&restore_completed_payload).expect("serialize"),
        )
        .expect("deserialize");
        assert_eq!(restored, restore_completed_payload);

        let edit_event = ServerEvent::MessageEditRecorded(edit_payload);
        assert_eq!(edit_event.method_name(), "message/edit/recorded");
        assert_eq!(edit_event.session_id(), Some(session_id));

        let superseded_event = ServerEvent::TurnSuperseded(superseded_payload);
        assert_eq!(superseded_event.method_name(), "turn/superseded");
        assert_eq!(superseded_event.session_id(), Some(session_id));

        let restore_started_event = ServerEvent::WorkspaceRestoreStarted(restore_started_payload);
        assert_eq!(
            restore_started_event.method_name(),
            "workspace_restore_started"
        );
        assert_eq!(restore_started_event.session_id(), Some(session_id));

        let restore_completed_event =
            ServerEvent::WorkspaceRestoreCompleted(restore_completed_payload);
        assert_eq!(
            restore_completed_event.method_name(),
            "workspace_restore_completed"
        );
        assert_eq!(restore_completed_event.session_id(), Some(session_id));
    }

    #[test]
    fn server_event_input_queue_updated_method_name() {
        let event = ServerEvent::InputQueueUpdated(InputQueueUpdatedPayload {
            session_id: SessionId::new(),
            pending_count: 0,
            pending_texts: vec![],
        });
        assert_eq!(event.method_name(), "inputQueue/updated");
        assert!(event.session_id().is_some());
    }

    #[test]
    fn server_event_steer_accepted_method_name() {
        let event = ServerEvent::SteerAccepted(SteerAcceptedPayload {
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
        });
        assert_eq!(event.method_name(), "steer/accepted");
        assert!(event.session_id().is_some());
    }

    #[test]
    fn workspace_changes_updated_method_name() {
        let session_id = SessionId::new();
        let event = ServerEvent::WorkspaceChangesUpdated(WorkspaceChangesUpdatedPayload {
            session_id,
            turn_id: TurnId::new(),
            scope: WorkspaceChangeScope::Turn,
            status: WorkspaceChangeViewStatus::Ready,
            coverage: WorkspaceChangeCoverage::GitVisible,
            change_set_status: WorkspaceChangeSetStatus::Finalized,
            stats: WorkspaceChangeStats {
                files_changed: 1,
                additions: 2,
                deletions: 0,
            },
            version: 1,
            generated_at: Utc::now(),
        });

        assert_eq!(event.method_name(), "workspace/changes/updated");
        assert_eq!(event.session_id(), Some(session_id));
    }
}
