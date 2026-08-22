use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::ACP_AUTHENTICATE_METHOD;
use crate::ACP_INITIALIZE_METHOD;
use crate::ACP_LOGOUT_METHOD;
use crate::acp_auth_required_response;
use crate::acp_notification_from_server_event;
use devo_protocol::native::wire_projector::typed_item_notification_from_server_event;

use super::handlers::acp::AcpRoute;
use super::handlers::acp::acp_route;
use super::outbound::OutboundDeliveryPolicy;
use super::outbound::OutboundFrame;
use super::outbound::enqueue_outbound;
use super::outbound::enqueue_outbound_notification;

pub(crate) const INBOUND_CONCURRENCY_LIMIT: usize = 64;

#[derive(Debug)]
pub struct IncomingResponse {
    response: serde_json::Value,
    post_response_actions: PostResponseActions,
}

impl IncomingResponse {
    fn new(response: serde_json::Value) -> Self {
        Self {
            response,
            post_response_actions: PostResponseActions::default(),
        }
    }

    fn with_post_response_action(mut self, action: PostResponseAction) -> Self {
        self.post_response_actions.0.push(action);
        self
    }

    pub fn into_parts(self) -> (serde_json::Value, PostResponseActions) {
        (self.response, self.post_response_actions)
    }

    fn is_success(&self) -> bool {
        self.response.get("result").is_some() && self.response.get("error").is_none()
    }

    fn with_acp_session_state_snapshot_after_success(
        self,
        connection_id: u64,
        session_id: Option<SessionId>,
    ) -> Self {
        let Some(session_id) = session_id else {
            return self;
        };
        if !self.is_success() {
            return self;
        }
        self.with_post_response_action(PostResponseAction::SendAcpSessionStateSnapshot {
            connection_id,
            session_id,
        })
    }
}

#[derive(Debug, Default)]
pub struct PostResponseActions(Vec<PostResponseAction>);

#[derive(Debug)]
enum PostResponseAction {
    SendAcpSessionStateSnapshot {
        connection_id: u64,
        session_id: SessionId,
    },
}

/// Which protocol surface a connection is bound to, negotiated at
/// `initialize` via `_meta.devo.protocol` (L2-DES-APP-008 / L2-DES-APP-009).
///
/// ACP and the native protocol share method names (`session/new`,
/// `session/resume`, `session/list`, `session/delete`, …), so routing cannot
/// key on the method name alone: ACP connections keep the ACP adapter
/// behavior pinned by `protocol-lock.json`, native connections route
/// those names to the native handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionProtocol {
    /// ACP adapter surface.
    Acp,
    /// Native protocol surface for first-party clients.
    Native,
}

impl ServerRuntime {
    pub async fn register_connection(
        self: &Arc<Self>,
        transport: ClientTransportKind,
        outbound_tx: mpsc::Sender<OutboundFrame>,
    ) -> u64 {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::SeqCst);
        let mut connections = self.connections.lock().await;
        connections.insert(
            connection_id,
            ConnectionRuntime {
                transport,
                state: ConnectionState::Connected,
                protocol: None,
                acp_authenticated: false,
                acp_client_capabilities: crate::AcpClientCapabilities::default(),
                typed_items: false,
                event_selectors: Vec::new(),
                outbound_tx,
                opt_out_notification_methods: HashSet::new(),
                subscriptions: Vec::new(),
                next_event_seq: 1,
                next_client_request_id: 1,
                pending_client_requests: HashMap::new(),
            },
        );
        tracing::info!(
            connection_id,
            transport = ?connections
                .get(&connection_id)
                .map(|connection| connection.transport.clone())
                .expect("connection inserted"),
            active_connections = connections.len(),
            "registered client connection"
        );
        connection_id
    }

    pub async fn unregister_connection(&self, connection_id: u64) {
        let mut connections = self.connections.lock().await;
        let mut removed = connections.remove(&connection_id);
        drop(connections);
        if let Some(connection) = removed.as_mut() {
            for (_, pending) in connection.pending_client_requests.drain() {
                let _ = pending.send(Err("client connection closed".to_string()));
            }
        }
        self.drop_restore_plans_for_connection(connection_id).await;
        self.active_turns.drop_connection_id(connection_id).await;
        self.drop_event_subscriptions_for_connection(connection_id)
            .await;
        self.reference_searches
            .lock()
            .await
            .retain(|_, state| state.connection_id() != connection_id);
        self.command_exec_manager
            .terminate_connection(connection_id)
            .await;
        let active_connections = self.connections.lock().await.len();
        tracing::info!(
            connection_id,
            transport = ?removed.as_ref().map(|connection| connection.transport.clone()),
            active_connections,
            "unregistered client connection"
        );
    }

    pub async fn handle_incoming(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let (response, post_response_actions) = self
            .handle_incoming_with_actions(connection_id, message)
            .await?
            .into_parts();
        self.run_post_response_actions(post_response_actions).await;
        Some(response)
    }

    pub async fn handle_incoming_with_actions(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) -> Option<IncomingResponse> {
        if message.get("method").is_none()
            && message.get("id").is_some()
            && (message.get("result").is_some() || message.get("error").is_some())
        {
            self.resolve_pending_client_response(connection_id, message)
                .await;
            return None;
        }
        let method = message.get("method")?.as_str()?.to_string();
        let id = message.get("id").cloned();
        let params = message
            .get("params")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        tracing::debug!(
            connection_id,
            method,
            has_id = id.is_some(),
            "received client message"
        );

        if method == ACP_INITIALIZE_METHOD {
            return Some(IncomingResponse::new(
                self.handle_acp_initialize(connection_id, id, params).await,
            ));
        }
        // Before connection enter `Ready` state, only allowed method: "initialize"
        if !self.connection_ready(connection_id).await {
            return id.map(|request_id| {
                IncomingResponse::new(self.error_response(
                    request_id,
                    ProtocolErrorCode::NotInitialized,
                    "connection has not completed initialize",
                ))
            });
        }

        let protocol = self
            .connection_protocol(connection_id)
            .await
            .expect("ready connection must have a negotiated protocol");
        if protocol == ConnectionProtocol::Acp {
            if method == ACP_AUTHENTICATE_METHOD {
                return Some(IncomingResponse::new(
                    self.handle_acp_authenticate(connection_id, id, params)
                        .await,
                ));
            }
            if method == ACP_LOGOUT_METHOD {
                return Some(IncomingResponse::new(
                    self.handle_acp_logout(connection_id, id, params).await,
                ));
            }
            if !self.connection_authenticated(connection_id).await {
                if let Some(request_id) = id {
                    return Some(IncomingResponse::new(acp_auth_required_response(
                        request_id,
                    )));
                }
                tracing::warn!(
                    connection_id,
                    method,
                    "dropping unauthenticated ACP client notification"
                );
                return None;
            }
            if let Some(route) = acp_route(&method) {
                return self
                    .handle_acp_route(route, connection_id, id, params)
                    .await;
            }
            return id.map(|request_id| {
                IncomingResponse::new(crate::acp_error_response(
                    request_id,
                    crate::AcpErrorCode::MethodNotFound,
                    format!("unknown ACP method: {method}"),
                ))
            });
        }

        let response = match method.as_str() {
            "session/start" => {
                let request_id = id?;
                let params: SessionStartParams = match serde_json::from_value(params) {
                    Ok(params) => params,
                    Err(error) => {
                        return Some(IncomingResponse::new(self.error_response(
                            request_id,
                            ProtocolErrorCode::InvalidParams,
                            format!("invalid session/start params: {error}"),
                        )));
                    }
                };
                let response = self
                    .start_session_with_registry(connection_id, request_id, params, None)
                    .await;
                if let Ok(success) =
                    serde_json::from_value::<SuccessResponse<SessionStartResult>>(response.clone())
                {
                    self.subscribe_connection_to_session(
                        connection_id,
                        success.result.session.session_id,
                        None,
                    )
                    .await;
                }
                Some(response)
            }
            // Update session metadata, including the current model and reasoning effort.
            "session/metadata/update" => Some(
                self.handle_native_session_metadata_update(id?, params)
                    .await,
            ),
            // resume a history session, server load the jsonl file then replay the events in jsonl
            "session/resume" => Some(self.handle_session_resume(connection_id, id?, params).await),
            // fork a given session at given user turn index
            "session/fork" => Some(self.handle_session_fork(connection_id, id?, params).await),
            "session/rollback/preview" => Some(
                self.handle_session_rollback_preview(connection_id, id?, params)
                    .await,
            ),
            "session/rollback/commit" => Some(
                self.handle_session_rollback_commit(connection_id, id?, params)
                    .await,
            ),
            // compact session context history
            "session/compact/start" => {
                Some(self.handle_native_session_compact_start(id?, params).await)
            }
            "session/new" => Some(
                self.handle_native_session_new(connection_id, id?, params)
                    .await,
            ),
            "session/list" => Some(self.handle_native_session_list(id?, params).await),
            "session/read" => Some(self.handle_native_session_read(id?, params).await),
            "session/interrupt" => Some(
                self.handle_session_interrupt(connection_id, id?, params)
                    .await,
            ),
            "runtime/ping" => Some(
                serde_json::to_value(SuccessResponse {
                    id: id?,
                    result: devo_protocol::native::rpc_admin::RuntimePingResult {
                        server_time_ms: Utc::now().timestamp_millis(),
                    },
                })
                .expect("serialize runtime/ping response"),
            ),
            "model/list" => Some(self.handle_native_model_list(id?, params).await),
            "model/preferences/read" => {
                Some(self.handle_native_model_preferences_read(id?, params).await)
            }
            "model/preferences/write" => Some(
                self.handle_native_model_preferences_write(id?, params)
                    .await,
            ),
            "session/delete" => Some(self.handle_native_session_delete(id?, params).await),
            "session/goal/set" => Some(self.handle_native_session_goal_set(id?, params).await),
            "session/goal/read" => Some(self.handle_native_session_goal_read(id?, params).await),
            "session/goal/update" => {
                Some(self.handle_native_session_goal_update(id?, params).await)
            }
            "session/message/edit" => {
                Some(self.handle_native_session_message_edit(id?, params).await)
            }
            "task/start" => Some(
                self.handle_native_task_start(connection_id, id?, params)
                    .await,
            ),
            "task/read" => Some(self.handle_native_task_read(id?, params).await),
            "task/list" => Some(self.handle_native_task_list(id?, params).await),
            "task/interrupt" => Some(
                self.handle_native_task_interrupt(connection_id, id?, params)
                    .await,
            ),
            "task/write_stdin" => Some(
                self.handle_native_task_write_stdin(connection_id, id?, params)
                    .await,
            ),
            "task/resize" => Some(
                self.handle_native_task_resize(connection_id, id?, params)
                    .await,
            ),
            "agent/cancel" => Some(self.handle_native_agent_cancel(id?, params).await),
            "agent/list" => Some(self.handle_native_agent_list(id?, params).await),
            "agent/message" => Some(self.handle_native_agent_message(id?, params).await),
            "agent/read" => Some(self.handle_native_agent_read(id?, params).await),
            "session/goal/pause"
            | "session/goal/resume"
            | "session/goal/complete"
            | "session/goal/cancel"
            | "session/goal/clear" => Some(
                self.handle_native_session_goal_transition(&method, id?, params)
                    .await,
            ),
            "skill/list" => Some(self.handle_native_skill_list(id?, params).await),
            "skill/set_enabled" => Some(self.handle_native_skill_set_enabled(id?, params).await),
            "mcp/list" => Some(self.handle_mcp_list(id?, params).await),
            "mcp/tools" => Some(self.handle_mcp_tools(id?, params).await),
            "mcp/set_enabled" => Some(self.handle_mcp_set_enabled(id?, params).await),
            "context/usage/read" => Some(self.handle_context_usage_read(id?, params).await),
            "search/start" => Some(
                self.handle_native_search_start(connection_id, id?, params)
                    .await,
            ),
            "search/update" => Some(self.handle_native_search_update(id?, params).await),
            "search/cancel" => Some(self.handle_native_search_cancel(id?, params).await),
            "command/exec" => Some(self.handle_command_exec(connection_id, id?, params).await),
            // TODO: start a new user turn, maybe should change name to "turn/submit"
            "turn/start" => Some(
                self.handle_turn_start_for_connection(Some(connection_id), id?, params)
                    .await,
            ),
            "workspace/changes/read" => Some(self.handle_workspace_changes_read(id?, params).await),
            "provider/list" => Some(self.handle_native_provider_list(id?).await),
            "provider/validate" => Some(self.handle_native_provider_validate(id?, params).await),
            "provider/upsert" => Some(self.handle_native_provider_upsert(id?, params).await),
            // Paged history reads of the new Native API (native types).
            "session/turns/list" => Some(self.handle_session_turns_list(id?, params).await),
            "session/items/list" => Some(self.handle_session_items_list(id?, params).await),
            // Durable event subscriptions (08 §4).
            "subscription/create" => Some(
                self.handle_subscription_create(connection_id, id?, params)
                    .await,
            ),
            "subscription/update" => Some(
                self.handle_subscription_update(connection_id, id?, params)
                    .await,
            ),
            "subscription/ack" => Some(
                self.handle_subscription_ack(connection_id, id?, params)
                    .await,
            ),
            "subscription/unsubscribe" => Some(
                self.handle_subscription_unsubscribe(connection_id, id?, params)
                    .await,
            ),
            // Session input queue of the new Native API (01 §4.3).
            "session/queue/push" => Some(
                self.handle_session_queue_push(connection_id, id?, params)
                    .await,
            ),
            "session/queue/list" => Some(self.handle_session_queue_list(id?, params).await),
            "session/queue/update" => Some(self.handle_session_queue_update(id?, params).await),
            "session/queue/remove" => Some(self.handle_session_queue_remove(id?, params).await),
            "session/queue/steer" => Some(
                self.handle_session_queue_steer(connection_id, id?, params)
                    .await,
            ),
            // TODO: add endpoint to kill background process opened by unified exec command.
            // TODO: add endpoint to list current background processes.
            _ => Some(self.error_response(
                id?,
                ProtocolErrorCode::InvalidParams,
                format!("unknown method: {method}"),
            )),
        };
        // Filter out responses already dispatched via the high-priority channel.
        match response {
            Some(serde_json::Value::Null) => None,
            Some(response) => Some(IncomingResponse::new(response)),
            None => None,
        }
    }

    async fn handle_acp_route(
        self: &Arc<Self>,
        route: AcpRoute,
        connection_id: u64,
        request_id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> Option<IncomingResponse> {
        match route {
            AcpRoute::Cancel => {
                self.handle_acp_session_cancel(params).await;
                Some(IncomingResponse::new(crate::acp_success_response(
                    request_id?,
                    crate::AcpEmptyResult::default(),
                )))
            }
            AcpRoute::SessionInterrupt => Some(IncomingResponse::new(
                self.handle_session_interrupt(connection_id, request_id?, params)
                    .await,
            )),
            AcpRoute::Close => Some(IncomingResponse::new(
                self.handle_acp_session_close(request_id?, params).await,
            )),
            AcpRoute::Delete => Some(IncomingResponse::new(
                self.handle_acp_session_delete(request_id?, params).await,
            )),
            AcpRoute::List => Some(IncomingResponse::new(
                self.handle_acp_session_list(request_id?, params).await,
            )),
            AcpRoute::Load => {
                let session_id =
                    serde_json::from_value::<crate::AcpLoadSessionParams>(params.clone())
                        .ok()
                        .map(|params| params.session_id);
                let response = IncomingResponse::new(
                    self.handle_acp_session_load(connection_id, request_id?, params)
                        .await,
                );
                Some(
                    response
                        .with_acp_session_state_snapshot_after_success(connection_id, session_id),
                )
            }
            AcpRoute::New => {
                let response = IncomingResponse::new(
                    self.handle_acp_session_new(connection_id, request_id?, params)
                        .await,
                );
                let session_id = serde_json::from_value::<
                    crate::AcpSuccessResponse<crate::AcpNewSessionResult>,
                >(response.response.clone())
                .ok()
                .map(|response| response.result.session_id);
                Some(
                    response
                        .with_acp_session_state_snapshot_after_success(connection_id, session_id),
                )
            }
            AcpRoute::Prompt => self
                .handle_acp_session_prompt(connection_id, request_id?, params)
                .await
                .map(IncomingResponse::new),
            AcpRoute::Resume => {
                let session_id =
                    serde_json::from_value::<crate::AcpResumeSessionParams>(params.clone())
                        .ok()
                        .map(|params| params.session_id);
                let response = IncomingResponse::new(
                    self.handle_acp_session_resume(connection_id, request_id?, params)
                        .await,
                );
                Some(
                    response
                        .with_acp_session_state_snapshot_after_success(connection_id, session_id),
                )
            }
            AcpRoute::SetConfigOption => Some(IncomingResponse::new(
                self.handle_acp_session_set_config_option(request_id?, params)
                    .await,
            )),
            AcpRoute::SetMode => Some(IncomingResponse::new(
                self.handle_acp_session_set_mode(request_id?, params).await,
            )),
        }
    }

    pub async fn run_post_response_actions(self: &Arc<Self>, actions: PostResponseActions) {
        for action in actions.0 {
            match action {
                PostResponseAction::SendAcpSessionStateSnapshot {
                    connection_id,
                    session_id,
                } => {
                    self.send_acp_session_state_snapshot(connection_id, session_id)
                        .await;
                }
            }
        }
    }

    pub(super) async fn subscribe_connection_to_session(
        &self,
        connection_id: u64,
        session_id: SessionId,
        event_types: Option<HashSet<String>>,
    ) {
        if let Some(connection) = self.connections.lock().await.get_mut(&connection_id) {
            let desired = event_types.unwrap_or_default();
            let already = connection.subscriptions.iter().any(|subscription| {
                subscription.session_id == Some(session_id) && subscription.event_types == desired
            });
            if already {
                return;
            }
            let include_child_agents = matches!(
                connection.transport,
                ClientTransportKind::Stdio | ClientTransportKind::StdioProxy
            );
            connection.subscriptions.push(SubscriptionFilter {
                session_id: Some(session_id),
                event_types: desired,
                include_child_agents,
            });
        }
    }

    pub(super) async fn connection_ready(&self, connection_id: u64) -> bool {
        self.connections
            .lock()
            .await
            .get(&connection_id)
            .is_some_and(|connection| connection.state == ConnectionState::Ready)
    }

    pub(super) async fn connection_protocol(
        &self,
        connection_id: u64,
    ) -> Option<ConnectionProtocol> {
        self.connections
            .lock()
            .await
            .get(&connection_id)
            .and_then(|connection| connection.protocol)
    }

    pub async fn resolve_client_response(
        self: &Arc<Self>,
        connection_id: u64,
        message: serde_json::Value,
    ) {
        self.resolve_pending_client_response(connection_id, message)
            .await;
    }

    /// Hot path for child/parent assistant token streaming.
    ///
    /// Avoids per-token `child_parent_by_session` registry scans and never waits
    /// on the wait_agent output buffer. Uses `active_turn_connections` to find
    /// the owning stdio connection directly.
    pub(super) async fn broadcast_streaming_agent_message_delta(&self, event: &ServerEvent) {
        let ServerEvent::ItemDelta {
            delta_kind: ItemDeltaKind::AgentMessageDelta,
            payload,
        } = event
        else {
            self.broadcast_event(event.clone()).await;
            return;
        };
        let session_id = payload.context.session_id;
        let Some(connection_id) = self.active_turns.active_connection_id(session_id).await else {
            self.broadcast_event(event.clone()).await;
            return;
        };
        let notification = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            let method = event.method_name();
            if connection.opt_out_notification_methods.contains(method) {
                return;
            }
            if !connection.should_project_event(event) {
                return;
            }
            let event_seq = connection.next_seq();
            let event = event.clone().with_seq(event_seq);
            let (method, value) = connection.notification_for(method, &event);
            Some((
                connection.outbound_tx.clone(),
                OutboundFrame::notification(connection_id, method, event_seq, value),
            ))
        };
        if let Some((outbound_tx, frame)) = notification {
            let _ = enqueue_outbound_notification(
                &outbound_tx,
                frame,
                OutboundDeliveryPolicy::BestEffort,
                "connection_notifications",
            )
            .await;
        }
        self.record_subagent_output_event(event).await;
    }

    /// Deliver a connection-local notification to one client connection.
    ///
    /// Connection-local search notifications (`search/updated`, `search/completed`,
    /// `search/failed`) are not session transcript events. They carry no
    /// `session_id` and must reach the requesting connection even when the only
    /// active subscriptions are session-scoped (`session_id=Some(...)`).
    pub(super) async fn emit_connection_local_to_connection(
        &self,
        connection_id: u64,
        method: &str,
        event: ServerEvent,
    ) {
        debug_assert!(is_connection_local_notification(method));
        let delivery_policy = outbound_delivery_policy(&event);
        let notification = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            if !connection.should_deliver_connection_local(method) {
                return;
            }
            if !connection.should_project_event(&event) {
                return;
            }
            let event_seq = connection.next_seq();
            let event = event.with_seq(event_seq);
            let (method, value) = connection.notification_for(method, &event);
            Some((
                connection.outbound_tx.clone(),
                OutboundFrame::notification(connection_id, method, event_seq, value),
            ))
        };
        if let Some((outbound_tx, frame)) = notification {
            let _ = enqueue_outbound_notification(
                &outbound_tx,
                frame,
                delivery_policy,
                "connection_notifications",
            )
            .await;
        }
    }

    pub(super) async fn emit_to_connection(
        &self,
        connection_id: u64,
        method: &str,
        event: ServerEvent,
    ) {
        if is_connection_local_notification(method) {
            self.emit_connection_local_to_connection(connection_id, method, event)
                .await;
            return;
        }
        let session_id = event.session_id();
        let delivery_policy = outbound_delivery_policy(&event);
        let child_parent_by_session = self.child_parent_by_session().await;
        let notification = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return;
            };
            if !connection.should_deliver(method, session_id, &child_parent_by_session) {
                return;
            }
            if !connection.should_project_event(&event) {
                return;
            }
            let event_seq = connection.next_seq();
            let event = event.with_seq(event_seq);
            let (method, value) = connection.notification_for(method, &event);
            Some((
                connection.outbound_tx.clone(),
                OutboundFrame::notification(connection_id, method, event_seq, value),
            ))
        };
        if let Some((outbound_tx, frame)) = notification {
            let _ = enqueue_outbound_notification(
                &outbound_tx,
                frame,
                delivery_policy,
                "connection_notifications",
            )
            .await;
        }
    }

    pub(super) async fn broadcast_event(&self, event: ServerEvent) {
        if let ServerEvent::TurnCompleted(payload) = &event {
            self.account_goal_turn_completed(&payload.turn).await;
        }
        self.update_session_last_activity_from_event(&event).await;
        // Deliver to the client first. wait_agent's output buffer must not gate
        // token streaming: when supervisor is blocked in wait_agent and children
        // contend on that buffer, TUI tokens would stall while the provider SSE
        // (visible in Burp) keeps flowing into a full event channel.
        let method = event.method_name();
        let session_id = event.session_id();
        let delivery_policy = outbound_delivery_policy(&event);
        let child_parent_by_session = self.child_parent_by_session().await;
        let active_turn_connections = self.active_turns.connection_map().await;
        // New-style SessionsByCwd selectors match on the event session's cwd;
        // resolve it once per event, and only when such a selector exists.
        let event_cwd = if self
            .sessions_by_cwd_subscriptions
            .load(std::sync::atomic::Ordering::Relaxed)
            > 0
            && let Some(session_id) = session_id
        {
            match self.session(session_id).await {
                Some(handle) => handle.summary().await.map(|summary| summary.cwd),
                None => None,
            }
        } else {
            None
        };
        let notifications = {
            let mut connections = self.connections.lock().await;
            connections
                .iter_mut()
                .filter_map(|(connection_id, connection)| {
                    if should_skip_non_owner_stdio_stream(
                        *connection_id,
                        connection,
                        &event,
                        &active_turn_connections,
                    ) {
                        return None;
                    }
                    // Union of the legacy events/subscribe filter and the new
                    // subscription/* selectors (legacy clients unaffected).
                    if !connection.should_deliver(method, session_id, &child_parent_by_session)
                        && !crate::runtime::handlers::subscription::event_matches_selectors(
                            &connection.event_selectors,
                            &event,
                            event_cwd.as_deref(),
                        )
                    {
                        return None;
                    }
                    if !connection.should_project_event(&event) {
                        return None;
                    }
                    let event_seq = connection.next_seq();
                    let event = event.clone().with_seq(event_seq);
                    let (method, value) = connection.notification_for(method, &event);
                    Some((
                        connection.outbound_tx.clone(),
                        OutboundFrame::notification(*connection_id, method, event_seq, value),
                    ))
                })
                .collect::<Vec<_>>()
        };
        for (outbound_tx, frame) in notifications {
            let _ = enqueue_outbound_notification(
                &outbound_tx,
                frame,
                delivery_policy,
                "connection_notifications",
            )
            .await;
        }
        self.record_subagent_output_event(&event).await;
    }

    async fn update_session_last_activity_from_event(&self, event: &ServerEvent) {
        let Some(session_id) = session_activity_event_id(event) else {
            return;
        };
        if let Some(stream) = self.active_stream_state(session_id).await {
            let mut stream = stream.lock().await;
            if let Some(inline) = stream.turn_inline.as_mut() {
                inline.summary.last_activity_at = inline.summary.last_activity_at.max(Utc::now());
                return;
            }
        }
        // Event broadcast runs on turn event streams; never block those tasks on
        // a session actor mailbox that may be awaiting the same stream.
        if let Some(session_handle) = self.session(session_id).await {
            let _ = session_handle.try_touch_last_activity();
        }
    }

    pub(super) async fn send_raw_to_connection(
        &self,
        connection_id: u64,
        value: serde_json::Value,
    ) {
        let (outbound_tx, frame) = {
            let connections = self.connections.lock().await;
            let Some(connection) = connections.get(&connection_id) else {
                return;
            };
            let (delivered_tx, delivered_rx) = oneshot::channel();
            (
                connection.outbound_tx.clone(),
                (
                    OutboundFrame::json_rpc_response_with_delivery(
                        connection_id,
                        value,
                        delivered_tx,
                    ),
                    delivered_rx,
                ),
            )
        };
        let (frame, delivered_rx) = frame;
        if !enqueue_outbound(&outbound_tx, frame, "connection_responses").await {
            return;
        }
        let _ = delivered_rx.await;
    }

    pub(super) async fn send_request_to_connection_cancellable_with_enqueue_signal(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        cancel_token: CancellationToken,
        enqueued_tx: tokio::sync::mpsc::UnboundedSender<()>,
    ) -> Result<serde_json::Value, String> {
        self.send_request_to_connection_inner(
            connection_id,
            method,
            params,
            /*timeout_duration*/ None,
            cancel_token,
            Some(enqueued_tx),
        )
        .await
    }

    pub(super) async fn send_request_to_connection_with_timeout(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        timeout_duration: Duration,
        cancel_token: CancellationToken,
    ) -> Result<serde_json::Value, String> {
        self.send_request_to_connection_inner(
            connection_id,
            method,
            params,
            Some(timeout_duration),
            cancel_token,
            /*enqueued_tx*/ None,
        )
        .await
    }

    async fn send_request_to_connection_inner(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        timeout_duration: Option<Duration>,
        cancel_token: CancellationToken,
        enqueued_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    ) -> Result<serde_json::Value, String> {
        let (request_id, receiver, outbound_tx, frame) = {
            let mut connections = self.connections.lock().await;
            let Some(connection) = connections.get_mut(&connection_id) else {
                return Err("client connection does not exist".to_string());
            };
            let request_id = connection.next_client_request_id;
            connection.next_client_request_id += 1;
            let (tx, rx) = oneshot::channel();
            connection.pending_client_requests.insert(request_id, tx);
            let value = serde_json::to_value(devo_protocol::AcpClientRequest::new(
                serde_json::json!(request_id),
                method,
                params,
            ))
            .map_err(|error| format!("failed to serialize client request: {error}"))?;
            (
                request_id,
                rx,
                connection.outbound_tx.clone(),
                OutboundFrame::client_request(connection_id, method.to_string(), value),
            )
        };
        let mut pending_request = PendingClientRequestGuard::new(
            Arc::clone(&self.connections),
            connection_id,
            request_id,
        );
        if !enqueue_outbound(&outbound_tx, frame, "connection_requests").await {
            pending_request.remove().await;
            return Err("client connection closed before request was sent".to_string());
        }
        if let Some(enqueued_tx) = enqueued_tx {
            let _ = enqueued_tx.send(());
        }
        let message = match timeout_duration {
            Some(timeout_duration) => {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        pending_request.remove().await;
                        return Err("client request cancelled".to_string());
                    }
                    result = tokio::time::timeout(timeout_duration, receiver) => {
                        match result {
                            Ok(Ok(message)) => {
                                pending_request.disarm();
                                message?
                            }
                            Ok(Err(_)) => {
                                pending_request.disarm();
                                return Err("client connection closed before responding".to_string());
                            }
                            Err(_) => {
                                pending_request.remove().await;
                                return Err(format!(
                                    "client request timed out after {}s",
                                    timeout_duration.as_secs()
                                ));
                            }
                        }
                    }
                }
            }
            None => {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        pending_request.remove().await;
                        return Err("client request cancelled".to_string());
                    }
                    result = receiver => {
                        pending_request.disarm();
                        result.map_err(|_| "client connection closed before responding".to_string())??
                    }
                }
            }
        };
        if let Some(error) = message.get("error") {
            return Err(error
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("client returned an error response")
                .to_string());
        }
        Ok(message
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    async fn resolve_pending_client_response(
        &self,
        connection_id: u64,
        message: serde_json::Value,
    ) {
        let Some(request_id) = message.get("id").and_then(serde_json::Value::as_u64) else {
            tracing::warn!(connection_id, "dropping client response with non-u64 id");
            return;
        };
        let pending = {
            let mut connections = self.connections.lock().await;
            connections
                .get_mut(&connection_id)
                .and_then(|connection| connection.pending_client_requests.remove(&request_id))
        };
        if let Some(pending) = pending {
            let _ = pending.send(Ok(message));
        } else {
            tracing::warn!(
                connection_id,
                request_id,
                "dropping response for unknown server-initiated request"
            );
        }
    }

    pub(super) fn error_response(
        &self,
        request_id: serde_json::Value,
        code: ProtocolErrorCode,
        message: impl Into<String>,
    ) -> serde_json::Value {
        let message = message.into();
        tracing::warn!(
            request_id = %request_id,
            code = ?code,
            error_message = %message,
            "returning protocol error"
        );
        serde_json::to_value(ErrorResponse {
            id: request_id,
            error: ProtocolError {
                code,
                message,
                data: serde_json::json!({}),
            },
        })
        .expect("serialize error response")
    }
}

impl ServerRuntime {
    pub(super) async fn controlling_connection_ids(
        &self,
        session_id: SessionId,
        owner_connection_id: Option<u64>,
    ) -> Vec<u64> {
        let native_session_id =
            devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(session_id));
        let connections = self.connections.lock().await;
        let mut connection_ids = connections
            .iter()
            .filter_map(|(connection_id, connection)| {
                let subscribed = connection.event_selectors.iter().any(|selector| {
                    matches!(
                        selector,
                        devo_protocol::native::event::StreamSelector::Session {
                            session_id
                        } if session_id == &native_session_id
                    )
                });
                (subscribed || Some(*connection_id) == owner_connection_id)
                    .then_some(*connection_id)
            })
            .collect::<Vec<_>>();
        connection_ids.sort_unstable();
        connection_ids
    }

    async fn child_parent_by_session(&self) -> HashMap<SessionId, SessionId> {
        self.agent_registries
            .lock()
            .await
            .values()
            .flat_map(|registry| {
                registry
                    .child_to_parent
                    .iter()
                    .map(|(child, parent)| (*child, *parent))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

struct PendingClientRequestGuard {
    connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
    connection_id: u64,
    request_id: u64,
    active: bool,
}

impl PendingClientRequestGuard {
    fn new(
        connections: Arc<Mutex<HashMap<u64, ConnectionRuntime>>>,
        connection_id: u64,
        request_id: u64,
    ) -> Self {
        Self {
            connections,
            connection_id,
            request_id,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }

    async fn remove(&mut self) {
        if !self.active {
            return;
        }
        remove_pending_client_request(&self.connections, self.connection_id, self.request_id).await;
        self.active = false;
    }
}

impl Drop for PendingClientRequestGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let connections = Arc::clone(&self.connections);
        let connection_id = self.connection_id;
        let request_id = self.request_id;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                remove_pending_client_request(&connections, connection_id, request_id).await;
            });
        }
    }
}

async fn remove_pending_client_request(
    connections: &Mutex<HashMap<u64, ConnectionRuntime>>,
    connection_id: u64,
    request_id: u64,
) {
    if let Some(connection) = connections.lock().await.get_mut(&connection_id) {
        connection.pending_client_requests.remove(&request_id);
    }
}

fn outbound_delivery_policy(event: &ServerEvent) -> OutboundDeliveryPolicy {
    match event {
        ServerEvent::ItemDelta { .. }
        | ServerEvent::TurnUsageUpdated(_)
        | ServerEvent::ContextUsageUpdated(_)
        | ServerEvent::WorkspaceChangesUpdated(_)
        | ServerEvent::ReferenceSearchUpdated(_)
        | ServerEvent::CommandExecOutputDelta(_) => OutboundDeliveryPolicy::BestEffort,
        ServerEvent::SessionStarted(_)
        | ServerEvent::SessionTitleUpdated(_)
        | ServerEvent::SessionCompactionStarted(_)
        | ServerEvent::SessionCompactionCompleted(_)
        | ServerEvent::SessionCompactionFailed(_)
        | ServerEvent::SessionStatusChanged(_)
        | ServerEvent::SessionEffectiveContextWindowUpdated(_)
        | ServerEvent::SessionArchived(_)
        | ServerEvent::SessionUnarchived(_)
        | ServerEvent::SessionClosed(_)
        | ServerEvent::SessionDeleted(_)
        | ServerEvent::TurnStarted(_)
        | ServerEvent::TurnCompleted(_)
        | ServerEvent::TurnInterrupted(_)
        | ServerEvent::TurnFailed(_)
        | ServerEvent::TurnPlanUpdated(_)
        | ServerEvent::TurnDiffUpdated(_)
        | ServerEvent::TurnProviderRetryStatus(_)
        | ServerEvent::ToolCallStatusUpdated(_)
        | ServerEvent::RequestUserInput(_)
        | ServerEvent::MessageEditRecorded(_)
        | ServerEvent::TurnSuperseded(_)
        | ServerEvent::WorkspaceRestoreStarted(_)
        | ServerEvent::WorkspaceRestoreCompleted(_)
        | ServerEvent::ItemStarted(_)
        | ServerEvent::ItemCompleted(_)
        | ServerEvent::ServerRequestResolved(_)
        | ServerEvent::ReferenceSearchCompleted(_)
        | ServerEvent::ReferenceSearchFailed(_)
        | ServerEvent::CommandExecExited(_) => OutboundDeliveryPolicy::Reliable,
    }
}

fn session_activity_event_id(event: &ServerEvent) -> Option<SessionId> {
    match event {
        ServerEvent::ToolCallStatusUpdated(payload) => Some(payload.session_id),
        ServerEvent::ItemDelta {
            delta_kind:
                ItemDeltaKind::AgentMessageDelta
                | ItemDeltaKind::ReasoningSummaryTextDelta
                | ItemDeltaKind::ReasoningTextDelta,
            payload,
        } => Some(payload.context.session_id),
        ServerEvent::ItemStarted(payload)
            if matches!(
                payload.item.item_kind,
                ItemKind::ToolCall | ItemKind::CommandExecution
            ) =>
        {
            Some(payload.context.session_id)
        }
        ServerEvent::ItemCompleted(payload)
            if matches!(
                payload.item.item_kind,
                ItemKind::UserMessage
                    | ItemKind::ToolResult
                    | ItemKind::CommandExecution
                    | ItemKind::FileChange
            ) =>
        {
            Some(payload.context.session_id)
        }
        _ => None,
    }
}

fn should_skip_non_owner_stdio_stream(
    connection_id: u64,
    connection: &ConnectionRuntime,
    event: &ServerEvent,
    active_turn_connections: &HashMap<SessionId, u64>,
) -> bool {
    if !matches!(connection.transport, ClientTransportKind::StdioProxy) {
        return false;
    }

    let ServerEvent::ItemDelta {
        delta_kind:
            ItemDeltaKind::AgentMessageDelta
            | ItemDeltaKind::ReasoningSummaryTextDelta
            | ItemDeltaKind::ReasoningTextDelta,
        payload,
    } = event
    else {
        return false;
    };

    if payload.context.turn_id.is_none() {
        return false;
    }

    active_turn_connections
        .get(&payload.context.session_id)
        .is_some_and(|active_connection_id| *active_connection_id != connection_id)
}

pub(crate) struct ConnectionRuntime {
    pub(crate) transport: ClientTransportKind,
    pub(crate) state: ConnectionState,
    /// Negotiated protocol surface; gates ACP route dispatch.
    pub(crate) protocol: Option<ConnectionProtocol>,
    pub(crate) acp_authenticated: bool,
    pub(crate) acp_client_capabilities: crate::AcpClientCapabilities,
    /// Whether the client opted in to native typed `item/*` notifications
    /// via `_meta.devo.typedItems` on ACP initialize (P2).
    pub(crate) typed_items: bool,
    /// Cached union of this connection's new-style (`subscription/*`)
    /// selector sets; rebuilt on every create/update/unsubscribe. Delivery
    /// reads only this cache (the registry is authoritative for ack state).
    pub(crate) event_selectors: Vec<devo_protocol::native::event::StreamSelector>,
    pub(crate) outbound_tx: mpsc::Sender<OutboundFrame>,
    pub(crate) opt_out_notification_methods: HashSet<String>,
    pub(crate) subscriptions: Vec<SubscriptionFilter>,
    next_event_seq: u64,
    next_client_request_id: u64,
    pending_client_requests: HashMap<u64, oneshot::Sender<Result<serde_json::Value, String>>>,
}

/// Returns whether `method` is a connection-local composer notification.
///
/// These events are scoped to the requesting client connection, not to a
/// durable session subscription.
pub(super) fn is_connection_local_notification(method: &str) -> bool {
    matches!(
        method,
        "search/updated" | "search/completed" | "search/failed"
    )
}

impl ConnectionRuntime {
    /// Returns whether this connection should project an internal server event.
    ///
    /// Legacy failure and interruption paths emit a specialized terminal event followed by a
    /// generic `TurnCompleted`. Both specialized events already project to the canonical Native
    /// `turn/completed` shape, so suppress the trailing duplicate only for Native connections.
    /// ACP keeps both legacy events for compatibility.
    pub(super) fn should_project_event(&self, event: &ServerEvent) -> bool {
        !matches!(
            (self.protocol, event),
            (
                Some(ConnectionProtocol::Native),
                ServerEvent::TurnCompleted(payload)
            ) if matches!(
                payload.turn.status,
                TurnStatus::Failed | TurnStatus::Interrupted
            )
        )
    }

    /// Connection-local notifications bypass session subscription filters.
    pub(super) fn should_deliver_connection_local(&self, method: &str) -> bool {
        !self.opt_out_notification_methods.contains(method)
    }

    /// Routes one server event to its wire notification for this connection
    /// (L2-DES-APP-009 DD-4): native-surface connections get the native
    /// typed notification when the event projects, otherwise the raw devo
    /// envelope — no ACP wrap, no `_meta` smuggling. ACP-surface connections
    /// keep the ACP envelope (with `_meta` for devo-only events) unchanged.
    pub(super) fn notification_for(
        &self,
        method: &str,
        event: &ServerEvent,
    ) -> (String, serde_json::Value) {
        match self.protocol {
            Some(ConnectionProtocol::Native) => {
                if let Some(typed) = typed_item_notification_from_server_event(event) {
                    return typed;
                }
                (
                    method.to_string(),
                    serde_json::to_value(event).expect("serialize devo envelope event"),
                )
            }
            Some(ConnectionProtocol::Acp) => acp_notification_from_server_event(method, event),
            None => panic!("uninitialized connection cannot project events"),
        }
    }

    pub(super) fn should_deliver(
        &self,
        method: &str,
        session_id: Option<SessionId>,
        child_parent_by_session: &HashMap<SessionId, SessionId>,
    ) -> bool {
        if self.opt_out_notification_methods.contains(method) {
            return false;
        }
        if self.subscriptions.is_empty() {
            return false;
        }
        self.subscriptions.iter().any(|subscription| {
            let session_matches = subscription.session_matches(session_id, child_parent_by_session);
            let event_matches =
                subscription.event_types.is_empty() || subscription.event_types.contains(method);
            session_matches && event_matches
        })
    }

    pub(super) fn next_seq(&mut self) -> u64 {
        let seq = self.next_event_seq;
        self.next_event_seq += 1;
        seq
    }
}

pub(crate) struct SubscriptionFilter {
    pub(crate) session_id: Option<SessionId>,
    pub(crate) event_types: HashSet<String>,
    pub(crate) include_child_agents: bool,
}

impl SubscriptionFilter {
    fn session_matches(
        &self,
        session_id: Option<SessionId>,
        child_parent_by_session: &HashMap<SessionId, SessionId>,
    ) -> bool {
        let Some(expected) = self.session_id else {
            return true;
        };
        if session_id == Some(expected) {
            return true;
        }
        self.include_child_agents
            && session_id.and_then(|session_id| child_parent_by_session.get(&session_id).copied())
                == Some(expected)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::ItemDeltaPayload;
    use anyhow::Context;
    use anyhow::Result;
    use async_trait::async_trait;
    use devo_core::AppConfigStore;
    use devo_core::BundledSkillsConfig;
    use devo_core::FileSystemSkillCatalog;
    use devo_core::PresetModelCatalog;
    use devo_core::ProviderVendorCatalog;
    use devo_core::SkillsConfig;
    use devo_core::tools::ToolRegistry;
    use devo_protocol::DEVO_ACTIVITY_AT_META;
    use devo_protocol::DEVO_ITEM_ID_META;
    use devo_protocol::DEVO_TURN_ID_META;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::StreamEvent;
    use devo_provider::ModelProviderSDK;
    use devo_provider::SingleProviderRouter;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use super::*;

    struct NoopProvider;

    #[async_trait]
    impl ModelProviderSDK for NoopProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("noop provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            anyhow::bail!("noop provider does not support streaming")
        }

        fn name(&self) -> &str {
            "noop-provider"
        }
    }

    fn build_runtime(data_root: &std::path::Path) -> Arc<ServerRuntime> {
        build_runtime_with_provider(data_root, Arc::new(NoopProvider))
    }

    fn build_runtime_with_provider(
        data_root: &std::path::Path,
        provider: Arc<dyn ModelProviderSDK>,
    ) -> Arc<ServerRuntime> {
        build_runtime_with_provider_and_catalog(
            data_root,
            provider,
            Arc::new(PresetModelCatalog::default()),
        )
    }

    fn build_runtime_with_provider_and_catalog(
        data_root: &std::path::Path,
        provider: Arc<dyn ModelProviderSDK>,
        model_catalog: Arc<PresetModelCatalog>,
    ) -> Arc<ServerRuntime> {
        build_runtime_with_provider_catalog_and_protocols(
            data_root,
            provider,
            model_catalog,
            ProtocolSet::all(),
        )
    }

    fn build_runtime_with_provider_catalog_and_protocols(
        data_root: &std::path::Path,
        provider: Arc<dyn ModelProviderSDK>,
        model_catalog: Arc<PresetModelCatalog>,
        protocols: ProtocolSet,
    ) -> Arc<ServerRuntime> {
        let db = Arc::new(
            crate::db::Database::open(data_root.join("connection.db")).expect("open test database"),
        );
        ServerRuntime::with_protocols(
            data_root.to_path_buf(),
            ServerRuntimeDependencies::new(
                Arc::clone(&provider),
                Arc::new(SingleProviderRouter::new(provider)),
                Arc::new(ToolRegistry::new()),
                crate::empty_mcp_manager(),
                "test-model".to_string(),
                model_catalog,
                Arc::new(ProviderVendorCatalog::default()),
                Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                    bundled: Some(BundledSkillsConfig { enabled: false }),
                    ..SkillsConfig::default()
                })),
                devo_core::AgentsMdConfig::default(),
                db,
                Arc::new(std::sync::Mutex::new(
                    AppConfigStore::load(data_root.to_path_buf(), None)
                        .expect("load app config store"),
                )),
            ),
            protocols,
        )
    }

    #[tokio::test]
    async fn mcp_tools_returns_invalid_params_for_unknown_server() {
        let temp = TempDir::new().expect("temp dir");
        let runtime = build_runtime(temp.path());
        let connection_id = initialized_connection(&runtime).await;
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 3,
                    "method": "mcp/tools",
                    "params": { "name": "missing-server" }
                }),
            )
            .await
            .expect("mcp/tools response");
        let error: ErrorResponse = serde_json::from_value(response).expect("deserialize error");
        assert_eq!(error.error.code, ProtocolErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn mcp_list_includes_bundled_disabled_code_search() {
        let temp = TempDir::new().expect("temp dir");
        let runtime = build_runtime(temp.path());
        let connection_id = initialized_connection(&runtime).await;
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 2,
                    "method": "mcp/list",
                    "params": {}
                }),
            )
            .await
            .expect("mcp/list response");
        let result: SuccessResponse<devo_protocol::native::rpc_admin::McpListResult> =
            serde_json::from_value(response.clone()).expect("deserialize mcp/list");
        let code_search = result
            .result
            .servers
            .iter()
            .find(|server| server.name == "code_search")
            .expect("bundled code_search should be listed");
        assert_eq!(code_search.status, "disabled");
        assert_eq!(code_search.tool_count, 0);
    }

    #[tokio::test]
    async fn mcp_set_enabled_applies_bad_binary_as_failed_status() {
        let temp = TempDir::new().expect("temp dir");
        {
            let mut store = AppConfigStore::load(temp.path().to_path_buf(), None)
                .expect("load app config store");
            store
                .upsert_mcp_server(devo_core::McpServerRecord {
                    id: devo_core::McpServerId("bad_mcp".to_string()),
                    display_name: "Bad MCP".to_string(),
                    transport: devo_core::McpTransportConfig::Stdio {
                        command: vec!["__devo_missing_mcp_binary__".to_string()],
                        cwd: None,
                        env: Default::default(),
                        env_vars: Vec::new(),
                    },
                    startup_policy: devo_core::McpStartupPolicy::Lazy,
                    enabled: false,
                    trust_policy: Default::default(),
                    allowed_capabilities: Vec::new(),
                    roots_policy: Default::default(),
                    output_limits: Default::default(),
                    auth_ref: None,
                })
                .expect("upsert bad mcp server");
        }

        let mcp_manager = Arc::new(devo_mcp::manager::RmcpMcpManager::new(
            {
                let store = AppConfigStore::load(temp.path().to_path_buf(), None)
                    .expect("reload app config store");
                store.effective_config().mcp_runtime.clone()
            },
            Default::default(),
        ));
        let db = Arc::new(
            crate::db::Database::open(temp.path().join("connection.db"))
                .expect("open test database"),
        );
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let runtime = ServerRuntime::new(
            temp.path().to_path_buf(),
            ServerRuntimeDependencies::new(
                Arc::clone(&provider),
                Arc::new(SingleProviderRouter::new(provider)),
                Arc::new(ToolRegistry::new()),
                mcp_manager,
                "test-model".to_string(),
                Arc::new(PresetModelCatalog::default()),
                Arc::new(ProviderVendorCatalog::default()),
                Box::new(FileSystemSkillCatalog::new(SkillsConfig {
                    bundled: Some(BundledSkillsConfig { enabled: false }),
                    ..SkillsConfig::default()
                })),
                devo_core::AgentsMdConfig::default(),
                db,
                Arc::new(std::sync::Mutex::new(
                    AppConfigStore::load(temp.path().to_path_buf(), None)
                        .expect("load app config store"),
                )),
            ),
        );
        let connection_id = initialized_connection(&runtime).await;

        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 4,
                    "method": "mcp/set_enabled",
                    "params": { "name": "bad_mcp", "enabled": true }
                }),
            )
            .await
            .expect("mcp/set_enabled response");
        let result: SuccessResponse<devo_protocol::native::rpc_admin::McpSetEnabledResult> =
            serde_json::from_value(response).expect("deserialize mcp/set_enabled");
        let bad = result
            .result
            .servers
            .iter()
            .find(|server| server.name == "bad_mcp")
            .expect("bad_mcp should be listed");
        assert_eq!(bad.status, "failed");
        assert_eq!(bad.tool_count, 0);

        let list_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 5,
                    "method": "mcp/list",
                    "params": {}
                }),
            )
            .await
            .expect("mcp/list response");
        let list: SuccessResponse<devo_protocol::native::rpc_admin::McpListResult> =
            serde_json::from_value(list_response).expect("deserialize mcp/list");
        let listed = list
            .result
            .servers
            .iter()
            .find(|server| server.name == "bad_mcp")
            .expect("bad_mcp should remain listed");
        assert_eq!(listed.status, "failed");
    }

    #[tokio::test]
    async fn mcp_set_enabled_rejects_unknown_server() {
        let temp = TempDir::new().expect("temp dir");
        let runtime = build_runtime(temp.path());
        let connection_id = initialized_connection(&runtime).await;
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 6,
                    "method": "mcp/set_enabled",
                    "params": { "name": "missing-server", "enabled": true }
                }),
            )
            .await
            .expect("mcp/set_enabled response");
        let error: ErrorResponse = serde_json::from_value(response).expect("deserialize error");
        assert_eq!(error.error.code, ProtocolErrorCode::InternalError);
    }

    fn assert_agent_message_chunk_update(
        update: &serde_json::Value,
        turn_id: TurnId,
        item_id: ItemId,
    ) {
        assert!(update["_meta"][DEVO_ACTIVITY_AT_META].is_string());
        let mut stable_update = update.clone();
        stable_update["_meta"]
            .as_object_mut()
            .expect("ACP update meta")
            .remove(DEVO_ACTIVITY_AT_META);
        assert_eq!(
            stable_update,
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {
                    "type": "text",
                    "text": "hello",
                },
                "messageId": item_id.to_string(),
                "_meta": {
                    DEVO_TURN_ID_META: turn_id.to_string(),
                    DEVO_ITEM_ID_META: item_id.to_string(),
                },
            })
        );
    }

    #[test]
    fn item_lifecycle_is_reliable_while_item_deltas_are_best_effort() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let context = EventContext {
            session_id,
            turn_id: Some(turn_id),
            item_id: Some(item_id),
            seq: 0,
            item_seq: None,
        };
        let events = [
            ServerEvent::ItemCompleted(ItemEventPayload {
                context: context.clone(),
                item: ItemEnvelope {
                    item_id,
                    item_kind: ItemKind::ContextCompaction,
                    payload: serde_json::json!({ "title": "Context compacted" }),
                },
            }),
            ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::AgentMessageDelta,
                payload: ItemDeltaPayload {
                    context,
                    delta: "token".to_string(),
                    stream_index: None,
                    channel: None,
                    chunk_index: None,
                },
            },
        ];

        assert_eq!(
            events.map(|event| outbound_delivery_policy(&event)),
            [
                OutboundDeliveryPolicy::Reliable,
                OutboundDeliveryPolicy::BestEffort,
            ]
        );
    }

    #[test]
    fn subscription_filter_can_match_direct_child_agents() {
        let parent = SessionId::new();
        let child = SessionId::new();
        let unrelated = SessionId::new();
        let child_parent_by_session = HashMap::from([(child, parent)]);
        let subscription = SubscriptionFilter {
            session_id: Some(parent),
            event_types: HashSet::new(),
            include_child_agents: true,
        };

        assert_eq!(
            vec![true, true, false],
            vec![
                subscription.session_matches(Some(parent), &child_parent_by_session),
                subscription.session_matches(Some(child), &child_parent_by_session),
                subscription.session_matches(Some(unrelated), &child_parent_by_session),
            ]
        );
    }

    #[tokio::test]
    async fn post_response_actions_run_after_backpressured_response_enqueue() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (outbound_tx, mut receiver) = super::outbound::test_outbound_channel(1);
        let transport_outbound = outbound_tx.clone();
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound_tx)
            .await;
        let session_id = SessionId::new();
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {},
        });
        let expected_response = response.clone();

        assert!(
            enqueue_outbound(
                &transport_outbound,
                OutboundFrame::json_rpc_response(
                    connection_id,
                    serde_json::json!({ "queued": "backpressure" }),
                ),
                "test_prefill",
            )
            .await
        );
        let runtime_for_task = Arc::clone(&runtime);
        let mut transport_task = tokio::spawn(async move {
            let incoming = IncomingResponse::new(response).with_post_response_action(
                PostResponseAction::SendAcpSessionStateSnapshot {
                    connection_id,
                    session_id,
                },
            );
            let (response, post_response_actions) = incoming.into_parts();
            assert!(
                enqueue_outbound(
                    &transport_outbound,
                    OutboundFrame::json_rpc_response(connection_id, response),
                    "test_response",
                )
                .await
            );
            runtime_for_task
                .run_post_response_actions(post_response_actions)
                .await;
        });

        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut transport_task)
                .await
                .is_err()
        );
        assert_eq!(
            receiver.recv().await.expect("prefilled queue item"),
            serde_json::json!({ "queued": "backpressure" })
        );
        assert_eq!(
            receiver.recv().await.expect("json-rpc response"),
            expected_response
        );
        let notification = receiver.recv().await.expect("post-response notification");
        assert_eq!(
            notification.get("method"),
            Some(&serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD))
        );
        assert_eq!(
            notification
                .get("params")
                .and_then(|params| params.get("sessionId")),
            Some(&serde_json::to_value(session_id).expect("serialize session id"))
        );
        assert_eq!(
            notification
                .get("params")
                .and_then(|params| params.get("update"))
                .and_then(|update| update.get("sessionUpdate")),
            Some(&serde_json::json!("available_commands_update"))
        );
        transport_task.await.expect("transport sequence completes");

        Ok(())
    }

    #[tokio::test]
    async fn stdio_live_agent_deltas_only_deliver_to_active_turn_owner() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let (owner_outbound, mut owner_receiver) = super::outbound::test_outbound_channel(4);
        let owner_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, owner_outbound)
            .await;
        let (observer_outbound, mut observer_receiver) = super::outbound::test_outbound_channel(4);
        let observer_connection_id = runtime
            .register_connection(ClientTransportKind::StdioProxy, observer_outbound)
            .await;
        {
            let mut connections = runtime.connections.lock().await;
            connections
                .get_mut(&owner_connection_id)
                .expect("owner connection")
                .protocol = Some(ConnectionProtocol::Acp);
            connections
                .get_mut(&observer_connection_id)
                .expect("observer connection")
                .protocol = Some(ConnectionProtocol::Acp);
        }

        runtime
            .subscribe_connection_to_session(owner_connection_id, session_id, None)
            .await;
        runtime
            .subscribe_connection_to_session(observer_connection_id, session_id, None)
            .await;
        runtime
            .active_turns
            .set_connection_id(session_id, owner_connection_id)
            .await;

        runtime
            .broadcast_event(ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::AgentMessageDelta,
                payload: ItemDeltaPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: Some(item_id),
                        seq: 0,
                        item_seq: None,
                    },
                    delta: "hello".to_string(),
                    stream_index: None,
                    channel: None,
                    chunk_index: None,
                },
            })
            .await;

        let owner_message = tokio::time::timeout(Duration::from_secs(1), owner_receiver.recv())
            .await?
            .expect("owner receives live agent delta");
        assert_agent_message_chunk_update(&owner_message["params"]["update"], turn_id, item_id);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), observer_receiver.recv())
                .await
                .is_err(),
            "non-owner stdio proxy connection must not receive live agent delta"
        );

        Ok(())
    }

    #[tokio::test]
    async fn stdio_live_agent_deltas_deliver_to_regular_stdio_watchers() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let (owner_outbound, mut owner_receiver) = super::outbound::test_outbound_channel(4);
        let owner_connection_id = runtime
            .register_connection(ClientTransportKind::StdioProxy, owner_outbound)
            .await;
        let (watcher_outbound, mut watcher_receiver) = super::outbound::test_outbound_channel(4);
        let watcher_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, watcher_outbound)
            .await;
        {
            let mut connections = runtime.connections.lock().await;
            connections
                .get_mut(&owner_connection_id)
                .expect("owner connection")
                .protocol = Some(ConnectionProtocol::Acp);
            connections
                .get_mut(&watcher_connection_id)
                .expect("watcher connection")
                .protocol = Some(ConnectionProtocol::Acp);
        }

        runtime
            .subscribe_connection_to_session(owner_connection_id, session_id, None)
            .await;
        runtime
            .subscribe_connection_to_session(watcher_connection_id, session_id, None)
            .await;
        runtime
            .active_turns
            .set_connection_id(session_id, owner_connection_id)
            .await;

        runtime
            .broadcast_event(ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::AgentMessageDelta,
                payload: ItemDeltaPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(turn_id),
                        item_id: Some(item_id),
                        seq: 0,
                        item_seq: None,
                    },
                    delta: "hello".to_string(),
                    stream_index: None,
                    channel: None,
                    chunk_index: None,
                },
            })
            .await;

        let owner_message = tokio::time::timeout(Duration::from_secs(1), owner_receiver.recv())
            .await?
            .expect("owner receives live agent delta");
        let watcher_message = tokio::time::timeout(Duration::from_secs(1), watcher_receiver.recv())
            .await?
            .expect("regular stdio watcher receives live agent delta");
        assert_agent_message_chunk_update(&owner_message["params"]["update"], turn_id, item_id);
        assert_agent_message_chunk_update(&watcher_message["params"]["update"], turn_id, item_id);

        Ok(())
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: connection-local search notifications deliver despite session-only subscriptions.
    #[tokio::test]
    async fn connection_local_search_notifications_ignore_session_subscriptions() -> Result<()> {
        use devo_protocol::ReferenceSearchId;
        use devo_protocol::ReferenceSearchSnapshot;

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let (owner_outbound, mut owner_receiver) = super::outbound::test_outbound_channel(4);
        let owner_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, owner_outbound)
            .await;
        let (other_outbound, mut other_receiver) = super::outbound::test_outbound_channel(4);
        let _other_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, other_outbound)
            .await;
        runtime
            .connections
            .lock()
            .await
            .get_mut(&owner_connection_id)
            .expect("owner connection")
            .protocol = Some(ConnectionProtocol::Native);

        runtime
            .subscribe_connection_to_session(owner_connection_id, session_id, None)
            .await;

        let snapshot = ReferenceSearchSnapshot {
            search_id: ReferenceSearchId::new(),
            query: "src".to_string(),
            results: Vec::new(),
            total_file_match_count: 0,
            scanned_file_count: 0,
            file_search_complete: true,
        };
        runtime
            .emit_connection_local_to_connection(
                owner_connection_id,
                "search/completed",
                ServerEvent::ReferenceSearchCompleted(snapshot),
            )
            .await;

        let owner_message = tokio::time::timeout(Duration::from_secs(1), owner_receiver.recv())
            .await?
            .expect("owner receives connection-local search notification");
        assert_eq!(owner_message["method"], "search/completed");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), other_receiver.recv())
                .await
                .is_err(),
            "unrelated connection must not receive connection-local search notification"
        );

        Ok(())
    }

    /// Trace: L2-DES-CLIENT-002
    /// Verifies: session-scoped notifications still require matching subscriptions.
    #[test]
    fn session_scoped_notifications_require_matching_subscription() {
        let subscribed_session = SessionId::new();
        let subscription = SubscriptionFilter {
            session_id: Some(subscribed_session),
            event_types: HashSet::new(),
            include_child_agents: false,
        };
        let child_parent_by_session = HashMap::new();

        assert!(!subscription.session_matches(None, &child_parent_by_session));
        assert!(subscription.session_matches(Some(subscribed_session), &child_parent_by_session));
    }

    /// Verifies (P2): an opted-in connection receives native typed
    /// `item/completed` while a legacy connection on the same session keeps
    /// the ACP-wrapped shape for the same event.
    #[tokio::test]
    async fn native_and_acp_connections_receive_one_projected_event() -> Result<()> {
        use devo_protocol::TypedItemEventPayload;
        use devo_protocol::native::item::{Item, ItemState};

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let (typed_outbound, mut typed_receiver) = super::outbound::test_outbound_channel(1);
        let (legacy_outbound, mut legacy_receiver) = super::outbound::test_outbound_channel(1);
        let typed_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, typed_outbound)
            .await;
        let legacy_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, legacy_outbound)
            .await;
        runtime
            .handle_acp_initialize(
                typed_connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        runtime
            .handle_acp_initialize(
                legacy_connection_id,
                Some(serde_json::json!(2)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;
        runtime
            .subscribe_connection_to_session(typed_connection_id, session_id, None)
            .await;
        runtime
            .subscribe_connection_to_session(legacy_connection_id, session_id, None)
            .await;
        runtime
            .connections
            .lock()
            .await
            .get_mut(&typed_connection_id)
            .expect("Native connection")
            .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
            session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(
                session_id,
            )),
        }];

        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        runtime
            .broadcast_event(ServerEvent::ItemCompleted(ItemEventPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(item_id),
                    seq: 0,
                    item_seq: Some(3),
                },
                item: ItemEnvelope {
                    item_id,
                    item_kind: ItemKind::AgentMessage,
                    payload: serde_json::json!({ "title": "Assistant", "text": "hello" }),
                },
            }))
            .await;

        let typed = tokio::time::timeout(Duration::from_secs(1), typed_receiver.recv())
            .await?
            .expect("typed connection receives notification");
        assert_eq!(typed["method"], serde_json::json!("item/completed"));
        let payload: TypedItemEventPayload =
            serde_json::from_value(typed["params"].clone()).expect("typed item payload");
        assert_eq!(payload.item.id.as_str(), item_id.to_string());
        assert_eq!(payload.item.session_id.as_str(), session_id.to_string());
        assert_eq!(payload.item.turn_id.as_str(), turn_id.to_string());
        assert_eq!((payload.item.seq, payload.item.revision), (3, 1));
        assert_eq!(payload.item.state, ItemState::Completed);
        assert_eq!(
            payload.item.item,
            Item::AssistantMessage {
                text: "hello".into(),
                phase: None,
            }
        );

        let legacy = tokio::time::timeout(Duration::from_secs(1), legacy_receiver.recv())
            .await?
            .expect("legacy connection receives notification");
        assert_eq!(
            legacy["method"],
            serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), typed_receiver.recv())
                .await
                .is_err(),
            "overlapping subscription selectors must not duplicate Native delivery"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), legacy_receiver.recv())
                .await
                .is_err(),
            "ACP delivery must occur once"
        );

        Ok(())
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: Native collapses legacy abnormal terminal pairs into one canonical completion,
    /// while ACP retains both compatibility notifications.
    #[tokio::test]
    async fn native_collapses_abnormal_terminal_pairs_while_acp_keeps_both() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let (native_outbound, mut native_receiver) = super::outbound::test_outbound_channel(16);
        let (acp_outbound, mut acp_receiver) = super::outbound::test_outbound_channel(16);
        let native_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, native_outbound)
            .await;
        let acp_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, acp_outbound)
            .await;
        runtime
            .handle_acp_initialize(
                native_connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        runtime
            .handle_acp_initialize(
                acp_connection_id,
                Some(serde_json::json!(2)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;
        runtime
            .subscribe_connection_to_session(native_connection_id, session_id, None)
            .await;
        runtime
            .subscribe_connection_to_session(acp_connection_id, session_id, None)
            .await;

        let turn_metadata = |sequence, status| crate::turn::TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence,
            status,
            kind: devo_core::TurnKind::Regular,
            model: "test-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".to_string(),
            request_thinking: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        let failed_turn = turn_metadata(1, TurnStatus::Failed);
        let interrupted_turn = turn_metadata(2, TurnStatus::Interrupted);
        let cases = vec![
            (
                failed_turn.clone(),
                ServerEvent::TurnFailed(devo_protocol::TurnFailedPayload {
                    session_id,
                    turn: failed_turn,
                    error: Some(devo_protocol::TurnErrorPayload {
                        code: "PROVIDER_SERVER_ERROR".to_string(),
                        message: "provider failed".to_string(),
                        recovery_hint: Some("retry later".to_string()),
                    }),
                }),
                "turn/failed",
                "failed",
                Some("provider failed"),
            ),
            (
                interrupted_turn.clone(),
                ServerEvent::TurnInterrupted(TurnEventPayload {
                    session_id,
                    turn: interrupted_turn,
                }),
                "turn/interrupted",
                "interrupted",
                None,
            ),
        ];

        for (turn, specialized_event, specialized_method, expected_status, expected_error) in cases
        {
            runtime.broadcast_event(specialized_event).await;
            runtime
                .broadcast_event(ServerEvent::TurnCompleted(TurnEventPayload {
                    session_id,
                    turn: turn.clone(),
                }))
                .await;

            let native = tokio::time::timeout(Duration::from_secs(1), native_receiver.recv())
                .await?
                .expect("Native connection receives terminal notification");
            assert_eq!(native["method"], serde_json::json!("turn/completed"));
            assert_eq!(
                native["params"]["turn"]["id"],
                serde_json::json!(turn.turn_id.to_string())
            );
            assert_eq!(
                native["params"]["turn"]["status"],
                serde_json::json!(expected_status)
            );
            assert_eq!(
                native["params"]["turn"]["error"]["message"].as_str(),
                expected_error
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(50), native_receiver.recv())
                    .await
                    .is_err(),
                "Native must receive one terminal notification per turn"
            );

            let mut acp_methods = Vec::new();
            for _ in 0..2 {
                let notification =
                    tokio::time::timeout(Duration::from_secs(1), acp_receiver.recv())
                        .await?
                        .expect("ACP connection receives compatibility notification");
                assert_eq!(
                    notification["method"],
                    serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
                );
                let notification = serde_json::from_value::<devo_protocol::AcpSessionNotification>(
                    notification["params"].clone(),
                )
                .expect("decode ACP session notification");
                let (method, _event) =
                    devo_protocol::original_event_from_acp_notification(&notification)
                        .expect("ACP terminal notification preserves original event");
                acp_methods.push(method);
            }
            assert_eq!(
                acp_methods,
                vec![specialized_method.to_string(), "turn/completed".to_string()]
            );
        }

        let connections = runtime.connections.lock().await;
        assert_eq!(
            (
                connections
                    .get(&native_connection_id)
                    .expect("Native connection")
                    .next_event_seq,
                connections
                    .get(&acp_connection_id)
                    .expect("ACP connection")
                    .next_event_seq,
            ),
            (3, 5),
            "suppressed Native duplicates must not consume event sequence numbers"
        );

        Ok(())
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: a Native connection receives typed assistant deltas carrying
    /// the emit-site `chunk_index`, while an ACP connection receives the ACP
    /// v1 projection of the same application event.
    #[tokio::test]
    async fn native_connection_receives_assistant_delta_with_chunk_index() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let (native_outbound, mut native_receiver) = super::outbound::test_outbound_channel(1);
        let (acp_outbound, mut acp_receiver) = super::outbound::test_outbound_channel(1);
        let native_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, native_outbound)
            .await;
        let acp_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, acp_outbound)
            .await;
        runtime
            .subscribe_connection_to_session(native_connection_id, session_id, None)
            .await;
        runtime
            .subscribe_connection_to_session(acp_connection_id, session_id, None)
            .await;
        {
            let mut connections = runtime.connections.lock().await;
            let native = connections
                .get_mut(&native_connection_id)
                .expect("Native connection");
            native.protocol = Some(ConnectionProtocol::Native);
            native.typed_items = true;
            connections
                .get_mut(&acp_connection_id)
                .expect("ACP connection")
                .protocol = Some(ConnectionProtocol::Acp);
        }

        runtime
            .broadcast_event(ServerEvent::ItemDelta {
                delta_kind: ItemDeltaKind::AgentMessageDelta,
                payload: ItemDeltaPayload {
                    context: EventContext {
                        session_id,
                        turn_id: Some(TurnId::new()),
                        item_id: Some(ItemId::new()),
                        seq: 0,
                        item_seq: None,
                    },
                    delta: "hello".to_string(),
                    stream_index: None,
                    channel: None,
                    chunk_index: Some(4),
                },
            })
            .await;

        let native = tokio::time::timeout(Duration::from_secs(1), native_receiver.recv())
            .await?
            .expect("Native connection receives delta");
        assert_eq!(
            native["method"],
            serde_json::json!("item/assistantMessage/delta")
        );
        let delta: devo_protocol::native::event::ItemDelta =
            serde_json::from_value(native["params"].clone()).expect("typed delta payload");
        assert_eq!(delta.chunk_index, 4);
        assert_eq!(delta.base_revision, 1);
        assert_eq!(delta.delta, "hello");

        let acp = tokio::time::timeout(Duration::from_secs(1), acp_receiver.recv())
            .await?
            .expect("ACP connection receives delta");
        assert_eq!(
            acp["method"],
            serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
        );

        Ok(())
    }

    /// Verifies: ACP projection remains selected even if an old client sends
    /// the historical typed-items opt-in.
    #[tokio::test]
    async fn acp_projection_ignores_typed_items_opt_in() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();
        let (outbound, mut receiver) = super::outbound::test_outbound_channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .subscribe_connection_to_session(connection_id, session_id, None)
            .await;
        {
            let mut connections = runtime.connections.lock().await;
            let connection = connections.get_mut(&connection_id).expect("connection");
            connection.protocol = Some(ConnectionProtocol::Acp);
            connection.typed_items = true;
        }

        runtime
            .broadcast_event(ServerEvent::ItemStarted(ItemEventPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(TurnId::new()),
                    item_id: Some(ItemId::new()),
                    seq: 0,
                    item_seq: Some(1),
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::ToolCall,
                    // Missing tool_call_id/tool_name: cannot project.
                    payload: serde_json::json!({ "bogus": true }),
                },
            }))
            .await;

        let notification = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await?
            .expect("connection receives notification");
        assert_eq!(
            notification["method"],
            serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
        );

        Ok(())
    }

    /// Verifies (P2): `_meta.devo.typedItems` on initialize is stored on the
    /// connection and echoed in the initialize result; absence defaults off.
    #[tokio::test]
    async fn initialize_typed_items_opt_in_is_stored_and_echoed() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (typed_outbound, _typed_receiver) = super::outbound::test_outbound_channel(1);
        let (plain_outbound, _plain_receiver) = super::outbound::test_outbound_channel(1);
        let typed_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, typed_outbound)
            .await;
        let plain_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, plain_outbound)
            .await;

        let response = runtime
            .handle_acp_initialize(
                typed_connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "typedItems": true } },
                }),
            )
            .await;
        assert_eq!(
            response["result"]["_meta"]["devo"]["typedItems"],
            serde_json::json!(true)
        );

        let response = runtime
            .handle_acp_initialize(
                plain_connection_id,
                Some(serde_json::json!(2)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;
        assert!(response["result"]["_meta"].get("devo").is_none());

        let connections = runtime.connections.lock().await;
        assert!(
            connections
                .get(&typed_connection_id)
                .expect("typed connection")
                .typed_items
        );
        assert!(
            !connections
                .get(&plain_connection_id)
                .expect("plain connection")
                .typed_items
        );

        Ok(())
    }

    // ── Paged history reads (P4a: session/turns/list, session/items/list) ──

    /// Writes a session rollout (3 turns, 5 agent-message items across two
    /// turns) through the real v2 write path and returns its session id.
    async fn write_history_rollout(data_root: &std::path::Path) -> SessionId {
        use devo_core::{TextItem, TurnItem};

        let rollout_store = crate::persistence::RolloutStore::new(data_root.to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            data_root.to_path_buf(),
            Vec::new(),
            Some("history session".into()),
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        let mut item_seq = 1u64;
        for turn_index in 1..=3u32 {
            let metadata = crate::turn::TurnMetadata {
                turn_id: TurnId::new(),
                session_id: record.id,
                sequence: turn_index,
                status: TurnStatus::Completed,
                kind: devo_core::TurnKind::Regular,
                model: "test-model".into(),
                model_binding_id: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                request_model: "test-model".into(),
                request_thinking: None,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                usage: None,
                stop_reason: None,
                failure_reason: None,
            };
            let turn = crate::persistence::build_turn_record(&metadata, None, None, None, None);
            rollout_store
                .append_turn(&record, turn)
                .expect("append turn");
            // Turns 1 and 2 get two items each, turn 3 gets one.
            for text in match turn_index {
                1 | 2 => vec!["first", "second"],
                _ => vec!["third"],
            } {
                let item = crate::persistence::build_item_record(
                    record.id,
                    metadata.turn_id,
                    ItemId::new(),
                    item_seq,
                    TurnItem::AgentMessage(TextItem {
                        text: format!("{text}-t{turn_index}"),
                    }),
                    Some(TurnStatus::Running),
                    None,
                );
                rollout_store
                    .append_item(&record, item)
                    .expect("append item");
                item_seq += 1;
            }
        }
        record.id
    }

    async fn initialized_connection(runtime: &Arc<ServerRuntime>) -> u64 {
        initialized_native_connection(runtime).await
    }

    async fn initialized_acp_connection(runtime: &Arc<ServerRuntime>) -> u64 {
        let (outbound, _rx) = super::outbound::test_outbound_channel(16);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;
        connection_id
    }

    /// Initializes a connection that declares the native protocol surface
    /// (`_meta.devo.protocol = "native"`), so colliding method names such
    /// as `session/new` route to the native handlers.
    async fn initialized_native_connection(runtime: &Arc<ServerRuntime>) -> u64 {
        let (outbound, _rx) = super::outbound::test_outbound_channel(16);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        connection_id
    }

    #[tokio::test]
    async fn initialize_respects_protocol_exposure_matrix() {
        let native_temp = TempDir::new().expect("native temp");
        let native_runtime = build_runtime_with_provider_catalog_and_protocols(
            native_temp.path(),
            Arc::new(NoopProvider),
            Arc::new(PresetModelCatalog::default()),
            ProtocolSet::only(ServerProtocol::Native),
        );
        let (outbound, _rx) = super::outbound::test_outbound_channel(4);
        let acp_id = native_runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        let rejected = native_runtime
            .handle_acp_initialize(
                acp_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;
        assert_eq!(rejected["error"]["code"], serde_json::json!(-32600));
        assert_eq!(native_runtime.connection_protocol(acp_id).await, None);
        assert!(!native_runtime.connection_ready(acp_id).await);

        let native_id = initialized_native_connection(&native_runtime).await;
        assert_eq!(
            native_runtime.connection_protocol(native_id).await,
            Some(ConnectionProtocol::Native)
        );

        let acp_temp = TempDir::new().expect("ACP temp");
        let acp_runtime = build_runtime_with_provider_catalog_and_protocols(
            acp_temp.path(),
            Arc::new(NoopProvider),
            Arc::new(PresetModelCatalog::default()),
            ProtocolSet::only(ServerProtocol::Acp),
        );
        let acp_id = initialized_acp_connection(&acp_runtime).await;
        assert_eq!(
            acp_runtime.connection_protocol(acp_id).await,
            Some(ConnectionProtocol::Acp)
        );
        let (outbound, _rx) = super::outbound::test_outbound_channel(4);
        let native_id = acp_runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        let rejected = acp_runtime
            .handle_acp_initialize(
                native_id,
                Some(serde_json::json!(2)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        assert_eq!(rejected["error"]["code"], serde_json::json!(-32600));
        assert_eq!(acp_runtime.connection_protocol(native_id).await, None);
    }

    #[tokio::test]
    async fn rejected_initialize_can_retry_after_protocol_extension() {
        let temp = TempDir::new().expect("temp");
        let runtime = build_runtime_with_provider_catalog_and_protocols(
            temp.path(),
            Arc::new(NoopProvider),
            Arc::new(PresetModelCatalog::default()),
            ProtocolSet::only(ServerProtocol::Native),
        );
        let (outbound, _rx) = super::outbound::test_outbound_channel(4);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        let params = serde_json::json!({
            "protocolVersion": 1,
            "clientCapabilities": { "terminal": false },
        });

        let rejected = runtime
            .handle_acp_initialize(connection_id, Some(serde_json::json!(1)), params.clone())
            .await;
        assert_eq!(rejected["error"]["code"], serde_json::json!(-32600));

        assert_eq!(
            runtime
                .enable_protocols(&ProtocolSet::only(ServerProtocol::Acp))
                .await,
            ProtocolSet::all()
        );
        let accepted = runtime
            .handle_acp_initialize(connection_id, Some(serde_json::json!(2)), params)
            .await;
        assert!(
            accepted.get("result").is_some(),
            "initialize failed: {accepted}"
        );
        assert_eq!(
            runtime.connection_protocol(connection_id).await,
            Some(ConnectionProtocol::Acp)
        );
    }

    #[tokio::test]
    async fn initialized_connection_cannot_switch_adapters() {
        let temp = TempDir::new().expect("temp");
        let runtime = build_runtime(temp.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let response = runtime
            .handle_acp_initialize(
                connection_id,
                Some(serde_json::json!(2)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;

        assert_eq!(response["error"]["code"], serde_json::json!(-32600));
        assert_eq!(
            runtime.connection_protocol(connection_id).await,
            Some(ConnectionProtocol::Native)
        );
    }

    #[tokio::test]
    async fn acp_connection_cannot_fall_through_to_native_routes() {
        let temp = TempDir::new().expect("temp");
        let runtime = build_runtime(temp.path());
        let connection_id = initialized_acp_connection(&runtime).await;

        let response = history_request(
            &runtime,
            connection_id,
            9,
            "runtime/ping",
            serde_json::json!({}),
        )
        .await;

        assert_eq!(response["error"]["code"], serde_json::json!(-32601));
    }

    #[tokio::test]
    async fn controlling_connections_union_owner_and_session_subscribers() {
        let temp = TempDir::new().expect("temp dir");
        let runtime = build_runtime(temp.path());
        let owner_id = initialized_connection(&runtime).await;
        let subscriber_id = initialized_connection(&runtime).await;
        let unrelated_id = initialized_connection(&runtime).await;
        let session_id = SessionId::new();
        let native_session_id =
            devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(session_id));
        {
            let mut connections = runtime.connections.lock().await;
            connections
                .get_mut(&subscriber_id)
                .expect("subscriber connection")
                .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
                session_id: native_session_id,
            }];
        }

        assert_eq!(
            runtime
                .controlling_connection_ids(session_id, Some(owner_id))
                .await,
            vec![owner_id, subscriber_id]
        );
        assert!(
            !runtime
                .controlling_connection_ids(session_id, Some(owner_id))
                .await
                .contains(&unrelated_id)
        );
    }

    async fn history_request(
        runtime: &Arc<ServerRuntime>,
        connection_id: u64,
        id: u64,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({ "id": id, "method": method, "params": params }),
            )
            .await
            .expect("history response")
    }

    #[tokio::test]
    async fn turns_and_items_list_paginate_without_gaps_or_duplicates() -> Result<()> {
        use devo_protocol::native::page::Page;
        use devo_protocol::native::turn::Turn;

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_history_rollout(data_root.path()).await;
        let connection_id = initialized_connection(&runtime).await;

        let first = history_request(
            &runtime,
            connection_id,
            1,
            "session/turns/list",
            serde_json::json!({ "sessionId": session_id.to_string(), "limit": 2 }),
        )
        .await;
        let first: Page<Turn> =
            serde_json::from_value(first["result"].clone()).expect("page 1 result");
        assert_eq!(first.data.len(), 2);
        assert_eq!(
            first
                .data
                .iter()
                .map(|turn| turn.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("2"));
        let first_turn = &first.data[0];
        assert_eq!(first_turn.session_id.as_str(), session_id.to_string());
        assert_eq!(
            first_turn.status,
            devo_protocol::native::turn::TurnStatus::Completed
        );
        assert_eq!(
            first_turn.kind,
            devo_protocol::native::turn::TurnKind::Regular
        );

        let second = history_request(
            &runtime,
            connection_id,
            2,
            "session/turns/list",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "limit": 2,
                "cursor": first.next_cursor.expect("cursor"),
            }),
        )
        .await;
        let second: Page<Turn> =
            serde_json::from_value(second["result"].clone()).expect("page 2 result");
        assert_eq!(second.data.len(), 1);
        assert_eq!(second.data[0].sequence, 3);
        assert_eq!(second.next_cursor, None);

        let first_items = history_request(
            &runtime,
            connection_id,
            3,
            "session/items/list",
            serde_json::json!({ "sessionId": session_id.to_string(), "limit": 3 }),
        )
        .await;
        let first_items: Page<devo_protocol::native::item::ItemEnvelope> =
            serde_json::from_value(first_items["result"].clone()).expect("items page 1");
        assert_eq!(first_items.data.len(), 3);
        assert_eq!(
            first_items
                .data
                .iter()
                .map(|item| item.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(first_items.next_cursor.as_deref(), Some("3"));
        let envelope = &first_items.data[0];
        assert_eq!(envelope.session_id.as_str(), session_id.to_string());
        assert_eq!(envelope.revision, 1);
        assert_eq!(
            envelope.state,
            devo_protocol::native::item::ItemState::Completed
        );
        assert!(
            matches!(&envelope.item, devo_protocol::native::item::Item::AssistantMessage { text, .. } if text == "first-t1")
        );

        let second_items = history_request(
            &runtime,
            connection_id,
            4,
            "session/items/list",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "limit": 3,
                "cursor": first_items.next_cursor.expect("cursor"),
            }),
        )
        .await;
        let second_items: Page<devo_protocol::native::item::ItemEnvelope> =
            serde_json::from_value(second_items["result"].clone()).expect("items page 2");
        assert_eq!(
            second_items
                .data
                .iter()
                .map(|item| item.seq)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
        assert_eq!(second_items.next_cursor, None);

        Ok(())
    }

    #[tokio::test]
    async fn items_list_filters_by_turn_id() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_history_rollout(data_root.path()).await;
        let connection_id = initialized_connection(&runtime).await;

        let turns = history_request(
            &runtime,
            connection_id,
            1,
            "session/turns/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let turns: devo_protocol::native::page::Page<devo_protocol::native::turn::Turn> =
            serde_json::from_value(turns["result"].clone()).expect("turns result");
        let turn_two = &turns.data[1];

        let items = history_request(
            &runtime,
            connection_id,
            2,
            "session/items/list",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "turnId": turn_two.id.as_str(),
            }),
        )
        .await;
        let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
            serde_json::from_value(items["result"].clone()).expect("items result");
        assert_eq!(items.data.len(), 2);
        assert!(items.data.iter().all(|item| item.turn_id == turn_two.id));
        assert!(
            matches!(&items.data[0].item, devo_protocol::native::item::Item::AssistantMessage { text, .. } if text == "first-t2")
        );

        Ok(())
    }

    #[tokio::test]
    async fn items_list_reads_cold_session_without_resume() -> Result<()> {
        let data_root = TempDir::new()?;
        let session_id = write_history_rollout(data_root.path()).await;
        // A brand-new runtime that never loaded the session: the read must
        // resolve the rollout from disk, not from the session map or index.
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;

        let items = history_request(
            &runtime,
            connection_id,
            1,
            "session/items/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
            serde_json::from_value(items["result"].clone()).expect("items result");
        assert_eq!(items.data.len(), 5);
        assert_eq!(items.next_cursor, None);

        Ok(())
    }

    #[tokio::test]
    async fn history_lists_handle_empty_session_bad_cursor_and_unknown_session() -> Result<()> {
        use devo_protocol::native::page::Page;
        use devo_protocol::native::turn::Turn;

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        // A session with only its meta line (no turns, no items).
        let rollout_store =
            crate::persistence::RolloutStore::new(data_root.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            data_root.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        let connection_id = initialized_connection(&runtime).await;

        let empty = history_request(
            &runtime,
            connection_id,
            1,
            "session/turns/list",
            serde_json::json!({ "sessionId": record.id.to_string() }),
        )
        .await;
        let empty: Page<Turn> = serde_json::from_value(empty["result"].clone()).expect("empty");
        assert_eq!(
            empty,
            Page {
                data: Vec::new(),
                next_cursor: None,
            }
        );

        let bad_cursor = history_request(
            &runtime,
            connection_id,
            2,
            "session/items/list",
            serde_json::json!({ "sessionId": record.id.to_string(), "cursor": "not-a-cursor" }),
        )
        .await;
        assert!(
            bad_cursor.get("error").is_some(),
            "malformed cursor must error: {bad_cursor}"
        );

        let unknown = history_request(
            &runtime,
            connection_id,
            3,
            "session/turns/list",
            serde_json::json!({ "sessionId": SessionId::new().to_string() }),
        )
        .await;
        assert!(unknown.get("error").is_some(), "unknown session must error");

        Ok(())
    }

    // ── Durable event subscriptions (P4b: subscription/*) ─────────────

    /// Writes a session rollout (meta + one completed turn + two agent
    /// items) through the runtime's own store, so the outbox projects the
    /// derived events into `event_log`. Expected stream rows on
    /// `session:<id>`: session/created seq 1, turn/completed seq 2,
    /// item/completed seq 3..4.
    async fn write_subscribed_rollout(runtime: &Arc<ServerRuntime>) -> SessionId {
        use devo_core::{TextItem, TurnItem};

        let store = &runtime.rollout_store;
        let record = store.create_session_record(
            SessionId::new(),
            Utc::now(),
            std::path::PathBuf::from("/tmp/subscription-test"),
            Vec::new(),
            Some("subscribed session".into()),
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        store.append_session_meta(&record).expect("append meta");
        let metadata = crate::turn::TurnMetadata {
            turn_id: TurnId::new(),
            session_id: record.id,
            sequence: 1,
            status: TurnStatus::Completed,
            kind: devo_core::TurnKind::Regular,
            model: "test-model".into(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".into(),
            request_thinking: None,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        let turn = crate::persistence::build_turn_record(&metadata, None, None, None, None);
        store.append_turn(&record, turn).expect("append turn");
        for (seq, text) in [(1u64, "one"), (2, "two")] {
            let item = crate::persistence::build_item_record(
                record.id,
                metadata.turn_id,
                ItemId::new(),
                seq,
                TurnItem::AgentMessage(TextItem { text: text.into() }),
                Some(TurnStatus::Running),
                None,
            );
            store.append_item(&record, item).expect("append item");
        }
        record.id
    }

    #[tokio::test]
    async fn subscription_create_returns_barrier_consistent_replay_and_snapshot() -> Result<()> {
        use devo_protocol::native::event::{SnapshotData, SubscriptionCreateResult};

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let stream_id = devo_core::session_stream_id(
            &devo_protocol::native::ids::SessionId::from_string(session_id.to_string()),
        );
        let connection_id = initialized_connection(&runtime).await;

        let response = history_request(
            &runtime,
            connection_id,
            1,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": true,
            }),
        )
        .await;
        let result: SubscriptionCreateResult =
            serde_json::from_value(response["result"].clone()).expect("create result");

        // Barrier = 4 (created/completed/completed/completed); cursors are
        // barrier-consistent and replay rows carry hydrated log seqs.
        assert_eq!(result.cursors.len(), 1);
        assert_eq!(result.cursors[0].stream_id, stream_id);
        assert_eq!(result.cursors[0].seq, 4);
        assert_eq!(result.replay.len(), 4);
        assert_eq!(
            result
                .replay
                .iter()
                .map(|event| event.meta.seq.expect("hydrated seq"))
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(matches!(
            &result.replay[0].notification,
            devo_protocol::native::event::ServerNotification::SessionCreated { .. }
        ));
        assert!(result.replay.iter().all(|event| event.meta.persisted));

        // Snapshot: the session from the rollout history, no active turn.
        assert_eq!(result.snapshots.len(), 1);
        let SnapshotData::Session {
            session,
            active_turn,
            queue,
        } = &result.snapshots[0].data
        else {
            panic!("expected session snapshot");
        };
        assert_eq!(session.id.as_str(), session_id.to_string());
        assert_eq!(active_turn, &None);
        assert!(queue.is_empty());
        assert_eq!(result.snapshots[0].barrier_seq, 4);
        assert!(result.pending_control_requests.is_empty());
        assert!(result.recovery_snapshots.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn subscription_snapshot_reads_in_memory_queue_with_1_based_positions() -> Result<()> {
        use devo_protocol::native::event::{SnapshotData, SubscriptionCreateResult};
        use devo_protocol::native::rpc_turn::SessionQueuePushResult;

        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Default::default(),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        // Idle push starts a turn the gate holds open; the next two pushes
        // land in the in-memory pending turn queue.
        let started = history_request(
            &runtime,
            connection_id,
            2,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "first" }],
                "idempotencyKey": "push-1",
            }),
        )
        .await;
        let started: SessionQueuePushResult =
            serde_json::from_value(started["result"].clone()).expect("push result");
        assert!(
            matches!(started, SessionQueuePushResult::Started { .. }),
            "idle push must start a turn"
        );
        for (request, key, text) in [(3, "push-2", "second"), (4, "push-3", "third")] {
            let pushed = history_request(
                &runtime,
                connection_id,
                request,
                "session/queue/push",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "input": [{ "type": "text", "text": text }],
                    "idempotencyKey": key,
                }),
            )
            .await;
            let pushed: SessionQueuePushResult =
                serde_json::from_value(pushed["result"].clone()).expect("push result");
            assert!(
                matches!(pushed, SessionQueuePushResult::Queued { .. }),
                "busy push must queue"
            );
        }

        // The snapshot must mirror the in-memory queue (same source as
        // session/queue/list), with 1-based positions.
        let response = history_request(
            &runtime,
            connection_id,
            5,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": true,
            }),
        )
        .await;
        let result: SubscriptionCreateResult =
            serde_json::from_value(response["result"].clone()).expect("create result");
        assert_eq!(result.snapshots.len(), 1);
        let SnapshotData::Session { queue, .. } = &result.snapshots[0].data else {
            panic!("expected session snapshot");
        };
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].position, 1);
        assert_eq!(queue[0].preview, "second");
        assert_eq!(queue[1].position, 2);
        assert_eq!(queue[1].preview, "third");

        // Let the held turn settle so teardown does not race the provider.
        open.store(true, std::sync::atomic::Ordering::SeqCst);

        Ok(())
    }

    #[tokio::test]
    async fn subscription_snapshot_falls_back_to_persisted_queue_for_unloaded_session() -> Result<()>
    {
        use devo_protocol::native::event::{SnapshotData, SubscriptionCreateResult};

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        // Rollout-only session the runtime has never resumed (no actor), so
        // the snapshot must read its queue from SQLite.
        let session_id = write_subscribed_rollout(&runtime).await;
        // pending_items has a FK to sessions; a durable session with queued
        // input always has a sessions row in production, so seed one here.
        let now = Utc::now();
        runtime.deps.db.upsert_session(
            &devo_protocol::SessionMetadata {
                session_id,
                cwd: std::path::PathBuf::from("/tmp/subscription-test"),
                additional_directories: Vec::new(),
                created_at: now,
                updated_at: now,
                last_activity_at: now,
                title: Some("subscribed session".into()),
                title_state: devo_protocol::SessionTitleState::Provisional,
                parent_session_id: None,
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
                ephemeral: false,
                model: Some("test-model".into()),
                model_binding_id: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_tokens: 0,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
                prompt_token_estimate: 0,
                last_query_usage: None,
                last_query_total_tokens: 0,
                last_context_occupancy: None,
                status: devo_protocol::SessionRuntimeStatus::Unloaded,
                collaboration_mode: Default::default(),
                effective_context_window: None,
                permission_preset: None,
            },
            None,
        )?;
        for text in ["first queued", "second queued"] {
            let item = devo_core::PendingInputItem::new(
                devo_core::PendingInputKind::UserText { text: text.into() },
                None,
                chrono::Utc::now(),
            );
            runtime
                .deps
                .db
                .push_pending(&session_id, crate::db::QueueType::Turn, &item)?;
        }
        let connection_id = initialized_connection(&runtime).await;

        let response = history_request(
            &runtime,
            connection_id,
            1,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": true,
            }),
        )
        .await;
        let result: SubscriptionCreateResult =
            serde_json::from_value(response["result"].clone()).expect("create result");
        assert_eq!(result.snapshots.len(), 1);
        let SnapshotData::Session { queue, .. } = &result.snapshots[0].data else {
            panic!("expected session snapshot");
        };
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0].position, 1);
        assert_eq!(queue[0].preview, "first queued");
        assert_eq!(queue[1].position, 2);
        assert_eq!(queue[1].preview, "second queued");

        Ok(())
    }

    #[tokio::test]
    async fn subscription_create_future_cursor_is_expired() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let stream_id = devo_core::session_stream_id(
            &devo_protocol::native::ids::SessionId::from_string(session_id.to_string()),
        );
        let connection_id = initialized_connection(&runtime).await;

        let response = history_request(
            &runtime,
            connection_id,
            1,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": false,
                "after": [{ "streamId": stream_id, "seq": 999 }],
            }),
        )
        .await;
        assert_eq!(
            response["error"]["code"],
            serde_json::json!("CursorExpired")
        );
        assert_eq!(
            response["error"]["data"]["errorCode"],
            serde_json::json!("CURSOR_EXPIRED")
        );
        assert_eq!(
            response["error"]["data"]["requiresSnapshot"],
            serde_json::json!(true)
        );

        Ok(())
    }

    #[tokio::test]
    async fn subscription_live_delivery_reaches_new_style_subscriber() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let (outbound, mut receiver) = super::outbound::test_outbound_channel(4);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        // The connection has NO legacy events/subscribe filter; only the
        // new-style selector can deliver.
        let created = history_request(
            &runtime,
            connection_id,
            2,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": false,
            }),
        )
        .await;
        assert!(created.get("error").is_none(), "create failed: {created}");

        let turn_id = TurnId::new();
        runtime
            .broadcast_event(ServerEvent::ItemCompleted(ItemEventPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(ItemId::new()),
                    seq: 0,
                    item_seq: Some(5),
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::AgentMessage,
                    payload: serde_json::json!({ "title": "Assistant", "text": "live" }),
                },
            }))
            .await;

        let frame = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await?
            .expect("new-style subscriber receives live event");
        assert_eq!(frame["method"], serde_json::json!("item/completed"));
        assert_eq!(
            frame["params"]["item"]["item"]["text"],
            serde_json::json!("live")
        );

        // A connection without any selector sees nothing.
        let (other_outbound, mut other_receiver) = super::outbound::test_outbound_channel(4);
        let other_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, other_outbound)
            .await;
        runtime
            .handle_acp_initialize(
                other_connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        runtime
            .broadcast_event(ServerEvent::ItemCompleted(ItemEventPayload {
                context: EventContext {
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: Some(ItemId::new()),
                    seq: 0,
                    item_seq: Some(6),
                },
                item: ItemEnvelope {
                    item_id: ItemId::new(),
                    item_kind: ItemKind::AgentMessage,
                    payload: serde_json::json!({ "title": "Assistant", "text": "second" }),
                },
            }))
            .await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), other_receiver.recv())
                .await
                .is_err(),
            "connection without selectors must not receive the event"
        );
        // The subscribed connection gets the second event too.
        let second = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await?
            .expect("second live event");
        assert_eq!(second["method"], serde_json::json!("item/completed"));
        assert_eq!(
            second["params"]["item"]["item"]["text"],
            serde_json::json!("second")
        );

        Ok(())
    }

    #[tokio::test]
    async fn subscription_ack_is_monotonic_and_unsubscribe_removes() -> Result<()> {
        use devo_protocol::native::event::SubscriptionCreateResult;

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let stream_id = devo_core::session_stream_id(
            &devo_protocol::native::ids::SessionId::from_string(session_id.to_string()),
        );
        let connection_id = initialized_connection(&runtime).await;

        let created = history_request(
            &runtime,
            connection_id,
            1,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": false,
            }),
        )
        .await;
        let created: SubscriptionCreateResult =
            serde_json::from_value(created["result"].clone()).expect("create result");
        let subscription_id = created.subscription_id.as_str().to_owned();

        // Future ack → expired.
        let future = history_request(
            &runtime,
            connection_id,
            2,
            "subscription/ack",
            serde_json::json!({
                "subscriptionId": subscription_id,
                "cursors": [{ "streamId": stream_id, "seq": 99 }],
            }),
        )
        .await;
        assert_eq!(future["error"]["code"], serde_json::json!("CursorExpired"));
        // Barrier ack → ok.
        let ok = history_request(
            &runtime,
            connection_id,
            3,
            "subscription/ack",
            serde_json::json!({
                "subscriptionId": subscription_id,
                "cursors": [{ "streamId": stream_id, "seq": 4 }],
            }),
        )
        .await;
        assert!(ok.get("error").is_none(), "barrier ack must succeed: {ok}");
        // Regression → expired.
        let regression = history_request(
            &runtime,
            connection_id,
            4,
            "subscription/ack",
            serde_json::json!({
                "subscriptionId": subscription_id,
                "cursors": [{ "streamId": stream_id, "seq": 2 }],
            }),
        )
        .await;
        assert_eq!(
            regression["error"]["code"],
            serde_json::json!("CursorExpired")
        );
        // Unknown stream → expired.
        let unknown_stream = history_request(
            &runtime,
            connection_id,
            5,
            "subscription/ack",
            serde_json::json!({
                "subscriptionId": subscription_id,
                "cursors": [{ "streamId": "session:00000000-0000-0000-0000-000000000000", "seq": 1 }],
            }),
        )
        .await;
        assert_eq!(
            unknown_stream["error"]["code"],
            serde_json::json!("CursorExpired")
        );

        let removed = history_request(
            &runtime,
            connection_id,
            6,
            "subscription/unsubscribe",
            serde_json::json!({ "subscriptionId": subscription_id }),
        )
        .await;
        assert!(removed.get("error").is_none(), "unsubscribe must succeed");
        let removed_again = history_request(
            &runtime,
            connection_id,
            7,
            "subscription/unsubscribe",
            serde_json::json!({ "subscriptionId": subscription_id }),
        )
        .await;
        assert!(removed_again.get("error").is_some());

        Ok(())
    }

    // ── Session input queue (P4c: session/queue/*) ────────────────────

    /// A provider whose stream blocks until `open` flips, so tests can hold
    /// a turn open and finish it on demand. The gate is level-triggered:
    /// once open, every later call completes immediately (a plain
    /// `Notify::notify_waiters` only wakes current waiters and deadlocks
    /// follow-up turns). `started` flips once the first stream is requested,
    /// letting tests distinguish "model call in flight" from "turn
    /// registered but query not started yet".
    struct GatedProvider {
        open: Arc<std::sync::atomic::AtomicBool>,
        started: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ModelProviderSDK for GatedProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            anyhow::bail!("gated provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            self.started
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let open = Arc::clone(&self.open);
            // Tick like a real provider stream so the session actor keeps
            // servicing its mailbox (a perfectly silent stream would stall
            // every mailbox round-trip the way no production stream can).
            Ok(Box::pin(futures::stream::unfold(false, move |done| {
                let open = Arc::clone(&open);
                async move {
                    if done {
                        return None;
                    }
                    let gate_open = async {
                        while !open.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                            Some((
                                Ok(StreamEvent::TextDelta {
                                    index: 0,
                                    text: "tick".into(),
                                }),
                                false,
                            ))
                        }
                        _ = gate_open => {
                            Some((
                                Ok(StreamEvent::MessageDone {
                                    response: ModelResponse {
                                        id: "gated-response".into(),
                                        content: vec![devo_protocol::ResponseContent::Text(
                                            "done".into(),
                                        )],
                                        stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                        usage: devo_protocol::Usage::default(),
                                        metadata: devo_protocol::ResponseMetadata::default(),
                                    },
                                }),
                                true,
                            ))
                        }
                    }
                }
            })))
        }

        fn name(&self) -> &str {
            "gated-provider"
        }
    }

    async fn start_durable_session(
        runtime: &Arc<ServerRuntime>,
        connection_id: u64,
        data_root: &std::path::Path,
    ) -> Result<SessionId> {
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 100,
                    "method": "session/start",
                    "params": {
                        "cwd": data_root,
                        "ephemeral": false,
                        "title": "queue session",
                        "model": "test-model"
                    }
                }),
            )
            .await
            .expect("session/start response");
        Ok(
            serde_json::from_value::<crate::SuccessResponse<crate::SessionStartResult>>(response)?
                .result
                .session
                .session_id,
        )
    }

    async fn start_turn(
        runtime: &Arc<ServerRuntime>,
        connection_id: u64,
        session_id: SessionId,
        text: &str,
    ) -> Result<TurnId> {
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 101,
                    "method": "turn/start",
                    "params": {
                        "session_id": session_id,
                        "input": [{ "type": "text", "text": text }],
                        "model": null,
                        "sandbox": null,
                        "approval_policy": null,
                        "cwd": null
                    }
                }),
            )
            .await
            .expect("turn/start response");
        let result: crate::SuccessResponse<crate::TurnStartResult> =
            serde_json::from_value(response)?;
        Ok(result.result.turn_id().expect("turn started"))
    }

    #[tokio::test]
    async fn rollback_preview_commit_restores_git_and_is_idempotent() -> Result<()> {
        use devo_protocol::native::rpc_session::{RestorePlan, SessionRollbackCommitResult};

        let data_root = TempDir::new()?;
        let repo = TempDir::new()?;
        let normalize_line_endings = |s: String| s.replace("\r\n", "\n");
        for args in [
            vec!["init"],
            vec!["config", "user.email", "rollback@example.com"],
            vec!["config", "user.name", "Rollback Test"],
        ] {
            let status = std::process::Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .status()?;
            assert!(status.success());
        }
        std::fs::write(repo.path().join("tracked.txt"), "initial\n")?;
        for args in [vec!["add", "tracked.txt"], vec!["commit", "-m", "initial"]] {
            let status = std::process::Command::new("git")
                .current_dir(repo.path())
                .args(args)
                .status()?;
            assert!(status.success());
        }

        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, repo.path()).await?;
        start_turn(&runtime, connection_id, session_id, "first").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(runtime.runtime_active_turn_id(session_id).await.is_none());

        std::fs::write(repo.path().join("tracked.txt"), "before second\n")?;
        start_turn(&runtime, connection_id, session_id, "second").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
        std::fs::write(repo.path().join("tracked.txt"), "after second\n")?;
        std::fs::write(repo.path().join("new.txt"), "new\n")?;
        let turns_before_preview = session_turns_json(&runtime, connection_id, session_id).await;

        let preview = history_request(
            &runtime,
            connection_id,
            200,
            "session/rollback/preview",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "userTurnIndex": 1,
                "mode": "beforeUserTurn",
            }),
        )
        .await;
        let plan: RestorePlan =
            serde_json::from_value(preview["result"].clone()).expect("rollback preview result");
        assert_eq!(
            plan.affected_files,
            vec![PathBuf::from("new.txt"), PathBuf::from("tracked.txt")]
        );
        assert_eq!(plan.dropped_turn_count, 1);
        assert_eq!(
            session_turns_json(&runtime, connection_id, session_id).await,
            turns_before_preview
        );
        let turn_ids_before: HashSet<String> = turns_before_preview
            .iter()
            .filter_map(|turn| turn["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(turn_ids_before.len(), 2);
        let index_before = std::process::Command::new("git")
            .current_dir(repo.path())
            .args(["show", ":tracked.txt"])
            .output()?;
        assert!(index_before.status.success());

        let commit_params = serde_json::json!({
            "restorePlanId": plan.restore_plan_id.as_str(),
            "expectedWorkspaceVersion": plan.workspace_version,
        });
        let other_connection_id = initialized_connection(&runtime).await;
        let wrong_connection = history_request(
            &runtime,
            other_connection_id,
            201,
            "session/rollback/commit",
            commit_params.clone(),
        )
        .await;
        assert_eq!(
            wrong_connection["error"]["code"],
            serde_json::json!("RESTORE_PLAN_NOT_FOUND")
        );
        std::fs::write(repo.path().join("drift.txt"), "drift\n")?;
        let conflicted = history_request(
            &runtime,
            connection_id,
            202,
            "session/rollback/commit",
            commit_params.clone(),
        )
        .await;
        assert_eq!(
            conflicted["error"]["code"],
            serde_json::json!("WORKSPACE_VERSION_CONFLICT")
        );
        std::fs::remove_file(repo.path().join("drift.txt"))?;
        let title_update = history_request(
            &runtime,
            connection_id,
            206,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id,
                "expectedVersion": 0,
                "title": "Preserved rollback title",
            }),
        )
        .await;
        assert!(title_update.get("error").is_none(), "{title_update}");
        let queued_input = devo_protocol::PendingInputItem::new(
            devo_protocol::PendingInputKind::UserText {
                text: "preserve queued input".to_string(),
            },
            None,
            Utc::now(),
        );
        let queued_input_id = queued_input.id;
        runtime
            .session_turn_reservation_snapshot(session_id)
            .await
            .expect("turn reservation")
            .pending_turn_queue
            .lock()
            .expect("pending queue")
            .push_back(queued_input);
        let (committed, concurrent_retry) = tokio::join!(
            history_request(
                &runtime,
                connection_id,
                203,
                "session/rollback/commit",
                commit_params.clone(),
            ),
            history_request(
                &runtime,
                connection_id,
                204,
                "session/rollback/commit",
                commit_params.clone(),
            )
        );
        assert_eq!(concurrent_retry["result"], committed["result"]);
        let result: SessionRollbackCommitResult =
            serde_json::from_value(committed["result"].clone()).expect("rollback commit result");
        assert_eq!(
            result,
            SessionRollbackCommitResult {
                restored_turn_count: 1,
                restored_file_count: 2,
            }
        );
        assert_eq!(
            normalize_line_endings(std::fs::read_to_string(repo.path().join("tracked.txt"))?),
            "before second\n"
        );
        assert!(!repo.path().join("new.txt").exists());
        assert_eq!(
            std::process::Command::new("git")
                .current_dir(repo.path())
                .args(["show", ":tracked.txt"])
                .output()?
                .stdout,
            index_before.stdout
        );
        let turn_ids_after: HashSet<String> =
            session_turns_json(&runtime, connection_id, session_id)
                .await
                .iter()
                .filter_map(|turn| turn["id"].as_str().map(str::to_string))
                .collect();
        assert_eq!(turn_ids_after.len(), 1);
        assert!(turn_ids_after.is_subset(&turn_ids_before));
        assert_eq!(
            runtime
                .session(session_id)
                .await
                .expect("session")
                .summary()
                .await
                .expect("summary")
                .title,
            Some("Preserved rollback title".to_string())
        );
        let pending_after = runtime
            .session_turn_reservation_snapshot(session_id)
            .await
            .expect("turn reservation")
            .pending_turn_queue
            .lock()
            .expect("pending queue")
            .front()
            .map(|item| item.id);
        assert_eq!(pending_after, Some(queued_input_id));

        let retried = history_request(
            &runtime,
            connection_id,
            205,
            "session/rollback/commit",
            commit_params,
        )
        .await;
        assert_eq!(retried["result"], committed["result"]);
        Ok(())
    }

    #[tokio::test]
    async fn turn_start_waits_for_session_state_change_gate() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        let session_handle = runtime.session(session_id).await.expect("session");
        let state_change_guard = session_handle.lock_state_change().await;
        let runtime_for_turn = Arc::clone(&runtime);
        let turn_start = tokio::spawn(async move {
            runtime_for_turn
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 300,
                        "method": "turn/start",
                        "params": {
                            "session_id": session_id,
                            "input": [{ "type": "text", "text": "wait" }],
                            "model": null,
                            "sandbox": null,
                            "approval_policy": null,
                            "cwd": null
                        }
                    }),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!turn_start.is_finished());
        drop(state_change_guard);
        let response = turn_start.await?.expect("turn/start response");
        assert!(response.get("error").is_none(), "turn/start: {response}");
        Ok(())
    }

    #[tokio::test]
    async fn manual_compaction_waits_for_session_state_change_gate() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        let session_handle = runtime.session(session_id).await.expect("session");
        let state_change_guard = session_handle.lock_state_change().await;
        let now = Utc::now();
        let turn = crate::turn::TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence: 1,
            status: TurnStatus::Running,
            kind: devo_core::TurnKind::ManualCompaction,
            model: "test-model".to_string(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".to_string(),
            request_thinking: None,
            started_at: now,
            completed_at: None,
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };
        let compaction = tokio::spawn(Arc::clone(&runtime).run_session_compaction(
            session_id,
            session_handle,
            turn,
        ));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!compaction.is_finished());
        drop(state_change_guard);
        tokio::time::timeout(Duration::from_secs(5), compaction).await??;
        Ok(())
    }

    struct TurnOkCompactHangProvider;

    #[async_trait]
    impl ModelProviderSDK for TurnOkCompactHangProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            std::future::pending::<()>().await;
            unreachable!("hanging compaction completion should be canceled")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "turn-ok".into(),
                        content: vec![devo_protocol::ResponseContent::Text("ok".into())],
                        stop_reason: Some(devo_protocol::StopReason::EndTurn),
                        usage: devo_protocol::Usage::default(),
                        metadata: devo_protocol::ResponseMetadata::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "turn-ok-compact-hang"
        }
    }

    /// Trace: L2-DES-AGENT-002
    /// Verifies: Native session/interrupt stops hanging compaction
    /// and reopens admission.
    #[tokio::test]
    async fn session_interrupt_stops_in_flight_compaction() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime =
            build_runtime_with_provider(data_root.path(), Arc::new(TurnOkCompactHangProvider));
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        start_turn(&runtime, connection_id, session_id, "seed history").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            runtime.runtime_active_turn_id(session_id).await.is_none(),
            "seed turn should finish before compact"
        );

        let compact_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 200,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("session/compact response");
        assert!(
            compact_response.get("error").is_none(),
            "session/compact: {compact_response}"
        );
        let compact_result: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(
                compact_response
                    .get("result")
                    .cloned()
                    .expect("compact result"),
            )?;
        let turn_id = devo_protocol::TurnId::try_from(compact_result.turn.id.as_str())?;

        for _ in 0..50 {
            if runtime.runtime_active_turn_id(session_id).await == Some(turn_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            runtime.runtime_active_turn_id(session_id).await,
            Some(turn_id),
            "compaction should own the active turn while summarizer hangs"
        );

        let interrupt_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 201,
                    "method": "session/interrupt",
                    "params": {
                        "scope": { "scope": "session", "sessionId": session_id }
                    }
                }),
            )
            .await
            .expect("session/interrupt response");
        assert!(
            interrupt_response.get("error").is_none(),
            "session/interrupt: {interrupt_response}"
        );

        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            runtime.runtime_active_turn_id(session_id).await.is_none(),
            "cancel should clear the active compaction turn"
        );

        let compact_again = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 203,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("second session/compact response");
        assert!(
            compact_again.get("error").is_none(),
            "session should accept compact after cancel: {compact_again}"
        );
        if runtime.runtime_active_turn_id(session_id).await.is_some() {
            let _ = runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 204,
                        "method": "session/interrupt",
                        "params": {
                            "scope": { "scope": "session", "sessionId": session_id }
                        }
                    }),
                )
                .await;
        }
        Ok(())
    }

    /// Trace: L2-DES-AGENT-002
    /// Verifies: if compaction claimed active_turn then failed to record terminal
    /// status, interrupt recovery still emits canceled lifecycle events.
    ///
    /// Deadlock note: a live compaction task holds `state_change_gate` across
    /// `compact_history`. Recovery must cancel that work before a later compact
    /// can admit; this test waits for the gate after recovery.
    #[tokio::test]
    async fn recover_orphaned_compaction_after_actor_claim() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime =
            build_runtime_with_provider(data_root.path(), Arc::new(TurnOkCompactHangProvider));
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        start_turn(&runtime, connection_id, session_id, "seed history").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let compact_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 210,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("session/compact response");
        let compact_result: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(
                compact_response
                    .get("result")
                    .cloned()
                    .expect("compact result"),
            )?;
        let turn_id = devo_protocol::TurnId::try_from(compact_result.turn.id.as_str())?;
        for _ in 0..50 {
            if runtime.runtime_active_turn_id(session_id).await == Some(turn_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            runtime.runtime_active_turn_id(session_id).await,
            Some(turn_id)
        );

        let session_handle = runtime.session(session_id).await.expect("session");
        // Simulate: task claimed actor active_turn, then was aborted before
        // record_terminal_turn_status. Keep the abort handle so recovery can
        // still abort the hanging summarizer and release state_change_gate.
        assert_eq!(
            session_handle.clear_active_turn_if_matches(turn_id).await,
            Some(true)
        );

        let recovered = runtime
            .recover_orphaned_manual_compaction_interrupt(&session_handle, session_id, turn_id)
            .await
            .expect("orphaned compaction should recover");
        assert_eq!(recovered.status, TurnStatus::Interrupted);
        assert!(
            runtime.runtime_active_turn_id(session_id).await.is_none(),
            "recovery should clear runtime handles"
        );
        assert!(
            runtime
                .recent_terminal_turn_status(turn_id)
                .await
                .is_some_and(|snapshot| snapshot.status == TurnStatus::Interrupted)
        );

        // Hanging compact_history held state_change_gate; wait until cancel/abort
        // lets that task drop it before admitting another compact.
        let gate = tokio::time::timeout(Duration::from_secs(5), session_handle.lock_state_change())
            .await
            .context("timed out waiting for compaction to release state_change_gate")?;
        drop(gate);

        let compact_again = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 211,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("second session/compact response");
        assert!(
            compact_again.get("error").is_none(),
            "session should accept compact after orphan recovery: {compact_again}"
        );
        if runtime.runtime_active_turn_id(session_id).await.is_some() {
            let _ = runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 212,
                        "method": "session/interrupt",
                        "params": {
                            "scope": { "scope": "session", "sessionId": session_id }
                        }
                    }),
                )
                .await;
        }
        Ok(())
    }

    /// Turn stream is gated (the turn hangs until `stream_open`); the
    /// non-stream completion used by title generation returns a valid title
    /// once `completion_open`.
    struct TitleCompletionStreamGatedProvider {
        stream_open: Arc<std::sync::atomic::AtomicBool>,
        stream_started: Arc<std::sync::atomic::AtomicBool>,
        completion_open: Arc<std::sync::atomic::AtomicBool>,
        completion_requested: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait]
    impl ModelProviderSDK for TitleCompletionStreamGatedProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            self.completion_requested
                .store(true, std::sync::atomic::Ordering::SeqCst);
            while !self
                .completion_open
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(ModelResponse {
                id: "title-response".into(),
                content: vec![devo_protocol::ResponseContent::Text(
                    "Generated session title".into(),
                )],
                stop_reason: Some(devo_protocol::StopReason::EndTurn),
                usage: devo_protocol::Usage::default(),
                metadata: devo_protocol::ResponseMetadata::default(),
            })
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = Result<StreamEvent>> + Send>>>
        {
            self.stream_started
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let open = Arc::clone(&self.stream_open);
            Ok(Box::pin(futures::stream::unfold(false, move |done| {
                let open = Arc::clone(&open);
                async move {
                    if done {
                        return None;
                    }
                    let gate_open = async {
                        while !open.load(std::sync::atomic::Ordering::SeqCst) {
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                            Some((
                                Ok(StreamEvent::TextDelta {
                                    index: 0,
                                    text: "tick".into(),
                                }),
                                false,
                            ))
                        }
                        _ = gate_open => {
                            Some((
                                Ok(StreamEvent::MessageDone {
                                    response: ModelResponse {
                                        id: "gated-response".into(),
                                        content: vec![devo_protocol::ResponseContent::Text(
                                            "done".into(),
                                        )],
                                        stop_reason: Some(devo_protocol::StopReason::EndTurn),
                                        usage: devo_protocol::Usage::default(),
                                        metadata: devo_protocol::ResponseMetadata::default(),
                                    },
                                }),
                                true,
                            ))
                        }
                    }
                }
            })))
        }

        fn name(&self) -> &str {
            "title-completion-stream-gated"
        }
    }

    /// Regression: during the first turn of an untitled session, final
    /// title generation takes `state_change_gate`
    /// (`maybe_generate_final_title` in runtime/items.rs) and then parks on
    /// the session-actor mailbox (`update_title`) while the actor is busy
    /// executing the turn, so the gate stays held for the rest of the turn.
    /// A `session/queue/push` in that window must still answer `Queued`
    /// immediately: the busy path no longer touches the gate
    /// (runtime/handlers/turn.rs).
    #[tokio::test]
    async fn queue_push_responds_immediately_while_title_generation_holds_gate() -> Result<()> {
        let data_root = TempDir::new()?;
        let provider = Arc::new(TitleCompletionStreamGatedProvider {
            stream_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stream_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            completion_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            completion_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let stream_open = Arc::clone(&provider.stream_open);
        let stream_started = Arc::clone(&provider.stream_started);
        let completion_open = Arc::clone(&provider.completion_open);
        let completion_requested = Arc::clone(&provider.completion_requested);
        let runtime = build_runtime_with_provider(data_root.path(), provider);
        let connection_id = initialized_connection(&runtime).await;
        // No `title` param: final title generation runs on the first turn.
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 100,
                    "method": "session/start",
                    "params": {
                        "cwd": data_root.path(),
                        "ephemeral": false,
                        "model": "test-model"
                    }
                }),
            )
            .await
            .expect("session/start response");
        let session_id =
            serde_json::from_value::<crate::SuccessResponse<crate::SessionStartResult>>(response)?
                .result
                .session
                .session_id;
        start_turn(&runtime, connection_id, session_id, "first prompt").await?;

        // The title task reaching its provider call proves it passed every
        // actor mailbox round-trip; the turn is executing inside the actor.
        for _ in 0..500 {
            if completion_requested.load(std::sync::atomic::Ordering::SeqCst)
                && stream_started.load(std::sync::atomic::Ordering::SeqCst)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            completion_requested.load(std::sync::atomic::Ordering::SeqCst),
            "title generation should have requested its completion"
        );
        assert!(
            stream_started.load(std::sync::atomic::Ordering::SeqCst),
            "turn should be executing"
        );

        // Let the title model call finish: the title task now grabs
        // `state_change_gate` and parks on the busy actor mailbox.
        completion_open.store(true, std::sync::atomic::Ordering::SeqCst);
        let session_handle = runtime.session(session_id).await.expect("session");
        let mut gate_held = false;
        for _ in 0..50 {
            match tokio::time::timeout(
                Duration::from_millis(100),
                session_handle.lock_state_change(),
            )
            .await
            {
                Ok(guard) => {
                    drop(guard);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(_) => {
                    gate_held = true;
                    break;
                }
            }
        }
        assert!(
            gate_held,
            "title generation should be holding state_change_gate across the actor mailbox wait"
        );

        // Fixed behavior: the busy push no longer touches
        // `state_change_gate`, so it answers `Queued` immediately even
        // while title generation holds the gate.
        let push_runtime = Arc::clone(&runtime);
        let push = tokio::spawn(async move {
            push_runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 300,
                        "method": "session/queue/push",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": "second prompt" }],
                            "idempotencyKey": "push-wedge-1"
                        }
                    }),
                )
                .await
        });
        let response = tokio::time::timeout(Duration::from_secs(5), push)
            .await
            .context("busy push must respond immediately while the gate is held")??
            .expect("push response");
        assert!(response.get("error").is_none(), "push: {response}");
        let result: devo_protocol::native::rpc_turn::SessionQueuePushResult =
            serde_json::from_value(response["result"].clone()).expect("push result");
        assert!(
            matches!(
                result,
                devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { .. }
            ),
            "busy push must queue: {response}"
        );

        // Cleanup: let the turn finish so the gate is released.
        stream_open.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    #[tokio::test]
    async fn native_turn_start_rejects_immediately_while_title_generation_holds_gate() -> Result<()>
    {
        let data_root = TempDir::new()?;
        let provider = Arc::new(TitleCompletionStreamGatedProvider {
            stream_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stream_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            completion_open: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            completion_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let stream_open = Arc::clone(&provider.stream_open);
        let stream_started = Arc::clone(&provider.stream_started);
        let completion_open = Arc::clone(&provider.completion_open);
        let completion_requested = Arc::clone(&provider.completion_requested);
        let runtime = build_runtime_with_provider(data_root.path(), provider);
        let connection_id = initialized_connection(&runtime).await;
        let response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 100,
                    "method": "session/start",
                    "params": {
                        "cwd": data_root.path(),
                        "ephemeral": false,
                        "model": "test-model"
                    }
                }),
            )
            .await
            .expect("session/start response");
        let session_id =
            serde_json::from_value::<crate::SuccessResponse<crate::SessionStartResult>>(response)?
                .result
                .session
                .session_id;
        start_turn(&runtime, connection_id, session_id, "first prompt").await?;

        for _ in 0..500 {
            if completion_requested.load(std::sync::atomic::Ordering::SeqCst)
                && stream_started.load(std::sync::atomic::Ordering::SeqCst)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            completion_requested.load(std::sync::atomic::Ordering::SeqCst),
            "title generation should have requested its completion"
        );
        completion_open.store(true, std::sync::atomic::Ordering::SeqCst);
        let session_handle = runtime.session(session_id).await.expect("session");
        let mut gate_held = false;
        for _ in 0..50 {
            match tokio::time::timeout(
                Duration::from_millis(100),
                session_handle.lock_state_change(),
            )
            .await
            {
                Ok(guard) => {
                    drop(guard);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(_) => {
                    gate_held = true;
                    break;
                }
            }
        }
        assert!(
            gate_held,
            "title generation should be holding state_change_gate across the actor mailbox wait"
        );

        let start_runtime = Arc::clone(&runtime);
        let native_start = tokio::spawn(async move {
            start_runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 301,
                        "method": "turn/start",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": "second prompt" }],
                            "idempotencyKey": "native-wedge-1"
                        }
                    }),
                )
                .await
        });
        let response = tokio::time::timeout(Duration::from_secs(2), native_start)
            .await
            .context("native turn/start must respond while the gate is held")??
            .expect("turn/start response");
        assert_eq!(
            response["error"]["code"].as_str(),
            Some("TurnAlreadyRunning"),
            "busy native turn/start must reject: {response}"
        );

        let update_runtime = Arc::clone(&runtime);
        let metadata_update = tokio::spawn(async move {
            update_runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 302,
                        "method": "session/metadata/update",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "expectedVersion": 0,
                            "model": { "provider": "", "model": "test-model" }
                        }
                    }),
                )
                .await
        });
        let response = tokio::time::timeout(Duration::from_secs(2), metadata_update)
            .await
            .context("session/metadata/update must respond while the gate is held")??
            .expect("metadata update response");
        assert!(
            response.get("error").is_none(),
            "metadata update: {response}"
        );

        stream_open.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// Same gate holder, same fix: a manual compaction task takes
    /// `state_change_gate` across the `compact_history` provider call
    /// (runtime/handlers/compaction.rs), but the compaction turn is the
    /// active turn, so a `session/queue/push` takes the gate-free busy
    /// path and responds immediately.
    #[tokio::test]
    async fn queue_push_responds_immediately_during_manual_compaction() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime =
            build_runtime_with_provider(data_root.path(), Arc::new(TurnOkCompactHangProvider));
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        start_turn(&runtime, connection_id, session_id, "seed history").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(runtime.runtime_active_turn_id(session_id).await.is_none());

        let compact_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 200,
                    "method": "session/compact/start",
                    "params": { "sessionId": session_id }
                }),
            )
            .await
            .expect("session/compact response");
        let compact_result: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(
                compact_response
                    .get("result")
                    .cloned()
                    .expect("compact result"),
            )?;
        let compaction_turn_id = devo_protocol::TurnId::try_from(compact_result.turn.id.as_str())?;
        for _ in 0..50 {
            if runtime.runtime_active_turn_id(session_id).await == Some(compaction_turn_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            runtime.runtime_active_turn_id(session_id).await,
            Some(compaction_turn_id)
        );

        // Wait until the compaction task actually holds `state_change_gate`
        // inside `compact_history` (a push that slips in beforehand queues
        // fine — the stall only applies while the summarizer is in flight).
        let session_handle = runtime.session(session_id).await.expect("session");
        let mut gate_held = false;
        for _ in 0..50 {
            match tokio::time::timeout(
                Duration::from_millis(100),
                session_handle.lock_state_change(),
            )
            .await
            {
                Ok(guard) => {
                    drop(guard);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(_) => {
                    gate_held = true;
                    break;
                }
            }
        }
        assert!(
            gate_held,
            "compaction should be holding state_change_gate across compact_history"
        );

        // Fixed behavior: the compaction turn is the active turn, so the
        // push takes the gate-free busy path and answers `Queued`
        // immediately even while compaction holds `state_change_gate`
        // across `compact_history`.
        let push_runtime = Arc::clone(&runtime);
        let push = tokio::spawn(async move {
            push_runtime
                .handle_incoming(
                    connection_id,
                    serde_json::json!({
                        "id": 300,
                        "method": "session/queue/push",
                        "params": {
                            "sessionId": session_id.to_string(),
                            "input": [{ "type": "text", "text": "queued while compacting" }],
                            "idempotencyKey": "push-wedge-2"
                        }
                    }),
                )
                .await
        });
        let response = tokio::time::timeout(Duration::from_secs(5), push)
            .await
            .context("busy push must respond immediately during compaction")??
            .expect("push response");
        assert!(response.get("error").is_none(), "push: {response}");
        let result: devo_protocol::native::rpc_turn::SessionQueuePushResult =
            serde_json::from_value(response["result"].clone()).expect("push result");
        assert!(
            matches!(
                result,
                devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { .. }
            ),
            "busy push must queue: {response}"
        );

        // Cleanup: interrupt cancels the hanging summarizer.
        let interrupt_response = runtime
            .handle_incoming(
                connection_id,
                serde_json::json!({
                    "id": 201,
                    "method": "session/interrupt",
                    "params": {
                        "scope": { "scope": "session", "sessionId": session_id }
                    }
                }),
            )
            .await
            .expect("session/interrupt response");
        assert!(
            interrupt_response.get("error").is_none(),
            "session/interrupt: {interrupt_response}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn disconnect_wakes_concurrent_rollback_commit_waiter() -> Result<()> {
        let data_root = TempDir::new()?;
        let workspace = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, workspace.path()).await?;
        start_turn(&runtime, connection_id, session_id, "first").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let preview = history_request(
            &runtime,
            connection_id,
            350,
            "session/rollback/preview",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "userTurnIndex": 0,
                "mode": "beforeUserTurn",
            }),
        )
        .await;
        let plan: devo_protocol::native::rpc_session::RestorePlan =
            serde_json::from_value(preview["result"].clone())?;
        let params = serde_json::json!({
            "restorePlanId": plan.restore_plan_id.as_str(),
            "expectedWorkspaceVersion": plan.workspace_version,
        });
        let session_handle = runtime.session(session_id).await.expect("session");
        let state_change_guard = session_handle.lock_state_change().await;
        let first_runtime = Arc::clone(&runtime);
        let first_params = params.clone();
        let first = tokio::spawn(async move {
            first_runtime
                .handle_session_rollback_commit(connection_id, serde_json::json!(351), first_params)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let second_runtime = Arc::clone(&runtime);
        let second = tokio::spawn(async move {
            second_runtime
                .handle_session_rollback_commit(connection_id, serde_json::json!(352), params)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        runtime.unregister_connection(connection_id).await;
        drop(state_change_guard);

        let first_response = tokio::time::timeout(Duration::from_secs(5), first).await??;
        let second_response = tokio::time::timeout(Duration::from_secs(5), second).await??;
        assert!(first_response.get("error").is_none(), "{first_response}");
        assert_eq!(
            second_response["error"]["code"],
            serde_json::json!("RESTORE_PLAN_NOT_FOUND")
        );
        Ok(())
    }

    #[tokio::test]
    async fn rollback_in_non_git_workspace_is_history_only() -> Result<()> {
        use devo_protocol::native::rpc_session::{RestorePlan, SessionRollbackCommitResult};

        let data_root = TempDir::new()?;
        let workspace = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, workspace.path()).await?;
        for text in ["first", "second"] {
            start_turn(&runtime, connection_id, session_id, text).await?;
            for _ in 0..200 {
                if runtime.runtime_active_turn_id(session_id).await.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
        }

        let preview = history_request(
            &runtime,
            connection_id,
            400,
            "session/rollback/preview",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "userTurnIndex": 1,
                "mode": "beforeUserTurn",
            }),
        )
        .await;
        let plan: RestorePlan =
            serde_json::from_value(preview["result"].clone()).expect("rollback preview result");
        assert_eq!(plan.affected_files, Vec::<PathBuf>::new());
        assert_eq!(plan.workspace_version, "history-only");

        start_turn(&runtime, connection_id, session_id, "third").await?;
        for _ in 0..200 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(runtime.runtime_active_turn_id(session_id).await.is_none());
        let stale_commit = history_request(
            &runtime,
            connection_id,
            401,
            "session/rollback/commit",
            serde_json::json!({
                "restorePlanId": plan.restore_plan_id.as_str(),
                "expectedWorkspaceVersion": plan.workspace_version,
            }),
        )
        .await;
        assert_eq!(
            stale_commit["error"]["code"],
            serde_json::json!("WORKSPACE_VERSION_CONFLICT")
        );

        let preview = history_request(
            &runtime,
            connection_id,
            402,
            "session/rollback/preview",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "userTurnIndex": 1,
                "mode": "beforeUserTurn",
            }),
        )
        .await;
        let plan: RestorePlan =
            serde_json::from_value(preview["result"].clone()).expect("rollback preview result");
        let committed = history_request(
            &runtime,
            connection_id,
            403,
            "session/rollback/commit",
            serde_json::json!({
                "restorePlanId": plan.restore_plan_id.as_str(),
                "expectedWorkspaceVersion": plan.workspace_version,
            }),
        )
        .await;
        assert_eq!(
            serde_json::from_value::<SessionRollbackCommitResult>(committed["result"].clone())?,
            SessionRollbackCommitResult {
                restored_turn_count: 2,
                restored_file_count: 0,
            }
        );
        let turn_ids: HashSet<String> = session_turns_json(&runtime, connection_id, session_id)
            .await
            .iter()
            .filter_map(|turn| turn["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(turn_ids.len(), 1);

        let disconnecting_connection_id = initialized_connection(&runtime).await;
        let disconnect_preview = history_request(
            &runtime,
            disconnecting_connection_id,
            404,
            "session/rollback/preview",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "userTurnIndex": 0,
                "mode": "beforeUserTurn",
            }),
        )
        .await;
        let disconnect_plan: RestorePlan =
            serde_json::from_value(disconnect_preview["result"].clone())?;
        runtime
            .unregister_connection(disconnecting_connection_id)
            .await;
        let disconnected_commit = runtime
            .handle_session_rollback_commit(
                disconnecting_connection_id,
                serde_json::json!(405),
                serde_json::json!({
                    "restorePlanId": disconnect_plan.restore_plan_id.as_str(),
                    "expectedWorkspaceVersion": disconnect_plan.workspace_version,
                }),
            )
            .await;
        assert_eq!(
            disconnected_commit["error"]["code"],
            serde_json::json!("RESTORE_PLAN_NOT_FOUND")
        );
        Ok(())
    }

    async fn queue_list(
        runtime: &Arc<ServerRuntime>,
        connection_id: u64,
        session_id: SessionId,
    ) -> Vec<devo_protocol::native::queue::QueueEntry> {
        let response = history_request(
            runtime,
            connection_id,
            1,
            "session/queue/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let result: devo_protocol::native::rpc_turn::SessionQueueListResult =
            serde_json::from_value(response["result"].clone()).expect("queue/list result");
        result.entries
    }

    async fn session_turns_json(
        runtime: &Arc<ServerRuntime>,
        connection_id: u64,
        session_id: SessionId,
    ) -> Vec<serde_json::Value> {
        let response = history_request(
            runtime,
            connection_id,
            90,
            "session/turns/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        response["result"]["data"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn queue_push_idle_starts_turn_and_busy_queues_then_update_remove() -> Result<()> {
        use devo_protocol::native::rpc_turn::{SessionQueuePushResult, SessionQueueUpdateResult};
        use devo_protocol::native::turn::TurnStatus as NativeTurnStatus;

        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Default::default(),
            }),
        );
        let (outbound, mut notifications) = super::outbound::test_outbound_channel(64);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        // Subscribe so queue/updated notifications are observable.
        let created = history_request(
            &runtime,
            connection_id,
            2,
            "subscription/create",
            serde_json::json!({
                "selectors": [{ "kind": "session", "sessionId": session_id.to_string() }],
                "includeSnapshot": false,
            }),
        )
        .await;
        assert!(created.get("error").is_none(), "subscribe: {created}");

        // Idle push → a turn starts immediately.
        let pushed = history_request(
            &runtime,
            connection_id,
            3,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "first" }],
                "idempotencyKey": "push-1",
            }),
        )
        .await;
        let pushed: SessionQueuePushResult =
            serde_json::from_value(pushed["result"].clone()).expect("push result");
        let SessionQueuePushResult::Started { turn } = pushed else {
            panic!("idle push must start a turn");
        };
        assert_eq!(turn.session_id.as_str(), session_id.to_string());
        assert_eq!(turn.status, NativeTurnStatus::InProgress);
        assert_eq!(turn.sequence, 1);

        // Busy push → queued pre-item.
        let queued = history_request(
            &runtime,
            connection_id,
            4,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "second" }],
                "idempotencyKey": "push-2",
            }),
        )
        .await;
        let queued: SessionQueuePushResult =
            serde_json::from_value(queued["result"].clone()).expect("push result");
        let SessionQueuePushResult::Queued { entry } = queued else {
            panic!("busy push must queue");
        };
        assert_eq!(entry.position, 1);
        assert_eq!(entry.preview, "second");
        assert!(
            matches!(&entry.input.as_slice(), [devo_protocol::native::item::UserInput::Text { text }] if text == "second")
        );

        // Update replaces the content wholesale.
        let updated = history_request(
            &runtime,
            connection_id,
            5,
            "session/queue/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "queueItemId": entry.queue_item_id.as_str(),
                "input": [{ "type": "text", "text": "edited" }],
            }),
        )
        .await;
        let updated: SessionQueueUpdateResult =
            serde_json::from_value(updated["result"].clone()).expect("update result");
        assert!(
            matches!(&updated.entry.input.as_slice(), [devo_protocol::native::item::UserInput::Text { text }] if text == "edited")
        );

        // Reorder: push another entry, then move it to position 1.
        let third = history_request(
            &runtime,
            connection_id,
            6,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "third" }],
                "idempotencyKey": "push-3",
            }),
        )
        .await;
        let third: SessionQueuePushResult =
            serde_json::from_value(third["result"].clone()).expect("push result");
        let SessionQueuePushResult::Queued { entry: third_entry } = third else {
            panic!("busy push must queue");
        };
        let reordered = history_request(
            &runtime,
            connection_id,
            7,
            "session/queue/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "queueItemId": third_entry.queue_item_id.as_str(),
                "position": 1,
            }),
        )
        .await;
        let reordered: SessionQueueUpdateResult =
            serde_json::from_value(reordered["result"].clone()).expect("reorder result");
        assert_eq!(reordered.entry.position, 1);
        let entries = queue_list(&runtime, connection_id, session_id).await;
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.queue_item_id.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec![
                third_entry.queue_item_id.as_str().to_owned(),
                entry.queue_item_id.as_str().to_owned()
            ]
        );
        // The reorder survives in SQLite too.
        let db_entries = runtime
            .deps
            .db
            .list_pending(&session_id, crate::db::QueueType::Turn)?;
        assert_eq!(db_entries.len(), 2);
        assert_eq!(
            db_entries[0].id.to_string(),
            third_entry.queue_item_id.as_str()
        );

        // Remove works; removing again reports the entry is gone.
        let removed = history_request(
            &runtime,
            connection_id,
            8,
            "session/queue/remove",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "queueItemId": third_entry.queue_item_id.as_str(),
            }),
        )
        .await;
        assert!(removed.get("error").is_none(), "remove: {removed}");
        let removed_again = history_request(
            &runtime,
            connection_id,
            9,
            "session/queue/remove",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "queueItemId": third_entry.queue_item_id.as_str(),
            }),
        )
        .await;
        assert_eq!(
            removed_again["error"]["code"],
            serde_json::json!("QueueItemNotFound")
        );

        // queue/updated notifications reached the new-style subscriber.
        let mut changes = Vec::new();
        while let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(50), notifications.recv()).await
        {
            if frame["method"] == serde_json::json!("queue/updated") {
                changes.push(frame["params"]["change"].clone());
            }
        }
        assert!(
            changes.contains(&serde_json::json!("added"))
                && changes.contains(&serde_json::json!("updated"))
                && changes.contains(&serde_json::json!("removed")),
            "expected added/updated/removed notifications, got {changes:?}"
        );
        // Let the held turn settle so teardown does not race the provider.
        open.store(true, std::sync::atomic::Ordering::SeqCst);

        Ok(())
    }

    #[tokio::test]
    async fn queue_steer_promotes_and_late_steer_degrades_back_to_queue() -> Result<()> {
        use devo_protocol::native::rpc_turn::SessionQueueSteerResult;

        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Arc::clone(&started),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        let turn_id = start_turn(&runtime, connection_id, session_id, "go").await?;
        // Wait until the model call is actually in flight: a steer promoted
        // before the query loop's first pending-input drain would be consumed
        // into the prompt (the correct injection outcome), which is not the
        // degrade path this test exercises.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;

        let queued = history_request(
            &runtime,
            connection_id,
            3,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "steer me" }],
                "idempotencyKey": "push-steer",
            }),
        )
        .await;
        let queued: devo_protocol::native::rpc_turn::SessionQueuePushResult =
            serde_json::from_value(queued["result"].clone()).expect("push result");
        let devo_protocol::native::rpc_turn::SessionQueuePushResult::Queued { entry } = queued
        else {
            panic!("busy push must queue");
        };

        let steered = history_request(
            &runtime,
            connection_id,
            4,
            "session/queue/steer",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "queueItemId": entry.queue_item_id.as_str(),
                "expectedTurnId": turn_id.to_string(),
            }),
        )
        .await;
        let steered: SessionQueueSteerResult =
            serde_json::from_value(steered["result"].clone()).expect("steer result");
        assert!(!steered.item_id.as_str().is_empty());
        assert!(
            queue_list(&runtime, connection_id, session_id)
                .await
                .is_empty()
        );

        // Interrupt the turn before the next injection boundary: the
        // promoted steer is never consumed; it degrades back into the
        // session queue, and the now-idle session drains it into a new
        // turn — the message is never lost (01 §4.3).
        let interrupted = history_request(
            &runtime,
            connection_id,
            5,
            "session/interrupt",
            serde_json::json!({
                "scope": {
                    "scope": "session",
                    "sessionId": session_id.to_string()
                },
            }),
        )
        .await;
        assert!(
            interrupted.get("error").is_none(),
            "interrupt: {interrupted}"
        );
        // Open the gate: turn 1 settles as interrupted; the follow-up turn
        // started by the queue drain runs to completion immediately.
        open.store(true, std::sync::atomic::Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let turns = session_turns_json(&runtime, connection_id, session_id).await;
                let has_completed_followup = turns
                    .iter()
                    .any(|turn| turn["status"] == serde_json::json!("completed"));
                if has_completed_followup
                    && queue_list(&runtime, connection_id, session_id)
                        .await
                        .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        // Turn 1 is interrupted; the degraded steer drained into a second,
        // completed turn (two distinct turn ids, message processed).
        let turns = session_turns_json(&runtime, connection_id, session_id).await;
        let statuses: Vec<&str> = turns
            .iter()
            .filter_map(|turn| turn["status"].as_str())
            .collect();
        assert!(
            statuses.contains(&"interrupted") && statuses.contains(&"completed"),
            "expected interrupted + completed turns, got {statuses:?}"
        );
        let turn_ids: std::collections::HashSet<&str> = turns
            .iter()
            .filter_map(|turn| turn["id"].as_str())
            .collect();
        assert_eq!(turn_ids.len(), 2, "expected two distinct turns: {turns:?}");

        // Native semantics: steering after the turn ends is an explicit
        // error (nothing is silently dropped), and a fresh push on the now
        // idle session starts a new turn (message preserved).
        let late_steer = history_request(
            &runtime,
            connection_id,
            5,
            "session/queue/steer",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "queueItemId": entry.queue_item_id.as_str(),
                "expectedTurnId": turn_id.to_string(),
            }),
        )
        .await;
        assert!(
            late_steer.get("error").is_some(),
            "steer after turn end must fail: {late_steer}"
        );
        let pushed = history_request(
            &runtime,
            connection_id,
            6,
            "session/queue/push",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "too late" }],
                "idempotencyKey": "push-late",
            }),
        )
        .await;
        let pushed: devo_protocol::native::rpc_turn::SessionQueuePushResult =
            serde_json::from_value(pushed["result"].clone()).expect("push result");
        assert!(
            matches!(
                pushed,
                devo_protocol::native::rpc_turn::SessionQueuePushResult::Started { .. }
            ),
            "idle push must start a new turn: {pushed:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn queue_survives_restart_and_steer_restores_into_turn_queue() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        // Seed one queued and one stale-steer row directly in SQLite.
        let queued_item = devo_core::PendingInputItem::new(
            devo_core::PendingInputKind::UserText {
                text: "queued text".into(),
            },
            None,
            chrono::Utc::now(),
        );
        let steer_item = devo_core::PendingInputItem::new(
            devo_core::PendingInputKind::UserText {
                text: "stale steer".into(),
            },
            None,
            chrono::Utc::now(),
        );
        runtime
            .deps
            .db
            .push_pending(&session_id, crate::db::QueueType::Turn, &queued_item)?;
        runtime
            .deps
            .db
            .push_pending(&session_id, crate::db::QueueType::Steer, &steer_item)?;
        drop(runtime);

        let rebuilt = build_runtime(data_root.path());
        rebuilt.load_persisted_sessions().await?;
        let rebuilt_connection = initialized_connection(&rebuilt).await;
        let entries = queue_list(&rebuilt, rebuilt_connection, session_id).await;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].preview, "queued text");
        assert_eq!(entries[1].preview, "stale steer");
        // The steer row moved into the turn queue table; the original queued
        // row stays in the database as the durable mirror of the in-memory
        // queue until it is actually consumed.
        assert!(
            rebuilt
                .deps
                .db
                .list_pending(&session_id, crate::db::QueueType::Steer)?
                .is_empty()
        );
        let turn_rows = rebuilt
            .deps
            .db
            .list_pending(&session_id, crate::db::QueueType::Turn)?;
        assert_eq!(turn_rows.len(), 2);
        assert_eq!(turn_rows[0].id, queued_item.id);
        assert_eq!(turn_rows[1].id, steer_item.id);
        drop(rebuilt);

        // Restart again without consuming the queue: the durable mirror must
        // still hold both entries.
        let restarted = build_runtime(data_root.path());
        restarted.load_persisted_sessions().await?;
        let restarted_connection = initialized_connection(&restarted).await;
        let entries = queue_list(&restarted, restarted_connection, session_id).await;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].preview, "queued text");
        assert_eq!(entries[1].preview, "stale steer");

        Ok(())
    }

    /// Rapid consecutive `session/queue/push` requests while a turn is active
    /// must all complete: a stalled push parks an inbound permit and
    /// eventually wedges the whole connection.
    #[tokio::test]
    async fn concurrent_queue_pushes_while_turn_active_all_respond() -> Result<()> {
        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Arc::clone(&started),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        start_turn(&runtime, connection_id, session_id, "hold the turn open").await?;
        for _ in 0..200 {
            if started.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            started.load(std::sync::atomic::Ordering::SeqCst),
            "provider stream should have started"
        );

        const PUSH_COUNT: usize = 8;
        let mut tasks = Vec::new();
        for index in 0..PUSH_COUNT {
            let runtime = Arc::clone(&runtime);
            tasks.push(tokio::spawn(async move {
                runtime
                    .handle_incoming(
                        connection_id,
                        serde_json::json!({
                            "id": 500 + index as u64,
                            "method": "session/queue/push",
                            "params": {
                                "sessionId": session_id.to_string(),
                                "input": [{ "type": "text", "text": format!("queued {index}") }],
                                "idempotencyKey": format!("push-{index}"),
                            },
                        }),
                    )
                    .await
            }));
        }
        for (index, task) in tasks.into_iter().enumerate() {
            let response = tokio::time::timeout(Duration::from_secs(10), task)
                .await
                .with_context(|| format!("queue push {index} did not respond in time"))?
                .context("queue push task panicked")?
                .expect("queue push response");
            assert!(
                response.get("error").is_none(),
                "queue push {index} failed: {response}"
            );
        }
        let entries = queue_list(&runtime, connection_id, session_id).await;
        assert_eq!(entries.len(), PUSH_COUNT);

        open.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    // ── Native session/metadata/update (L2-DES-CONV-002 Phase 2) ──

    /// Trace: L2-DES-CONV-002, L2-DES-APP-008
    /// Verifies: the native settings update persists a field-level
    /// settings line and returns the updated native session built from
    /// the rollout (persist-first; no actor wait).
    #[tokio::test]
    async fn native_metadata_update_persists_preset_and_returns_updated_session() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let response = history_request(
            &runtime,
            connection_id,
            7,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 1,
                "settings": { "permissionProfile": "fullAccess" },
            }),
        )
        .await;
        let result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
            serde_json::from_value(response["result"].clone()).expect("native update result");
        assert_eq!(
            result.session.settings.permission_profile,
            devo_protocol::native::model::PermissionProfile::FullAccess
        );
        assert_eq!(result.session.version, 2);

        // Crash-recovery: the field line alone restores the preset.
        let rollout_path = runtime
            .rollout_store
            .find_rollout_by_session_id(&session_id)?
            .expect("rollout exists");
        let history = devo_core::read_canonical_history(&rollout_path)?;
        assert_eq!(
            history
                .session
                .expect("session")
                .settings
                .permission_profile,
            devo_protocol::native::model::PermissionProfile::FullAccess
        );
        Ok(())
    }

    /// Trace: L2-DES-CONV-002, L2-DES-APP-008
    /// Verifies: a stale expectedVersion is rejected with
    /// WORKSPACE_VERSION_CONFLICT and writes nothing.
    #[tokio::test]
    async fn native_metadata_update_rejects_stale_expected_version() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let params = |version| {
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": version,
                "settings": { "permissionProfile": "autoReview" },
            })
        };
        let first = history_request(
            &runtime,
            connection_id,
            7,
            "session/metadata/update",
            params(1),
        )
        .await;
        assert!(first.get("result").is_some(), "first update: {first}");

        let conflict = history_request(
            &runtime,
            connection_id,
            8,
            "session/metadata/update",
            params(1),
        )
        .await;
        assert_eq!(
            conflict["error"]["code"].as_str(),
            Some("WORKSPACE_VERSION_CONFLICT"),
            "stale write must conflict: {conflict}"
        );
        Ok(())
    }

    /// Trace: L2-DES-CONV-002, L2-DES-APP-008
    /// Verifies: the native settings update returns while a turn is in
    /// flight (the persist-first path never waits on the session actor).
    #[tokio::test]
    async fn native_metadata_update_returns_during_active_turn() -> Result<()> {
        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Default::default(),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        let _turn_id = start_turn(&runtime, connection_id, session_id, "hold the turn").await?;

        let update = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            history_request(
                &runtime,
                connection_id,
                7,
                "session/metadata/update",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "expectedVersion": 1,
                    "settings": { "permissionProfile": "fullAccess" },
                }),
            ),
        )
        .await;
        open.store(true, std::sync::atomic::Ordering::SeqCst);
        let response = update.expect("settings update must return while the turn is in flight");
        assert!(
            response.get("result").is_some(),
            "update during active turn: {response}"
        );
        Ok(())
    }

    /// Trace: L2-DES-CONV-002
    /// Verifies: a settings update during an active turn is delivered to the
    /// turn-inline override — permission profile, live sandbox handle — and
    /// implicit approval caches are invalidated (Phase 3).
    #[tokio::test]
    async fn native_metadata_update_applies_overlay_to_active_turn() -> Result<()> {
        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Arc::clone(&started),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        let _turn_id = start_turn(&runtime, connection_id, session_id, "hold the turn").await?;
        // Wait until the provider stream is actually in flight so the
        // turn-inline state is registered before the update arrives.
        let wait_started = async {
            while !started.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), wait_started)
            .await
            .expect("turn should start streaming");

        let response = history_request(
            &runtime,
            connection_id,
            7,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 1,
                "settings": { "permissionProfile": "fullAccess" },
            }),
        )
        .await;
        let result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
            serde_json::from_value(response["result"].clone()).expect("native update result");
        assert!(
            result.applied_to_active_turn,
            "update during an active turn must report overlay delivery"
        );

        let stream = runtime
            .active_stream_state(session_id)
            .await
            .expect("active stream state");
        let stream = stream.lock().await;
        let inline = stream.turn_inline.as_ref().expect("turn inline state");
        assert_eq!(
            inline.hook_context.config.permission_profile.preset,
            devo_safety::PermissionPreset::FullAccess
        );
        let implied = inline
            .hook_context
            .config
            .permission_profile
            .implied_sandbox_profile()
            .to_string();
        assert_eq!(
            inline.hook_context.config.sandbox_profile.as_deref(),
            Some(implied.as_str())
        );
        assert_eq!(
            inline
                .sandbox_profile_live
                .lock()
                .expect("live mutex")
                .as_deref(),
            Some(implied.as_str())
        );
        drop(stream);
        open.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    // ── Native turn/start (L2-DES-APP-008 Phase B) ──

    /// Trace: L2-DES-APP-008
    /// Verifies: native turn/start admits a turn and returns the
    /// native turn snapshot; a busy session rejects with
    /// TURN_ALREADY_RUNNING instead of queueing.
    #[tokio::test]
    async fn native_turn_start_admits_and_busy_rejects() -> Result<()> {
        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Default::default(),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let started = tokio::time::timeout(
            Duration::from_secs(2),
            history_request(
                &runtime,
                connection_id,
                7,
                "turn/start",
                serde_json::json!({
                    "sessionId": session_id.to_string(),
                    "input": [{ "type": "text", "text": "hello" }],
                    "idempotencyKey": "turn-1",
                }),
            ),
        )
        .await
        .context("native turn/start must return before ExecuteTurn finishes")?;
        let result: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(started["result"].clone()).expect("native turn/start result");
        assert_eq!(result.turn.session_id.as_str(), session_id.to_string());
        assert_eq!(
            result.turn.status,
            devo_protocol::native::turn::TurnStatus::InProgress
        );

        let busy = history_request(
            &runtime,
            connection_id,
            8,
            "turn/start",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "second" }],
                "idempotencyKey": "turn-2",
            }),
        )
        .await;
        assert_eq!(
            busy["error"]["code"].as_str(),
            Some("TurnAlreadyRunning"),
            "busy session must reject: {busy}"
        );

        open.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: an idempotent replay of native turn/start returns the
    /// originally started turn without starting a second one.
    #[tokio::test]
    async fn native_turn_start_idempotent_replay_returns_same_turn() -> Result<()> {
        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Default::default(),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let params = serde_json::json!({
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "hello" }],
            "idempotencyKey": "turn-replay",
        });
        let first = tokio::time::timeout(
            Duration::from_secs(2),
            history_request(&runtime, connection_id, 7, "turn/start", params.clone()),
        )
        .await
        .context("native turn/start replay setup must return before ExecuteTurn finishes")?;
        let first: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(first["result"].clone()).expect("first result");
        let replay = history_request(&runtime, connection_id, 8, "turn/start", params).await;
        let replay: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(replay["result"].clone()).expect("replay result");
        assert_eq!(replay.turn, first.turn);

        open.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: unsupported native input variants (remote image, audio,
    /// skill) are rejected with a clear InvalidParams error.
    #[tokio::test]
    async fn native_turn_start_rejects_unsupported_input_variants() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let response = history_request(
            &runtime,
            connection_id,
            7,
            "turn/start",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "image", "uri": "https://example.com/x.png" }],
                "idempotencyKey": "turn-image",
            }),
        )
        .await;
        assert!(
            response["error"]["code"]
                .as_str()
                .is_some_and(|code| { code.contains("InvalidParams") || code == "INVALID_PARAMS" }),
            "unsupported input must be rejected: {response}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/compact/start rejects while a turn is
    /// active, and starts a compaction turn with a native turn snapshot
    /// once the session is idle again.
    #[tokio::test]
    async fn native_session_compact_start_busy_rejects_and_idle_compacts() -> Result<()> {
        let data_root = TempDir::new()?;
        let open = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let runtime = build_runtime_with_provider(
            data_root.path(),
            Arc::new(GatedProvider {
                open: Arc::clone(&open),
                started: Default::default(),
            }),
        );
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        let _turn_id = start_turn(&runtime, connection_id, session_id, "hold the turn").await?;

        let busy = history_request(
            &runtime,
            connection_id,
            7,
            "session/compact/start",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        assert_eq!(
            busy["error"]["code"].as_str(),
            Some("TurnAlreadyRunning"),
            "active turn must reject compaction: {busy}"
        );

        // Interrupting the held turn ends it deterministically; the session
        // is idle afterwards and compaction may start.
        let interrupted = history_request(
            &runtime,
            connection_id,
            8,
            "session/interrupt",
            serde_json::json!({
                "scope": {
                    "scope": "session",
                    "sessionId": session_id.to_string()
                },
            }),
        )
        .await;
        assert!(
            interrupted.get("error").is_none(),
            "interrupt failed: {interrupted}"
        );
        let started = history_request(
            &runtime,
            connection_id,
            9,
            "session/compact/start",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let result: devo_protocol::native::rpc_turn::TurnStartResult =
            serde_json::from_value(started["result"].clone()).expect("native compact/start result");
        assert_eq!(
            result.turn.kind,
            devo_protocol::native::turn::TurnKind::Compaction
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/new creates a durable session with a
    /// native snapshot, an idempotency-key replay returns the same
    /// session, and native session/resume answers with the rollout-backed
    /// snapshot.
    #[tokio::test]
    async fn native_session_new_idempotent_replay_and_resume() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let params = serde_json::json!({
            "cwd": data_root.path(),
            "idempotencyKey": "session-new-1",
        });
        let created =
            history_request(&runtime, connection_id, 7, "session/new", params.clone()).await;
        let created: devo_protocol::native::rpc_session::SessionNewResult =
            serde_json::from_value(created["result"].clone()).expect("native session/new result");
        assert_eq!(created.session.cwd, data_root.path());

        let replay = history_request(&runtime, connection_id, 8, "session/new", params).await;
        let replay: devo_protocol::native::rpc_session::SessionNewResult =
            serde_json::from_value(replay["result"].clone()).expect("replay result");
        assert_eq!(replay.session.id, created.session.id);

        let resumed = history_request(
            &runtime,
            connection_id,
            9,
            "session/resume",
            serde_json::json!({ "sessionId": created.session.id.as_str() }),
        )
        .await;
        let resumed: devo_protocol::native::rpc_session::SessionResumeResult =
            serde_json::from_value(resumed["result"].clone())
                .expect("native session/resume result");
        assert_eq!(resumed.session.id, created.session.id);
        Ok(())
    }

    /// Trace: L2-DES-APP-009
    /// Verifies: a connection that did not declare the native protocol
    /// surface keeps ACP routing for the colliding `session/new` method
    /// (ACP-shaped result), while a native connection gets the native
    /// handler.
    #[tokio::test]
    async fn acp_connection_keeps_acp_routing_for_colliding_method_names() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let acp_connection_id = initialized_acp_connection(&runtime).await;
        let native_connection_id = initialized_native_connection(&runtime).await;

        let acp_params = serde_json::json!({
            "cwd": data_root.path(),
            "mcpServers": [],
        });
        let acp_response =
            history_request(&runtime, acp_connection_id, 7, "session/new", acp_params).await;
        assert!(
            acp_response["result"]["sessionId"].is_string()
                && acp_response["result"]["session"].is_null(),
            "ACP connection must get the ACP session/new shape: {acp_response}"
        );

        let native_params = serde_json::json!({
            "cwd": data_root.path(),
            "idempotencyKey": "session-new-native",
        });
        let native_response = history_request(
            &runtime,
            native_connection_id,
            7,
            "session/new",
            native_params,
        )
        .await;
        assert!(
            native_response["result"]["session"]["id"].is_string(),
            "native connection must get the native session/new shape: {native_response}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/list returns a created session as a
    /// native snapshot, and native session/delete removes it from the
    /// listing.
    #[tokio::test]
    async fn native_session_list_and_delete() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let created = history_request(
            &runtime,
            connection_id,
            7,
            "session/new",
            serde_json::json!({
                "cwd": data_root.path(),
                "idempotencyKey": "session-list-delete",
            }),
        )
        .await;
        let created: devo_protocol::native::rpc_session::SessionNewResult =
            serde_json::from_value(created["result"].clone()).expect("native session/new result");

        let listed = history_request(
            &runtime,
            connection_id,
            8,
            "session/list",
            serde_json::json!({}),
        )
        .await;
        let listed: devo_protocol::native::rpc_session::SessionListResult =
            serde_json::from_value(listed["result"].clone()).expect("native session/list result");
        assert_eq!(listed.data.len(), 1);
        assert_eq!(listed.data[0].id, created.session.id);
        assert_eq!(listed.next_cursor, None);

        let deleted = history_request(
            &runtime,
            connection_id,
            9,
            "session/delete",
            serde_json::json!({ "sessionId": created.session.id.as_str() }),
        )
        .await;
        assert!(
            deleted.get("error").is_none(),
            "session/delete failed: {deleted}"
        );

        let listed_after = history_request(
            &runtime,
            connection_id,
            10,
            "session/list",
            serde_json::json!({}),
        )
        .await;
        let listed_after: devo_protocol::native::rpc_session::SessionListResult =
            serde_json::from_value(listed_after["result"].clone())
                .expect("native session/list result after delete");
        assert!(listed_after.data.is_empty());
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/read returns one session's rollout-backed
    /// native snapshot, and SessionNotFound for an unknown id.
    #[tokio::test]
    async fn native_session_read_returns_snapshot() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let created = history_request(
            &runtime,
            connection_id,
            7,
            "session/new",
            serde_json::json!({
                "cwd": data_root.path(),
                "idempotencyKey": "session-read",
            }),
        )
        .await;
        let created: devo_protocol::native::rpc_session::SessionNewResult =
            serde_json::from_value(created["result"].clone()).expect("native session/new result");

        let read = history_request(
            &runtime,
            connection_id,
            8,
            "session/read",
            serde_json::json!({ "sessionId": created.session.id.as_str() }),
        )
        .await;
        let read: devo_protocol::native::rpc_session::SessionReadResult =
            serde_json::from_value(read["result"].clone()).expect("native session/read result");
        assert_eq!(read.session, created.session);

        let missing = history_request(
            &runtime,
            connection_id,
            9,
            "session/read",
            serde_json::json!({ "sessionId": SessionId::new().to_string() }),
        )
        .await;
        assert_eq!(
            missing["error"]["code"].as_str(),
            Some("SessionNotFound"),
            "unknown session must be SessionNotFound: {missing}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native runtime/ping answers with a server timestamp.
    #[tokio::test]
    async fn native_runtime_ping_returns_server_time() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;
        let pong = history_request(
            &runtime,
            connection_id,
            7,
            "runtime/ping",
            serde_json::json!({}),
        )
        .await;
        let pong: devo_protocol::native::rpc_admin::RuntimePingResult =
            serde_json::from_value(pong["result"].clone()).expect("runtime/ping result");
        assert!(pong.server_time_ms > 0);
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/goal/set creates a goal with a native
    /// snapshot and idempotent replay; session/goal/read returns it;
    /// ifExists=reject on an existing goal fails.
    #[tokio::test]
    async fn native_session_goal_set_and_read() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let params = serde_json::json!({
            "sessionId": session_id.to_string(),
            "objective": "ship the protocol unification",
            "ifExists": "replace",
            "idempotencyKey": "goal-set-1",
        });
        let created = history_request(
            &runtime,
            connection_id,
            7,
            "session/goal/set",
            params.clone(),
        )
        .await;
        let created: devo_protocol::native::rpc_session::SessionGoalSetResult =
            serde_json::from_value(created["result"].clone()).expect("goal/set result");
        assert_eq!(created.goal.objective, "ship the protocol unification");
        assert_eq!(
            created.goal.status,
            devo_protocol::native::goal::GoalStatus::Active
        );

        let replay = history_request(&runtime, connection_id, 8, "session/goal/set", params).await;
        let replay: devo_protocol::native::rpc_session::SessionGoalSetResult =
            serde_json::from_value(replay["result"].clone()).expect("replay result");
        assert_eq!(replay.goal, created.goal);

        let read = history_request(
            &runtime,
            connection_id,
            9,
            "session/goal/read",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let read: devo_protocol::native::rpc_session::SessionGoalReadResult =
            serde_json::from_value(read["result"].clone()).expect("goal/read result");
        assert_eq!(read.goal, Some(created.goal.clone()));

        let rejected = history_request(
            &runtime,
            connection_id,
            10,
            "session/goal/set",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "objective": "a different objective",
                "ifExists": "reject",
                "idempotencyKey": "goal-set-2",
            }),
        )
        .await;
        assert!(
            rejected.get("error").is_some(),
            "ifExists=reject must fail on an existing goal: {rejected}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/fork without atTurnId forks at the
    /// session tip and returns the native snapshot of the forked session.
    #[tokio::test]
    async fn native_session_fork_at_tip_returns_forked_session() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let forked = history_request(
            &runtime,
            connection_id,
            7,
            "session/fork",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let result: devo_protocol::native::rpc_session::SessionForkResult =
            serde_json::from_value(forked["result"].clone()).expect("native fork result");
        assert_ne!(result.session.id.as_str(), session_id.to_string());
        assert!(
            result.session.parent.is_some(),
            "fork must record parentage"
        );

        let missing = history_request(
            &runtime,
            connection_id,
            8,
            "session/fork",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "atTurnId": "turn_00000000-0000-0000-0000-000000000000",
            }),
        )
        .await;
        assert!(
            missing.get("error").is_some(),
            "unknown atTurnId must be rejected: {missing}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: a title patch on native session/metadata/update renames
    /// the session and the returned native snapshot carries the new title
    /// (SessionTitleUpdated line folds into the rollout read).
    #[tokio::test]
    async fn native_metadata_update_title_patch_renames_session() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let renamed = history_request(
            &runtime,
            connection_id,
            7,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 0,
                "title": "renamed session",
            }),
        )
        .await;
        let result: devo_protocol::native::rpc_session::SessionMetadataUpdateResult =
            serde_json::from_value(renamed["result"].clone()).expect("metadata update result");
        assert_eq!(result.session.title.as_deref(), Some("renamed session"));

        let cleared = history_request(
            &runtime,
            connection_id,
            8,
            "session/metadata/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "expectedVersion": 0,
                "title": null,
            }),
        )
        .await;
        assert!(
            cleared.get("error").is_some(),
            "title clearing must be rejected: {cleared}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native task/start kind=process spawns a process with a
    /// server-assigned item id, idempotent replay returns the same id, and
    /// task/interrupt terminates it; kind=agent returns a child task item.
    #[tokio::test]
    async fn native_task_start_process_and_interrupt() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let params = serde_json::json!({
            "kind": "process",
            "sessionId": session_id.to_string(),
            "command": "sleep 60",
            "idempotencyKey": "task-1",
        });
        let started =
            history_request(&runtime, connection_id, 7, "task/start", params.clone()).await;
        let started: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(started["result"].clone()).expect("task/start result");
        assert!(started.item_id.as_str().starts_with("item_"));

        let replay = history_request(&runtime, connection_id, 8, "task/start", params).await;
        let replay: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(replay["result"].clone()).expect("replay result");
        assert_eq!(replay.item_id, started.item_id);

        let interrupted = history_request(
            &runtime,
            connection_id,
            9,
            "session/interrupt",
            serde_json::json!({
                "scope": { "scope": "task", "itemId": started.item_id.as_str() }
            }),
        )
        .await;
        assert!(
            interrupted.get("error").is_none(),
            "session/interrupt failed: {interrupted}"
        );

        let agent = history_request(
            &runtime,
            connection_id,
            10,
            "task/start",
            serde_json::json!({
                "kind": "agent",
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "hi" }],
                "idempotencyKey": "task-2",
            }),
        )
        .await;
        let agent: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(agent["result"].clone()).expect("agent task/start result");
        assert!(agent.item_id.as_str().starts_with("item_"));
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native task/write_stdin feeds a running process and
    /// task/resize is accepted (DD-7 facade verbs share the process lookup).
    #[tokio::test]
    async fn native_task_write_stdin_and_resize() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let started = history_request(
            &runtime,
            connection_id,
            7,
            "task/start",
            serde_json::json!({
                "kind": "process",
                "sessionId": session_id.to_string(),
                "command": "cat",
                "idempotencyKey": "task-io",
            }),
        )
        .await;
        let started: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(started["result"].clone()).expect("task/start result");

        let written = history_request(
            &runtime,
            connection_id,
            8,
            "task/write_stdin",
            serde_json::json!({
                "itemId": started.item_id.as_str(),
                "data": "hello task\n",
            }),
        )
        .await;
        assert!(
            written.get("error").is_none(),
            "task/write_stdin failed: {written}"
        );

        let resized = history_request(
            &runtime,
            connection_id,
            9,
            "task/resize",
            serde_json::json!({
                "itemId": started.item_id.as_str(),
                "rows": 40,
                "cols": 120,
            }),
        )
        .await;
        assert!(
            resized.get("error").is_none(),
            "task/resize failed: {resized}"
        );

        let _ = history_request(
            &runtime,
            connection_id,
            10,
            "task/interrupt",
            serde_json::json!({ "itemId": started.item_id.as_str() }),
        )
        .await;
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: the approval fan-out projects one logical approval request
    /// to both Native and ACP controller surfaces.
    #[tokio::test]
    async fn approval_fanout_mixes_native_and_acp_surfaces() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = SessionId::new();

        let (native_outbound, mut native_receiver) = super::outbound::test_outbound_channel(4);
        let native_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, native_outbound)
            .await;
        runtime
            .handle_acp_initialize(
                native_connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        let (acp_outbound, mut acp_receiver) = super::outbound::test_outbound_channel(4);
        let acp_connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, acp_outbound)
            .await;
        runtime
            .handle_acp_initialize(
                acp_connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                }),
            )
            .await;
        // Controllers = owner ∪ connections with a Session stream selector.
        let native_session_id =
            devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(session_id));
        {
            let mut connections = runtime.connections.lock().await;
            connections
                .get_mut(&acp_connection_id)
                .expect("acp connection")
                .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
                session_id: native_session_id.clone(),
            }];
            connections
                .get_mut(&native_connection_id)
                .expect("native connection")
                .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
                session_id: native_session_id,
            }];
        }

        let acp_params: devo_protocol::AcpRequestPermissionParams =
            serde_json::from_value(serde_json::json!({
                "sessionId": session_id.to_string(),
                "toolCall": { "toolCallId": "call-1" },
                "options": [],
            }))?;
        let runtime_for_request = Arc::clone(&runtime);
        let (request_ready_tx, request_ready_rx) = oneshot::channel();
        let request = tokio::spawn(async move {
            runtime_for_request
                .request_permission_from_controllers(
                    session_id,
                    Some(native_connection_id),
                    super::control_requests::PermissionControllerRequest {
                        acp_params,
                        native_method: "approval/command/request".to_string(),
                        native_params: serde_json::json!({
                            "type": "approval",
                            "approvalId": "call-1",
                            "actionSummary": "run tests",
                            "justification": "",
                        }),
                        ready: request_ready_tx,
                    },
                    CancellationToken::new(),
                )
                .await
        });

        assert_eq!(request_ready_rx.await?, Ok(()));
        let native_request = native_receiver.recv().await.expect("native request");
        assert_eq!(
            native_request["method"].as_str(),
            Some("approval/command/request"),
            "native connection must get the native reverse request: {native_request}"
        );
        assert_eq!(
            native_request["params"]["approvalId"].as_str(),
            Some("call-1")
        );
        let acp_request = acp_receiver.recv().await.expect("ACP request");
        assert_eq!(
            acp_request["method"].as_str(),
            Some("session/request_permission"),
            "ACP connection must get the ACP permission envelope: {acp_request}"
        );

        runtime
            .resolve_client_response(
                native_connection_id,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": native_request["id"].clone(),
                    "result": {
                        "requestId": "call-1",
                        "decision": {
                            "decision": "approved",
                            "scope": "session",
                            "decidedAt": "2026-08-09T00:00:00Z",
                        },
                    },
                }),
            )
            .await;
        let (decision, scope) = request
            .await?
            .map_err(|error| anyhow::anyhow!("permission fan-out failed: {error}"))?;
        assert_eq!(decision, devo_protocol::ApprovalDecisionValue::Approve);
        assert_eq!(scope, devo_protocol::ApprovalScopeValue::Session);
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: a tool's user-input question fans out to native-surface
    /// controllers as `userInput/request` with the waiting-state payload, and
    /// the native answer resolves the pending tool call (DD-8).
    #[tokio::test]
    async fn native_user_input_request_resolves_pending_question() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (outbound, mut receiver) = super::outbound::test_outbound_channel(8);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound)
            .await;
        runtime
            .handle_acp_initialize(
                connection_id,
                Some(serde_json::json!(1)),
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": { "terminal": false },
                    "_meta": { "devo": { "protocol": "native" } },
                }),
            )
            .await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;
        {
            let mut connections = runtime.connections.lock().await;
            connections
                .get_mut(&connection_id)
                .expect("native connection")
                .event_selectors = vec![devo_protocol::native::event::StreamSelector::Session {
                session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(
                    session_id,
                )),
            }];
        }

        let turn_id = TurnId::new();
        let runtime_for_tool = Arc::clone(&runtime);
        let tool_call = tokio::spawn(async move {
            runtime_for_tool
                .request_user_input_for_tool(
                    session_id,
                    turn_id,
                    "question-call-1".to_string(),
                    devo_protocol::RequestUserInputArgs {
                        questions: vec![
                            serde_json::from_value(serde_json::json!({
                                "id": "q1",
                                "header": "Pick one",
                                "question": "Which color?",
                                "isOther": false,
                                "isSecret": false,
                            }))
                            .expect("question"),
                        ],
                    },
                )
                .await
        });

        // Session setup emits its own notifications first; scan forward to
        // the reverse request frame.
        let request = {
            let mut request = None;
            for _ in 0..8 {
                let frame = receiver.recv().await.expect("userInput/request frame");
                if frame["method"].as_str() == Some("userInput/request") {
                    request = Some(frame);
                    break;
                }
            }
            request.expect("userInput/request must be fanned out")
        };
        assert_eq!(request["method"].as_str(), Some("userInput/request"));
        assert_eq!(
            request["params"]["requestId"].as_str(),
            Some("question-call-1")
        );

        runtime
            .resolve_client_response(
                connection_id,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"].clone(),
                    "result": {
                        "requestId": "question-call-1",
                        "answers": { "q1": { "answers": ["blue"] } },
                    },
                }),
            )
            .await;
        let response = tool_call
            .await?
            .map_err(|error| anyhow::anyhow!("user input tool call failed: {error}"))?;
        assert_eq!(
            response.answers.get("q1"),
            Some(&devo_protocol::RequestUserInputAnswer {
                answers: vec!["blue".to_string()],
            })
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native search/start answers with the camelCase native
    /// snapshot on a native-surface connection (connection-local search;
    /// params are wire-identical to legacy, results project by surface).
    #[tokio::test]
    async fn native_search_start_returns_camel_case_snapshot() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let started = history_request(
            &runtime,
            connection_id,
            7,
            "search/start",
            serde_json::json!({
                "cwd": data_root.path(),
                "query": "nothing-matches-this-query",
            }),
        )
        .await;
        let result: devo_protocol::native::rpc_search::SearchStartResult =
            serde_json::from_value(started["result"].clone()).expect("native search/start result");
        assert_eq!(result.snapshot.query, "nothing-matches-this-query");
        assert!(
            started["result"]["snapshot"]["searchId"].is_string()
                && started["result"]["snapshot"]["fileSearchComplete"].is_boolean(),
            "native snapshot must use camelCase keys: {started}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native workspace/changes/read answers with camelCase
    /// native views on a native-surface connection (the desktop
    /// client's diff read model).
    #[tokio::test]
    async fn native_workspace_changes_read_returns_camel_case_views() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let read = history_request(
            &runtime,
            connection_id,
            7,
            "workspace/changes/read",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "scopes": ["uncommitted"],
            }),
        )
        .await;
        let result: devo_protocol::native::rpc_workspace::WorkspaceChangesReadResult =
            serde_json::from_value(read["result"].clone())
                .expect("native workspace/changes/read result");
        assert_eq!(result.views.len(), 1);
        assert_eq!(
            result.views[0].scope,
            devo_protocol::WorkspaceChangeScope::Uncommitted
        );
        assert!(
            read["result"]["views"][0]["workspaceRoot"].is_string()
                && read["result"]["views"][0]["changeSetStatus"].is_string()
                && read["result"]["views"][0]["generatedAt"].is_string(),
            "native views must use camelCase keys: {read}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native model/list answers with the parity ModelInfo
    /// shape (slug, provider wire API, context window, reasoning capability)
    /// from the same catalog source as model/catalog (ratified #7).
    #[tokio::test]
    async fn native_model_list_returns_parity_entries() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime_with_provider_and_catalog(
            data_root.path(),
            Arc::new(NoopProvider),
            Arc::new(PresetModelCatalog::load().expect("embedded model catalog")),
        );
        let connection_id = initialized_native_connection(&runtime).await;
        let listed = history_request(
            &runtime,
            connection_id,
            7,
            "model/list",
            serde_json::json!({}),
        )
        .await;
        let listed: devo_protocol::native::rpc_admin::ModelListResult =
            serde_json::from_value(listed["result"].clone()).expect("native model/list result");
        assert!(
            !listed.models.is_empty(),
            "test catalog must list at least one model"
        );
        let model = &listed.models[0];
        assert!(!model.slug.is_empty());
        assert!(!model.display_name.is_empty());
        assert!(model.context_window > 0);
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native model/preferences read returns the workspace's
    /// effective defaults with selectable values, and write patches them
    /// with validation (ratified #12).
    #[tokio::test]
    async fn native_model_preferences_read_write_round_trip() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime_with_provider_and_catalog(
            data_root.path(),
            Arc::new(NoopProvider),
            Arc::new(PresetModelCatalog::load().expect("embedded model catalog")),
        );
        let connection_id = initialized_native_connection(&runtime).await;

        let read = history_request(
            &runtime,
            connection_id,
            7,
            "model/preferences/read",
            serde_json::json!({ "cwd": data_root.path() }),
        )
        .await;
        let read: devo_protocol::native::rpc_admin::ModelPreferencesReadResult =
            serde_json::from_value(read["result"].clone())
                .expect("native model/preferences/read result");
        assert!(
            !read.preferences.available_models.is_empty(),
            "embedded catalog must offer models"
        );
        assert!(
            !read.preferences.available_efforts.is_empty(),
            "reasoning effort levels must be offered"
        );

        let slug = read.preferences.available_models[0].value.clone();
        let written = history_request(
            &runtime,
            connection_id,
            8,
            "model/preferences/write",
            serde_json::json!({
                "cwd": data_root.path(),
                "patch": { "reasoningEffort": "high" },
            }),
        )
        .await;
        let written: devo_protocol::native::rpc_admin::ModelPreferencesWriteResult =
            serde_json::from_value(written["result"].clone())
                .expect("native model/preferences/write result");
        assert_eq!(
            written.preferences.reasoning_effort.as_deref(),
            Some("high")
        );

        // The user config store keys the default model by provider binding
        // id, so a bare slug is rejected with the legacy store semantics.
        let slug_write = history_request(
            &runtime,
            connection_id,
            9,
            "model/preferences/write",
            serde_json::json!({
                "cwd": data_root.path(),
                "patch": { "model": slug },
            }),
        )
        .await;
        assert!(
            slug_write["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("does not exist")),
            "bare slug without bindings must surface the store error: {slug_write}"
        );

        let rejected = history_request(
            &runtime,
            connection_id,
            10,
            "model/preferences/write",
            serde_json::json!({
                "cwd": data_root.path(),
                "patch": { "model": "no-such-model" },
            }),
        )
        .await;
        assert!(
            rejected.get("error").is_some(),
            "unknown model must be rejected: {rejected}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native provider/list answers with camelCase vendor
    /// entries and native provider/upsert (dual-shape via providerVendor)
    /// writes through the legacy store path (ratified #11).
    #[tokio::test]
    async fn native_provider_list_and_upsert_round_trip() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let listed = history_request(
            &runtime,
            connection_id,
            7,
            "provider/list",
            serde_json::json!({}),
        )
        .await;
        let listed: devo_protocol::native::rpc_admin::ProviderListResult =
            serde_json::from_value(listed["result"].clone()).expect("native provider/list result");
        assert!(listed.providers.is_empty());

        let upserted = history_request(
            &runtime,
            connection_id,
            8,
            "provider/upsert",
            serde_json::json!({
                "providerVendor": {
                    "name": "test-provider",
                    "baseUrl": "https://example.com/v1",
                    "wireApis": ["openai_chat_completions"],
                    "enabled": true,
                },
            }),
        )
        .await;
        assert!(
            upserted.get("error").is_none(),
            "native provider/upsert failed: {upserted}"
        );

        let listed = history_request(
            &runtime,
            connection_id,
            9,
            "provider/list",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            listed["result"]["providers"][0]["name"].as_str(),
            Some("test-provider")
        );
        assert_eq!(
            listed["result"]["providers"][0]["baseUrl"].as_str(),
            Some("https://example.com/v1"),
            "native vendors must use camelCase keys: {listed}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native skill/list answers with native SkillInfo
    /// records (path-keyed, workspace-scoped via cwd), and native
    /// skill/set_enabled surfaces the store error for unknown paths
    /// (ratified #4).
    #[tokio::test]
    async fn native_skill_list_and_set_enabled() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;

        let listed = history_request(
            &runtime,
            connection_id,
            7,
            "skill/list",
            serde_json::json!({ "cwd": data_root.path() }),
        )
        .await;
        assert!(
            listed.get("error").is_none(),
            "native skill/list must succeed: {listed}"
        );
        let listed: devo_protocol::native::rpc_admin::SkillListResult =
            serde_json::from_value(listed["result"].clone()).expect("native skill/list result");
        for skill in &listed.skills {
            assert!(!skill.id.is_empty() && !skill.name.is_empty());
        }

        // Legacy store semantics: an unknown path is a no-op that returns the
        // current list unchanged (no error).
        let noop = history_request(
            &runtime,
            connection_id,
            8,
            "skill/set_enabled",
            serde_json::json!({
                "path": data_root.path().join("no-such-skill/SKILL.md"),
                "enabled": true,
                "cwd": data_root.path(),
            }),
        )
        .await;
        let noop: devo_protocol::native::rpc_admin::SkillSetEnabledResult =
            serde_json::from_value(noop["result"].clone())
                .expect("native skill/set_enabled result");
        assert_eq!(noop.skills.len(), listed.skills.len());
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/goal/update edits the current goal in
    /// place (same goal id, updated objective/status/budget), replays by
    /// idempotency key, rejects system-computed statuses, and fails with
    /// GoalNotFound without an active goal (ratified #3).
    #[tokio::test]
    async fn native_session_goal_update_in_place() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let created = history_request(
            &runtime,
            connection_id,
            7,
            "session/goal/set",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "objective": "original objective",
                "ifExists": "reject",
                "idempotencyKey": "goal-update-seed",
            }),
        )
        .await;
        let created: devo_protocol::native::rpc_session::SessionGoalSetResult =
            serde_json::from_value(created["result"].clone()).expect("goal/set result");

        let update_params = serde_json::json!({
            "sessionId": session_id.to_string(),
            "expectedGoalId": created.goal.id.as_str(),
            "patch": {
                "objective": "edited objective",
                "status": "paused",
                "tokenBudget": 5000,
            },
            "idempotencyKey": "goal-update-1",
        });
        let updated = history_request(
            &runtime,
            connection_id,
            8,
            "session/goal/update",
            update_params.clone(),
        )
        .await;
        let updated: devo_protocol::native::rpc_session::SessionGoalUpdateResult =
            serde_json::from_value(updated["result"].clone()).expect("goal/update result");
        assert_eq!(
            updated.goal.id, created.goal.id,
            "edit preserves the goal id"
        );
        assert_eq!(updated.goal.objective, "edited objective");
        assert_eq!(
            updated.goal.status,
            devo_protocol::native::goal::GoalStatus::Paused
        );

        let replay = history_request(
            &runtime,
            connection_id,
            9,
            "session/goal/update",
            update_params,
        )
        .await;
        let replay: devo_protocol::native::rpc_session::SessionGoalUpdateResult =
            serde_json::from_value(replay["result"].clone()).expect("replay result");
        assert_eq!(replay.goal, updated.goal);

        let system_status = history_request(
            &runtime,
            connection_id,
            10,
            "session/goal/update",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "patch": { "status": "budgetLimited" },
                "idempotencyKey": "goal-update-2",
            }),
        )
        .await;
        assert!(
            system_status.get("error").is_some(),
            "system-computed status must be rejected: {system_status}"
        );

        let no_goal_session =
            start_durable_session(&runtime, connection_id, &data_root.path().join("empty")).await?;
        let no_goal = history_request(
            &runtime,
            connection_id,
            11,
            "session/goal/update",
            serde_json::json!({
                "sessionId": no_goal_session.to_string(),
                "patch": { "objective": "x" },
                "idempotencyKey": "goal-update-3",
            }),
        )
        .await;
        assert_eq!(
            no_goal["error"]["code"].as_str(),
            Some("GoalNotFound"),
            "update without an active goal must fail GoalNotFound: {no_goal}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native session/message/edit supersedes the previous user
    /// message with a new revision, starts a replacement turn, and enforces
    /// expectedRevision against the native history (ratified #10).
    #[tokio::test]
    async fn native_session_message_edit_supersedes_previous_message() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_native_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let _turn = history_request(
            &runtime,
            connection_id,
            7,
            "turn/start",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "input": [{ "type": "text", "text": "original message" }],
                "idempotencyKey": "edit-target-turn",
            }),
        )
        .await;
        // The noop provider fails the turn immediately; wait for terminal so
        // the edit is not rejected as active-turn.
        for _ in 0..100 {
            if runtime.runtime_active_turn_id(session_id).await.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(runtime.runtime_active_turn_id(session_id).await.is_none());

        let items = history_request(
            &runtime,
            connection_id,
            8,
            "session/items/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let items: devo_protocol::native::page::Page<devo_protocol::native::item::ItemEnvelope> =
            serde_json::from_value(items["result"].clone()).expect("items/list result");
        let user_item = items
            .data
            .iter()
            .find(|item| {
                matches!(
                    &item.item,
                    devo_protocol::native::item::Item::UserMessage { .. }
                )
            })
            .expect("user message item");
        let expected_revision = user_item.revision;

        let edited = history_request(
            &runtime,
            connection_id,
            9,
            "session/message/edit",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "itemId": user_item.id.as_str(),
                "expectedRevision": expected_revision,
                "content": [{ "type": "text", "text": "edited message" }],
                "idempotencyKey": "edit-1",
            }),
        )
        .await;
        let edited: devo_protocol::native::rpc_session::SessionMessageEditResult =
            serde_json::from_value(edited["result"].clone())
                .expect("native session/message/edit result");
        assert_eq!(edited.item.revision, expected_revision + 1);
        assert_eq!(
            edited.edit_state,
            devo_protocol::native::rpc_session::MessageEditState::Accepted
        );
        assert!(edited.replacement_turn_id.is_some());
        match &edited.item.item {
            devo_protocol::native::item::Item::UserMessage { content, .. } => {
                assert!(
                    matches!(&content[0], devo_protocol::native::item::UserInput::Text { text } if text == "edited message")
                );
            }
            other => panic!("edited item must be a UserMessage: {other:?}"),
        }

        let stale = history_request(
            &runtime,
            connection_id,
            10,
            "session/message/edit",
            serde_json::json!({
                "sessionId": session_id.to_string(),
                "itemId": user_item.id.as_str(),
                "expectedRevision": 999,
                "content": [{ "type": "text", "text": "stale edit" }],
                "idempotencyKey": "edit-2",
            }),
        )
        .await;
        assert_eq!(
            stale["error"]["code"].as_str(),
            Some("WORKSPACE_VERSION_CONFLICT"),
            "stale revision must conflict: {stale}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native task/read returns a process task's BackgroundTask
    /// item with exit code and captured output tail once the process exits,
    /// and native task/list includes the session's process task.
    #[tokio::test]
    async fn native_task_read_and_list() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let started = history_request(
            &runtime,
            connection_id,
            7,
            "task/start",
            serde_json::json!({
                "kind": "process",
                "sessionId": session_id.to_string(),
                "command": "echo native-task-read",
                "idempotencyKey": "task-read",
            }),
        )
        .await;
        let started: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(started["result"].clone()).expect("task/start result");

        // The echo process exits quickly; poll task/read until the terminal
        // snapshot (exit code + output tail) is retained.
        let mut terminal = None;
        for _ in 0..100 {
            let read = history_request(
                &runtime,
                connection_id,
                8,
                "task/read",
                serde_json::json!({ "itemId": started.item_id.as_str() }),
            )
            .await;
            let result: devo_protocol::native::rpc_turn::TaskReadResult =
                serde_json::from_value(read["result"].clone()).expect("native task/read result");
            let devo_protocol::native::item::Item::BackgroundTask {
                exit_code, state, ..
            } = &result.item.item
            else {
                panic!(
                    "process task must project as BackgroundTask: {:?}",
                    result.item.item
                );
            };
            if let Some(exit_code) = exit_code {
                terminal = Some((*exit_code, *state, result.output_tail.clone()));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let (exit_code, state, output_tail) =
            terminal.expect("process task must reach a terminal snapshot");
        assert_eq!(exit_code, 0);
        assert_eq!(
            state,
            devo_protocol::native::item::SpawnedWorkState::Completed
        );
        assert!(
            output_tail
                .as_deref()
                .unwrap_or_default()
                .contains("native-task-read"),
            "output tail must capture the echo: {output_tail:?}"
        );

        let listed = history_request(
            &runtime,
            connection_id,
            9,
            "task/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let listed: devo_protocol::native::rpc_turn::TaskListResult =
            serde_json::from_value(listed["result"].clone()).expect("native task/list result");
        assert!(
            listed.tasks.iter().any(|task| task.id == started.item_id),
            "task/list must include the process task: {:?}",
            listed
                .tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native agent/list answers with the SubAgent item result
    /// shape for a session (empty when no agents), and requires a session id.
    #[tokio::test]
    async fn native_agent_list_returns_subagent_item_result() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let listed = history_request(
            &runtime,
            connection_id,
            7,
            "agent/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let result: devo_protocol::native::rpc_turn::AgentListResult =
            serde_json::from_value(listed["result"].clone()).expect("native agent/list result");
        assert!(result.agents.is_empty());

        let missing = history_request(
            &runtime,
            connection_id,
            8,
            "agent/list",
            serde_json::json!({}),
        )
        .await;
        assert!(
            missing.get("error").is_some(),
            "session-less native agent/list must be rejected: {missing}"
        );
        Ok(())
    }

    /// Trace: L2-DES-APP-008
    /// Verifies: native task/start kind=agent spawns a child-session
    /// agent, returns the child as an `item_<uuid>` id, and honors the
    /// spawn policies (DD-7).
    #[tokio::test]
    async fn native_task_start_agent_spawns_child_session() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let connection_id = initialized_connection(&runtime).await;
        let session_id = start_durable_session(&runtime, connection_id, data_root.path()).await?;

        let params = serde_json::json!({
            "kind": "agent",
            "sessionId": session_id.to_string(),
            "input": [{ "type": "text", "text": "quick question" }],
            "forkTurns": "all",
            "maxTurns": 1,
            "toolPolicy": "deny_all",
            "ephemeral": true,
            "idempotencyKey": "agent-task-1",
        });
        let started =
            history_request(&runtime, connection_id, 7, "task/start", params.clone()).await;
        let started: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(started["result"].clone())
                .expect("task/start kind=agent result");
        assert!(
            started.item_id.as_str().starts_with("item_"),
            "agent item id must be item_-prefixed: {}",
            started.item_id.as_str()
        );

        let replay = history_request(&runtime, connection_id, 8, "task/start", params).await;
        let replay: devo_protocol::native::rpc_turn::TaskStartResult =
            serde_json::from_value(replay["result"].clone()).expect("replay result");
        assert_eq!(replay.item_id, started.item_id);

        // The spawned child is visible through native agent/list.
        let listed = history_request(
            &runtime,
            connection_id,
            9,
            "agent/list",
            serde_json::json!({ "sessionId": session_id.to_string() }),
        )
        .await;
        let listed: devo_protocol::native::rpc_turn::AgentListResult =
            serde_json::from_value(listed["result"].clone()).expect("agent/list result");
        assert_eq!(listed.agents.len(), 1, "spawned agent must be listed");
        Ok(())
    }
}
