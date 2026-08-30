use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ACP_JSONRPC_VERSION;
use super::AcpMeta;
use super::content::AcpContentBlock;
use crate::AcpAuthCapabilities;
use crate::AcpAuthMethod;
use crate::AcpClientCapabilities;
use crate::AcpSessionAdditionalDirectoriesCapabilities;
use crate::AcpSessionCloseCapabilities;
use crate::AcpSessionConfigOption;
use crate::AcpSessionDeleteCapabilities;
use crate::AcpSessionListCapabilities;
use crate::AcpSessionModeState;
use crate::AcpSessionResumeCapabilities;
use crate::SessionId;

pub type AcpAuthMethodId = String;
pub type AcpSessionId = SessionId;
pub type AcpMessageId = String;
pub type AcpPermissionOptionId = String;
pub type AcpProtocolVersion = u16;
pub type AcpRequestId = serde_json::Value;
pub type AcpToolCallId = String;

fn jsonrpc_version() -> String {
    ACP_JSONRPC_VERSION.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpClientRequest<T> {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    pub params: T,
}

impl<T> AcpClientRequest<T> {
    pub fn new(id: serde_json::Value, method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpClientNotification<T> {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
}

impl<T> AcpClientNotification<T> {
    pub fn new(method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpSuccessResponse<T> {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub result: T,
}

impl<T> AcpSuccessResponse<T> {
    pub fn new(id: serde_json::Value, result: T) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            result,
        }
    }
}

pub fn acp_success_response<T: Serialize>(
    request_id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(AcpSuccessResponse::new(request_id, result))
        .expect("serialize ACP success response")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpErrorResponse {
    #[serde(default = "jsonrpc_version")]
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub error: AcpProtocolError,
}

impl AcpErrorResponse {
    pub fn new(
        id: serde_json::Value,
        code: AcpErrorCode,
        message: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            jsonrpc: jsonrpc_version(),
            id,
            error: AcpProtocolError {
                code: code as i64,
                message: message.into(),
                data,
            },
        }
    }
}

pub fn acp_error_response(
    request_id: serde_json::Value,
    code: AcpErrorCode,
    message: impl Into<String>,
) -> serde_json::Value {
    acp_error_response_with_data(request_id, code, message, serde_json::Value::Null)
}

pub fn acp_error_response_with_data(
    request_id: serde_json::Value,
    code: AcpErrorCode,
    message: impl Into<String>,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::to_value(AcpErrorResponse::new(request_id, code, message, data))
        .expect("serialize ACP error response")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpProtocolError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
    AuthenticationRequired = -32000,
    ServerError = -32001,
    ResourceNotFound = -32002,
    RequestCancelled = -32800,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpImplementation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

impl AcpImplementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            title: None,
            version: version.into(),
            meta: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeParams {
    pub protocol_version: AcpProtocolVersion,
    #[serde(default)]
    pub client_capabilities: AcpClientCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<AcpImplementation>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeResult {
    pub protocol_version: AcpProtocolVersion,
    #[serde(default)]
    pub agent_capabilities: AcpAgentCapabilities,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auth_methods: Vec<AcpAuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<AcpImplementation>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpAgentCapabilities {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default)]
    pub prompt_capabilities: AcpPromptCapabilities,
    #[serde(default)]
    pub mcp_capabilities: AcpMcpCapabilities,
    #[serde(default)]
    pub session_capabilities: AcpSessionCapabilities,
    #[serde(default, skip_serializing_if = "AcpAuthCapabilities::is_empty")]
    pub auth: AcpAuthCapabilities,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptCapabilities {
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub embedded_context: bool,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpMcpCapabilities {
    #[serde(default)]
    pub http: bool,
    #[serde(default)]
    pub sse: bool,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpHttpHeader {
    pub name: String,
    pub value: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum AcpMcpServer {
    Http(AcpMcpServerHttp),
    Sse(AcpMcpServerSse),
    Stdio(AcpMcpServerStdio),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum AcpMcpServerHttpType {
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum AcpMcpServerSseType {
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpMcpServerHttp {
    #[serde(rename = "type")]
    pub transport_type: AcpMcpServerHttpType,
    pub name: String,
    pub url: String,
    pub headers: Vec<AcpHttpHeader>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpMcpServerSse {
    #[serde(rename = "type")]
    pub transport_type: AcpMcpServerSseType,
    pub name: String,
    pub url: String,
    pub headers: Vec<AcpHttpHeader>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpMcpServerStdio {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<AcpEnvVariable>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpEnvVariable {
    pub name: String,
    pub value: String,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<AcpSessionListCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<AcpSessionDeleteCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<AcpSessionAdditionalDirectoriesCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<AcpSessionResumeCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<AcpSessionCloseCapabilities>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpNewSessionParams {
    pub cwd: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_directories: Vec<PathBuf>,
    pub mcp_servers: Vec<AcpMcpServer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_binding_id: Option<String>,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpNewSessionResult {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modes: Option<AcpSessionModeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<AcpSessionConfigOption>>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptParams {
    pub session_id: SessionId,
    pub prompt: Vec<AcpContentBlock>,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptResult {
    pub stop_reason: AcpStopReason,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AcpStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct AcpCancelParams {
    pub session_id: SessionId,
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}
