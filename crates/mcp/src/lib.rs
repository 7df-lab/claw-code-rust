use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Strongly typed identifier for one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpServerId(
    /// The stable string identifier for the server.
    pub SmolStr,
);

impl fmt::Display for McpServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Stores the configured metadata for one MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerRecord {
    /// The stable unique server identifier.
    pub id: McpServerId,
    /// The human-readable display name for the server.
    pub display_name: String,
    /// The transport configuration used to connect to the server.
    pub transport: McpTransportConfig,
    /// The startup policy applied to the server.
    pub startup_policy: McpStartupPolicy,
    /// Whether the server is enabled for runtime use.
    pub enabled: bool,
}

/// Describes how the runtime connects to an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    /// Launch the server as a stdio child process.
    Stdio {
        /// The command and arguments used to launch the server.
        command: Vec<String>,
        /// The working directory for the child process, if any.
        cwd: Option<PathBuf>,
        /// Environment variables provided to the child process.
        env: BTreeMap<String, String>,
    },
    /// Connect to the server over streamable HTTP.
    StreamableHttp {
        /// The base URL for the MCP server endpoint.
        base_url: String,
        /// Optional authentication configuration.
        auth: Option<McpAuthConfig>,
    },
}

/// Stores authentication configuration for MCP HTTP transports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAuthConfig {
    /// Use a bearer token for authorization.
    BearerToken {
        /// The bearer token value.
        token: String,
    },
    /// Use a static API key header.
    ApiKey {
        /// The header name that carries the API key.
        header_name: String,
        /// The API key value.
        value: String,
    },
}

/// Controls when an enabled MCP server should be started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupPolicy {
    /// Start the server automatically during runtime bootstrap.
    Eager,
    /// Start the server lazily on first use.
    Lazy,
    /// Never auto-start the server; start only by explicit request.
    Manual,
}

/// Stores the observed runtime status for one MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerStatus {
    /// The server whose status is being reported.
    pub server_id: McpServerId,
    /// The current startup state.
    pub startup_state: McpStartupState,
    /// The current authentication state.
    pub auth_state: McpAuthState,
    /// The discovered tool catalog.
    pub tools: Vec<McpToolDescriptor>,
    /// The discovered resource catalog.
    pub resources: Vec<McpResourceDescriptor>,
    /// The discovered resource-template catalog.
    pub resource_templates: Vec<McpResourceTemplateDescriptor>,
    /// The last refresh timestamp, if the catalog has been loaded.
    pub last_refreshed_at: Option<DateTime<Utc>>,
}

/// Tracks the startup lifecycle of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupState {
    /// The server has not been started yet.
    Stopped,
    /// The server is currently starting.
    Starting,
    /// The server is ready to serve MCP requests.
    Ready,
    /// The server failed to start.
    Failed,
}

/// Tracks the authentication state of an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuthState {
    /// No authentication is required.
    NotRequired,
    /// Authentication is configured and currently valid.
    Authenticated,
    /// The server requires authentication before requests may proceed.
    AuthRequired,
    /// Authentication failed.
    AuthFailed,
}

/// Describes one tool discovered from an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolDescriptor {
    /// The originating server identifier.
    pub server_id: McpServerId,
    /// The stable MCP tool name.
    pub name: String,
    /// The human-readable tool description.
    pub description: String,
    /// The JSON schema describing the input shape.
    pub input_schema: serde_json::Value,
}

/// Describes one resource discovered from an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceDescriptor {
    /// The originating server identifier.
    pub server_id: McpServerId,
    /// The stable resource URI.
    pub uri: String,
    /// The resource display name.
    pub name: String,
    /// Optional resource description text.
    pub description: Option<String>,
}

/// Describes one resource template discovered from an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpResourceTemplateDescriptor {
    /// The originating server identifier.
    pub server_id: McpServerId,
    /// The template URI or URI pattern.
    pub uri_template: String,
    /// The template display name.
    pub name: String,
    /// Optional template description text.
    pub description: Option<String>,
}

/// Stores normalized MCP runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// The configured MCP servers.
    pub servers: Vec<McpServerRecord>,
    /// Whether enabled servers should be auto-started during bootstrap.
    pub auto_start: bool,
    /// Whether config reload should refresh running server catalogs.
    pub refresh_on_config_reload: bool,
}

/// Provides lifecycle and dispatch operations for MCP integration.
#[async_trait]
pub trait McpManager: Send + Sync {
    /// Returns the current runtime status for every configured server.
    async fn statuses(&self) -> Result<Vec<McpServerStatus>, McpError>;

    /// Refreshes discovery metadata for one server and returns the updated status.
    async fn refresh(&self, server_id: &McpServerId) -> Result<McpServerStatus, McpError>;

    /// Dispatches one MCP tool call and returns normalized JSON output.
    async fn invoke_tool(
        &self,
        server_id: &McpServerId,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<serde_json::Value, McpError>;

    /// Reads one MCP resource and returns normalized JSON output.
    async fn read_resource(
        &self,
        server_id: &McpServerId,
        uri: &str,
    ) -> Result<serde_json::Value, McpError>;
}

/// Enumerates the normalized MCP failure categories exposed to the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum McpError {
    /// The requested server is unavailable.
    #[error("mcp server unavailable: {server_id}")]
    McpServerUnavailable {
        /// The server that could not be reached.
        server_id: McpServerId,
    },
    /// The server failed during startup.
    #[error("mcp startup failed: {server_id}: {message}")]
    McpStartupFailed {
        /// The server that failed to start.
        server_id: McpServerId,
        /// The human-readable failure message.
        message: String,
    },
    /// The server requires authentication before the request may proceed.
    #[error("mcp auth required: {server_id}")]
    McpAuthRequired {
        /// The server requiring authentication.
        server_id: McpServerId,
    },
    /// The server returned a protocol-level failure.
    #[error("mcp protocol error: {server_id}: {message}")]
    McpProtocolError {
        /// The server that reported the protocol error.
        server_id: McpServerId,
        /// The human-readable failure message.
        message: String,
    },
    /// The tool invocation failed.
    #[error("mcp tool invocation failed: {server_id}: {tool_name}: {message}")]
    McpToolInvocationFailed {
        /// The server that owns the tool.
        server_id: McpServerId,
        /// The tool name that failed.
        tool_name: String,
        /// The human-readable failure message.
        message: String,
    },
    /// Reading the resource failed.
    #[error("mcp resource read failed: {server_id}: {uri}: {message}")]
    McpResourceReadFailed {
        /// The server that owns the resource.
        server_id: McpServerId,
        /// The resource URI that failed.
        uri: String,
        /// The human-readable failure message.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        McpAuthState, McpConfig, McpResourceDescriptor, McpServerId, McpServerRecord,
        McpServerStatus, McpStartupPolicy, McpStartupState, McpToolDescriptor, McpTransportConfig,
    };

    #[test]
    fn server_status_roundtrip() {
        let status = McpServerStatus {
            server_id: McpServerId("docs".into()),
            startup_state: McpStartupState::Ready,
            auth_state: McpAuthState::Authenticated,
            tools: vec![McpToolDescriptor {
                server_id: McpServerId("docs".into()),
                name: "search".into(),
                description: "Search docs".into(),
                input_schema: serde_json::json!({"type":"object"}),
            }],
            resources: vec![McpResourceDescriptor {
                server_id: McpServerId("docs".into()),
                uri: "resource://doc".into(),
                name: "Doc".into(),
                description: None,
            }],
            resource_templates: Vec::new(),
            last_refreshed_at: None,
        };

        let json = serde_json::to_string(&status).expect("serialize");
        let restored: McpServerStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(status, restored);
    }

    #[test]
    fn config_holds_server_records() {
        let config = McpConfig {
            servers: vec![McpServerRecord {
                id: McpServerId("docs".into()),
                display_name: "Docs".into(),
                transport: McpTransportConfig::Stdio {
                    command: vec!["node".into(), "server.js".into()],
                    cwd: None,
                    env: Default::default(),
                },
                startup_policy: McpStartupPolicy::Lazy,
                enabled: true,
            }],
            auto_start: true,
            refresh_on_config_reload: true,
        };

        assert_eq!(config.servers.len(), 1);
        assert!(config.auto_start);
    }
}
