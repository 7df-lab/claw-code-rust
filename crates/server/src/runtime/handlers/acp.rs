use super::super::*;
use super::session::RuntimeSessionToolRegistryUpdate;

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

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

use crate::ACP_SESSION_CANCEL_METHOD;
use crate::ACP_SESSION_CLOSE_METHOD;
use crate::ACP_SESSION_DELETE_METHOD;
use crate::ACP_SESSION_LIST_METHOD;
use crate::ACP_SESSION_LOAD_METHOD;
use crate::ACP_SESSION_NEW_METHOD;
use crate::ACP_SESSION_PROMPT_METHOD;
use crate::ACP_SESSION_RESUME_METHOD;
use crate::ACP_SESSION_SET_CONFIG_OPTION_METHOD;
use crate::ACP_SESSION_SET_MODE_METHOD;
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
use crate::ServerProtocol;
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
use session_support::validate_acp_session_roots;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AcpRoute {
    Cancel,
    SessionInterrupt,
    Close,
    Delete,
    List,
    Load,
    New,
    Prompt,
    Resume,
    SetConfigOption,
    SetMode,
}

pub(crate) fn acp_route(method: &str) -> Option<AcpRoute> {
    static ACP_ROUTES: OnceLock<BTreeMap<&'static str, AcpRoute>> = OnceLock::new();
    ACP_ROUTES
        .get_or_init(|| {
            BTreeMap::from([
                (ACP_SESSION_CANCEL_METHOD, AcpRoute::Cancel),
                ("session/interrupt", AcpRoute::SessionInterrupt),
                (ACP_SESSION_CLOSE_METHOD, AcpRoute::Close),
                (ACP_SESSION_DELETE_METHOD, AcpRoute::Delete),
                (ACP_SESSION_LIST_METHOD, AcpRoute::List),
                (ACP_SESSION_LOAD_METHOD, AcpRoute::Load),
                (ACP_SESSION_NEW_METHOD, AcpRoute::New),
                (ACP_SESSION_PROMPT_METHOD, AcpRoute::Prompt),
                (ACP_SESSION_RESUME_METHOD, AcpRoute::Resume),
                (
                    ACP_SESSION_SET_CONFIG_OPTION_METHOD,
                    AcpRoute::SetConfigOption,
                ),
                (ACP_SESSION_SET_MODE_METHOD, AcpRoute::SetMode),
            ])
        })
        .get(method)
        .copied()
}

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
        {
            let mut connections = self.connections.lock().await;
            let state = connections
                .get_mut(&connection_id)
                .map(|connection| match connection.state {
                    ConnectionState::Connected => {
                        connection.state = ConnectionState::Initializing;
                        Ok(())
                    }
                    ConnectionState::Initializing => {
                        Err("connection initialization is in progress")
                    }
                    ConnectionState::Ready => Err("connection is already initialized"),
                    ConnectionState::Closed => Err("connection is closed"),
                })
                .unwrap_or(Err("connection is not registered"));
            if let Err(message) = state {
                return acp_error_response(request_id, AcpErrorCode::InvalidRequest, message);
            }
        }
        let acp_auth_config = self.acp_auth_config();
        // Typed-items opt-in (P2): accepted from the initialize params `_meta`
        // or from `clientCapabilities._meta`, both in the
        // `{ "devo": { "typedItems": true } }` shape.
        let typed_items = devo_protocol::devo_typed_items_opted_in(params.meta.as_ref())
            || devo_protocol::devo_typed_items_opted_in(params.client_capabilities.meta.as_ref());
        // Protocol-surface negotiation (L2-DES-APP-008/009): first-party
        // First-party clients declare `_meta.devo.protocol = "native"` so
        // colliding method names route to Native handlers instead of the ACP
        // adapter. Absent the flag the connection stays on the ACP surface.
        let (protocol, surface) =
            if devo_protocol::devo_native_protocol_opted_in(params.meta.as_ref()) {
                (
                    crate::runtime::connection::ConnectionProtocol::Native,
                    ServerProtocol::Native,
                )
            } else {
                (
                    crate::runtime::connection::ConnectionProtocol::Acp,
                    ServerProtocol::Acp,
                )
            };
        if !self.protocol_enabled(surface).await {
            let enabled = self.enabled_protocols().await;
            if let Some(connection) = self.connections.lock().await.get_mut(&connection_id)
                && connection.state == ConnectionState::Initializing
            {
                connection.state = ConnectionState::Connected;
            }
            return acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                format!("protocol surface {surface} is not enabled; enabled protocols: {enabled}"),
            );
        }
        let mut connections = self.connections.lock().await;
        let Some(connection) = connections.get_mut(&connection_id) else {
            return acp_error_response(
                request_id,
                AcpErrorCode::InvalidRequest,
                "connection is not registered",
            );
        };
        connection.state = ConnectionState::Ready;
        connection.protocol = Some(protocol);
        connection.acp_authenticated = protocol
            == crate::runtime::connection::ConnectionProtocol::Native
            || !acp_auth_config.enabled;
        connection.acp_client_capabilities = params.client_capabilities.clone();
        connection.typed_items = typed_items;
        drop(connections);
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
        {
            let mut devo_ext = serde_json::Map::new();
            if typed_items {
                devo_ext.insert(
                    devo_protocol::DEVO_TYPED_ITEMS_META.to_string(),
                    serde_json::Value::Bool(true),
                );
            }
            if protocol == crate::runtime::connection::ConnectionProtocol::Native {
                devo_ext.insert(
                    devo_protocol::DEVO_PROTOCOL_META.to_string(),
                    serde_json::Value::String(devo_protocol::DEVO_PROTOCOL_NATIVE.to_string()),
                );
            }
            if !devo_ext.is_empty() {
                meta.insert(
                    devo_protocol::DEVO_EXTENSION_META.to_string(),
                    serde_json::Value::Object(devo_ext),
                );
            }
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
