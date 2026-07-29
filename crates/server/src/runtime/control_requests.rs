use futures::StreamExt;
use futures::stream::FuturesUnordered;
use tokio_util::sync::CancellationToken;

use super::*;

impl ServerRuntime {
    /// Sends the same logical approval request to every connection controlling
    /// the session. The first syntactically valid decision wins; transport
    /// failures and malformed responses do not prevent another controller from
    /// answering. Dropping the remaining futures ignores late ACP JSON-RPC
    /// responses: a response cannot itself be answered with an
    /// `CONTROL_REQUEST_ALREADY_RESOLVED` error.
    pub(super) async fn request_permission_from_controllers(
        &self,
        host_session_id: SessionId,
        owner_connection_id: Option<u64>,
        request_params: devo_protocol::AcpRequestPermissionParams,
        cancel_token: CancellationToken,
    ) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
        let connection_ids = self
            .controlling_connection_ids(host_session_id, owner_connection_id)
            .await;
        if connection_ids.is_empty() {
            return Err("no ACP client connection is available for permission request".to_string());
        }

        let params =
            serde_json::to_value(request_params).expect("serialize ACP permission request params");
        let mut pending = FuturesUnordered::new();
        for connection_id in connection_ids {
            pending.push(self.send_request_to_connection_cancellable(
                connection_id,
                devo_protocol::ACP_SESSION_REQUEST_PERMISSION_METHOD,
                params.clone(),
                cancel_token.clone(),
            ));
        }

        let mut last_error = None;
        while let Some(response) = pending.next().await {
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            let response: devo_protocol::AcpRequestPermissionResponse =
                match serde_json::from_value(response) {
                    Ok(response) => response,
                    Err(error) => {
                        last_error = Some(format!(
                            "invalid session/request_permission response: {error}"
                        ));
                        continue;
                    }
                };
            match super::approval::approval_decision_from_acp_outcome(response.outcome) {
                Ok(decision) => return Ok(decision),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| "all permission controllers disconnected".to_string()))
    }
}
