use std::sync::Arc;

use super::super::*;

impl ServerRuntime {
    /// Native `session/interrupt` is the one Devo interrupt command. It
    /// resolves the requested work scope and delegates to the existing turn
    /// cancellation or command termination application services.
    pub(crate) async fn handle_session_interrupt(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_session::SessionInterruptParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid session/interrupt params: {error}"),
                    );
                }
            };

        let interrupted = match params.scope {
            devo_protocol::native::rpc_session::SessionInterruptScope::Session { session_id } => {
                self.interrupt_session_scope(connection_id, request_id.clone(), session_id)
                    .await
            }
            devo_protocol::native::rpc_session::SessionInterruptScope::Task { item_id } => {
                let response = self
                    .handle_native_task_interrupt(
                        connection_id,
                        request_id.clone(),
                        serde_json::json!({ "itemId": item_id.as_str() }),
                    )
                    .await;
                if response.get("error").is_some() {
                    return response;
                }
                Ok(true)
            }
            devo_protocol::native::rpc_session::SessionInterruptScope::Command { process_id } => {
                let response = self
                    .handle_command_exec_terminate(
                        connection_id,
                        request_id.clone(),
                        serde_json::json!({ "process_id": process_id }),
                    )
                    .await;
                if response.get("error").is_some() {
                    return response;
                }
                Ok(true)
            }
        };

        let interrupted = match interrupted {
            Ok(interrupted) => interrupted,
            Err((code, message)) => return self.error_response(request_id, code, message),
        };
        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: devo_protocol::native::rpc_session::SessionInterruptResult { interrupted },
        })
        .expect("serialize session/interrupt response")
    }

    async fn interrupt_session_scope(
        self: &Arc<Self>,
        connection_id: u64,
        request_id: serde_json::Value,
        session_id: devo_protocol::native::ids::SessionId,
    ) -> std::result::Result<bool, (ProtocolErrorCode, String)> {
        let session_id = SessionId::try_from(session_id.as_str()).map_err(|_| {
            (
                ProtocolErrorCode::SessionNotFound,
                "session id is not addressable by this server".to_string(),
            )
        })?;
        if self.session(session_id).await.is_none() {
            return Err((
                ProtocolErrorCode::SessionNotFound,
                "session does not exist".to_string(),
            ));
        }

        let active_turn_id = self.runtime_active_turn_id(session_id).await;
        let interrupted_turn = if let Some(turn_id) = active_turn_id {
            let response = self
                .interrupt_turn(
                    request_id,
                    serde_json::to_value(TurnInterruptParams {
                        session_id,
                        turn_id,
                        reason: Some("interrupted by session/interrupt".to_string()),
                    })
                    .expect("serialize internal turn interruption"),
                )
                .await;
            if response.get("error").is_some() {
                return Err((
                    ProtocolErrorCode::InternalError,
                    "failed to interrupt active session turn".to_string(),
                ));
            }
            true
        } else {
            false
        };
        let task_count = self
            .command_exec_manager
            .terminate_session(connection_id, session_id)
            .await;
        Ok(interrupted_turn || task_count > 0)
    }
}
