//! Interactive `/mcps` picker helpers: merge config + runtime status.

use std::path::Path;

use devo_core::McpAuthConfig;
use devo_core::McpCapability;
use devo_core::McpConfig;
use devo_core::McpServerRecord;
use devo_core::McpTransportConfig;
use devo_core::sanitize_model_name;
use devo_protocol::canonical::rpc_admin::McpServerInfo;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::list_selection_view::SelectionItem;
use crate::bottom_pane::list_selection_view::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::text_formatting::truncate_text;

/// One MCP server row shown in the interactive `/mcps` flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpPickerServer {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) enabled: bool,
    pub(crate) transport_kind: String,
    pub(crate) target: String,
    pub(crate) auth_summary: String,
    pub(crate) capabilities: String,
    pub(crate) config_path: String,
    pub(crate) status: Option<String>,
    pub(crate) tool_count: Option<u32>,
}

/// Merge configured servers with runtime `mcp/list` statuses.
pub(crate) fn merge_mcp_picker_servers(
    config: &McpConfig,
    runtime: &[McpServerInfo],
    config_path: &Path,
) -> Vec<McpPickerServer> {
    let config_path = config_path.display().to_string();
    let mut servers: Vec<McpPickerServer> = config
        .servers
        .iter()
        .map(|record| {
            let runtime = runtime.iter().find(|server| server.name == record.id.0);
            picker_server_from_record(record, runtime, &config_path)
        })
        .collect();

    for runtime_server in runtime {
        if servers
            .iter()
            .any(|server| server.id == runtime_server.name)
        {
            continue;
        }
        servers.push(McpPickerServer {
            id: runtime_server.name.clone(),
            display_name: runtime_server.name.clone(),
            enabled: true,
            transport_kind: "unknown".to_string(),
            target: "(runtime only)".to_string(),
            auth_summary: "unknown".to_string(),
            capabilities: "unknown".to_string(),
            config_path: config_path.clone(),
            status: Some(runtime_server.status.clone()),
            tool_count: Some(runtime_server.tool_count),
        });
    }

    servers.sort_by(|left, right| left.id.cmp(&right.id));
    servers
}

fn picker_server_from_record(
    record: &McpServerRecord,
    runtime: Option<&McpServerInfo>,
    config_path: &str,
) -> McpPickerServer {
    let (transport_kind, target, auth_summary) = transport_summary(&record.transport);
    McpPickerServer {
        id: record.id.0.clone(),
        display_name: record.display_name.clone(),
        enabled: record.enabled,
        transport_kind,
        target,
        auth_summary,
        capabilities: capabilities_summary(&record.allowed_capabilities),
        config_path: config_path.to_string(),
        status: runtime.map(|server| server.status.clone()),
        tool_count: runtime.map(|server| server.tool_count),
    }
}

fn transport_summary(transport: &McpTransportConfig) -> (String, String, String) {
    match transport {
        McpTransportConfig::Stdio { command, .. } => {
            let target = if command.is_empty() {
                "(empty command)".to_string()
            } else {
                command.join(" ")
            };
            ("stdio".to_string(), target, "none".to_string())
        }
        McpTransportConfig::StreamableHttp { url, auth, .. } => (
            "streamable_http".to_string(),
            url.clone(),
            auth_summary(auth.as_ref()),
        ),
        McpTransportConfig::Sse { url, auth, .. } => {
            ("sse".to_string(), url.clone(), auth_summary(auth.as_ref()))
        }
    }
}

fn auth_summary(auth: Option<&McpAuthConfig>) -> String {
    match auth {
        Some(McpAuthConfig::BearerToken { .. }) => "bearer token".to_string(),
        None => "none".to_string(),
    }
}

fn capabilities_summary(capabilities: &[McpCapability]) -> String {
    if capabilities.is_empty() {
        return "none".to_string();
    }
    capabilities
        .iter()
        .map(|capability| match capability {
            McpCapability::Tools => "tools",
            McpCapability::Resources => "resources",
            McpCapability::Prompts => "prompts",
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn list_status_label(server: &McpPickerServer) -> &str {
    if !server.enabled {
        "disabled"
    } else {
        server.status.as_deref().unwrap_or("unknown")
    }
}

/// Compact right-column meta for the server list (no command/URL).
fn list_row_meta(server: &McpPickerServer) -> String {
    let status = list_status_label(server);
    match server.tool_count {
        Some(count) => format!("{status} · {} · {count} tools", server.transport_kind),
        None => format!("{status} · {}", server.transport_kind),
    }
}

/// Build the searchable server list selection params.
pub(crate) fn mcp_server_list_params(servers: &[McpPickerServer]) -> SelectionViewParams {
    let items = if servers.is_empty() {
        vec![SelectionItem {
            name: "No MCP servers configured".to_string(),
            description: Some("Add one with `devo mcp add`, then restart the session.".to_string()),
            is_disabled: true,
            dismiss_on_select: false,
            ..SelectionItem::default()
        }]
    } else {
        servers
            .iter()
            .map(|server| {
                let name = server.id.clone();
                SelectionItem {
                    name: server.display_name.clone(),
                    description: Some(list_row_meta(server)),
                    search_value: Some(format!(
                        "{} {} {} {} {}",
                        server.display_name,
                        server.id,
                        server.transport_kind,
                        server.target,
                        list_status_label(server)
                    )),
                    dismiss_on_select: true,
                    actions: vec![Box::new(move |tx: &AppEventSender| {
                        tx.send(AppEvent::McpServerSelected { name: name.clone() });
                    })],
                    ..SelectionItem::default()
                }
            })
            .collect()
    };

    SelectionViewParams {
        title: Some("MCP servers".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to search".to_string()),
        ..SelectionViewParams::default()
    }
}

fn detail_status_label(server: &McpPickerServer) -> &str {
    if !server.enabled {
        "disabled"
    } else {
        server.status.as_deref().unwrap_or("unknown")
    }
}

fn detail_subtitle_lines(server: &McpPickerServer) -> Vec<ratatui::text::Line<'static>> {
    use ratatui::style::Style;
    use ratatui::style::Stylize;
    use ratatui::text::Line;
    use ratatui::text::Span;

    let status = detail_status_label(server);
    let summary = match server.tool_count {
        Some(count) => format!("{status} · {} · {count} tools", server.transport_kind),
        None => format!("{status} · {}", server.transport_kind),
    };

    let mut lines = vec![Line::from(summary.dim())];

    lines.push(Line::from(vec![
        Span::styled("name  ".to_string(), Style::default().dim()),
        Span::raw(server.id.clone()),
    ]));

    let target = server.target.trim();
    if !target.is_empty() && target != "(runtime only)" {
        lines.push(Line::from(vec![
            Span::styled("cmd   ".to_string(), Style::default().dim()),
            Span::styled(target.to_string(), Style::default().cyan()),
        ]));
    }

    if server.auth_summary != "none" {
        lines.push(Line::from(vec![
            Span::styled("auth  ".to_string(), Style::default().dim()),
            Span::raw(server.auth_summary.clone()),
        ]));
    }

    lines
}

/// Detail actions for one MCP server (same shell as `/skills` detail).
pub(crate) fn mcp_server_detail_params(server: &McpPickerServer) -> SelectionViewParams {
    let tools_id = server.id.clone();
    let toggle_id = server.id.clone();
    let enabled = server.enabled;

    let mut items = vec![SelectionItem {
        name: "View tools".to_string(),
        description: Some("Browse and insert tool names".to_string()),
        dismiss_on_select: true,
        actions: vec![Box::new(move |tx: &AppEventSender| {
            tx.send(AppEvent::Command(AppCommand::ListMcpTools {
                name: tools_id.clone(),
            }));
        })],
        ..SelectionItem::default()
    }];

    items.push(SelectionItem {
        name: if enabled {
            "Disable".to_string()
        } else {
            "Enable".to_string()
        },
        description: Some("Writes config; restart for live runtime".to_string()),
        dismiss_on_select: true,
        actions: vec![Box::new(move |tx: &AppEventSender| {
            tx.send(AppEvent::Command(AppCommand::SetMcpServerEnabled {
                name: toggle_id.clone(),
                enabled: !enabled,
            }));
        })],
        ..SelectionItem::default()
    });

    items.push(SelectionItem {
        name: "Back".to_string(),
        description: Some("Return to server list".to_string()),
        dismiss_on_select: true,
        actions: vec![Box::new(|tx: &AppEventSender| {
            tx.send(AppEvent::McpOpenServerList);
        })],
        ..SelectionItem::default()
    });

    SelectionViewParams {
        title: Some(server.display_name.clone()),
        subtitle_lines: detail_subtitle_lines(server),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        on_cancel: Some(Box::new(|tx: &AppEventSender| {
            tx.send(AppEvent::McpOpenServerList);
        })),
        ..SelectionViewParams::default()
    }
}

/// Model-facing flat tool name (`mcp__server__tool`).
pub(crate) fn mcp_flat_tool_name(server_id: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_model_name(server_id),
        sanitize_model_name(tool_name)
    )
}

/// Build the searchable tools list for one MCP server.
pub(crate) fn mcp_tools_list_params(
    server_name: &str,
    tools: &[devo_protocol::canonical::rpc_admin::McpToolEntry],
) -> SelectionViewParams {
    let server_name_for_cancel = server_name.to_string();
    let items = if tools.is_empty() {
        vec![SelectionItem {
            name: "No tools advertised".to_string(),
            description: Some("This server did not expose a tools catalog.".to_string()),
            is_disabled: true,
            dismiss_on_select: false,
            ..SelectionItem::default()
        }]
    } else {
        tools
            .iter()
            .map(|tool| {
                let flat_name = mcp_flat_tool_name(server_name, &tool.name);
                let tool_name = tool.name.clone();
                let description = {
                    let collapsed = tool
                        .description
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    (!collapsed.is_empty()).then(|| truncate_text(&collapsed, 48))
                };
                SelectionItem {
                    name: tool.name.clone(),
                    description,
                    search_value: Some(format!("{} {}", tool.name, tool.description)),
                    dismiss_on_select: true,
                    actions: vec![Box::new(move |tx: &AppEventSender| {
                        tx.send(AppEvent::InsertComposerText {
                            text: format!("@{tool_name}"),
                            binding: Some(flat_name.clone()),
                        });
                    })],
                    ..SelectionItem::default()
                }
            })
            .collect()
    };

    SelectionViewParams {
        title: Some(format!("Tools · {server_name}")),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to search".to_string()),
        on_cancel: Some(Box::new(move |tx: &AppEventSender| {
            tx.send(AppEvent::McpOpenServerDetail {
                name: server_name_for_cancel.clone(),
            });
        })),
        ..SelectionViewParams::default()
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use tokio::sync::mpsc;

    use super::*;
    use crate::app_command::AppCommand;
    use crate::app_event::AppEvent;
    use crate::app_event_sender::AppEventSender;
    use devo_core::McpServerId;
    use devo_core::McpStartupPolicy;

    #[test]
    fn merge_mcp_picker_servers_overlays_runtime_status() {
        let config = McpConfig {
            servers: vec![McpServerRecord {
                id: McpServerId("time".to_string()),
                display_name: "Time".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: vec!["docker".to_string(), "run".to_string()],
                    cwd: None,
                    env: Default::default(),
                    env_vars: Vec::new(),
                },
                startup_policy: McpStartupPolicy::Eager,
                enabled: true,
                trust_policy: Default::default(),
                allowed_capabilities: vec![McpCapability::Tools],
                roots_policy: Default::default(),
                output_limits: Default::default(),
                auth_ref: None,
            }],
            ..McpConfig::default()
        };
        let runtime = vec![McpServerInfo {
            name: "time".to_string(),
            status: "ready".to_string(),
            tool_count: 3,
        }];
        let merged = merge_mcp_picker_servers(&config, &runtime, Path::new("/tmp/config.toml"));
        assert_eq!(
            merged,
            vec![McpPickerServer {
                id: "time".to_string(),
                display_name: "Time".to_string(),
                enabled: true,
                transport_kind: "stdio".to_string(),
                target: "docker run".to_string(),
                auth_summary: "none".to_string(),
                capabilities: "tools".to_string(),
                config_path: PathBuf::from("/tmp/config.toml").display().to_string(),
                status: Some("ready".to_string()),
                tool_count: Some(3),
            }]
        );
    }

    #[test]
    fn mcp_server_list_params_action_emits_server_selected() {
        let servers = vec![McpPickerServer {
            id: "time".to_string(),
            display_name: "Time".to_string(),
            enabled: true,
            transport_kind: "stdio".to_string(),
            target: "docker run".to_string(),
            auth_summary: "none".to_string(),
            capabilities: "tools".to_string(),
            config_path: "/tmp/config.toml".to_string(),
            status: Some("ready".to_string()),
            tool_count: Some(3),
        }];
        let params = mcp_server_list_params(&servers);
        assert_eq!(params.items.len(), 1);
        assert_eq!(params.title.as_deref(), Some("MCP servers"));
        assert!(params.items[0].name_prefix_spans.is_empty());
        let description = params.items[0]
            .description
            .as_deref()
            .expect("compact meta");
        assert!(!description.contains('\n'));
        assert!(!description.contains("docker"));
        assert_eq!(description, "ready · stdio · 3 tools");

        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = AppEventSender::new(tx);
        params.items[0].actions[0](&sender);
        assert_eq!(
            rx.try_recv().expect("selection event"),
            AppEvent::McpServerSelected {
                name: "time".to_string(),
            }
        );
    }

    #[test]
    fn mcp_server_detail_params_exposes_tools_and_toggle() {
        let server = McpPickerServer {
            id: "time".to_string(),
            display_name: "Time".to_string(),
            enabled: true,
            transport_kind: "stdio".to_string(),
            target: "docker run".to_string(),
            auth_summary: "none".to_string(),
            capabilities: "tools".to_string(),
            config_path: "/tmp/config.toml".to_string(),
            status: Some("ready".to_string()),
            tool_count: Some(3),
        };
        let params = mcp_server_detail_params(&server);
        assert_eq!(params.title.as_deref(), Some("Time"));
        assert_eq!(params.subtitle_lines.len(), 3);
        assert_eq!(
            params.subtitle_lines[0].to_string(),
            "ready · stdio · 3 tools"
        );
        assert_eq!(params.subtitle_lines[1].to_string(), "name  time");
        assert_eq!(params.subtitle_lines[2].to_string(), "cmd   docker run");
        assert!(
            params.subtitle_lines[2]
                .spans
                .iter()
                .any(|span| { span.content.as_ref() == "docker run" && span.style.fg.is_some() })
        );
        assert_eq!(params.items[0].name, "View tools");
        assert_eq!(params.items[1].name, "Disable");

        let (tx, mut rx) = mpsc::unbounded_channel();
        params.items[0].actions[0](&AppEventSender::new(tx));
        assert_eq!(
            rx.try_recv().expect("tools"),
            AppEvent::Command(AppCommand::ListMcpTools {
                name: "time".to_string(),
            })
        );
    }

    #[test]
    fn mcp_tools_list_params_enter_inserts_flat_tool_name() {
        assert_eq!(
            mcp_flat_tool_name("Docs Server", "echo-tool"),
            "mcp__docs_server__echo_tool"
        );
        let params = mcp_tools_list_params(
            "time",
            &[devo_protocol::canonical::rpc_admin::McpToolEntry {
                name: "get_current_time".to_string(),
                description: "Return the current time".to_string(),
            }],
        );
        assert!(params.items[0].dismiss_on_select);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = AppEventSender::new(tx);
        params.items[0].actions[0](&sender);
        assert_eq!(
            rx.try_recv().expect("insert event"),
            AppEvent::InsertComposerText {
                text: "@get_current_time".to_string(),
                binding: Some("mcp__time__get_current_time".to_string()),
            }
        );
    }
}
