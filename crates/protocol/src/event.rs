use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::session::{SessionRuntimeStatus, SessionSummary};
use crate::turn::TurnSummary;
use crate::{ItemId, SessionId, TurnId, TurnUsage};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventContext {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEnvelope {
    pub item_id: ItemId,
    pub item_kind: ItemKind,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEventPayload {
    pub context: EventContext,
    pub item: ItemEnvelope,
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
    pub turn: TurnSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnUsageUpdatedPayload {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub usage: TurnUsage,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventPayload {
    pub session: SessionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusChangedPayload {
    pub session_id: SessionId,
    pub status: SessionRuntimeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRequestResolvedPayload {
    pub session_id: SessionId,
    pub request_id: SmolStr,
    pub turn_id: Option<TurnId>,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestUserInputPayload {
    pub request: PendingServerRequestContext,
    pub prompt: String,
    pub schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerEvent {
    SessionStarted(SessionEventPayload),
    SessionTitleUpdated(SessionEventPayload),
    SessionStatusChanged(SessionStatusChangedPayload),
    SessionArchived(SessionEventPayload),
    SessionUnarchived(SessionEventPayload),
    SessionClosed(SessionEventPayload),
    TurnStarted(TurnEventPayload),
    TurnCompleted(TurnEventPayload),
    TurnInterrupted(TurnEventPayload),
    TurnFailed(TurnEventPayload),
    TurnPlanUpdated(TurnEventPayload),
    TurnDiffUpdated(TurnEventPayload),
    TurnUsageUpdated(TurnUsageUpdatedPayload),
    ItemStarted(ItemEventPayload),
    ItemCompleted(ItemEventPayload),
    ItemDelta {
        delta_kind: ItemDeltaKind,
        payload: ItemDeltaPayload,
    },
    ServerRequestResolved(ServerRequestResolvedPayload),
}

impl ServerEvent {
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionStarted(payload)
            | Self::SessionTitleUpdated(payload)
            | Self::SessionArchived(payload)
            | Self::SessionUnarchived(payload)
            | Self::SessionClosed(payload) => Some(payload.session.session_id),
            Self::SessionStatusChanged(payload) => Some(payload.session_id),
            Self::TurnStarted(payload)
            | Self::TurnCompleted(payload)
            | Self::TurnInterrupted(payload)
            | Self::TurnFailed(payload)
            | Self::TurnPlanUpdated(payload)
            | Self::TurnDiffUpdated(payload) => Some(payload.session_id),
            Self::TurnUsageUpdated(payload) => Some(payload.session_id),
            Self::ItemStarted(payload) | Self::ItemCompleted(payload) => {
                Some(payload.context.session_id)
            }
            Self::ItemDelta { payload, .. } => Some(payload.context.session_id),
            Self::ServerRequestResolved(payload) => Some(payload.session_id),
        }
    }

    pub fn method_name(&self) -> &'static str {
        match self {
            Self::SessionStarted(_) => "session/started",
            Self::SessionTitleUpdated(_) => "session/title/updated",
            Self::SessionStatusChanged(_) => "session/status/changed",
            Self::SessionArchived(_) => "session/archived",
            Self::SessionUnarchived(_) => "session/unarchived",
            Self::SessionClosed(_) => "session/closed",
            Self::TurnStarted(_) => "turn/started",
            Self::TurnCompleted(_) => "turn/completed",
            Self::TurnInterrupted(_) => "turn/interrupted",
            Self::TurnFailed(_) => "turn/failed",
            Self::TurnPlanUpdated(_) => "turn/plan/updated",
            Self::TurnDiffUpdated(_) => "turn/diff/updated",
            Self::TurnUsageUpdated(_) => "turn/usage/updated",
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
        }
    }

    pub fn with_seq(mut self, seq: u64) -> Self {
        match &mut self {
            Self::ItemStarted(payload) | Self::ItemCompleted(payload) => {
                payload.context.seq = seq;
            }
            Self::ItemDelta { payload, .. } => payload.context.seq = seq,
            Self::TurnUsageUpdated(_) => {}
            _ => {}
        }
        self
    }
}
