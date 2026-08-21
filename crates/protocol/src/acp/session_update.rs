use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::AcpMeta;
use super::content::AcpContentBlock;
use crate::AcpMessageId;
use crate::AcpPermissionOptionId;
use crate::AcpSessionConfigOption;
use crate::AcpSessionModeId;
use crate::AcpToolCallId;
use crate::SessionId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionNotification {
    pub session_id: SessionId,
    pub update: AcpSessionUpdate,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpRequestPermissionParams {
    pub session_id: SessionId,
    pub tool_call: AcpToolCallUpdate,
    pub options: Vec<AcpPermissionOption>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpToolCallUpdate {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: AcpToolCallId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AcpToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AcpToolCallStatus>,
    #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<serde_json::Value>,
    #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<AcpToolCallContent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<AcpToolCallLocation>>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpToolCallLocation {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    #[serde(rename = "optionId")]
    pub option_id: AcpPermissionOptionId,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpRequestPermissionResponse {
    pub outcome: AcpPermissionOutcome,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpCost {
    pub amount: f64,
    pub currency: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAvailableCommandInput {
    pub hint: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAvailableCommand {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<AcpAvailableCommandInput>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AcpPermissionOutcome {
    Selected {
        #[serde(rename = "optionId")]
        option_id: AcpPermissionOptionId,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum AcpSessionUpdate {
    UserMessageChunk {
        content: AcpContentBlock,
        #[serde(default, rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<AcpMessageId>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    AgentMessageChunk {
        content: AcpContentBlock,
        #[serde(default, rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<AcpMessageId>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    AgentThoughtChunk {
        content: AcpContentBlock,
        #[serde(default, rename = "messageId", skip_serializing_if = "Option::is_none")]
        message_id: Option<AcpMessageId>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    ToolCall {
        #[serde(rename = "toolCallId")]
        tool_call_id: AcpToolCallId,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<AcpToolKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<AcpToolCallStatus>,
        #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
        raw_input: Option<serde_json::Value>,
        #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
        raw_output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<AcpToolCallContent>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locations: Vec<AcpToolCallLocation>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    ToolCallUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: AcpToolCallId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<AcpToolKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<AcpToolCallStatus>,
        #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
        raw_input: Option<serde_json::Value>,
        #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
        raw_output: Option<serde_json::Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<Vec<AcpToolCallContent>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<AcpToolCallLocation>>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Plan {
        entries: Vec<AcpPlanEntry>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    AvailableCommandsUpdate {
        #[serde(rename = "availableCommands")]
        available_commands: Vec<AcpAvailableCommand>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    CurrentModeUpdate {
        #[serde(rename = "currentModeId")]
        current_mode_id: AcpSessionModeId,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    ConfigOptionUpdate {
        #[serde(rename = "configOptions")]
        config_options: Vec<AcpSessionConfigOption>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    SessionInfoUpdate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, rename = "updatedAt", skip_serializing_if = "Option::is_none")]
        updated_at: Option<String>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    UsageUpdate {
        used: u64,
        size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost: Option<AcpCost>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpToolCallContent {
    Content {
        content: Box<AcpContentBlock>,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Diff {
        path: PathBuf,
        #[serde(default, rename = "oldText", skip_serializing_if = "Option::is_none")]
        old_text: Option<String>,
        #[serde(rename = "newText")]
        new_text: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
    Terminal {
        #[serde(rename = "terminalId")]
        terminal_id: String,
        #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
        meta: Option<AcpMeta>,
    },
}

impl AcpToolCallContent {
    /// Convenience constructor for a content block payload.
    pub fn content(content: AcpContentBlock) -> Self {
        Self::Content {
            content: Box::new(content),
            meta: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpPlanEntry {
    pub content: String,
    pub priority: AcpPlanEntryPriority,
    pub status: AcpPlanEntryStatus,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanEntryPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpPlanEntryStatus {
    Pending,
    InProgress,
    Completed,
}
