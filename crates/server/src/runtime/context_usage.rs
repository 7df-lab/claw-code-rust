//! `context/usage/read` RPC handler and mid-turn occupancy broadcasts.

use devo_core::RawContextBreakdown;
use devo_core::SessionId;
use devo_protocol::SuccessResponse;
use devo_protocol::native::item::ContextOccupancy;
use devo_protocol::native::rpc_admin::ContextUsageReadParams;
use devo_protocol::native::rpc_admin::ContextUsageReadResult;

use super::ServerRuntime;
use crate::ContextUsageUpdatedPayload;
use crate::ProtocolErrorCode;
use crate::ServerEvent;

impl ServerRuntime {
    /// Publish a live context occupancy snapshot during an in-flight turn.
    ///
    /// Call only with a provider-anchored total (from `Usage` / `UsageDelta`).
    /// The window always resolves to the same effective/compaction limit used
    /// at finalize (and by TUI), never the raw hard model context window.
    pub(super) async fn publish_live_context_occupancy(
        &self,
        session_id: SessionId,
        context_window_hint: Option<u64>,
        raw: RawContextBreakdown,
        anchor_total: u64,
    ) {
        let window = self
            .live_occupancy_window(session_id, context_window_hint)
            .await
            .max(1);
        let occupancy =
            super::context_occupancy::occupancy_from_raw(window, raw, anchor_total.max(1));
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                inline.summary.last_query_total_tokens = occupancy.total_tokens as usize;
                inline.summary.last_context_occupancy = Some(occupancy.clone());
                inline.hook_context.summary = inline.summary.clone();
            }
        }
        self.broadcast_event(ServerEvent::ContextUsageUpdated(
            ContextUsageUpdatedPayload {
                session_id,
                occupancy,
            },
        ))
        .await;
    }

    async fn live_occupancy_window(
        &self,
        session_id: SessionId,
        context_window_hint: Option<u64>,
    ) -> u64 {
        let global = self
            .deps
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .compaction_token_limit;

        if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_ref() {
                let model = inline
                    .summary
                    .model
                    .as_deref()
                    .and_then(|slug| self.deps.model_catalog.get(slug))
                    .or_else(|| {
                        inline
                            .summary
                            .model_binding_id
                            .as_deref()
                            .and_then(|binding| self.deps.model_catalog.get(binding))
                    });
                return super::context_occupancy::occupancy_window_tokens(
                    inline.hook_context.config.effective_context_window_override,
                    model,
                    global,
                );
            }
        }

        if let Some(summary) = self.session_summary_snapshot(session_id).await {
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
            if let Some(occupancy) = summary.last_context_occupancy.as_ref()
                && occupancy.context_window_tokens > 0
                && model.is_none()
            {
                return occupancy.context_window_tokens;
            }
            return super::context_occupancy::occupancy_window_tokens(
                summary.effective_context_window.map(|limit| limit as usize),
                model,
                global,
            );
        }

        context_window_hint.unwrap_or(1).max(1)
    }

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
