//! MCP admin RPC handlers (`mcp/list`, `mcp/tools`).

use devo_core::McpServerId;
use devo_core::McpStartupState;
use devo_protocol::SuccessResponse;
use devo_protocol::canonical::rpc_admin::McpListParams;
use devo_protocol::canonical::rpc_admin::McpListResult;
use devo_protocol::canonical::rpc_admin::McpServerInfo;
use devo_protocol::canonical::rpc_admin::McpToolEntry;
use devo_protocol::canonical::rpc_admin::McpToolsParams;
use devo_protocol::canonical::rpc_admin::McpToolsResult;

use super::ServerRuntime;
use crate::ProtocolErrorCode;

impl ServerRuntime {
    pub(super) async fn handle_mcp_list(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        if let Err(error) = serde_json::from_value::<McpListParams>(params) {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid mcp/list params: {error}"),
            );
        }

        match self.deps.process_context.mcp_manager.statuses().await {
            Ok(statuses) => {
                let mut servers = statuses
                    .into_iter()
                    .map(|status| McpServerInfo {
                        name: status.server_id.0,
                        status: startup_state_label(&status.startup_state).to_string(),
                        tool_count: status.tools.len() as u32,
                    })
                    .collect::<Vec<_>>();
                servers.sort_by(|left, right| left.name.cmp(&right.name));
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: McpListResult { servers },
                })
                .expect("serialize mcp/list response")
            }
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to list mcp servers: {error}"),
            ),
        }
    }

    pub(super) async fn handle_mcp_tools(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<McpToolsParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid mcp/tools params: {error}"),
                );
            }
        };

        let name = params.name.trim();
        if name.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "mcp/tools requires a non-empty name".to_string(),
            );
        }
        let server_id = McpServerId(name.to_string());
        let manager = &self.deps.process_context.mcp_manager;

        let needs_refresh = match manager.statuses().await {
            Ok(statuses) => {
                let status = statuses.iter().find(|status| status.server_id == server_id);
                match status {
                    None => true,
                    Some(status) => {
                        matches!(
                            status.startup_state,
                            McpStartupState::NotStarted
                                | McpStartupState::Stopped
                                | McpStartupState::Failed
                        ) || status.tools.is_empty()
                            && !matches!(status.startup_state, McpStartupState::Disabled)
                    }
                }
            }
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read mcp statuses: {error}"),
                );
            }
        };

        if needs_refresh && let Err(error) = manager.refresh(&server_id).await {
            // Still try to return whatever tools we have after a failed refresh.
            tracing::warn!(server = %server_id, error = %error, "mcp/tools refresh failed");
        }

        match manager.statuses().await {
            Ok(statuses) => {
                let Some(status) = statuses
                    .into_iter()
                    .find(|status| status.server_id == server_id)
                else {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("mcp server `{name}` not found"),
                    );
                };
                let mut tools = status
                    .tools
                    .into_iter()
                    .map(|tool| McpToolEntry {
                        name: tool.name,
                        description: tool.description,
                    })
                    .collect::<Vec<_>>();
                tools.sort_by(|left, right| left.name.cmp(&right.name));
                serde_json::to_value(SuccessResponse {
                    id: request_id,
                    result: McpToolsResult { tools },
                })
                .expect("serialize mcp/tools response")
            }
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to list mcp tools: {error}"),
            ),
        }
    }
}

fn startup_state_label(state: &McpStartupState) -> &'static str {
    match state {
        McpStartupState::Disabled => "disabled",
        McpStartupState::NotStarted => "not_started",
        McpStartupState::Starting => "starting",
        McpStartupState::Ready => "ready",
        McpStartupState::Failed => "failed",
        McpStartupState::AuthRequired => "auth_required",
        McpStartupState::Degraded => "degraded",
        McpStartupState::Stopped => "stopped",
    }
}
