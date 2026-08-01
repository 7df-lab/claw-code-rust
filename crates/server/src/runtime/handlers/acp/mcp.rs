use super::*;

pub(super) fn acp_mcp_config(
    method: &str,
    mcp_servers: &[AcpMcpServer],
) -> Result<McpConfig, String> {
    let mut ids = HashSet::new();
    let mut records = Vec::with_capacity(mcp_servers.len());

    for mcp_server in mcp_servers {
        let (id, transport) = match mcp_server {
            AcpMcpServer::Stdio(server) => (
                acp_mcp_server_id(method, server.name.as_str())?,
                McpTransportConfig::Stdio {
                    command: acp_stdio_command(method, server)?,
                    cwd: None,
                    env: acp_stdio_env(method, server)?,
                    env_vars: Vec::new(),
                },
            ),
            AcpMcpServer::Http(server) => {
                if server.url.trim().is_empty() {
                    return Err(format!(
                        "{method} mcpServers http entry '{}' must include a non-empty url",
                        server.name
                    ));
                }
                (
                    acp_mcp_server_id(method, server.name.as_str())?,
                    McpTransportConfig::StreamableHttp {
                        url: server.url.clone(),
                        auth: None,
                        http_headers: acp_http_headers(
                            method,
                            "http",
                            server.name.as_str(),
                            &server.headers,
                        )?,
                        env_http_headers: BTreeMap::new(),
                    },
                )
            }
            AcpMcpServer::Sse(server) => {
                if server.url.trim().is_empty() {
                    return Err(format!(
                        "{method} mcpServers sse entry '{}' must include a non-empty url",
                        server.name
                    ));
                }
                (
                    acp_mcp_server_id(method, server.name.as_str())?,
                    McpTransportConfig::Sse {
                        url: server.url.clone(),
                        auth: None,
                        http_headers: acp_http_headers(
                            method,
                            "sse",
                            server.name.as_str(),
                            &server.headers,
                        )?,
                        env_http_headers: BTreeMap::new(),
                    },
                )
            }
            AcpMcpServer::Unsupported(server) => {
                return Err(format!(
                    "{method} mcpServers transport '{}' is not supported",
                    server.transport_type
                ));
            }
        };
        if !ids.insert(id.clone()) {
            return Err(format!(
                "{method} mcpServers contains duplicate server name '{id}'"
            ));
        }

        records.push(McpServerRecord {
            id: McpServerId(id.clone()),
            display_name: id,
            transport,
            startup_policy: McpStartupPolicy::Eager,
            enabled: true,
            trust_policy: McpTrustPolicy::default(),
            allowed_capabilities: Vec::new(),
            roots_policy: McpRootsPolicy::default(),
            output_limits: McpOutputLimits::default(),
            auth_ref: None,
        });
    }

    Ok(McpConfig {
        servers: records,
        auto_start: true,
    })
}

fn acp_mcp_server_id(method: &str, name: &str) -> Result<String, String> {
    let id = name.trim().to_string();
    if id.is_empty() {
        return Err(format!(
            "{method} mcpServers entries must include a non-empty name"
        ));
    }
    Ok(id)
}

fn acp_stdio_command(method: &str, server: &AcpMcpServerStdio) -> Result<Vec<String>, String> {
    if server.command.as_os_str().is_empty() {
        return Err(format!(
            "{method} mcpServers entry '{}' must include a non-empty command",
            server.name
        ));
    }

    let mut command = Vec::with_capacity(server.args.len() + 1);
    command.push(server.command.to_string_lossy().into_owned());
    command.extend(server.args.iter().cloned());
    Ok(command)
}

fn acp_stdio_env(
    method: &str,
    server: &AcpMcpServerStdio,
) -> Result<BTreeMap<String, String>, String> {
    let mut env = BTreeMap::new();
    for variable in &server.env {
        let name = variable.name.trim();
        if name.is_empty() {
            return Err(format!(
                "{method} mcpServers entry '{}' contains an env variable with an empty name",
                server.name
            ));
        }
        if env
            .insert(name.to_string(), variable.value.clone())
            .is_some()
        {
            return Err(format!(
                "{method} mcpServers entry '{}' contains duplicate env variable '{name}'",
                server.name
            ));
        }
    }
    Ok(env)
}

fn acp_http_headers(
    method: &str,
    transport: &str,
    server_name: &str,
    headers: &[crate::AcpHttpHeader],
) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for header in headers {
        let name = header.name.trim();
        if name.is_empty() {
            return Err(format!(
                "{method} mcpServers {transport} entry '{server_name}' contains a header with an empty name"
            ));
        }
        if result
            .insert(name.to_string(), header.value.clone())
            .is_some()
        {
            return Err(format!(
                "{method} mcpServers {transport} entry '{server_name}' contains duplicate header '{name}'"
            ));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use devo_protocol::AcpEnvVariable;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn acp_mcp_config_converts_stdio_servers() {
        #[cfg(windows)]
        let command_path = PathBuf::from(r"C:\mcp\filesystem.exe");
        #[cfg(windows)]
        let command = r"C:\mcp\filesystem.exe".to_string();
        #[cfg(unix)]
        let command_path = PathBuf::from("/mcp/filesystem");
        #[cfg(unix)]
        let command = "/mcp/filesystem".to_string();

        let config = acp_mcp_config(
            "session/new",
            &[AcpMcpServer::Stdio(AcpMcpServerStdio {
                name: "filesystem".to_string(),
                command: command_path,
                args: vec!["--stdio".to_string()],
                env: vec![AcpEnvVariable {
                    name: "API_KEY".to_string(),
                    value: "secret123".to_string(),
                    meta: None,
                }],
                meta: None,
            })],
        )
        .expect("stdio MCP server should convert");

        assert_eq!(
            config,
            McpConfig {
                servers: vec![McpServerRecord {
                    id: McpServerId("filesystem".to_string()),
                    display_name: "filesystem".to_string(),
                    transport: McpTransportConfig::Stdio {
                        command: vec![command, "--stdio".to_string()],
                        cwd: None,
                        env: BTreeMap::from([("API_KEY".to_string(), "secret123".to_string())]),
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
            }
        );
    }

    #[test]
    fn acp_mcp_config_converts_http_and_sse_servers() {
        let config = acp_mcp_config(
            "session/new",
            &[
                AcpMcpServer::Http(crate::AcpMcpServerHttp {
                    transport_type: crate::AcpMcpServerHttpType::Http,
                    name: "api-server".to_string(),
                    url: "https://api.example.com/mcp".to_string(),
                    headers: vec![crate::AcpHttpHeader {
                        name: "Authorization".to_string(),
                        value: "Bearer token123".to_string(),
                        meta: None,
                    }],
                    meta: None,
                }),
                AcpMcpServer::Sse(crate::AcpMcpServerSse {
                    transport_type: crate::AcpMcpServerSseType::Sse,
                    name: "event-stream".to_string(),
                    url: "https://events.example.com/mcp".to_string(),
                    headers: vec![crate::AcpHttpHeader {
                        name: "X-API-Key".to_string(),
                        value: "apikey456".to_string(),
                        meta: None,
                    }],
                    meta: None,
                }),
            ],
        )
        .expect("HTTP and SSE MCP servers should convert");

        assert_eq!(
            config,
            McpConfig {
                servers: vec![
                    McpServerRecord {
                        id: McpServerId("api-server".to_string()),
                        display_name: "api-server".to_string(),
                        transport: McpTransportConfig::StreamableHttp {
                            url: "https://api.example.com/mcp".to_string(),
                            auth: None,
                            http_headers: BTreeMap::from([(
                                "Authorization".to_string(),
                                "Bearer token123".to_string()
                            )]),
                            env_http_headers: BTreeMap::new(),
                        },
                        startup_policy: McpStartupPolicy::Eager,
                        enabled: true,
                        trust_policy: McpTrustPolicy::default(),
                        allowed_capabilities: Vec::new(),
                        roots_policy: McpRootsPolicy::default(),
                        output_limits: McpOutputLimits::default(),
                        auth_ref: None,
                    },
                    McpServerRecord {
                        id: McpServerId("event-stream".to_string()),
                        display_name: "event-stream".to_string(),
                        transport: McpTransportConfig::Sse {
                            url: "https://events.example.com/mcp".to_string(),
                            auth: None,
                            http_headers: BTreeMap::from([(
                                "X-API-Key".to_string(),
                                "apikey456".to_string()
                            )]),
                            env_http_headers: BTreeMap::new(),
                        },
                        startup_policy: McpStartupPolicy::Eager,
                        enabled: true,
                        trust_policy: McpTrustPolicy::default(),
                        allowed_capabilities: Vec::new(),
                        roots_policy: McpRootsPolicy::default(),
                        output_limits: McpOutputLimits::default(),
                        auth_ref: None,
                    },
                ],
                auto_start: true,
            }
        );
    }

    #[test]
    fn acp_mcp_config_rejects_duplicate_server_names() {
        #[cfg(windows)]
        let command_path = PathBuf::from(r"C:\mcp\filesystem.exe");
        #[cfg(unix)]
        let command_path = PathBuf::from("/mcp/filesystem");

        let server = AcpMcpServer::Stdio(AcpMcpServerStdio {
            name: "filesystem".to_string(),
            command: command_path,
            args: Vec::new(),
            env: Vec::new(),
            meta: None,
        });

        assert_eq!(
            acp_mcp_config("session/resume", &[server.clone(), server])
                .expect_err("duplicate names should fail"),
            "session/resume mcpServers contains duplicate server name 'filesystem'"
        );
    }
}
