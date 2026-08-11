//! `devo mcp` subcommands for managing user-level MCP server configuration.
//!
//! These commands mutate `~/.devo/config.toml` (`[[mcp.servers]]`) using the
//! same schema the runtime loads. They do not refresh an already-running
//! interactive session.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use clap::Subcommand;
use clap::ValueEnum;
use devo_core::AppConfigStore;
use devo_core::McpAuthConfig;
use devo_core::McpTransportConfig;
use devo_core::mcp_server_record_for_cli;
use devo_util_paths::find_devo_home;

/// Nested `devo mcp` management commands.
#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Add or replace an MCP server in the user config.
    Add {
        /// Stable server id (also used as the default display name).
        name: String,
        /// Transport kind. `http` maps to `streamable_http` in config.toml.
        #[arg(long, value_enum, default_value_t = McpTransportKind::Stdio)]
        transport: McpTransportKind,
        /// Environment variables for stdio servers (`KEY=VALUE`).
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Static HTTP headers for http/sse servers (`KEY=VALUE`).
        #[arg(long = "header", value_name = "KEY=VALUE")]
        headers: Vec<String>,
        /// Bearer token for http/sse authentication.
        #[arg(long = "bearer-token")]
        bearer_token: Option<String>,
        /// Working directory for stdio servers.
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Remaining args: stdio command (after `--`) or remote URL.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },
    /// List MCP servers from the effective user/workspace config.
    List,
    /// Remove an MCP server from the user config.
    Remove {
        /// Server id to remove.
        name: String,
    },
    /// Enable an MCP server in the user config.
    Enable {
        /// Server id to enable.
        name: String,
    },
    /// Disable an MCP server in the user config.
    Disable {
        /// Server id to disable.
        name: String,
    },
}

/// CLI transport flag values for `devo mcp add --transport`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpTransportKind {
    Stdio,
    Http,
    Sse,
}

impl McpTransportKind {
    fn as_display_label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }
}

/// Runs a `devo mcp` subcommand against the user-level config.
pub fn run_mcp(command: &McpCommand) -> Result<()> {
    let home = find_devo_home().context("resolve DEVO_HOME")?;
    let mut store =
        AppConfigStore::load(home, /*workspace_root*/ None).context("load user app config")?;
    match command {
        McpCommand::Add {
            name,
            transport,
            env,
            headers,
            bearer_token,
            cwd,
            rest,
        } => {
            let transport_config =
                build_transport(*transport, rest, env, headers, bearer_token.as_deref(), cwd)?;
            let record = mcp_server_record_for_cli(name.clone(), transport_config);
            store
                .upsert_mcp_server(record)
                .with_context(|| format!("add mcp server `{name}`"))?;
            let path = store.user_config_file().display();
            match transport {
                McpTransportKind::Stdio => {
                    println!(
                        "Added {} MCP server {name} to {path}",
                        transport.as_display_label()
                    );
                }
                McpTransportKind::Http | McpTransportKind::Sse => {
                    let url = rest.first().map(String::as_str).unwrap_or_default();
                    println!(
                        "Added {} MCP server {name} with URL: {url} to {path}",
                        transport.as_display_label()
                    );
                }
            }
            Ok(())
        }
        McpCommand::List => {
            let servers = store.mcp_servers();
            if servers.is_empty() {
                println!("No MCP servers configured.");
                return Ok(());
            }
            for server in servers {
                let (kind, target) = match &server.transport {
                    McpTransportConfig::Stdio { command, .. } => ("stdio", command.join(" ")),
                    McpTransportConfig::StreamableHttp { url, .. } => {
                        ("streamable_http", url.clone())
                    }
                    McpTransportConfig::Sse { url, .. } => ("sse", url.clone()),
                };
                let enabled = if server.enabled { "yes" } else { "no" };
                println!(
                    "{}\tenabled={enabled}\ttransport={kind}\ttarget={target}",
                    server.id
                );
            }
            Ok(())
        }
        McpCommand::Remove { name } => {
            store
                .remove_mcp_server(name)
                .with_context(|| format!("remove mcp server `{name}`"))?;
            println!(
                "Removed MCP server {name} from {}",
                store.user_config_file().display()
            );
            Ok(())
        }
        McpCommand::Enable { name } => {
            store
                .set_mcp_server_enabled(name, /*enabled*/ true)
                .with_context(|| format!("enable mcp server `{name}`"))?;
            println!("Enabled MCP server {name}");
            Ok(())
        }
        McpCommand::Disable { name } => {
            store
                .set_mcp_server_enabled(name, /*enabled*/ false)
                .with_context(|| format!("disable mcp server `{name}`"))?;
            println!("Disabled MCP server {name}");
            Ok(())
        }
    }
}

fn build_transport(
    kind: McpTransportKind,
    rest: &[String],
    env: &[String],
    headers: &[String],
    bearer_token: Option<&str>,
    cwd: &Option<PathBuf>,
) -> Result<McpTransportConfig> {
    match kind {
        McpTransportKind::Stdio => {
            if rest.is_empty() {
                anyhow::bail!(
                    "stdio transport requires a command after `--`, e.g. `devo mcp add name -- npx -y server`"
                );
            }
            Ok(McpTransportConfig::Stdio {
                command: rest.to_vec(),
                cwd: cwd.clone(),
                env: parse_key_values(env, "env")?,
                env_vars: Vec::new(),
            })
        }
        McpTransportKind::Http => {
            let url = require_remote_url(rest, "http")?;
            Ok(McpTransportConfig::StreamableHttp {
                url,
                auth: bearer_auth(bearer_token),
                http_headers: parse_key_values(headers, "header")?,
                env_http_headers: BTreeMap::new(),
            })
        }
        McpTransportKind::Sse => {
            let url = require_remote_url(rest, "sse")?;
            Ok(McpTransportConfig::Sse {
                url,
                auth: bearer_auth(bearer_token),
                http_headers: parse_key_values(headers, "header")?,
                env_http_headers: BTreeMap::new(),
            })
        }
    }
}

fn require_remote_url(rest: &[String], transport: &str) -> Result<String> {
    match rest {
        [url] if !url.trim().is_empty() => Ok(url.clone()),
        _ => anyhow::bail!(
            "{transport} transport requires exactly one URL argument, e.g. `devo mcp add --transport {transport} name https://example.com/mcp`"
        ),
    }
}

fn bearer_auth(token: Option<&str>) -> Option<McpAuthConfig> {
    token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| McpAuthConfig::BearerToken {
            token: token.to_string(),
        })
}

fn parse_key_values(entries: &[String], flag: &str) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    for entry in entries {
        let Some((key, value)) = entry.split_once('=') else {
            anyhow::bail!("invalid --{flag} `{entry}`; expected KEY=VALUE");
        };
        let key = key.trim();
        if key.is_empty() {
            anyhow::bail!("invalid --{flag} `{entry}`; key must not be empty");
        }
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn build_stdio_transport_joins_command_args() {
        let transport = build_transport(
            McpTransportKind::Stdio,
            &[
                "docker".to_string(),
                "run".to_string(),
                "-i".to_string(),
                "mcp/time".to_string(),
            ],
            &["LOCAL_TIMEZONE=UTC".to_string()],
            &[],
            None,
            &None,
        )
        .expect("stdio");
        assert_eq!(
            transport,
            McpTransportConfig::Stdio {
                command: vec![
                    "docker".to_string(),
                    "run".to_string(),
                    "-i".to_string(),
                    "mcp/time".to_string(),
                ],
                cwd: None,
                env: BTreeMap::from([("LOCAL_TIMEZONE".to_string(), "UTC".to_string())]),
                env_vars: Vec::new(),
            }
        );
    }

    #[test]
    fn build_http_transport_maps_to_streamable_http() {
        let transport = build_transport(
            McpTransportKind::Http,
            &["http://localhost:8080/mcp".to_string()],
            &[],
            &["X-Custom=1".to_string()],
            Some("secret"),
            &None,
        )
        .expect("http");
        assert_eq!(
            transport,
            McpTransportConfig::StreamableHttp {
                url: "http://localhost:8080/mcp".to_string(),
                auth: Some(McpAuthConfig::BearerToken {
                    token: "secret".to_string(),
                }),
                http_headers: BTreeMap::from([("X-Custom".to_string(), "1".to_string())]),
                env_http_headers: BTreeMap::new(),
            }
        );
    }

    #[test]
    fn build_sse_transport_keeps_sse_kind() {
        let transport = build_transport(
            McpTransportKind::Sse,
            &["https://example.com/mcp/sse".to_string()],
            &[],
            &[],
            None,
            &None,
        )
        .expect("sse");
        assert_eq!(
            transport,
            McpTransportConfig::Sse {
                url: "https://example.com/mcp/sse".to_string(),
                auth: None,
                http_headers: BTreeMap::new(),
                env_http_headers: BTreeMap::new(),
            }
        );
    }
}
