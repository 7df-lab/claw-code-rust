//! Render current MCP server configuration into transcript-friendly markdown.

use devo_core::McpConfig;
use devo_core::McpStartupPolicy;
use devo_core::McpTransportConfig;

pub(crate) fn render_mcp_servers_markdown(config: &McpConfig) -> String {
    if config.servers.is_empty() {
        return "_No MCP servers configured._".to_string();
    }

    let mut body = String::new();
    for (index, server) in config.servers.iter().enumerate() {
        if index > 0 {
            body.push_str("\n\n");
        }

        let startup_policy = match server.startup_policy {
            McpStartupPolicy::Eager => "eager",
            McpStartupPolicy::Lazy => "lazy",
            McpStartupPolicy::Manual => "manual",
        };
        let transport_kind = match &server.transport {
            McpTransportConfig::Stdio { .. } => "stdio",
            McpTransportConfig::StreamableHttp { .. } => "streamable_http",
            McpTransportConfig::Sse { .. } => "sse",
        };
        let target = match &server.transport {
            McpTransportConfig::Stdio { command, .. } => {
                if command.is_empty() {
                    "(empty command)".to_string()
                } else {
                    command.join(" ")
                }
            }
            McpTransportConfig::StreamableHttp { url, .. } => url.clone(),
            McpTransportConfig::Sse { url, .. } => url.clone(),
        };
        let enabled = if server.enabled { "yes" } else { "no" };

        body.push_str(&format!(
            "- `{}` - {}\n  enabled: {}\n  startup: {}\n  transport: {}\n  target: `{}`",
            server.id.0, server.display_name, enabled, startup_policy, transport_kind, target
        ));
    }

    body
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_core::McpServerId;
    use devo_core::McpServerRecord;

    #[test]
    fn render_mcp_servers_markdown_handles_empty_config() {
        assert_eq!(
            render_mcp_servers_markdown(&McpConfig {
                servers: Vec::new(),
                auto_start: true,
            }),
            "_No MCP servers configured._"
        );
    }

    #[test]
    fn render_mcp_servers_markdown_lists_configured_servers() {
        let body = render_mcp_servers_markdown(&McpConfig {
            servers: vec![
                McpServerRecord {
                    id: McpServerId("docs".to_string()),
                    display_name: "Docs".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: vec!["npx".to_string(), "@mcp/docs".to_string()],
                        cwd: None,
                        env: Default::default(),
                        env_vars: Vec::new(),
                    },
                    startup_policy: McpStartupPolicy::Lazy,
                    enabled: true,
                    trust_policy: Default::default(),
                    allowed_capabilities: Vec::new(),
                    roots_policy: Default::default(),
                    output_limits: Default::default(),
                    auth_ref: None,
                },
                McpServerRecord {
                    id: McpServerId("browser".to_string()),
                    display_name: "Browser".to_string(),
                    transport: McpTransportConfig::StreamableHttp {
                        url: "https://mcp.example.com".to_string(),
                        auth: None,
                        http_headers: Default::default(),
                        env_http_headers: Default::default(),
                    },
                    startup_policy: McpStartupPolicy::Manual,
                    enabled: false,
                    trust_policy: Default::default(),
                    allowed_capabilities: Vec::new(),
                    roots_policy: Default::default(),
                    output_limits: Default::default(),
                    auth_ref: None,
                },
                McpServerRecord {
                    id: McpServerId("events".to_string()),
                    display_name: "Events".to_string(),
                    transport: McpTransportConfig::Sse {
                        url: "https://events.example.com/sse".to_string(),
                        auth: None,
                        http_headers: Default::default(),
                        env_http_headers: Default::default(),
                    },
                    startup_policy: McpStartupPolicy::Eager,
                    enabled: true,
                    trust_policy: Default::default(),
                    allowed_capabilities: Vec::new(),
                    roots_policy: Default::default(),
                    output_limits: Default::default(),
                    auth_ref: None,
                },
            ],
            auto_start: true,
        });

        assert!(body.contains("`docs` - Docs"));
        assert!(body.contains("startup: lazy"));
        assert!(body.contains("transport: stdio"));
        assert!(body.contains("target: `npx @mcp/docs`"));
        assert!(body.contains("`browser` - Browser"));
        assert!(body.contains("enabled: no"));
        assert!(body.contains("transport: streamable_http"));
        assert!(body.contains("target: `https://mcp.example.com`"));
        assert!(body.contains("`events` - Events"));
        assert!(body.contains("transport: sse"));
        assert!(body.contains("target: `https://events.example.com/sse`"));
    }
}
