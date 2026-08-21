use futures::StreamExt;
use futures::stream::FuturesUnordered;
use tokio_util::sync::CancellationToken;

use super::connection::ConnectionProtocol;
use super::*;

/// Maps a native `ApprovalDecision` (L2-DES-APP-008 DD-8) into the
/// internal decision/scope tuple, the inverse direction of the ACP outcome
/// mapping in `approval.rs`.
pub(super) fn approval_decision_from_native(
    decision: &devo_protocol::native::item::ApprovalDecision,
) -> (ApprovalDecisionValue, ApprovalScopeValue) {
    use devo_protocol::native::item::{ApprovalDecisionKind, ApprovalScope};
    let decision_value = match decision.decision {
        ApprovalDecisionKind::Approved => ApprovalDecisionValue::Approve,
        ApprovalDecisionKind::Denied => ApprovalDecisionValue::Deny,
        ApprovalDecisionKind::Cancelled => ApprovalDecisionValue::Cancel,
    };
    let scope = match decision.scope {
        ApprovalScope::Once => ApprovalScopeValue::Once,
        ApprovalScope::Turn => ApprovalScopeValue::Turn,
        ApprovalScope::Session => ApprovalScopeValue::Session,
        ApprovalScope::PathPrefix => ApprovalScopeValue::PathPrefix,
        ApprovalScope::Host => ApprovalScopeValue::Host,
        ApprovalScope::Tool => ApprovalScopeValue::Tool,
        ApprovalScope::CommandPrefix => ApprovalScopeValue::CommandPrefix,
        ApprovalScope::CommandPrefixPersist => ApprovalScopeValue::CommandPrefixPersist,
    };
    (decision_value, scope)
}

impl ServerRuntime {
    /// Sends the same logical approval request to every connection controlling
    /// the session. The first syntactically valid decision wins; transport
    /// failures and malformed responses do not prevent another controller from
    /// answering. Dropping the remaining futures ignores late ACP JSON-RPC
    /// responses: a response cannot itself be answered with an
    /// `CONTROL_REQUEST_ALREADY_RESOLVED` error.
    ///
    /// Mixed-surface fan-out (L2-DES-APP-008 DD-8): native-surface
    /// connections receive the Native request; ACP-surface connections
    /// receive the ACP permission request. Both responses map to the same
    /// internal decision tuple.
    pub(super) async fn request_permission_from_controllers(
        &self,
        host_session_id: SessionId,
        owner_connection_id: Option<u64>,
        request_params: devo_protocol::AcpRequestPermissionParams,
        native_method: &str,
        native_params: serde_json::Value,
        cancel_token: CancellationToken,
    ) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
        let connection_ids = self
            .controlling_connection_ids(host_session_id, owner_connection_id)
            .await;
        if connection_ids.is_empty() {
            return Err("no client connection is available for permission request".to_string());
        }

        let acp_params =
            serde_json::to_value(request_params).expect("serialize ACP permission request params");
        let mut pending = FuturesUnordered::new();
        for connection_id in connection_ids {
            let native = match self.connection_protocol(connection_id).await {
                Some(ConnectionProtocol::Native) => true,
                Some(ConnectionProtocol::Acp) => false,
                None => continue,
            };
            let (method, params) = if native {
                (native_method.to_string(), native_params.clone())
            } else {
                (
                    devo_protocol::ACP_SESSION_REQUEST_PERMISSION_METHOD.to_string(),
                    acp_params.clone(),
                )
            };
            let request_cancel_token = cancel_token.clone();
            pending.push(async move {
                let result = self
                    .send_request_to_connection_cancellable(
                        connection_id,
                        &method,
                        params,
                        request_cancel_token,
                    )
                    .await;
                (native, result)
            });
        }

        let mut last_error = None;
        while let Some((native, response)) = pending.next().await {
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let decision = if native {
                match serde_json::from_value::<devo_protocol::native::methods::ApprovalRespondParams>(
                    response,
                ) {
                    Ok(answer) => Ok(approval_decision_from_native(&answer.decision)),
                    Err(error) => Err(format!("invalid native approval response: {error}")),
                }
            } else {
                match serde_json::from_value::<devo_protocol::AcpRequestPermissionResponse>(
                    response,
                ) {
                    Ok(response) => {
                        super::approval::approval_decision_from_acp_outcome(response.outcome)
                    }
                    Err(error) => Err(format!(
                        "invalid session/request_permission response: {error}"
                    )),
                }
            };
            match decision {
                Ok(decision) => return Ok(decision),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| "all permission controllers disconnected".to_string()))
    }

    /// Fan-out for native `userInput/request` (L2-DES-APP-008 DD-8):
    /// sends the waiting-state question payload to every controlling
    /// connection; the first valid `UserInputRespondParams` answer wins.
    pub(super) async fn request_user_input_from_native_controllers(
        &self,
        session_id: SessionId,
        owner_connection_id: Option<u64>,
        questions_payload: serde_json::Value,
    ) -> Result<devo_protocol::RequestUserInputResponse, String> {
        let connection_ids = self
            .controlling_connection_ids(session_id, owner_connection_id)
            .await;
        let mut pending = FuturesUnordered::new();
        for connection_id in connection_ids {
            if self.connection_protocol(connection_id).await != Some(ConnectionProtocol::Native) {
                continue;
            }
            pending.push(self.send_request_to_connection_cancellable(
                connection_id,
                "userInput/request",
                questions_payload.clone(),
                CancellationToken::new(),
            ));
        }
        while let Some(response) = pending.next().await {
            let Ok(response) = response else {
                continue;
            };
            let Ok(answer) = serde_json::from_value::<
                devo_protocol::native::methods::UserInputRespondParams,
            >(response) else {
                continue;
            };
            if let Ok(answers) = serde_json::from_value::<
                std::collections::HashMap<String, devo_protocol::RequestUserInputAnswer>,
            >(answer.answers)
            {
                return Ok(devo_protocol::RequestUserInputResponse { answers });
            }
        }
        Err("no Native user-input controller answered".to_string())
    }
}
