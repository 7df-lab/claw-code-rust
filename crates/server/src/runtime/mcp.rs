//! MCP admin RPC handlers (`mcp/list`, `mcp/tools`, `mcp/set_enabled`).

use std::sync::Arc;

use devo_core::McpServerId;
use devo_core::McpServerStatus;
use devo_core::McpStartupState;
use devo_core::tools::ToolPlanConfig;
use devo_core::tools::handlers;
use devo_protocol::SuccessResponse;
use devo_protocol::native::rpc_admin::McpListParams;
use devo_protocol::native::rpc_admin::McpListResult;
use devo_protocol::native::rpc_admin::McpServerInfo;
use devo_protocol::native::rpc_admin::McpSetEnabledParams;
use devo_protocol::native::rpc_admin::McpSetEnabledResult;
use devo_protocol::native::rpc_admin::McpToolEntry;
use devo_protocol::native::rpc_admin::McpToolsParams;
use devo_protocol::native::rpc_admin::McpToolsResult;

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

        match self.mcp_server_infos().await {
            Ok(servers) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: McpListResult { servers },
            })
            .expect("serialize mcp/list response"),
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

        let statuses = match manager.statuses().await {
            Ok(statuses) => statuses,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to read mcp statuses: {error}"),
                );
            }
        };
        let existing = statuses
            .iter()
            .find(|status| status.server_id == server_id)
            .cloned();

        let status = if mcp_tools_need_refresh(existing.as_ref()) {
            match manager.refresh(&server_id).await {
                Ok(status) => status,
                Err(error) => {
                    tracing::warn!(server = %server_id, error = %error, "mcp/tools refresh failed");
                    match existing {
                        Some(status) => status,
                        None => {
                            return self.error_response(
                                request_id,
                                ProtocolErrorCode::InvalidParams,
                                format!("mcp server `{name}` not found"),
                            );
                        }
                    }
                }
            }
        } else {
            match existing {
                Some(status) => status,
                None => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("mcp server `{name}` not found"),
                    );
                }
            }
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

    pub(super) async fn handle_mcp_set_enabled(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<McpSetEnabledParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid mcp/set_enabled params: {error}"),
                );
            }
        };

        let name = params.name.trim();
        if name.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "mcp/set_enabled requires a non-empty name".to_string(),
            );
        }

        let config_file = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            store
                .user_config_dir()
                .join("config.toml")
                .display()
                .to_string()
        };
        if let Some(reason) = self
            .config_change_hook_block_reason("mcp", Some(config_file))
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::PolicyDenied,
                format!("mcp config change blocked by hook: {reason}"),
            );
        }

        {
            let mut store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            if let Err(error) = store.set_mcp_server_enabled(name, params.enabled) {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InternalError,
                    format!("failed to update mcp config: {error}"),
                );
            }
        }
        self.deps.invalidate_workspace_contexts();

        let server_id = McpServerId(name.to_string());
        let process_context = &self.deps.process_context;
        if let Err(error) = process_context
            .mcp_manager
            .set_enabled(&server_id, params.enabled)
            .await
        {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("failed to apply mcp enablement: {error}"),
            );
        }

        let tool_plan = {
            let store = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned");
            ToolPlanConfig::from_app_config(store.effective_config())
        };
        let previous = process_context.tool_registry();
        let new_registry = Arc::new(
            handlers::rebuild_registry_from_plan_with_mcp(
                &tool_plan,
                Arc::clone(&process_context.mcp_manager),
                Some(previous.as_ref()),
            )
            .await,
        );
        process_context.replace_tool_registry(Arc::clone(&new_registry));

        for handle in self.list_session_handles().await {
            let Some(session_context) = handle.runtime_context().await else {
                continue;
            };
            let session_registry =
                if Arc::ptr_eq(&session_context.mcp_manager, &process_context.mcp_manager) {
                    Arc::clone(&new_registry)
                } else {
                    if let Err(error) = session_context
                        .mcp_manager
                        .set_enabled(&server_id, params.enabled)
                        .await
                    {
                        tracing::warn!(
                            server = %server_id,
                            error = %error,
                            "failed to apply mcp enablement on session manager"
                        );
                        continue;
                    }
                    let previous = session_context.tool_registry();
                    let rebuilt = Arc::new(
                        handlers::rebuild_registry_from_plan_with_mcp(
                            &tool_plan,
                            Arc::clone(&session_context.mcp_manager),
                            Some(previous.as_ref()),
                        )
                        .await,
                    );
                    session_context.replace_tool_registry(Arc::clone(&rebuilt));
                    rebuilt
                };
            let _ = handle.set_tool_registry(Some(session_registry)).await;
        }

        match self.mcp_server_infos().await {
            Ok(servers) => serde_json::to_value(SuccessResponse {
                id: request_id,
                result: McpSetEnabledResult { servers },
            })
            .expect("serialize mcp/set_enabled response"),
            Err(error) => self.error_response(
                request_id,
                ProtocolErrorCode::InternalError,
                format!("mcp enablement applied but list failed: {error}"),
            ),
        }
    }

    async fn mcp_server_infos(&self) -> Result<Vec<McpServerInfo>, String> {
        let statuses = self
            .deps
            .process_context
            .mcp_manager
            .statuses()
            .await
            .map_err(|error| error.to_string())?;
        let mut servers = statuses
            .into_iter()
            .map(|status| McpServerInfo {
                name: status.server_id.0,
                status: startup_state_label(&status.startup_state).to_string(),
                tool_count: status.tools.len() as u32,
            })
            .collect::<Vec<_>>();
        servers.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(servers)
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

fn mcp_tools_need_refresh(status: Option<&McpServerStatus>) -> bool {
    let Some(status) = status else {
        return true;
    };
    match status.startup_state {
        McpStartupState::Disabled => false,
        McpStartupState::NotStarted | McpStartupState::Stopped | McpStartupState::Failed => true,
        McpStartupState::Starting
        | McpStartupState::Ready
        | McpStartupState::AuthRequired
        | McpStartupState::Degraded => status.tools.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use devo_core::McpAuthState;
    use devo_core::McpToolDescriptor;

    fn status_with(startup_state: McpStartupState, tool_count: usize) -> McpServerStatus {
        McpServerStatus {
            server_id: McpServerId("docs".to_string()),
            startup_state,
            auth_state: McpAuthState::NotRequired,
            tools: (0..tool_count)
                .map(|index| McpToolDescriptor {
                    server_id: McpServerId("docs".to_string()),
                    name: format!("tool_{index}"),
                    description: String::new(),
                    input_schema: serde_json::json!({}),
                })
                .collect(),
            resources: Vec::new(),
            resource_templates: Vec::new(),
            last_refreshed_at: None,
        }
    }

    /// Trace: L2-DES-MCP-001
    /// Verifies: mcp/tools does not start a disabled server and does start idle enabled ones.
    #[test]
    fn mcp_tools_refresh_skips_disabled_and_filled_ready_servers() {
        assert_eq!(
            [
                mcp_tools_need_refresh(None),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Disabled, 0))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::NotStarted, 0))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Ready, 0))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Ready, 2))),
                mcp_tools_need_refresh(Some(&status_with(McpStartupState::Failed, 0))),
            ],
            [true, false, true, true, false, true]
        );
    }
}
