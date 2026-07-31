//! Tool contracts per L3-BEH-TOOLS-001.
//!
//! Defines ToolContext, ToolResult (struct-based output), ToolCallError,
//! structured ToolProgress, and the ToolRegistry trait. These types coexist
//! with the existing invocation types to allow gradual migration.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::CollaborationMode;
use serde::Deserialize;
use serde::Serialize;

use crate::client_fs::ClientFilesystem;
use crate::coordinator::AgentToolCoordinator;
use crate::file_read_ledger::FileReadLedger;
use crate::invocation::ToolCallId;
use crate::tool_spec::ToolSpec;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
pub struct ToolBudgets {
    pub output_limit_bytes: usize,
    pub wall_time_limit_ms: Option<u64>,
}

// ── ToolContext ──────────────────────────────────────────────────────

/// Whether a tool invocation is running in a top-level session or a child agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAgentScope {
    /// A normal top-level session, allowed to use all configured tools.
    #[default]
    Parent,
    /// A child agent session. These sessions report through assistant output and cannot use agent coordination tools.
    Subagent,
}

/// Full execution context passed to every tool handler invocation.
#[derive(Clone)]
pub struct ToolContext {
    pub tool_call_id: ToolCallId,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub workspace_root: PathBuf,
    pub budgets: ToolBudgets,
    pub cancel_token: CancellationToken,
    pub agent_scope: ToolAgentScope,
    pub collaboration_mode: CollaborationMode,
    pub agent_coordinator: Option<Arc<dyn AgentToolCoordinator>>,
    pub client_filesystem: Option<Arc<dyn ClientFilesystem>>,
    /// Session-scoped ledger of files read/written by tools (used by `edit`).
    pub file_read_ledger: Option<Arc<FileReadLedger>>,
    pub network_proxy: Option<String>,
    pub network_no_proxy: Option<String>,
    /// Active sandbox profile name for child processes spawned by this tool.
    /// `None` means no sandboxing; otherwise the value is a profile name such
    /// as `"workspace"`, `"strict"`, or `"off"`.
    pub sandbox_profile: Option<String>,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("tool_call_id", &self.tool_call_id)
            .field("session_id", &self.session_id)
            .field("turn_id", &self.turn_id)
            .field("workspace_root", &self.workspace_root)
            .field("budgets", &self.budgets)
            .field("cancel_token", &self.cancel_token)
            .field("agent_scope", &self.agent_scope)
            .field("collaboration_mode", &self.collaboration_mode)
            .field(
                "agent_coordinator",
                &self.agent_coordinator.as_ref().map(|_| "<configured>"),
            )
            .field(
                "client_filesystem",
                &self.client_filesystem.as_ref().map(|_| "<configured>"),
            )
            .field(
                "file_read_ledger",
                &self.file_read_ledger.as_ref().map(|_| "<configured>"),
            )
            .field(
                "network_proxy",
                &self.network_proxy.as_ref().map(|_| "<configured>"),
            )
            .field(
                "network_no_proxy",
                &self.network_no_proxy.as_ref().map(|_| "<configured>"),
            )
            .finish_non_exhaustive()
    }
}

/// Minimal permission profile for tool context (full type in core).
#[derive(Debug, Clone, Copy)]
pub struct ToolPermissionProfile {
    pub can_read_workspace: bool,
    pub can_write_workspace: bool,
    pub can_execute_commands: bool,
    pub network_enabled: bool,
}

// ── ToolResult ───────────────────────────────────────────────────────

/// Structured tool output (struct, not trait — per L3-BEH-TOOLS-001).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Primary output content for model consumption.
    pub content: ToolResultContent,
    /// Optional display-only content (not sent to model).
    pub display_content: Option<String>,
    /// Structured terminal status of the tool execution.
    pub structured_status: ToolTerminalStatus,
    /// Human-readable summary for progress/status display.
    pub result_summary: String,
    /// Redaction state of the output.
    pub redaction_state: RedactionState,
    /// Safety notice if output was modified for safety reasons.
    pub safety_notice: Option<String>,
}

impl ToolResult {
    pub fn success(content: ToolResultContent, summary: impl Into<String>) -> Self {
        Self {
            content,
            display_content: None,
            structured_status: ToolTerminalStatus::Completed,
            result_summary: summary.into(),
            redaction_state: RedactionState::Clean,
            safety_notice: None,
        }
    }

    pub fn error(
        content: ToolResultContent,
        summary: impl Into<String>,
        error: ToolCallError,
    ) -> Self {
        Self {
            content,
            display_content: None,
            structured_status: ToolTerminalStatus::Failed(error),
            result_summary: summary.into(),
            redaction_state: RedactionState::Clean,
            safety_notice: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text(String),
    Json(serde_json::Value),
    Mixed {
        text: Option<String>,
        json: Option<serde_json::Value>,
    },
}

/// Terminal status of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolTerminalStatus {
    Completed,
    Denied { reason: String },
    BlockedByMode { reason: String },
    NeedsConfiguration { message: String },
    InvalidInput { details: String },
    Failed(ToolCallError),
    Canceled,
    Interrupted,
}

/// TODO: Should we keep it? Better to change the name `redaction` to `sanitize`.
/// Redaction state of tool output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactionState {
    Clean,
    Redacted,
    Blocked,
}

// ── ToolCallError ────────────────────────────────────────────────────

/// Structured tool error per L3-BEH-TOOLS-001.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
pub enum ToolCallError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("blocked by mode: {0}")]
    BlockedByMode(String),
    #[error("needs configuration: {0}")]
    NeedsConfiguration(String),
    #[error("denied: {0}")]
    Denied(String),
    #[error("approval required")]
    ApprovalRequired,
    #[error("timed out after {0}s")]
    TimedOut(u64),
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
    #[error("cancelled")]
    Cancelled,
    #[error("internal error: {0}")]
    InternalError(String),
}

impl ToolCallError {
    /// Whether this error is recoverable (retry may succeed).
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::TimedOut(_) | Self::InternalError(_))
    }
}

// ── ToolProgress ─────────────────────────────────────────────────────

/// Structured progress updates from long-running tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolProgress {
    /// Incremental output delta.
    OutputDelta { delta: String },
    /// Status update message.
    StatusUpdate {
        message: String,
        percent: Option<u8>,
    },
    /// Tool execution completed (terminal).
    Completion { summary: String },
}

/// Sender for tool progress updates.
pub type ToolProgressSender = tokio::sync::mpsc::UnboundedSender<ToolProgress>;

// ── ToolRegistry Trait ───────────────────────────────────────────────

/// Registry trait for looking up and listing available tools.
///
/// Per L3-BEH-TOOLS-001, ToolRegistry is a trait (not a concrete struct)
/// to allow different registry implementations.
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// Get a tool handler by name.
    fn get(&self, name: &str) -> Option<Arc<dyn ToolHandler>>;

    /// Get a tool spec by name.
    fn spec(&self, name: &str) -> Option<&ToolSpec>;

    /// List tools available in the given session mode and permission profile.
    fn list_available(
        &self,
        mode: &SessionMode,
        permission: &ToolPermissionProfile,
    ) -> Vec<&ToolSpec>;

    /// List all registered tool specs regardless of availability.
    fn list_all_specs(&self) -> Vec<&ToolSpec>;
}

/// No-op implementation of ToolRegistry for testing.
pub struct NoopToolRegistry;

impl ToolRegistry for NoopToolRegistry {
    fn get(&self, _name: &str) -> Option<Arc<dyn ToolHandler>> {
        None
    }

    fn spec(&self, _name: &str) -> Option<&ToolSpec> {
        None
    }

    fn list_available(
        &self,
        _mode: &SessionMode,
        _permission: &ToolPermissionProfile,
    ) -> Vec<&ToolSpec> {
        Vec::new()
    }

    fn list_all_specs(&self) -> Vec<&ToolSpec> {
        Vec::new()
    }
}

/// Session mode for tool availability gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Normal,
    Plan,
    Review,
}

// ToolHandler is defined in tool_handler.rs to avoid duplication.
// Re-exported here for convenience.
pub use crate::tool_handler::ToolHandler;

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_success() {
        let result = ToolResult::success(
            ToolResultContent::Text("file contents".into()),
            "Read 100 bytes",
        );
        assert!(matches!(
            result.structured_status,
            ToolTerminalStatus::Completed
        ));
        assert_eq!(result.result_summary, "Read 100 bytes");
        assert_eq!(result.redaction_state, RedactionState::Clean);
    }

    #[test]
    fn tool_result_error() {
        let result = ToolResult::error(
            ToolResultContent::Text("".into()),
            "Failed to read",
            ToolCallError::ExecutionFailed("permission denied".into()),
        );
        match &result.structured_status {
            ToolTerminalStatus::Failed(err) => {
                assert!(matches!(err, ToolCallError::ExecutionFailed(_)));
            }
            _ => panic!("expected Failed status"),
        }
    }

    #[test]
    fn tool_call_error_recoverability() {
        assert!(ToolCallError::TimedOut(30).is_recoverable());
        assert!(ToolCallError::InternalError("crash".into()).is_recoverable());
        assert!(!ToolCallError::InvalidInput("bad".into()).is_recoverable());
        assert!(!ToolCallError::Denied("nope".into()).is_recoverable());
    }

    #[test]
    fn tool_call_error_serde_roundtrip() {
        let errors = vec![
            ToolCallError::InvalidInput("missing field".into()),
            ToolCallError::BlockedByMode("plan mode".into()),
            ToolCallError::NeedsConfiguration("no API key".into()),
            ToolCallError::Denied("user said no".into()),
            ToolCallError::ApprovalRequired,
            ToolCallError::TimedOut(30),
            ToolCallError::ExecutionFailed("exit 1".into()),
            ToolCallError::Cancelled,
            ToolCallError::InternalError("panic".into()),
        ];
        for err in &errors {
            let json = serde_json::to_string(err).expect("serialize");
            let restored: ToolCallError = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(format!("{}", restored), format!("{}", err));
        }
    }

    #[test]
    fn tool_progress_serde_roundtrip() {
        let progress_items = vec![
            ToolProgress::OutputDelta {
                delta: "building...".into(),
            },
            ToolProgress::StatusUpdate {
                message: "50% done".into(),
                percent: Some(50),
            },
            ToolProgress::Completion {
                summary: "Build complete".into(),
            },
        ];
        for p in &progress_items {
            let json = serde_json::to_string(p).expect("serialize");
            let restored: ToolProgress = serde_json::from_str(&json).expect("deserialize");
            if let (
                ToolProgress::OutputDelta { delta: d1 },
                ToolProgress::OutputDelta { delta: d2 },
            ) = (p, &restored)
            {
                assert_eq!(d1, d2);
            }
        }
    }

    #[test]
    fn tool_terminal_status_all_variants() {
        let statuses = [
            ToolTerminalStatus::Completed,
            ToolTerminalStatus::Denied {
                reason: "nope".into(),
            },
            ToolTerminalStatus::BlockedByMode {
                reason: "plan".into(),
            },
            ToolTerminalStatus::NeedsConfiguration {
                message: "key missing".into(),
            },
            ToolTerminalStatus::InvalidInput {
                details: "bad field".into(),
            },
            ToolTerminalStatus::Failed(ToolCallError::Cancelled),
            ToolTerminalStatus::Canceled,
            ToolTerminalStatus::Interrupted,
        ];
        assert_eq!(statuses.len(), 8);
    }
}
