//! Unified application error model for all four API surfaces.
//!
//! Truth source: `devo-api-design/01-native-api.md` §7. Application errors use
//! the JSON-RPC `-32000..-32099` range; adapters translate this shape into
//! their own protocol's error form.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use ts_rs::TS;

/// Stable machine-readable error codes. UI localization keys off these, never
/// off `message`.
pub mod codes {
    pub const NOT_INITIALIZED: &str = "NOT_INITIALIZED";
    pub const UNSUPPORTED_PROTOCOL_VERSION: &str = "UNSUPPORTED_PROTOCOL_VERSION";
    pub const SESSION_NOT_FOUND: &str = "SESSION_NOT_FOUND";
    pub const GOAL_NOT_FOUND: &str = "GOAL_NOT_FOUND";
    pub const INVALID_CWD: &str = "INVALID_CWD";
    pub const CWD_ACCESS_DENIED: &str = "CWD_ACCESS_DENIED";
    pub const UNSUPPORTED_MODALITY: &str = "UNSUPPORTED_MODALITY";
    pub const INVALID_ITEM_SHAPE: &str = "INVALID_ITEM_SHAPE";
    pub const INVALID_TOOL_PAIRING: &str = "INVALID_TOOL_PAIRING";
    pub const TURN_ALREADY_ACTIVE: &str = "TURN_ALREADY_ACTIVE";
    pub const TURN_NOT_STEERABLE: &str = "TURN_NOT_STEERABLE";
    pub const QUEUE_ITEM_NOT_FOUND: &str = "QUEUE_ITEM_NOT_FOUND";
    pub const RESTORE_PLAN_NOT_FOUND: &str = "RESTORE_PLAN_NOT_FOUND";
    pub const RESTORE_PLAN_EXPIRED: &str = "RESTORE_PLAN_EXPIRED";
    pub const VERSION_CONFLICT: &str = "VERSION_CONFLICT";
    pub const WORKSPACE_VERSION_CONFLICT: &str = "WORKSPACE_VERSION_CONFLICT";
    pub const IDEMPOTENCY_CONFLICT: &str = "IDEMPOTENCY_CONFLICT";
    pub const CURSOR_EXPIRED: &str = "CURSOR_EXPIRED";
    pub const CONTROL_REQUEST_ALREADY_RESOLVED: &str = "CONTROL_REQUEST_ALREADY_RESOLVED";
    pub const GOAL_TRANSITION_INVALID: &str = "GOAL_TRANSITION_INVALID";
    pub const SERVER_OVERLOADED: &str = "SERVER_OVERLOADED";
    pub const PROVIDER_TEMPORARY_FAILURE: &str = "PROVIDER_TEMPORARY_FAILURE";
    pub const ROLLOUT_VERSION_UNSUPPORTED: &str = "ROLLOUT_VERSION_UNSUPPORTED";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentError {
    /// Stable machine-readable code, see `codes`.
    pub error_code: String,
    /// Developer-facing message.
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub field_violations: Vec<FieldViolation>,
    /// Current resource version when reporting a version conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<u64>,
    /// Whether the client must re-fetch a snapshot before retrying.
    pub requires_snapshot: bool,
    /// Must never contain secrets or restricted content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<JsonValue>,
}

impl AgentError {
    pub fn new(error_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error_code: error_code.into(),
            message: message.into(),
            retryable: false,
            retry_after_ms: None,
            field_violations: Vec::new(),
            current_version: None,
            requires_snapshot: false,
            details: None,
        }
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.error_code, self.message)
    }
}

impl std::error::Error for AgentError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct FieldViolation {
    pub field: String,
    pub message: String,
}
