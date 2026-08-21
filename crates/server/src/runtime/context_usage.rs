//! `context/usage/read` RPC handler.

use devo_core::SessionId;
use devo_protocol::SuccessResponse;
use devo_protocol::native::item::ContextOccupancy;
use devo_protocol::native::rpc_admin::ContextUsageReadParams;
use devo_protocol::native::rpc_admin::ContextUsageReadResult;

use super::ServerRuntime;
use crate::ProtocolErrorCode;

impl ServerRuntime {
    pub(super) async fn handle_context_usage_read(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params = match serde_json::from_value::<ContextUsageReadParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid context/usage/read params: {error}"),
                );
            }
        };

        let Ok(session_id) = SessionId::try_from(params.session_id.as_str()) else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                format!("invalid session id: {}", params.session_id),
            );
        };

        let Some(summary) = self.session_summary_snapshot(session_id).await else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                format!("session not found: {session_id}"),
            );
        };

        let occupancy = if let Some(occupancy) = summary.last_context_occupancy {
            occupancy
        } else {
            let global = self
                .deps
                .config_store
                .lock()
                .expect("app config store mutex should not be poisoned")
                .effective_config()
                .compaction_token_limit;
            let model = summary
                .model
                .as_deref()
                .and_then(|slug| self.deps.model_catalog.get(slug))
                .or_else(|| {
                    summary
                        .model_binding_id
                        .as_deref()
                        .and_then(|binding| self.deps.model_catalog.get(binding))
                });
            let window = match model {
                Some(model) => {
                    crate::runtime::context_occupancy::resolved_compaction_limit(global, model)
                }
                None => global.unwrap_or(0),
            };
            ContextOccupancy::empty(window)
        };

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ContextUsageReadResult { occupancy },
        })
        .expect("serialize context/usage/read response")
    }
}
