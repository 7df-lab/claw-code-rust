use super::super::*;
use super::session::RuntimeSessionToolRegistryUpdate;

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use devo_core::AppConfigStore;
use devo_core::McpConfig;
use devo_core::McpOutputLimits;
use devo_core::McpRootsPolicy;
use devo_core::McpServerId;
use devo_core::McpServerRecord;
use devo_core::McpStartupPolicy;
use devo_core::McpTransportConfig;
use devo_core::McpTrustPolicy;
use devo_core::tools::ToolPlanConfig;
use devo_mcp::manager::RmcpMcpManager;
use devo_protocol::AcpMeta;
use devo_protocol::DEVO_HISTORY_INDEX_META;
use devo_protocol::DEVO_PARENT_MESSAGE_ID_META;

use crate::ACP_SESSION_UPDATE_METHOD;
use crate::AcpAgentCapabilities;
use crate::AcpCancelParams;
use crate::AcpClientNotification;
use crate::AcpCloseSessionParams;
use crate::AcpCloseSessionResult;
use crate::AcpContentBlock;
use crate::AcpDeleteSessionParams;
use crate::AcpDeleteSessionResult;
use crate::AcpErrorCode;
use crate::AcpImplementation;
use crate::AcpInitializeParams;
use crate::AcpInitializeResult;
use crate::AcpListSessionsParams;
use crate::AcpListSessionsResult;
use crate::AcpLoadSessionParams;
use crate::AcpLoadSessionResult;
use crate::AcpMcpCapabilities;
use crate::AcpMcpServer;
use crate::AcpMcpServerStdio;
use crate::AcpNewSessionParams;
use crate::AcpNewSessionResult;
use crate::AcpPlanEntry;
use crate::AcpPlanEntryPriority;
use crate::AcpPlanEntryStatus;
use crate::AcpPromptCapabilities;
use crate::AcpPromptParams;
use crate::AcpPromptResult;
use crate::AcpResumeSessionParams;
use crate::AcpResumeSessionResult;
use crate::AcpSessionAdditionalDirectoriesCapabilities;
use crate::AcpSessionCapabilities;
use crate::AcpSessionCloseCapabilities;
use crate::AcpSessionDeleteCapabilities;
use crate::AcpSessionListCapabilities;
use crate::AcpSessionNotification;
use crate::AcpSessionResumeCapabilities;
use crate::AcpSessionUpdate;
use crate::AcpSetConfigOptionParams;
use crate::AcpSetConfigOptionResult;
use crate::AcpSetModeParams;
use crate::AcpStopReason;
use crate::AcpToolCallContent;
use crate::AcpToolCallStatus;
use crate::AcpToolKind;
use crate::CollaborationMode;
use crate::DEVO_SESSION_META;
use crate::DEVO_SESSION_RESUME_META;
use crate::SessionHistoryItem;
use crate::SessionHistoryItemKind;
use crate::SessionHistoryMetadata;
use crate::SessionPlanStepStatus;
use crate::TurnExecutionMode;
use crate::acp_error_response;
use crate::acp_session_info_from_metadata;
use crate::acp_success_response;
use crate::input_items_from_acp_prompt;

mod history;
mod mcp;
mod prompt;
mod response;
mod session;
mod session_support;

use history::acp_update_from_history_item;
use mcp::acp_mcp_config;
pub(super) use response::legacy_error_to_acp;
use session_support::decode_session_list_cursor;
use session_support::encode_session_list_cursor;
use session_support::history_limit_from_meta;
use session_support::validate_acp_session_roots;

impl ServerRuntime {
    pub(crate) async fn handle_acp_initialize(
        &self,
        connection_id: u64,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let request_id = id.unwrap_or(serde_json::Value::Null);
        let params: AcpInitializeParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return acp_error_response(
                    request_id,
                    AcpErrorCode::InvalidParams,
                    format!("invalid initialize params: {error}"),
                );
            }
        };
        let acp_auth_config = self.acp_auth_config();
        // Typed-items opt-in (P2): accepted from the initialize params `_meta`
        // or from `clientCapabilities._meta`, both in the
        // `{ "devo": { "typedItems": true } }` shape.
        let typed_items = devo_protocol::devo_typed_items_opted_in(params.meta.as_ref())
            || devo_protocol::devo_typed_items_opted_in(params.client_capabilities.meta.as_ref());
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            connection.state = ConnectionState::Ready;
            connection.acp_authenticated = !acp_auth_config.enabled;
            connection.acp_client_capabilities = params.client_capabilities.clone();
            connection.typed_items = typed_items;
        }
        tracing::info!(
            connection_id,
            protocol_version = params.protocol_version,
            client = ?params.client_info.as_ref().map(|info| info.name.as_str()),
            typed_items,
            "accepted ACP initialize request"
        );
        let mut meta = serde_json::Map::new();
        meta.insert(
            "devo/platformFamily".to_string(),
            serde_json::Value::String(self.metadata.platform_family.clone()),
        );
        meta.insert(
            "devo/platformOs".to_string(),
            serde_json::Value::String(self.metadata.platform_os.clone()),
        );
        if !acp_auth_config.enabled {
            meta.insert(
                "devo/serverHome".to_string(),
                serde_json::Value::String(self.metadata.server_home.display().to_string()),
            );
        }
        if typed_items {
            meta.insert(
                devo_protocol::DEVO_EXTENSION_META.to_string(),
                serde_json::json!({ devo_protocol::DEVO_TYPED_ITEMS_META: true }),
            );
        }
        acp_success_response(
            request_id,
            AcpInitializeResult {
                protocol_version: 1,
                agent_capabilities: AcpAgentCapabilities {
                    load_session: true,
                    prompt_capabilities: AcpPromptCapabilities {
                        embedded_context: true,
                        ..AcpPromptCapabilities::default()
                    },
                    mcp_capabilities: AcpMcpCapabilities {
                        http: true,
                        sse: true,
                        ..AcpMcpCapabilities::default()
                    },
                    auth: Self::acp_auth_capabilities(&acp_auth_config),
                    session_capabilities: AcpSessionCapabilities {
                        list: Some(AcpSessionListCapabilities::default()),
                        delete: Some(AcpSessionDeleteCapabilities::default()),
                        additional_directories: Some(
                            AcpSessionAdditionalDirectoriesCapabilities::default(),
                        ),
                        resume: Some(AcpSessionResumeCapabilities::default()),
                        close: Some(AcpSessionCloseCapabilities::default()),
                        ..AcpSessionCapabilities::default()
                    },
                    ..AcpAgentCapabilities::default()
                },
                auth_methods: Self::acp_auth_methods(&acp_auth_config),
                agent_info: Some(
                    AcpImplementation::new(
                        self.metadata.server_name.clone(),
                        self.metadata.server_version.clone(),
                    )
                    .with_title("Devo"),
                ),
                meta: Some(meta),
            },
        )
    }
}
