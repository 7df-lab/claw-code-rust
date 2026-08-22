use super::*;
use crate::PendingServerRequestContext;
use crate::ServerRequestKind;

impl ServerRuntime {
    pub(super) async fn request_user_input_for_tool(
        self: &Arc<Self>,
        session_id: SessionId,
        turn_id: TurnId,
        tool_call_id: String,
        args: RequestUserInputArgs,
    ) -> Result<RequestUserInputResponse, ToolCallError> {
        let request_id = tool_call_id;
        let (tx, rx) = oneshot::channel();

        if self.session(session_id).await.is_none() {
            return Err(ToolCallError::ExecutionFailed(
                "session does not exist".to_string(),
            ));
        }

        let host_session_id = self.permission_host_session_id(session_id).await;
        let persisted = self
            .persist_waiting_user_input_item(
                session_id,
                turn_id,
                request_id.clone(),
                &args.questions,
            )
            .await;
        self.session_interactive
            .register_pending_user_input(
                host_session_id,
                request_id.clone(),
                PendingUserInput {
                    owner_session_id: session_id,
                    turn_id,
                    questions: args.questions.clone(),
                    persisted,
                    tx,
                },
            )
            .await;

        // Native reverse request (L2-DES-APP-008 DD-8): canonical-surface
        // controllers get `userInput/request` with the waiting-state payload;
        // the first valid answer resolves through the same pending registry.
        let native_payload =
            serde_json::to_value(devo_protocol::native::item::Item::UserInputRequest {
                request_id: request_id.clone(),
                target_item_id: None,
                questions: super::interaction_items::native_questions(&args.questions),
                answers: None,
            })
            .expect("serialize canonical userInput/request params");
        let runtime = Arc::clone(self);
        let fanout_request_id = request_id.clone();
        let cancel_token = self
            .active_turns
            .cancel_token_for_host_or_session(host_session_id, session_id)
            .await;
        let fanout_cancel_token = cancel_token.clone();
        let (request_ready_tx, request_ready_rx) = tokio::sync::oneshot::channel();
        let (fanout_error_tx, mut fanout_error_rx) = tokio::sync::mpsc::unbounded_channel();
        let fanout = tokio::spawn(async move {
            let owner_connection_id = runtime
                .active_turns
                .active_connection_id(host_session_id)
                .await;
            match runtime
                .request_user_input_from_native_controllers(
                    host_session_id,
                    owner_connection_id,
                    super::control_requests::UserInputControllerRequest {
                        params: native_payload,
                        ready: request_ready_tx,
                    },
                    fanout_cancel_token,
                )
                .await
            {
                Ok(response) => {
                    runtime
                        .resolve_user_input_from_native(
                            session_id,
                            turn_id,
                            fanout_request_id,
                            response,
                        )
                        .await
                }
                Err(error) => {
                    let _ = fanout_error_tx.send(error);
                }
            }
        });

        match request_ready_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                fanout.abort();
                self.fail_pending_user_input(
                    session_id,
                    turn_id,
                    &request_id,
                    cancel_token.is_cancelled(),
                )
                .await;
                return Err(ToolCallError::ExecutionFailed(error));
            }
            Err(_) => {
                fanout.abort();
                self.fail_pending_user_input(session_id, turn_id, &request_id, true)
                    .await;
                return Err(ToolCallError::ExecutionFailed(
                    "user-input request readiness channel closed".to_string(),
                ));
            }
        }

        self.broadcast_event(ServerEvent::RequestUserInput(RequestUserInputPayload {
            request: PendingServerRequestContext {
                request_id: request_id.clone().into(),
                request_kind: ServerRequestKind::ItemToolRequestUserInput,
                session_id,
                turn_id: Some(turn_id),
                item_id: None,
            },
            questions: args.questions,
        }))
        .await;

        let mut rx = rx;
        let result = tokio::select! {
            response = &mut rx => response.map_err(|_| {
                ToolCallError::ExecutionFailed("request_user_input channel closed".to_string())
            }),
            error = fanout_error_rx.recv() => {
                match error {
                    Some(error) => {
                        self.fail_pending_user_input(
                            session_id,
                            turn_id,
                            &request_id,
                            cancel_token.is_cancelled(),
                        )
                        .await;
                        Err(ToolCallError::ExecutionFailed(error))
                    }
                    None => rx.await.map_err(|_| {
                        ToolCallError::ExecutionFailed(
                            "request_user_input channel closed".to_string(),
                        )
                    }),
                }
            }
        };
        // The question is answered (or the turn died); the losing reverse
        // requests are abandoned — late responses are ignored by design.
        fanout.abort();
        result
    }

    async fn fail_pending_user_input(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request_id: &str,
        interrupted: bool,
    ) {
        let Ok(pending) = self
            .session_interactive
            .take_pending_user_input(session_id, request_id, turn_id)
            .await
        else {
            return;
        };
        if let Some(persisted) = &pending.persisted {
            self.persist_terminal_user_input_item(
                session_id,
                turn_id,
                request_id.to_string(),
                &pending.questions,
                if interrupted {
                    devo_protocol::native::item::ItemState::Interrupted
                } else {
                    devo_protocol::native::item::ItemState::Failed
                },
                persisted,
            )
            .await;
        }
    }

    /// Resolves a pending user-input question from a canonical
    /// `userInput/request` answer (DD-8), mirroring the legacy respond path.
    /// A no-op when another controller already answered.
    pub(super) async fn resolve_user_input_from_native(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request_key: String,
        response: RequestUserInputResponse,
    ) {
        let Ok(pending) = self
            .session_interactive
            .take_pending_user_input(session_id, &request_key, turn_id)
            .await
        else {
            return;
        };
        if let Some(persisted) = &pending.persisted {
            self.persist_answered_user_input_item(
                session_id,
                turn_id,
                request_key.clone(),
                &pending.questions,
                &response,
                persisted,
            )
            .await;
        }
        let _ = pending.tx.send(response);
        self.broadcast_event(ServerEvent::ServerRequestResolved(
            ServerRequestResolvedPayload {
                session_id,
                request_id: request_key.into(),
                turn_id: Some(turn_id),
            },
        ))
        .await;
    }
}
