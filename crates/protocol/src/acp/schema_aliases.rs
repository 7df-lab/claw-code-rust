use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use super::AcpMeta;
use crate::AcpAnnotations;
use crate::AcpAuthenticateParams;
use crate::AcpAuthenticateResult;
use crate::AcpAvailableCommand;
use crate::AcpCancelParams;
use crate::AcpClientNotification;
use crate::AcpClientRequest;
use crate::AcpCloseSessionParams;
use crate::AcpCloseSessionResult;
use crate::AcpContentBlock;
use crate::AcpCost;
use crate::AcpDeleteSessionParams;
use crate::AcpDeleteSessionResult;
use crate::AcpEmbeddedResource;
use crate::AcpFsReadTextFileParams;
use crate::AcpFsReadTextFileResult;
use crate::AcpFsWriteTextFileParams;
use crate::AcpInitializeParams;
use crate::AcpInitializeResult;
use crate::AcpListSessionsParams;
use crate::AcpListSessionsResult;
use crate::AcpLoadSessionParams;
use crate::AcpLoadSessionResult;
use crate::AcpLogoutResult;
use crate::AcpNewSessionParams;
use crate::AcpNewSessionResult;
use crate::AcpPermissionOutcome;
use crate::AcpPlanEntry;
use crate::AcpPromptParams;
use crate::AcpPromptResult;
use crate::AcpRequestPermissionParams;
use crate::AcpResumeSessionParams;
use crate::AcpResumeSessionResult;
use crate::AcpSessionConfigOption;
use crate::AcpSessionModeId;
use crate::AcpSetConfigOptionParams;
use crate::AcpSetConfigOptionResult;
use crate::AcpSetModeParams;
use crate::AcpSetModeResult;
use crate::AcpSuccessResponse;
use crate::AcpToolCallContent;
use crate::AcpToolCallLocation;
use crate::AcpToolCallStatus;
use crate::AcpToolKind;

pub type AcpAuthenticateRequest = AcpAuthenticateParams;
pub type AcpAuthenticateResponse = AcpAuthenticateResult;
pub type AcpAgentAuthCapabilities = crate::AcpAuthCapabilities;
pub type AcpAuthMethodAgent = crate::AcpAuthMethod;
pub type AcpCancelNotification = AcpCancelParams;
pub type AcpCloseSessionRequest = AcpCloseSessionParams;
pub type AcpCloseSessionResponse = AcpCloseSessionResult;
pub type AcpDeleteSessionRequest = AcpDeleteSessionParams;
pub type AcpDeleteSessionResponse = AcpDeleteSessionResult;
pub type AcpEmbeddedResourceResource = AcpEmbeddedResource;
pub type AcpError = crate::AcpProtocolError;
pub type AcpExtNotification<T = serde_json::Value> = AcpClientNotification<T>;
pub type AcpExtRequest<T = serde_json::Value> = AcpClientRequest<T>;
pub type AcpExtResponse<T = serde_json::Value> = AcpSuccessResponse<T>;
pub type AcpInitializeRequest = AcpInitializeParams;
pub type AcpInitializeResponse = AcpInitializeResult;
pub type AcpListSessionsRequest = AcpListSessionsParams;
pub type AcpListSessionsResponse = AcpListSessionsResult;
pub type AcpLoadSessionRequest = AcpLoadSessionParams;
pub type AcpLoadSessionResponse = AcpLoadSessionResult;
pub type AcpLogoutRequest = ();
pub type AcpLogoutResponse = AcpLogoutResult;
pub type AcpNewSessionRequest = AcpNewSessionParams;
pub type AcpNewSessionResponse = AcpNewSessionResult;
pub type AcpContentChunk = AcpContentBlock;
pub type AcpPromptRequest = AcpPromptParams;
pub type AcpPromptResponse = AcpPromptResult;
pub type AcpReadTextFileRequest = AcpFsReadTextFileParams;
pub type AcpReadTextFileResponse = AcpFsReadTextFileResult;
pub type AcpRequestPermissionRequest = AcpRequestPermissionParams;
pub type AcpRequestPermissionOutcome = AcpPermissionOutcome;
pub type AcpResumeSessionRequest = AcpResumeSessionParams;
pub type AcpResumeSessionResponse = AcpResumeSessionResult;
pub type AcpSetSessionConfigOptionRequest = AcpSetConfigOptionParams;
pub type AcpSetSessionConfigOptionResponse = AcpSetConfigOptionResult;
pub type AcpSetSessionModeRequest = AcpSetModeParams;
pub type AcpSetSessionModeResponse = AcpSetModeResult;
pub type AcpWriteTextFileRequest = AcpFsWriteTextFileParams;
pub type AcpWriteTextFileResponse = crate::AcpEmptyResult;
pub type AcpUnstructuredCommandInput = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSelectedPermissionOutcome {
    #[serde(rename = "optionId")]
    pub option_id: crate::AcpPermissionOptionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTextContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AcpAnnotations>,
    pub text: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpImageContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AcpAnnotations>,
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAudioContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AcpAnnotations>,
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpResourceLink {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<AcpAnnotations>,
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "mimeType")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpContent {
    pub content: AcpContentBlock,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpDiff {
    pub path: PathBuf,
    #[serde(default, rename = "oldText", skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    #[serde(rename = "newText")]
    pub new_text: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpToolCall {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: crate::AcpToolCallId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AcpToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AcpToolCallStatus>,
    #[serde(default, rename = "rawInput", skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<serde_json::Value>,
    #[serde(default, rename = "rawOutput", skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<AcpToolCallContent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<AcpToolCallLocation>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPlan {
    pub entries: Vec<AcpPlanEntry>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAvailableCommandsUpdate {
    #[serde(rename = "availableCommands")]
    pub available_commands: Vec<AcpAvailableCommand>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpCurrentModeUpdate {
    #[serde(rename = "currentModeId")]
    pub current_mode_id: AcpSessionModeId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOptionUpdate {
    #[serde(rename = "configOptions")]
    pub config_options: Vec<AcpSessionConfigOption>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionInfoUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUsageUpdate {
    pub used: u64,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<AcpCost>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}
