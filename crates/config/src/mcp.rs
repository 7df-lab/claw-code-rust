use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Stable id for the bundled code_search MCP server.
pub const BUNDLED_CODE_SEARCH_MCP_SERVER_ID: &str = "code_search";

/// Environment variable forwarding rule for configured MCP servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServerEnvVar {
    /// Legacy config shape where the string is the environment variable name.
    Name(String),
    /// Explicit config shape that may choose where the value should be read.
    Config {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
}

impl McpServerEnvVar {
    pub fn name(&self) -> &str {
        match self {
            Self::Name(name) | Self::Config { name, .. } => name,
        }
    }

    pub fn is_remote_source(&self) -> bool {
        matches!(
            self,
            Self::Config {
                source: Some(source),
                ..
            } if source == "remote"
        )
    }
}

impl From<String> for McpServerEnvVar {
    fn from(value: String) -> Self {
        Self::Name(value)
    }
}

impl From<&str> for McpServerEnvVar {
    fn from(value: &str) -> Self {
        Self::Name(value.to_string())
    }
}

/// Stores normalized MCP runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// The configured MCP servers.
    #[serde(default)]
    pub servers: Vec<McpServerRecord>,
    /// Whether enabled servers should be auto-started during bootstrap.
    #[serde(default = "default_mcp_auto_start")]
    pub auto_start: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        let mut config = Self {
            servers: Vec::new(),
            auto_start: true,
        };
        ensure_bundled_mcp_servers(&mut config);
        config
    }
}

impl McpConfig {
    /// Inserts missing bundled MCP servers without overwriting user records.
    pub fn ensure_bundled_servers(&mut self) {
        ensure_bundled_mcp_servers(self);
    }

    /// Sets `cwd` on the bundled `code_search` stdio server when it is unset.
    ///
    /// Used so session workspace roots are passed through the existing MCP
    /// transport `cwd` field without special-casing the MCP manager.
    pub fn with_code_search_workspace_cwd(mut self, cwd: PathBuf) -> Self {
        self.apply_code_search_workspace_cwd(cwd);
        self
    }

    /// Sets `cwd` on the bundled `code_search` stdio server when it is unset.
    pub fn apply_code_search_workspace_cwd(&mut self, cwd: PathBuf) {
        for record in &mut self.servers {
            if record.id.0 != BUNDLED_CODE_SEARCH_MCP_SERVER_ID {
                continue;
            }
            if let McpTransportConfig::Stdio {
                cwd: server_cwd, ..
            } = &mut record.transport
                && server_cwd.is_none()
            {
                *server_cwd = Some(cwd);
            }
            break;
        }
    }

    /// Returns whether two MCP configs can share a live manager/registry.
    ///
    /// Used by `SessionRuntimeContext::load_for_workspace` to avoid rebuilding
    /// MCP state (and re-spawning lazy servers) for every workspace lookup.
    ///
    /// Workspace injection of `code_search` cwd is ignored while that server is
    /// disabled, so process-level and workspace-level managers stay reusable.
    /// When `code_search` is enabled, cwd must match or managers are not shared.
    pub fn is_operationally_equivalent_to(&self, other: &Self) -> bool {
        self.normalized_for_runtime_equivalence() == other.normalized_for_runtime_equivalence()
    }

    fn normalized_for_runtime_equivalence(&self) -> Self {
        let mut normalized = self.clone();
        for record in &mut normalized.servers {
            if record.id.0 != BUNDLED_CODE_SEARCH_MCP_SERVER_ID || record.enabled {
                continue;
            }
            if let McpTransportConfig::Stdio { cwd, .. } = &mut record.transport {
                *cwd = None;
            }
        }
        normalized
    }
}

/// Returns the bundled, disabled-by-default code_search MCP server record.
pub fn bundled_code_search_mcp_server() -> McpServerRecord {
    McpServerRecord {
        id: McpServerId(BUNDLED_CODE_SEARCH_MCP_SERVER_ID.to_string()),
        display_name: "Code Search".to_string(),
        transport: McpTransportConfig::Stdio {
            command: vec!["devo-code-search-mcp".to_string()],
            cwd: None,
            env: BTreeMap::new(),
            env_vars: vec![
                McpServerEnvVar::from("DEVO_HOME"),
                McpServerEnvVar::from("HTTP_PROXY"),
                McpServerEnvVar::from("HTTPS_PROXY"),
                McpServerEnvVar::from("ALL_PROXY"),
                McpServerEnvVar::from("NO_PROXY"),
                McpServerEnvVar::from("http_proxy"),
                McpServerEnvVar::from("https_proxy"),
                McpServerEnvVar::from("all_proxy"),
                McpServerEnvVar::from("no_proxy"),
            ],
        },
        startup_policy: McpStartupPolicy::Lazy,
        enabled: false,
        trust_policy: McpTrustPolicy::default(),
        allowed_capabilities: Vec::new(),
        roots_policy: McpRootsPolicy::default(),
        output_limits: McpOutputLimits::default(),
        auth_ref: None,
    }
}

fn ensure_bundled_mcp_servers(config: &mut McpConfig) {
    let bundled = bundled_code_search_mcp_server();
    if config
        .servers
        .iter()
        .any(|record| record.id.0 == bundled.id.0)
    {
        return;
    }
    config.servers.push(bundled);
}

fn default_mcp_auto_start() -> bool {
    true
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
    #[serde(default)]
    pub startup_policy: McpStartupPolicy,
    /// Whether the server is enabled for runtime use.
    #[serde(default = "default_mcp_server_enabled")]
    pub enabled: bool,
    /// Trust policy for this MCP server.
    #[serde(default)]
    pub trust_policy: McpTrustPolicy,
    /// Allowed capabilities.
    #[serde(default)]
    pub allowed_capabilities: Vec<McpCapability>,
    /// Filesystem roots policy for resource access.
    #[serde(default)]
    pub roots_policy: McpRootsPolicy,
    /// Output limits for tool invocations.
    #[serde(default)]
    pub output_limits: McpOutputLimits,
    /// Optional auth credential reference.
    #[serde(default)]
    pub auth_ref: Option<String>,
}

fn default_mcp_server_enabled() -> bool {
    true
}

/// Strongly typed identifier for one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpServerId(pub String);

impl std::fmt::Display for McpServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Describes how the runtime connects to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    /// Launch the server as a stdio child process.
    Stdio {
        /// The command and arguments used to launch the server.
        command: Vec<String>,
        /// The working directory for the child process, if any.
        cwd: Option<PathBuf>,
        /// Environment variables provided directly to the child process.
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Environment variables inherited from the local process.
        #[serde(default)]
        env_vars: Vec<McpServerEnvVar>,
    },
    /// Connect to the server over streamable HTTP.
    StreamableHttp {
        /// The MCP server endpoint URL.
        url: String,
        /// Optional authentication configuration.
        #[serde(default)]
        auth: Option<McpAuthConfig>,
        /// Static HTTP headers sent to the MCP server.
        #[serde(default)]
        http_headers: BTreeMap<String, String>,
        /// HTTP headers loaded from local environment variables.
        #[serde(default)]
        env_http_headers: BTreeMap<String, String>,
    },
    /// Connect to the server over the deprecated MCP HTTP+SSE transport.
    Sse {
        /// The MCP server SSE endpoint URL.
        url: String,
        /// Optional authentication configuration.
        #[serde(default)]
        auth: Option<McpAuthConfig>,
        /// Static HTTP headers sent to the MCP server.
        #[serde(default)]
        http_headers: BTreeMap<String, String>,
        /// HTTP headers loaded from local environment variables.
        #[serde(default)]
        env_http_headers: BTreeMap<String, String>,
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
}

/// Controls when an enabled MCP server should be started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpStartupPolicy {
    /// Start the server automatically during runtime bootstrap.
    #[default]
    Eager,
    /// Start the server lazily on first use.
    Lazy,
    /// Never auto-start the server; start only by explicit request.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTrustPolicy {
    #[default]
    User,
    Workspace,
    Untrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCapability {
    Tools,
    Resources,
    Prompts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpRootsPolicy {
    #[default]
    None,
    Workspace,
    Custom(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOutputLimits {
    #[serde(default)]
    pub max_tool_output_bytes: Option<u64>,
    #[serde(default)]
    pub max_resource_bytes: Option<u64>,
}

impl Default for McpOutputLimits {
    fn default() -> Self {
        Self {
            max_tool_output_bytes: Some(1_048_576),
            max_resource_bytes: Some(10_485_760),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    fn bundled_stdio_env_vars() -> Vec<McpServerEnvVar> {
        match bundled_code_search_mcp_server().transport {
            McpTransportConfig::Stdio { env_vars, .. } => env_vars,
            _ => Vec::new(),
        }
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: default MCP config includes the disabled bundled code_search server.
    #[test]
    fn default_mcp_config_includes_disabled_code_search_server() {
        let config = McpConfig::default();
        let server = config
            .servers
            .iter()
            .find(|record| record.id.0 == BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
            .expect("bundled code_search server");
        assert!(!server.enabled);
        assert_eq!(server.startup_policy, McpStartupPolicy::Lazy);
        assert_eq!(
            server.transport,
            McpTransportConfig::Stdio {
                command: vec!["devo-code-search-mcp".to_string()],
                cwd: None,
                env: BTreeMap::new(),
                env_vars: bundled_stdio_env_vars(),
            }
        );
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: ensure_bundled_servers inserts missing records but preserves user ones.
    #[test]
    fn ensure_bundled_servers_preserves_existing_code_search_record() {
        let mut config = McpConfig {
            servers: vec![McpServerRecord {
                id: McpServerId(BUNDLED_CODE_SEARCH_MCP_SERVER_ID.to_string()),
                display_name: "Custom".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: vec!["custom".to_string()],
                    cwd: None,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                },
                startup_policy: McpStartupPolicy::Eager,
                enabled: true,
                trust_policy: McpTrustPolicy::default(),
                allowed_capabilities: Vec::new(),
                roots_policy: McpRootsPolicy::default(),
                output_limits: McpOutputLimits::default(),
                auth_ref: None,
            }],
            auto_start: true,
        };
        config.ensure_bundled_servers();
        assert_eq!(config.servers.len(), 1);
        assert!(config.servers[0].enabled);
        assert_eq!(config.servers[0].display_name, "Custom");
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: ensure_bundled_servers inserts the bundled server when absent.
    #[test]
    fn ensure_bundled_servers_inserts_when_missing() {
        let mut config = McpConfig {
            servers: Vec::new(),
            auto_start: true,
        };
        config.ensure_bundled_servers();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].id.0, BUNDLED_CODE_SEARCH_MCP_SERVER_ID);
        assert!(!config.servers[0].enabled);
    }

    /// Trace: L2-DES-MCP-002
    /// Verifies: apply_code_search_workspace_cwd fills cwd only when unset.
    #[test]
    fn apply_code_search_workspace_cwd_sets_missing_cwd_only() {
        let mut config = McpConfig::default();
        config.apply_code_search_workspace_cwd(PathBuf::from("/workspace"));
        let server = config
            .servers
            .iter()
            .find(|record| record.id.0 == BUNDLED_CODE_SEARCH_MCP_SERVER_ID)
            .expect("bundled server");
        match &server.transport {
            McpTransportConfig::Stdio { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some(Path::new("/workspace")));
            }
            _ => panic!("expected stdio transport"),
        }

        let mut custom = McpConfig {
            servers: vec![McpServerRecord {
                id: McpServerId(BUNDLED_CODE_SEARCH_MCP_SERVER_ID.to_string()),
                display_name: "Custom".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: vec!["custom".to_string()],
                    cwd: Some(PathBuf::from("/explicit")),
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                },
                startup_policy: McpStartupPolicy::Eager,
                enabled: true,
                trust_policy: McpTrustPolicy::default(),
                allowed_capabilities: Vec::new(),
                roots_policy: McpRootsPolicy::default(),
                output_limits: McpOutputLimits::default(),
                auth_ref: None,
            }],
            auto_start: true,
        };
        custom.apply_code_search_workspace_cwd(PathBuf::from("/workspace"));
        match &custom.servers[0].transport {
            McpTransportConfig::Stdio { cwd, .. } => {
                assert_eq!(cwd.as_deref(), Some(Path::new("/explicit")));
            }
            _ => panic!("expected stdio transport"),
        }
    }

    #[test]
    fn operational_equivalence_ignores_disabled_code_search_cwd() {
        let mut left = McpConfig::default();
        let mut right = McpConfig::default();
        left.apply_code_search_workspace_cwd(PathBuf::from("/process-cwd"));
        right.apply_code_search_workspace_cwd(PathBuf::from("/workspace-cwd"));

        assert!(left.is_operationally_equivalent_to(&right));
    }

    #[test]
    fn operational_equivalence_requires_enabled_code_search_cwd_match() {
        let mut left = McpConfig::default();
        let mut right = McpConfig::default();
        left.servers[0].enabled = true;
        right.servers[0].enabled = true;
        left.apply_code_search_workspace_cwd(PathBuf::from("/process-cwd"));
        right.apply_code_search_workspace_cwd(PathBuf::from("/workspace-cwd"));

        assert!(!left.is_operationally_equivalent_to(&right));
    }
}
