use futures::StreamExt;
use futures::stream::FuturesUnordered;
use tokio_util::sync::CancellationToken;

use super::connection::ConnectionProtocol;
use super::*;

pub(super) struct PermissionControllerRequest {
    pub(super) acp_params: devo_protocol::AcpRequestPermissionParams,
    pub(super) native_method: String,
    pub(super) native_params: serde_json::Value,
    pub(super) ready: tokio::sync::oneshot::Sender<Result<(), String>>,
}

pub(super) struct UserInputControllerRequest {
    pub(super) params: serde_json::Value,
    pub(super) ready: tokio::sync::oneshot::Sender<Result<(), String>>,
}

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
    pub(super) async fn reissue_pending_control_requests(
        self: &Arc<Self>,
        connection_id: u64,
        requests: Vec<devo_protocol::native::event::PendingControlRequest>,
    ) -> Vec<devo_protocol::native::event::PendingControlRequest> {
        use devo_protocol::native::event::ControlRequestKind;
        use devo_protocol::native::item::Item;

        let mut answerable = Vec::new();
        for request in requests {
            let Ok(session_id) = SessionId::try_from(request.item.session_id.as_str()) else {
                continue;
            };
            let Ok(turn_id) = TurnId::try_from(request.item.turn_id.as_str()) else {
                continue;
            };
            let (method, approval_controller) = match (&request.kind, &request.item.item) {
                (ControlRequestKind::ApprovalCommand, Item::Approval { approval_id, .. }) => (
                    "approval/command/request",
                    self.session_interactive
                        .approval_controller(approval_id)
                        .await,
                ),
                (ControlRequestKind::ApprovalFileChange, Item::Approval { approval_id, .. }) => (
                    "approval/fileChange/request",
                    self.session_interactive
                        .approval_controller(approval_id)
                        .await,
                ),
                (ControlRequestKind::ApprovalPermission, Item::Approval { approval_id, .. }) => (
                    "approval/permission/request",
                    self.session_interactive
                        .approval_controller(approval_id)
                        .await,
                ),
                (ControlRequestKind::UserInput, Item::UserInputRequest { .. }) => {
                    ("userInput/request", None)
                }
                (ControlRequestKind::GoalCompletion, _) => continue,
                _ => continue,
            };
            if !matches!(request.kind, ControlRequestKind::UserInput)
                && approval_controller.is_none()
            {
                continue;
            }

            let params = serde_json::to_value(&request.item.item)
                .expect("serialize recovered control request item");
            let host_session_id = approval_controller
                .as_ref()
                .map_or(session_id, |(host_session_id, _)| *host_session_id);
            let cancel_token = self
                .active_turns
                .cancel_token_for_host_or_session(host_session_id, session_id)
                .await;
            let (enqueued_tx, mut enqueued_rx) = tokio::sync::mpsc::unbounded_channel();
            let runtime = Arc::clone(self);
            let request_id = request.request_id.clone();
            let method = method.to_string();
            tokio::spawn(async move {
                let response = runtime
                    .send_request_to_connection_cancellable_with_enqueue_signal(
                        connection_id,
                        &method,
                        params,
                        cancel_token,
                        enqueued_tx,
                    )
                    .await;
                let Ok(response) = response else {
                    return;
                };
                if method == "userInput/request" {
                    let Ok(answer) = serde_json::from_value::<
                        devo_protocol::native::methods::UserInputRespondParams,
                    >(response) else {
                        return;
                    };
                    let Ok(answers) = serde_json::from_value::<
                        std::collections::HashMap<String, devo_protocol::RequestUserInputAnswer>,
                    >(answer.answers) else {
                        return;
                    };
                    runtime
                        .resolve_user_input_from_native(
                            session_id,
                            turn_id,
                            request_id,
                            devo_protocol::RequestUserInputResponse { answers },
                        )
                        .await;
                } else if let Some((host_session_id, controller)) = approval_controller
                    && let Ok(answer) = serde_json::from_value::<
                        devo_protocol::native::methods::ApprovalRespondParams,
                    >(response)
                {
                    let (decision, scope) = approval_decision_from_native(&answer.decision);
                    if runtime.active_turns.has_session(session_id).await {
                        let _ = controller.send((decision, scope));
                    } else {
                        runtime
                            .resolve_approval_from_control_response(
                                host_session_id,
                                session_id,
                                turn_id,
                                &request_id,
                                decision,
                                scope,
                            )
                            .await;
                    }
                }
            });
            if enqueued_rx.recv().await.is_some() {
                answerable.push(request);
            }
        }
        answerable
    }

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
        request: PermissionControllerRequest,
        cancel_token: CancellationToken,
    ) -> Result<(ApprovalDecisionValue, ApprovalScopeValue), String> {
        let connection_ids = self
            .controlling_connection_ids(host_session_id, owner_connection_id)
            .await;
        if connection_ids.is_empty() {
            let error = "no client connection is available for permission request".to_string();
            let _ = request.ready.send(Err(error.clone()));
            return Err(error);
        }

        let acp_params = serde_json::to_value(request.acp_params)
            .expect("serialize ACP permission request params");
        let mut pending = FuturesUnordered::new();
        let (enqueued_tx, mut enqueued_rx) = tokio::sync::mpsc::unbounded_channel();
        for connection_id in connection_ids {
            let native = match self.connection_protocol(connection_id).await {
                Some(ConnectionProtocol::Native) => true,
                Some(ConnectionProtocol::Acp) => false,
                None => continue,
            };
            let (method, params) = if native {
                (request.native_method.clone(), request.native_params.clone())
            } else {
                (
                    devo_protocol::ACP_SESSION_REQUEST_PERMISSION_METHOD.to_string(),
                    acp_params.clone(),
                )
            };
            let request_cancel_token = cancel_token.clone();
            let request_enqueued_tx = enqueued_tx.clone();
            pending.push(async move {
                let result = self
                    .send_request_to_connection_cancellable_with_enqueue_signal(
                        connection_id,
                        &method,
                        params,
                        request_cancel_token,
                        request_enqueued_tx,
                    )
                    .await;
                (native, result)
            });
        }
        drop(enqueued_tx);

        let mut last_error = None;
        let mut first_response = None;
        let mut request_ready = Some(request.ready);
        let mut enqueued_open = true;
        loop {
            tokio::select! {
                enqueued = enqueued_rx.recv(), if enqueued_open => {
                    match enqueued {
                        Some(()) => {
                            if let Some(request_ready) = request_ready.take() {
                                let _ = request_ready.send(Ok(()));
                            }
                            break;
                        }
                        None => enqueued_open = false,
                    }
                }
                response = pending.next() => {
                    let Some(response) = response else {
                        let error = last_error.unwrap_or_else(|| {
                            "all permission controllers disconnected before request delivery"
                                .to_string()
                        });
                        if let Some(request_ready) = request_ready.take() {
                            let _ = request_ready.send(Err(error.clone()));
                        }
                        return Err(error);
                    };
                    if response.1.is_ok() {
                        if let Some(request_ready) = request_ready.take() {
                            let _ = request_ready.send(Ok(()));
                        }
                        first_response = Some(response);
                        break;
                    }
                    last_error = response.1.err();
                }
            }
        }

        loop {
            let response = match first_response.take() {
                Some(response) => Some(response),
                None => pending.next().await,
            };
            let Some((native, response)) = response else {
                break;
            };
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
        request: UserInputControllerRequest,
        cancel_token: CancellationToken,
    ) -> Result<devo_protocol::RequestUserInputResponse, String> {
        let connection_ids = self
            .controlling_connection_ids(session_id, owner_connection_id)
            .await;
        let mut pending = FuturesUnordered::new();
        let (enqueued_tx, mut enqueued_rx) = tokio::sync::mpsc::unbounded_channel();
        for connection_id in connection_ids {
            if self.connection_protocol(connection_id).await != Some(ConnectionProtocol::Native) {
                continue;
            }
            let params = request.params.clone();
            let request_cancel_token = cancel_token.clone();
            let request_enqueued_tx = enqueued_tx.clone();
            pending.push(async move {
                self.send_request_to_connection_cancellable_with_enqueue_signal(
                    connection_id,
                    "userInput/request",
                    params,
                    request_cancel_token,
                    request_enqueued_tx,
                )
                .await
            });
        }
        drop(enqueued_tx);

        let mut request_ready = Some(request.ready);
        let mut first_response = None;
        let mut enqueued_open = true;
        let mut last_error = None;
        loop {
            tokio::select! {
                enqueued = enqueued_rx.recv(), if enqueued_open => {
                    match enqueued {
                        Some(()) => {
                            if let Some(request_ready) = request_ready.take() {
                                let _ = request_ready.send(Ok(()));
                            }
                            break;
                        }
                        None => enqueued_open = false,
                    }
                }
                response = pending.next() => {
                    let Some(response) = response else {
                        let error = "no Native user-input controller is available".to_string();
                        if let Some(request_ready) = request_ready.take() {
                            let _ = request_ready.send(Err(error.clone()));
                        }
                        return Err(error);
                    };
                    if response.is_ok() {
                        if let Some(request_ready) = request_ready.take() {
                            let _ = request_ready.send(Ok(()));
                        }
                        first_response = Some(response);
                        break;
                    }
                }
            }
        }

        loop {
            let response = match first_response.take() {
                Some(response) => Some(response),
                None => pending.next().await,
            };
            let Some(response) = response else {
                break;
            };
            let Ok(response) = response else {
                continue;
            };
            let answer = match serde_json::from_value::<
                devo_protocol::native::methods::UserInputRespondParams,
            >(response)
            {
                Ok(answer) => answer,
                Err(error) => {
                    last_error = Some(format!("invalid native user-input response: {error}"));
                    continue;
                }
            };
            match serde_json::from_value::<
                std::collections::HashMap<String, devo_protocol::RequestUserInputAnswer>,
            >(answer.answers)
            {
                Ok(answers) => {
                    return Ok(devo_protocol::RequestUserInputResponse { answers });
                }
                Err(error) => {
                    last_error = Some(format!("invalid native user-input answers: {error}"));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "no Native user-input controller answered".to_string()))
    }

    /// After `session/resume` hydrates waiters, reissue only when this
    /// connection already subscribed (so `subscription/create` will not).
    pub(super) async fn reissue_pending_controls_if_subscribed(
        self: &Arc<Self>,
        connection_id: u64,
        session_id: &devo_protocol::native::ids::SessionId,
    ) {
        use devo_protocol::native::event::StreamSelector;
        let subscribed = self
            .event_subscriptions
            .lock()
            .await
            .values()
            .any(|subscription| {
                subscription.connection_id == connection_id
                    && subscription.selectors.iter().any(|selector| {
                        matches!(
                            selector,
                            StreamSelector::Session {
                                session_id: subscribed_id
                            } if subscribed_id.as_str() == session_id.as_str()
                        )
                    })
            });
        if !subscribed {
            return;
        }
        let pending = self
            .pending_control_requests(&[StreamSelector::Session {
                session_id: session_id.clone(),
            }])
            .await;
        self.reissue_pending_control_requests(connection_id, pending)
            .await;
    }
}
