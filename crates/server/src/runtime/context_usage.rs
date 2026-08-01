//! `context/usage/read` RPC handler.

use devo_core::SessionId;
use devo_protocol::SuccessResponse;
use devo_protocol::canonical::item::ContextOccupancy;
use devo_protocol::canonical::rpc_admin::ContextUsageReadParams;
use devo_protocol::canonical::rpc_admin::ContextUsageReadResult;

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

        let occupancy = summary.last_context_occupancy.unwrap_or_else(|| {
            let window = summary
                .model
                .as_deref()
                .and_then(|slug| self.deps.model_catalog.get(slug))
                .map(|model| u64::from(model.effective_context_window()))
                .unwrap_or(0);
            ContextOccupancy::empty(window)
        });

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: ContextUsageReadResult { occupancy },
        })
        .expect("serialize context/usage/read response")
    }
}
