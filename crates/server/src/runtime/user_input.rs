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
                session_id,
                request_id.clone(),
                PendingUserInput {
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
        let host_session_id = self.permission_host_session_id(session_id).await;
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
        let fanout = tokio::spawn(async move {
            let owner_connection_id = runtime
                .active_turns
                .active_connection_id(host_session_id)
                .await;
            if let Ok(response) = runtime
                .request_user_input_from_native_controllers(
                    host_session_id,
                    owner_connection_id,
                    native_payload,
                )
                .await
            {
                runtime
                    .resolve_user_input_from_native(
                        session_id,
                        turn_id,
                        fanout_request_id,
                        response,
                    )
                    .await;
            }
        });

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

        let result = rx.await.map_err(|_| {
            ToolCallError::ExecutionFailed("request_user_input channel closed".to_string())
        });
        // The question is answered (or the turn died); the losing reverse
        // requests are abandoned — late responses are ignored by design.
        fanout.abort();
        result
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
