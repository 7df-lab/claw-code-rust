use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

pub use devo_config::McpAuthConfig;
pub use devo_config::McpCapability;
pub use devo_config::McpConfig;
pub use devo_config::McpOutputLimits;
pub use devo_config::McpRootsPolicy;
pub use devo_config::McpServerEnvVar;
pub use devo_config::McpServerId;
pub use devo_config::McpServerRecord;
pub use devo_config::McpStartupPolicy;
pub use devo_config::McpTransportConfig;
pub use devo_config::McpTrustPolicy;

pub const DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD: usize = 100;

const MCP_TOOL_PREFIX: &str = "mcp__";
const MCP_UI_META_KEY: &str = "ui";
const MCP_UI_VISIBILITY_META_KEY: &str = "visibility";
const MCP_UI_MODEL_VISIBILITY: &str = "model";

/// Metadata for one MCP tool after Devo normalizes names for model use.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolInfo {
    /// Raw MCP server id used for routing calls.
    pub server_id: McpServerId,
    /// Human-readable source/server name used for search text.
    pub server_display_name: String,
    /// Raw MCP tool name sent back to the server.
    pub raw_tool_name: String,
    /// Sanitized namespace component used for model-facing names.
    pub callable_namespace: String,
    /// Sanitized tool-name component used for model-facing names.
    pub callable_name: String,
    /// Flat Devo function name exposed to providers.
    pub flat_name: String,
    /// Whether calls can be executed in parallel by default.
    pub supports_parallel_tool_calls: bool,
    /// Optional source description used by tool search.
    pub source_description: Option<String>,
    /// Optional source description provided by the MCP server.
    pub description: Option<String>,
    /// JSON schema describing the MCP tool input shape.
    pub input_schema: Value,
    /// Whether the MCP server annotated this tool as read-only.
    pub read_only_hint: bool,
    /// Protocol metadata used for model-facing visibility decisions.
    pub meta: Option<Value>,
}

impl McpToolInfo {
    pub fn new(
        server_id: McpServerId,
        server_display_name: String,
        raw_tool_name: String,
        description: Option<String>,
        input_schema: Value,
        read_only_hint: bool,
        supports_parallel_tool_calls: bool,
    ) -> Self {
        let callable_namespace = sanitize_model_name(&server_id.0);
        let callable_name = sanitize_model_name(&raw_tool_name);
        let flat_name = format!("{MCP_TOOL_PREFIX}{callable_namespace}__{callable_name}");
        Self {
            server_id,
            server_display_name,
            raw_tool_name,
            callable_namespace,
            callable_name,
            flat_name,
            supports_parallel_tool_calls,
            source_description: None,
            description,
            input_schema,
            read_only_hint,
            meta: None,
        }
    }

    pub fn description(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| format!("Call MCP tool {}", self.raw_tool_name))
    }

    pub fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only_hint
    }
}

/// Stores the observed runtime status for one MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub server_id: McpServerId,
    pub startup_state: McpStartupState,
    pub auth_state: McpAuthState,
    pub tools: Vec<McpToolDescriptor>,
    pub resources: Vec<McpResourceDescriptor>,
    pub resource_templates: Vec<McpResourceTemplateDescriptor>,
    pub last_refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupState {
    Disabled,
    NotStarted,
    Starting,
    Ready,
    Failed,
    AuthRequired,
    Degraded,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthState {
    NotRequired,
    Authenticated,
    AuthRequired,
    AuthFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    pub server_id: McpServerId,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceDescriptor {
    pub server_id: McpServerId,
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceTemplateDescriptor {
    pub server_id: McpServerId,
    pub uri_template: String,
    pub name: String,
    pub description: Option<String>,
}

pub struct McpToolExposure {
    pub direct_tools: Vec<McpToolInfo>,
    pub deferred_tools: Vec<McpToolInfo>,
}

pub fn build_mcp_tool_exposure(all_mcp_tools: &[McpToolInfo]) -> McpToolExposure {
    let visible_tools = all_mcp_tools
        .iter()
        .filter(|tool| tool_is_model_visible(tool))
        .cloned()
        .collect::<Vec<_>>();

    if visible_tools.len() >= DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD {
        return McpToolExposure {
            direct_tools: Vec::new(),
            deferred_tools: visible_tools,
        };
    }

    McpToolExposure {
        direct_tools: visible_tools,
        deferred_tools: Vec::new(),
    }
}

/// Returns whether an MCP tool may be included in model-facing declarations.
pub fn tool_is_model_visible(tool: &McpToolInfo) -> bool {
    let Some(visibility) = tool
        .meta
        .as_ref()
        .and_then(|meta| meta.get(MCP_UI_META_KEY))
        .and_then(Value::as_object)
        .and_then(|ui| ui.get(MCP_UI_VISIBILITY_META_KEY))
        .and_then(Value::as_array)
    else {
        return true;
    };

    visibility
        .iter()
        .any(|target| target.as_str() == Some(MCP_UI_MODEL_VISIBILITY))
}

pub fn sanitize_model_name(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut last_was_separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            output.push('_');
            last_was_separator = true;
        }
    }
    while output.ends_with('_') {
        output.pop();
    }
    let leading_separators = output.bytes().take_while(|byte| *byte == b'_').count();
    if leading_separators > 0 {
        output.drain(..leading_separators);
    }
    if output.is_empty() {
        "tool".to_string()
    } else if output
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
    {
        format!("_{output}")
    } else {
        output
    }
}

#[async_trait]
pub trait McpManager: Send + Sync {
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError>;

    async fn discover_tools(&self) -> Result<Vec<McpToolInfo>, McpError>;

    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError>;

    /// Enable or disable one MCP server without affecting sibling clients.
    ///
    /// When `enabled` is true, the manager starts (or refreshes) that server.
    /// When false, it drops the client and marks the server disabled. Startup
    /// failures should update status to [`McpStartupState::Failed`] rather than
    /// poisoning other servers.
    async fn set_enabled(
        &self,
        server_id: &McpServerId,
        enabled: bool,
    ) -> Result<McpServerStatus, McpError>;

    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        input: Value,
    ) -> Result<Value, McpError>;

    async fn read_resource(&self, server_id: &McpServerId, uri: &str) -> Result<Value, McpError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum McpError {
    #[error("mcp server unavailable: {server_id}")]
    McpServerUnavailable { server_id: McpServerId },
    #[error("mcp startup failed: {server_id}: {message}")]
    McpStartupFailed {
        server_id: McpServerId,
        message: String,
    },
    #[error("mcp auth required: {server_id}")]
    McpAuthRequired { server_id: McpServerId },
    #[error("mcp protocol error: {server_id}: {message}")]
    McpProtocolError {
        server_id: McpServerId,
        message: String,
    },
    #[error("mcp tool invocation failed: {server_id}: {tool_name}: {message}")]
    McpToolInvocationFailed {
        server_id: McpServerId,
        tool_name: String,
        message: String,
    },
    #[error("mcp resource read failed: {server_id}: {uri}: {message}")]
    McpResourceReadFailed {
        server_id: McpServerId,
        uri: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    fn tool_with_meta(meta: Option<Value>) -> McpToolInfo {
        let mut tool = McpToolInfo::new(
            McpServerId("Docs Server".into()),
            "Docs Server".into(),
            "search-docs".into(),
            Some("Search".into()),
            json!({
                "type": "object",
                "properties": {}
            }),
            false,
            false,
        );
        tool.meta = meta;
        tool
    }

    #[test]
    fn tool_visibility_defaults_to_model_visible() {
        assert!(tool_is_model_visible(&tool_with_meta(None)));
    }

    #[test]
    fn tool_visibility_accepts_model_target() {
        let tool = tool_with_meta(Some(json!({
            "ui": { "visibility": ["component", "model"] }
        })));
        assert!(tool_is_model_visible(&tool));
    }

    #[test]
    fn tool_visibility_rejects_non_model_target() {
        let tool = tool_with_meta(Some(json!({
            "ui": { "visibility": ["component"] }
        })));
        assert!(!tool_is_model_visible(&tool));
    }

    #[test]
    fn model_name_sanitization_is_stable() {
        assert_eq!(sanitize_model_name("Docs Server"), "docs_server");
        assert_eq!(sanitize_model_name("echo-tool"), "echo_tool");
        assert_eq!(sanitize_model_name("123"), "_123");
        assert_eq!(sanitize_model_name("  Docs Server! "), "docs_server");
        assert_eq!(sanitize_model_name("!!!"), "tool");
    }

    #[test]
    fn exposure_uses_direct_mcp_tool_threshold() {
        let tools = (0..DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD)
            .map(|idx| {
                let mut tool = tool_with_meta(None);
                tool.flat_name = format!("mcp__server__tool_{idx}");
                tool
            })
            .collect::<Vec<_>>();
        let exposure = build_mcp_tool_exposure(&tools[..99]);
        assert_eq!(exposure.direct_tools.len(), 99);
        assert_eq!(exposure.deferred_tools.len(), 0);

        let exposure = build_mcp_tool_exposure(&tools);
        assert_eq!(exposure.direct_tools.len(), 0);
        assert_eq!(
            exposure.deferred_tools.len(),
            DIRECT_MCP_TOOL_EXPOSURE_THRESHOLD
        );
    }
}
