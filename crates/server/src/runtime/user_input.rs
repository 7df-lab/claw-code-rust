use super::*;
use crate::PendingServerRequestContext;
use crate::ServerRequestKind;
use crate::runtime::session_interactive::UserInputTakeError;

impl ServerRuntime {
    pub(super) async fn handle_request_user_input_respond(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: RequestUserInputRespondParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid request_user_input/respond params: {error}"),
                );
            }
        };

        if self.session(params.session_id).await.is_none() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        }

        let request_key = params.request_id.to_string();
        let pending = match self
            .session_interactive
            .take_pending_user_input(params.session_id, &request_key, params.turn_id)
            .await
        {
            Ok(pending) => pending,
            Err(UserInputTakeError::NotFound) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "no pending request_user_input request exists for this runtime",
                );
            }
            Err(UserInputTakeError::WrongTurn) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "request_user_input belongs to a different turn",
                );
            }
        };

        let _ = pending.tx.send(params.response);
        self.broadcast_event(ServerEvent::ServerRequestResolved(
            ServerRequestResolvedPayload {
                session_id: params.session_id,
                request_id: params.request_id.clone(),
                turn_id: Some(params.turn_id),
            },
        ))
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({ "request_id": params.request_id }),
        })
        .expect("serialize request_user_input response")
    }

    pub(super) async fn request_user_input_for_tool(
        &self,
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

        self.session_interactive
            .register_pending_user_input(
                session_id,
                request_id.clone(),
                PendingUserInput {
                    turn_id,
                    questions: args.questions.clone(),
                    tx,
                },
            )
            .await;

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

        rx.await.map_err(|_| {
            ToolCallError::ExecutionFailed("request_user_input channel closed".to_string())
        })
    }
}
