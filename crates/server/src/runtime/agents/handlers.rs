use super::*;

/// Projects one legacy `AgentInfo` into a canonical `SubAgent` item
/// envelope (L2-DES-APP-008 Phase B facade). The item id is synthesized
/// from the child session's uuid; `task` carries the last task message when
/// known, and `state` maps the registry status string. The legacy model has
/// no spawning-turn reference, so `turn_id` is a session-derived placeholder
/// until agents become first-class items (DD-7 v2).
pub(in crate::runtime) fn subagent_item_from_agent_info(
    info: &devo_protocol::AgentInfo,
) -> devo_protocol::native::item::ItemEnvelope {
    use devo_protocol::native::ids::{ItemId, SessionId as NativeSessionId, TurnId};
    use devo_protocol::native::item::{Item, ItemEnvelope, ItemState, SpawnedWorkState};
    use uuid::Uuid;

    let state = match info.status.as_str() {
        "spawning" | "running" => SpawnedWorkState::Running,
        "completed" | "waiting_for_input" => SpawnedWorkState::Completed,
        "failed" => SpawnedWorkState::Failed,
        "interrupted" | "canceled" | "closed" => SpawnedWorkState::Cancelled,
        _ => SpawnedWorkState::Lost,
    };
    let item_state = match state {
        SpawnedWorkState::Running => ItemState::Running,
        SpawnedWorkState::Completed => ItemState::Completed,
        SpawnedWorkState::Failed => ItemState::Failed,
        SpawnedWorkState::Cancelled | SpawnedWorkState::Lost => ItemState::Interrupted,
    };
    let now = chrono::Utc::now();
    ItemEnvelope {
        id: ItemId::from_legacy_uuid(Uuid::from(info.session_id)),
        session_id: NativeSessionId::from_legacy_uuid(
            info.parent_session_id
                .map(Uuid::from)
                .unwrap_or_else(|| Uuid::from(info.session_id)),
        ),
        turn_id: TurnId::from_legacy_uuid(Uuid::from(info.session_id)),
        seq: 0,
        revision: 1,
        created_at: now,
        updated_at: now,
        state: item_state,
        item: Item::SubAgent {
            origin_call_id: None,
            agent_session_id: NativeSessionId::from_legacy_uuid(Uuid::from(info.session_id)),
            parent_session_id: info
                .parent_session_id
                .map(|parent| NativeSessionId::from_legacy_uuid(Uuid::from(parent)))
                .unwrap_or_else(|| NativeSessionId::from_legacy_uuid(Uuid::from(info.session_id))),
            role: (!info.agent_role.is_empty()).then(|| info.agent_role.clone()),
            task: info
                .last_task_message
                .clone()
                .unwrap_or_else(|| info.agent_nickname.clone()),
            state,
        },
    }
}

impl ServerRuntime {
    // ── Agent Handlers ────────────────────────────────────────────────

    /// Native `agent/list` (L2-DES-APP-008 Phase B): lists child agents
    /// as `SubAgent` item envelopes. The item id is synthesized from the
    /// child session id (stable, derivable); the internal model converges on
    /// items in DD-7 v2.
    pub(in crate::runtime) async fn handle_native_agent_list(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::AgentListParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical agent/list params: {error}"),
                    );
                }
            };
        let legacy_session_id = match &params.session_id {
            Some(session_id) => match SessionId::try_from(session_id.as_str()) {
                Ok(session_id) => Some(session_id),
                Err(_) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::SessionNotFound,
                        "session id is not addressable by this server",
                    );
                }
            },
            None => None,
        };
        let Some(legacy_session_id) = legacy_session_id else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "canonical agent/list requires a session id in this facade",
            );
        };
        match Arc::clone(self)
            .list_agents(devo_protocol::AgentListParams {
                session_id: legacy_session_id,
                path_prefix: None,
            })
            .await
        {
            Ok(agents) => {
                let items = agents
                    .into_iter()
                    .map(|info| subagent_item_from_agent_info(&info))
                    .collect::<Vec<_>>();
                success_response(
                    request_id,
                    devo_protocol::native::rpc_turn::AgentListResult { agents: items },
                )
            }
            Err(error) => self.tool_error_response(request_id, error),
        }
    }

    /// Native `agent/cancel` (L2-DES-APP-008 Phase B facade): the item id
    /// (session-derived, see `subagent_item_from_agent_info`) resolves the
    /// child session and its parent, then translates onto `agent/close`.
    pub(in crate::runtime) async fn handle_native_agent_cancel(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::AgentCancelParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical agent/cancel params: {error}"),
                    );
                }
            };
        let Some((parent_session_id, child_session_id)) =
            self.agent_item_target(params.item_id.as_str()).await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "agent item is not addressable by this server",
            );
        };
        match Arc::clone(self)
            .close_agent(devo_protocol::CloseAgentParams {
                session_id: parent_session_id,
                target: child_session_id.to_string(),
            })
            .await
        {
            Ok(_) => success_response(
                request_id,
                devo_protocol::native::rpc_turn::AgentCancelResult {},
            ),
            Err(error) => self.tool_error_response(request_id, error),
        }
    }

    /// Native `agent/message` (Phase B facade): text inputs join into the
    /// legacy message string.
    pub(in crate::runtime) async fn handle_native_agent_message(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::AgentMessageParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical agent/message params: {error}"),
                    );
                }
            };
        let Some((parent_session_id, child_session_id)) =
            self.agent_item_target(params.item_id.as_str()).await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "agent item is not addressable by this server",
            );
        };
        let message = params
            .input
            .iter()
            .filter_map(|input| match input {
                devo_protocol::native::item::UserInput::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if message.is_empty() {
            return self.error_response(
                request_id,
                ProtocolErrorCode::InvalidParams,
                "agent/message requires at least one text input",
            );
        }
        match Arc::clone(self)
            .send_message(devo_protocol::AgentMessageParams {
                session_id: parent_session_id,
                target: child_session_id.to_string(),
                message,
            })
            .await
        {
            Ok(_) => success_response(
                request_id,
                devo_protocol::native::rpc_turn::AgentMessageResult {},
            ),
            Err(error) => self.tool_error_response(request_id, error),
        }
    }

    /// Native `agent/read` (Phase B facade): the SubAgent item snapshot
    /// plus `recent_progress` from the registry's last task message.
    pub(in crate::runtime) async fn handle_native_agent_read(
        self: &Arc<Self>,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: devo_protocol::native::rpc_turn::AgentReadParams =
            match serde_json::from_value(params) {
                Ok(params) => params,
                Err(error) => {
                    return self.error_response(
                        request_id,
                        ProtocolErrorCode::InvalidParams,
                        format!("invalid canonical agent/read params: {error}"),
                    );
                }
            };
        let Some((parent_session_id, child_session_id)) =
            self.agent_item_target(params.item_id.as_str()).await
        else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "agent item is not addressable by this server",
            );
        };
        match self
            .agent_info(parent_session_id, &child_session_id.to_string())
            .await
        {
            Ok(info) => success_response(
                request_id,
                devo_protocol::native::rpc_turn::AgentReadResult {
                    item: subagent_item_from_agent_info(&info),
                    recent_progress: info.last_task_message.clone(),
                },
            ),
            Err(error) => self.tool_error_response(request_id, error),
        }
    }

    /// Resolves a canonical agent item id to `(parent, child)` session ids.
    /// The facade synthesizes item ids from child session uuids
    /// (`item_<uuid>`).
    pub(in crate::runtime) async fn agent_item_target(
        &self,
        item_id: &str,
    ) -> Option<(SessionId, SessionId)> {
        let child_str = item_id.strip_prefix("item_").unwrap_or(item_id);
        let child = SessionId::try_from(child_str).ok()?;
        let registries = self.agent_registries.lock().await;
        registries.values().find_map(|registry| {
            registry
                .get(child)
                .map(|meta| (meta.parent_session_id, child))
        })
    }

    pub(in crate::runtime) fn tool_error_response(
        &self,
        request_id: serde_json::Value,
        error: ToolCallError,
    ) -> serde_json::Value {
        let code = match error {
            ToolCallError::InvalidInput(_) => ProtocolErrorCode::InvalidParams,
            ToolCallError::Denied(_) => ProtocolErrorCode::PermissionDenied,
            ToolCallError::Cancelled => ProtocolErrorCode::AlreadyResolved,
            _ => ProtocolErrorCode::InternalError,
        };
        self.error_response(request_id, code, error.to_string())
    }
}

fn success_response<T: serde::Serialize>(
    request_id: serde_json::Value,
    result: T,
) -> serde_json::Value {
    serde_json::to_value(SuccessResponse {
        id: request_id,
        result,
    })
    .expect("serialize agent response")
}
