use std::fmt;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use strum_macros::Display;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRequest<T> {
    pub id: serde_json::Value,
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientNotification<T> {
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfigSetParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    pub config_id: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessResponse<T> {
    pub id: serde_json::Value,
    pub result: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub id: serde_json::Value,
    pub error: ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationEnvelope<T> {
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerRequestEnvelope<T> {
    pub id: serde_json::Value,
    pub method: String,
    pub params: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMethod {
    SessionMetadataUpdate,
    SessionPermissionsUpdate,
    SessionSandboxProfileUpdate,
    SessionTitleUpdate,
    SessionResume,
    SessionFork,
    SessionRollback,
    SessionCompact,
    SkillsList,
    SkillsChanged,
    SkillsSetEnabled,
    ModelCatalog,
    ModelConfig,
    ModelConfigSet,
    ModelSaved,
    CommandExec,
    CommandExecWrite,
    CommandExecResize,
    CommandExecTerminate,
    MessageEditPrevious,
    TurnStart,
    TurnShellCommand,
    TurnInterrupt,
    TurnSteer,
    TurnQueueRemove,
    TurnQueueSteer,
    WorkspaceChangesRead,
    RequestUserInputRespond,
    SearchStart,
    SearchUpdate,
    SearchCancel,
    EventsSubscribe,
    GoalCreate,
    GoalSet,
    GoalPause,
    GoalResume,
    GoalComplete,
    GoalCancel,
    GoalClear,
    GoalStatus,
    AgentSpawn,
    AgentSendMessage,
    AgentWait,
    AgentList,
    AgentStatus,
    AgentClose,
    ProviderVendorList,
    ProviderValidate,
    ProviderVendorUpsert,
    McpList,
    McpTools,
    // New Native API methods (canonical types; not part of the legacy
    // `_devo/*` alias surface).
    SessionTurnsList,
    SessionItemsList,
    SessionRollbackPreview,
    SessionRollbackCommit,
    SubscriptionCreate,
    SubscriptionUpdate,
    SubscriptionAck,
    SubscriptionUnsubscribe,
    SessionQueuePush,
    SessionQueueList,
    SessionQueueUpdate,
    SessionQueueRemove,
    SessionQueueSteer,
}

impl ClientMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionMetadataUpdate => "session/metadata/update",
            Self::SessionPermissionsUpdate => "session/permissions/update",
            Self::SessionSandboxProfileUpdate => "session/sandbox_profile/update",
            Self::SessionTitleUpdate => "session/title/update",
            Self::SessionResume => "session/resume",
            Self::SessionFork => "session/fork",
            Self::SessionRollback => "session/rollback",
            Self::SessionCompact => "session/compact",
            Self::SkillsList => "skills/list",
            Self::SkillsChanged => "skills/changed",
            Self::SkillsSetEnabled => "skills/set_enabled",
            Self::ModelCatalog => "model/catalog",
            Self::ModelConfig => "model/config",
            Self::ModelConfigSet => "model/config/set",
            Self::ModelSaved => "model/saved",
            Self::CommandExec => "command/exec",
            Self::CommandExecWrite => "command/exec/write",
            Self::CommandExecResize => "command/exec/resize",
            Self::CommandExecTerminate => "command/exec/terminate",
            Self::MessageEditPrevious => "message/editPrevious",
            Self::TurnStart => "turn/start",
            Self::TurnShellCommand => "turn/shell_command",
            Self::TurnInterrupt => "turn/interrupt",
            Self::TurnSteer => "turn/steer",
            Self::TurnQueueRemove => "turn/queue/remove",
            Self::TurnQueueSteer => "turn/queue/steer",
            Self::WorkspaceChangesRead => "workspace/changes/read",
            Self::RequestUserInputRespond => "request_user_input/respond",
            Self::SearchStart => "search/start",
            Self::SearchUpdate => "search/update",
            Self::SearchCancel => "search/cancel",
            Self::EventsSubscribe => "events/subscribe",
            Self::GoalCreate => "goal/create",
            Self::GoalSet => "goal/set",
            Self::GoalPause => "goal/pause",
            Self::GoalResume => "goal/resume",
            Self::GoalComplete => "goal/complete",
            Self::GoalCancel => "goal/cancel",
            Self::GoalClear => "goal/clear",
            Self::GoalStatus => "goal/status",
            Self::AgentSpawn => "agent/spawn",
            Self::AgentSendMessage => "agent/send_message",
            Self::AgentWait => "agent/wait",
            Self::AgentList => "agent/list",
            Self::AgentStatus => "agent/status",
            Self::AgentClose => "agent/close",
            Self::ProviderVendorList => "provider/list",
            Self::ProviderValidate => "provider/validate",
            Self::ProviderVendorUpsert => "provider/upsert",
            Self::McpList => "mcp/list",
            Self::McpTools => "mcp/tools",
            Self::SessionTurnsList => "session/turns/list",
            Self::SessionItemsList => "session/items/list",
            Self::SessionRollbackPreview => "session/rollback/preview",
            Self::SessionRollbackCommit => "session/rollback/commit",
            Self::SubscriptionCreate => "subscription/create",
            Self::SubscriptionUpdate => "subscription/update",
            Self::SubscriptionAck => "subscription/ack",
            Self::SubscriptionUnsubscribe => "subscription/unsubscribe",
            Self::SessionQueuePush => "session/queue/push",
            Self::SessionQueueList => "session/queue/list",
            Self::SessionQueueUpdate => "session/queue/update",
            Self::SessionQueueRemove => "session/queue/remove",
            Self::SessionQueueSteer => "session/queue/steer",
        }
    }

    pub fn parse(method: &str) -> Option<Self> {
        Some(match method {
            "session/metadata/update" => Self::SessionMetadataUpdate,
            "session/permissions/update" => Self::SessionPermissionsUpdate,
            "session/sandbox_profile/update" => Self::SessionSandboxProfileUpdate,
            "session/title/update" => Self::SessionTitleUpdate,
            "session/resume" => Self::SessionResume,
            "session/fork" => Self::SessionFork,
            "session/rollback" => Self::SessionRollback,
            "session/compact" => Self::SessionCompact,
            "skills/list" => Self::SkillsList,
            "skills/changed" => Self::SkillsChanged,
            "skills/set_enabled" => Self::SkillsSetEnabled,
            "model/catalog" => Self::ModelCatalog,
            "model/config" => Self::ModelConfig,
            "model/config/set" => Self::ModelConfigSet,
            "model/saved" => Self::ModelSaved,
            "command/exec" => Self::CommandExec,
            "command/exec/write" => Self::CommandExecWrite,
            "command/exec/resize" => Self::CommandExecResize,
            "command/exec/terminate" => Self::CommandExecTerminate,
            "message/editPrevious" => Self::MessageEditPrevious,
            "turn/start" => Self::TurnStart,
            "turn/shell_command" => Self::TurnShellCommand,
            "turn/interrupt" => Self::TurnInterrupt,
            "turn/steer" => Self::TurnSteer,
            "turn/queue/remove" => Self::TurnQueueRemove,
            "turn/queue/steer" => Self::TurnQueueSteer,
            "workspace/changes/read" => Self::WorkspaceChangesRead,
            "request_user_input/respond" => Self::RequestUserInputRespond,
            "search/start" => Self::SearchStart,
            "search/update" => Self::SearchUpdate,
            "search/cancel" => Self::SearchCancel,
            "events/subscribe" => Self::EventsSubscribe,
            "goal/create" => Self::GoalCreate,
            "goal/set" => Self::GoalSet,
            "goal/pause" => Self::GoalPause,
            "goal/resume" => Self::GoalResume,
            "goal/complete" => Self::GoalComplete,
            "goal/cancel" => Self::GoalCancel,
            "goal/clear" => Self::GoalClear,
            "goal/status" => Self::GoalStatus,
            "agent/spawn" => Self::AgentSpawn,
            "agent/send_message" => Self::AgentSendMessage,
            "agent/wait" => Self::AgentWait,
            "agent/list" => Self::AgentList,
            "agent/status" => Self::AgentStatus,
            "agent/close" => Self::AgentClose,
            "provider/list" => Self::ProviderVendorList,
            "provider/validate" => Self::ProviderValidate,
            "provider/upsert" => Self::ProviderVendorUpsert,
            "mcp/list" => Self::McpList,
            "mcp/tools" => Self::McpTools,
            "session/turns/list" => Self::SessionTurnsList,
            "session/items/list" => Self::SessionItemsList,
            "session/rollback/preview" => Self::SessionRollbackPreview,
            "session/rollback/commit" => Self::SessionRollbackCommit,
            "subscription/create" => Self::SubscriptionCreate,
            "subscription/update" => Self::SubscriptionUpdate,
            "subscription/ack" => Self::SubscriptionAck,
            "subscription/unsubscribe" => Self::SubscriptionUnsubscribe,
            "session/queue/push" => Self::SessionQueuePush,
            "session/queue/list" => Self::SessionQueueList,
            "session/queue/update" => Self::SessionQueueUpdate,
            "session/queue/remove" => Self::SessionQueueRemove,
            "session/queue/steer" => Self::SessionQueueSteer,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProtocolErrorCode {
    #[error("NotInitialized")]
    NotInitialized,
    #[error("InvalidParams")]
    InvalidParams,
    #[error("SessionNotFound")]
    SessionNotFound,
    #[error("TurnNotFound")]
    TurnNotFound,
    #[error("TurnAlreadyRunning")]
    TurnAlreadyRunning,
    #[error("ApprovalNotFound")]
    ApprovalNotFound,
    #[error("PolicyDenied")]
    PolicyDenied,
    #[error("ContextLimitExceeded")]
    ContextLimitExceeded,
    #[error("NoActiveTurn")]
    NoActiveTurn,
    #[error("ExpectedTurnMismatch")]
    ExpectedTurnMismatch,
    #[error("ActiveTurnNotSteerable")]
    ActiveTurnNotSteerable,
    #[error("EmptyInput")]
    EmptyInput,
    #[error("AlreadyResolved")]
    AlreadyResolved,
    #[error("ParentSessionNotFound")]
    ParentSessionNotFound,
    #[error("ForkTurnNotFound")]
    ForkTurnNotFound,
    #[error("ForkTurnNotStable")]
    ForkTurnNotStable,
    #[error("PermissionDenied")]
    PermissionDenied,
    /// The ack/replay cursor is outside the stored log (regression, future
    /// value, or unknown stream); the client must re-snapshot (08 §4).
    #[error("CursorExpired")]
    CursorExpired,
    /// The addressed queue entry is no longer queued (drained, removed, or
    /// never existed) — `session/queue/*` (01 §4.3).
    #[error("QueueItemNotFound")]
    QueueItemNotFound,
    #[error("WorkspaceUnavailable")]
    WorkspaceUnavailable,
    #[error("InheritedSegmentWriteFailed")]
    InheritedSegmentWriteFailed,
    #[error("ForkRetentionRequired")]
    ForkRetentionRequired,
    #[error("InvalidConfirmToken")]
    InvalidConfirmToken,
    #[error("UnsupportedDeletePolicy")]
    UnsupportedDeletePolicy,
    #[error("InheritedSegmentMaterializationFailed")]
    InheritedSegmentMaterializationFailed,
    #[error("ExpectedTargetMessageMismatch")]
    ExpectedTargetMessageMismatch,
    #[error("OlderMessageRequiresFork")]
    OlderMessageRequiresFork,
    #[error("ActiveTurnEditRejected")]
    ActiveTurnEditRejected,
    #[error("InvalidContentParts")]
    InvalidContentParts,
    #[error("InvalidMentions")]
    InvalidMentions,
    #[error("WorkspaceRestoreFailedToStart")]
    WorkspaceRestoreFailedToStart,
    #[serde(rename = "RESTORE_PLAN_NOT_FOUND")]
    #[error("RESTORE_PLAN_NOT_FOUND")]
    RestorePlanNotFound,
    #[serde(rename = "RESTORE_PLAN_EXPIRED")]
    #[error("RESTORE_PLAN_EXPIRED")]
    RestorePlanExpired,
    #[serde(rename = "WORKSPACE_VERSION_CONFLICT")]
    #[error("WORKSPACE_VERSION_CONFLICT")]
    WorkspaceVersionConflict,
    #[error("InternalError")]
    InternalError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
    pub data: serde_json::Value,
}

#[derive(
    Debug, Clone, Copy, Display, Deserialize, Serialize, JsonSchema, TS, PartialEq, Eq, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum ExecCommandSource {
    #[default]
    Agent,
    UserShell,
    UnifiedExecStartup,
    UnifiedExecInteraction,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileChange {
    Add {
        content: String,
    },
    Delete {
        content: String,
    },
    Update {
        unified_diff: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_text: Option<String>,
        move_path: Option<PathBuf>,
    },
}

/// Context/compaction display token usage, not the canonical provider response
/// usage shape. Provider/model usage is represented by `devo_protocol::Usage`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub total_tokens: i64,
}

// Includes prompts, tools and space to call compact.
const BASELINE_TOKENS: i64 = 12000;

impl TokenUsage {
    pub fn is_zero(&self) -> bool {
        self.total_tokens == 0
    }

    pub fn cached_input(&self) -> i64 {
        self.cached_input_tokens.max(0)
    }

    pub fn non_cached_input(&self) -> i64 {
        (self.input_tokens - self.cached_input()).max(0)
    }

    /// Primary count for display as a single absolute value: non-cached input + output.
    pub fn blended_total(&self) -> i64 {
        (self.non_cached_input() + self.output_tokens.max(0)).max(0)
    }

    pub fn tokens_in_context_window(&self) -> i64 {
        self.total_tokens
    }

    /// Estimate the remaining user-controllable percentage of the model's context window.
    ///
    /// `context_window` is the total size of the model's context window.
    /// `BASELINE_TOKENS` should capture tokens that are always present in
    /// the context (e.g., system prompt and fixed tool instructions) so that
    /// the percentage reflects the portion the user can influence.
    ///
    /// This normalizes both the numerator and denominator by subtracting the
    /// baseline, so immediately after the first prompt the UI shows 100% left
    /// and trends toward 0% as the user fills the effective window.
    pub fn percent_of_context_window_remaining(&self, context_window: i64) -> i64 {
        if context_window <= BASELINE_TOKENS {
            return 0;
        }

        let effective_window = context_window - BASELINE_TOKENS;
        let used = (self.tokens_in_context_window() - BASELINE_TOKENS).max(0);
        let remaining = (effective_window - used).max(0);
        ((remaining as f64 / effective_window as f64) * 100.0)
            .clamp(0.0, 100.0)
            .round() as i64
    }

    /// In-place element-wise sum of token counts.
    pub fn add_assign(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
        self.total_tokens += other.total_tokens;
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS, PartialEq, Eq)]
pub struct TokenUsageInfo {
    pub total_token_usage: TokenUsage,
    pub last_token_usage: TokenUsage,
    pub model_context_window: Option<i64>,
}

impl TokenUsageInfo {
    pub fn new_or_append(
        info: &Option<TokenUsageInfo>,
        last: &Option<TokenUsage>,
        model_context_window: Option<i64>,
    ) -> Option<Self> {
        if info.is_none() && last.is_none() {
            return None;
        }

        let mut info = match info {
            Some(info) => info.clone(),
            None => Self {
                total_token_usage: TokenUsage::default(),
                last_token_usage: TokenUsage::default(),
                model_context_window,
            },
        };
        if let Some(last) = last {
            info.append_last_usage(last);
        }
        if let Some(model_context_window) = model_context_window {
            info.model_context_window = Some(model_context_window);
        }
        Some(info)
    }

    pub fn append_last_usage(&mut self, last: &TokenUsage) {
        self.total_token_usage.add_assign(last);
        self.last_token_usage = last.clone();
    }

    pub fn fill_to_context_window(&mut self, context_window: i64) {
        let previous_total = self.total_token_usage.total_tokens;
        let delta = (context_window - previous_total).max(0);

        self.model_context_window = Some(context_window);
        self.total_token_usage = TokenUsage {
            total_tokens: context_window,
            ..TokenUsage::default()
        };
        self.last_token_usage = TokenUsage {
            total_tokens: delta,
            ..TokenUsage::default()
        };
    }

    pub fn full_context_window(context_window: i64) -> Self {
        let mut info = Self {
            total_token_usage: TokenUsage::default(),
            last_token_usage: TokenUsage::default(),
            model_context_window: Some(context_window),
        };
        info.fill_to_context_window(context_window);
        info
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS)]
pub struct McpStartupUpdateEvent {
    /// Server name being started.
    pub server: String,
    /// Current startup status.
    pub status: McpStartupStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum McpStartupStatus {
    Starting,
    Ready,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS, Default)]
pub struct McpStartupCompleteEvent {
    pub ready: Vec<String>,
    pub failed: Vec<McpStartupFailure>,
    pub cancelled: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS)]
pub struct McpStartupFailure {
    pub server: String,
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthStatus {
    Unsupported,
    NotLoggedIn,
    BearerToken,
    OAuth,
}

impl fmt::Display for McpAuthStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            McpAuthStatus::Unsupported => "Unsupported",
            McpAuthStatus::NotLoggedIn => "Not logged in",
            McpAuthStatus::BearerToken => "Bearer token",
            McpAuthStatus::OAuth => "OAuth",
        };
        f.write_str(text)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn protocol_error_code_serialization() {
        let codes = vec![
            (ProtocolErrorCode::AlreadyResolved, "AlreadyResolved"),
            (
                ProtocolErrorCode::ParentSessionNotFound,
                "ParentSessionNotFound",
            ),
            (ProtocolErrorCode::ForkTurnNotFound, "ForkTurnNotFound"),
            (ProtocolErrorCode::ForkTurnNotStable, "ForkTurnNotStable"),
            (ProtocolErrorCode::PermissionDenied, "PermissionDenied"),
            (
                ProtocolErrorCode::WorkspaceUnavailable,
                "WorkspaceUnavailable",
            ),
            (
                ProtocolErrorCode::InheritedSegmentWriteFailed,
                "InheritedSegmentWriteFailed",
            ),
            (
                ProtocolErrorCode::ForkRetentionRequired,
                "ForkRetentionRequired",
            ),
            (
                ProtocolErrorCode::InvalidConfirmToken,
                "InvalidConfirmToken",
            ),
            (
                ProtocolErrorCode::UnsupportedDeletePolicy,
                "UnsupportedDeletePolicy",
            ),
            (
                ProtocolErrorCode::InheritedSegmentMaterializationFailed,
                "InheritedSegmentMaterializationFailed",
            ),
            (
                ProtocolErrorCode::ExpectedTargetMessageMismatch,
                "ExpectedTargetMessageMismatch",
            ),
            (
                ProtocolErrorCode::OlderMessageRequiresFork,
                "OlderMessageRequiresFork",
            ),
            (
                ProtocolErrorCode::ActiveTurnEditRejected,
                "ActiveTurnEditRejected",
            ),
            (
                ProtocolErrorCode::InvalidContentParts,
                "InvalidContentParts",
            ),
            (ProtocolErrorCode::InvalidMentions, "InvalidMentions"),
            (
                ProtocolErrorCode::WorkspaceRestoreFailedToStart,
                "WorkspaceRestoreFailedToStart",
            ),
            (
                ProtocolErrorCode::RestorePlanNotFound,
                "RESTORE_PLAN_NOT_FOUND",
            ),
            (
                ProtocolErrorCode::RestorePlanExpired,
                "RESTORE_PLAN_EXPIRED",
            ),
            (
                ProtocolErrorCode::WorkspaceVersionConflict,
                "WORKSPACE_VERSION_CONFLICT",
            ),
        ];

        for (code, expected_str) in &codes {
            let json = serde_json::to_string(&code)
                .unwrap_or_else(|e| panic!("serialize {expected_str}: {e}"));
            let restored: ProtocolErrorCode = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("deserialize {expected_str}: {e}"));
            assert_eq!(restored, *code, "roundtrip failed for {expected_str}");
        }
    }

    #[test]
    fn protocol_error_roundtrips_with_new_codes() {
        let error = ProtocolError {
            code: ProtocolErrorCode::PermissionDenied,
            message: "not authorized".into(),
            data: serde_json::json!({"path": "/etc/shadow"}),
        };
        let json = serde_json::to_string(&error).expect("serialize");
        let restored: ProtocolError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored, error);
    }

    #[test]
    fn protocol_error_code_display() {
        assert_eq!(
            ProtocolErrorCode::AlreadyResolved.to_string(),
            "AlreadyResolved"
        );
        assert_eq!(
            ProtocolErrorCode::ForkRetentionRequired.to_string(),
            "ForkRetentionRequired"
        );
    }

    #[test]
    fn client_method_recognizes_message_edit_previous() {
        assert_eq!(
            ClientMethod::parse("message/editPrevious"),
            Some(ClientMethod::MessageEditPrevious)
        );
        assert_eq!(
            ClientMethod::MessageEditPrevious.as_str(),
            "message/editPrevious"
        );
    }

    #[test]
    fn client_method_recognizes_mcp_list() {
        assert_eq!(ClientMethod::parse("mcp/list"), Some(ClientMethod::McpList));
        assert_eq!(ClientMethod::McpList.as_str(), "mcp/list");
    }

    #[test]
    fn client_method_recognizes_mcp_tools() {
        assert_eq!(
            ClientMethod::parse("mcp/tools"),
            Some(ClientMethod::McpTools)
        );
        assert_eq!(ClientMethod::McpTools.as_str(), "mcp/tools");
    }

    #[test]
    fn client_method_recognizes_workspace_changes_read() {
        assert_eq!(
            ClientMethod::parse("workspace/changes/read"),
            Some(ClientMethod::WorkspaceChangesRead)
        );
        assert_eq!(
            ClientMethod::WorkspaceChangesRead.as_str(),
            "workspace/changes/read"
        );
    }

    #[test]
    fn client_method_recognizes_two_phase_session_rollback() {
        assert_eq!(
            [
                ClientMethod::parse("session/rollback/preview"),
                ClientMethod::parse("session/rollback/commit"),
            ],
            [
                Some(ClientMethod::SessionRollbackPreview),
                Some(ClientMethod::SessionRollbackCommit),
            ]
        );
        assert_eq!(
            [
                ClientMethod::SessionRollbackPreview.as_str(),
                ClientMethod::SessionRollbackCommit.as_str(),
            ],
            ["session/rollback/preview", "session/rollback/commit"]
        );
    }

    #[test]
    fn client_method_recognizes_model_config_set() {
        assert_eq!(
            ClientMethod::parse("model/config/set"),
            Some(ClientMethod::ModelConfigSet)
        );
        assert_eq!(ClientMethod::ModelConfigSet.as_str(), "model/config/set");
    }

    #[test]
    fn client_method_does_not_recognize_legacy_approval_respond() {
        assert_eq!(ClientMethod::parse("approval/respond"), None);
    }

    #[test]
    fn client_method_recognizes_session_sandbox_profile_update() {
        assert_eq!(
            ClientMethod::parse("session/sandbox_profile/update"),
            Some(ClientMethod::SessionSandboxProfileUpdate)
        );
        assert_eq!(
            ClientMethod::SessionSandboxProfileUpdate.as_str(),
            "session/sandbox_profile/update"
        );
    }
}
