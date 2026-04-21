use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use devo_safety::legacy_permissions::PermissionPolicy;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Strongly typed tool name used by the improved runtime subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(pub SmolStr);

/// Strongly typed identifier for one tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

/// Explicitly models where one tool comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolOrigin {
    Local,
    Mcp {
        server_id: String,
        tool_name: String,
    },
}

/// Describes one model-visible tool definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinitionSpec {
    pub name: ToolName,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_mode: ToolOutputMode,
    pub execution_mode: ToolExecutionMode,
    pub capability_tags: Vec<ToolCapabilityTag>,
}

/// Describes the output shape returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolOutputMode {
    StructuredJson,
    Text,
    Mixed,
}

/// Describes whether one tool may run in parallel with other read-only tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolExecutionMode {
    ReadOnly,
    Mutating,
}

/// Tags one tool with the resources it may touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCapabilityTag {
    ReadFiles,
    WriteFiles,
    ExecuteProcess,
    NetworkAccess,
    SearchWorkspace,
    ReadImages,
}

/// Stores one runtime tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub tool_call_id: ToolCallId,
    pub session_id: String,
    pub turn_id: String,
    pub tool_name: ToolName,
    pub input: serde_json::Value,
    pub requested_at: DateTime<Utc>,
}

/// Carries the execution context visible to one tool run.
#[derive(Clone)]
pub struct ToolExecutionContext {
    pub session_id: String,
    pub turn_id: String,
    pub cwd: PathBuf,
    /// Included for migration compatibility while legacy tools remain active.
    pub permissions: Arc<dyn PermissionPolicy>,
    pub policy_snapshot: ToolPolicySnapshot,
    pub app_config: Arc<ToolRuntimeConfigSnapshot>,
}

/// Stores the safety-policy snapshot visible to one tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolPolicySnapshot {
    pub mode: String,
    pub summary: Option<String>,
}

/// Stores the runtime config snapshot visible to one tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRuntimeConfigSnapshot {
    pub enabled_tools: Vec<String>,
    pub max_parallel_read_tools: u16,
}

impl Default for ToolRuntimeConfigSnapshot {
    fn default() -> Self {
        Self {
            enabled_tools: Vec::new(),
            max_parallel_read_tools: 1,
        }
    }
}

/// Describes the normalized terminal outcome of a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolExecutionOutcome {
    Completed(ToolResultPayload),
    Failed(ToolFailure),
    Denied(ToolDenied),
    Interrupted,
}

/// Stores one executed invocation together with its terminal outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    pub tool_call_id: ToolCallId,
    pub tool_name: ToolName,
    pub outcome: ToolExecutionOutcome,
}

/// Stores the normalized successful payload returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultPayload {
    pub content: ToolContent,
    pub metadata: ToolResultMetadata,
}

/// Stores the content returned by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolContent {
    Text(String),
    Json(serde_json::Value),
    Mixed {
        text: Option<String>,
        json: Option<serde_json::Value>,
    },
}

/// Stores structured metadata for a tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolResultMetadata {
    pub truncated: bool,
    pub duration_ms: Option<u64>,
}

/// Stores a normalized terminal failure for a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFailure {
    pub code: String,
    pub message: String,
}

/// Stores a normalized policy denial for a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDenied {
    pub reason: String,
}

/// Reports incremental progress from a running tool.
pub trait ToolProgressReporter: Send + Sync {
    fn report(&self, message: &str);
}

/// No-op reporter used when a caller does not need incremental progress.
pub struct NullToolProgressReporter;

impl ToolProgressReporter for NullToolProgressReporter {
    fn report(&self, _message: &str) {}
}

/// Improved runtime tool contract.
#[async_trait]
pub trait RuntimeTool: Send + Sync {
    fn definition(&self) -> ToolDefinitionSpec;

    async fn validate(&self, input: &serde_json::Value) -> Result<(), ToolInputError>;

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: ToolExecutionContext,
        reporter: Arc<dyn ToolProgressReporter>,
    ) -> Result<ToolExecutionOutcome, ToolExecuteError>;
}

/// Describes failures during tool input validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ToolInputError {
    #[error("invalid tool input: {message}")]
    Invalid { message: String },
}

/// Describes normalized failures produced by the tool runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ToolExecuteError {
    #[error("unknown tool: {tool_name}")]
    UnknownTool { tool_name: String },
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[error("approval required: {message}")]
    ApprovalRequired { message: String },
    #[error("permission denied: {message}")]
    PermissionDenied { message: String },
    #[error("sandbox unavailable: {message}")]
    SandboxUnavailable { message: String },
    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },
    #[error("timeout: {message}")]
    Timeout { message: String },
    #[error("interrupted: {message}")]
    Interrupted { message: String },
    #[error("internal tool error: {message}")]
    Internal { message: String },
}
