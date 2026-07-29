use super::*;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::ACP_AUTHENTICATE_METHOD;
use crate::ACP_INITIALIZE_METHOD;
use crate::ACP_LOGOUT_METHOD;
use crate::ACP_SESSION_CANCEL_METHOD;
use crate::ACP_SESSION_CLOSE_METHOD;
use crate::ACP_SESSION_DELETE_METHOD;
use crate::ACP_SESSION_LIST_METHOD;
use crate::ACP_SESSION_LOAD_METHOD;
use crate::ACP_SESSION_NEW_METHOD;
use crate::ACP_SESSION_PROMPT_METHOD;
use crate::ACP_SESSION_RESUME_METHOD;
use crate::ACP_SESSION_SET_CONFIG_OPTION_METHOD;
use crate::ACP_SESSION_SET_MODE_METHOD;
use crate::acp_auth_required_response;
use crate::acp_notification_from_server_event;
use crate::devo_extension_inner_method;
use devo_protocol::canonical::wire_projector::typed_item_notification_from_server_event;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum AcpRoute {
    CancelSession,
    CloseSession,
    DeleteSession,
    ListSession,
    LoadSession,
    NewSession,
    PromptSession,
    ResumeSession,
    SetConfigOptionSession,
    SetModeSession,
}

fn acp_route_registry() -> &'static BTreeMap<&'static str, AcpRoute> {
    static ACP_ROUTES: OnceLock<BTreeMap<&'static str, AcpRoute>> = OnceLock::new();
    ACP_ROUTES.get_or_init(|| {
        BTreeMap::from([
            (ACP_SESSION_CANCEL_METHOD, AcpRoute::CancelSession),
            (ACP_SESSION_CLOSE_METHOD, AcpRoute::CloseSession),
            (ACP_SESSION_DELETE_METHOD, AcpRoute::DeleteSession),
            (ACP_SESSION_LIST_METHOD, AcpRoute::ListSession),
            (ACP_SESSION_LOAD_METHOD, AcpRoute::LoadSession),
            (ACP_SESSION_NEW_METHOD, AcpRoute::NewSession),
            (ACP_SESSION_PROMPT_METHOD, AcpRoute::PromptSession),
            (ACP_SESSION_RESUME_METHOD, AcpRoute::ResumeSession),
            (
                ACP_SESSION_SET_CONFIG_OPTION_METHOD,
                AcpRoute::SetConfigOptionSession,
            ),
            (ACP_SESSION_SET_MODE_METHOD, AcpRoute::SetModeSession),
        ])
    })
}

fn acp_route(method: &str) -> Option<AcpRoute> {
    acp_route_registry().get(method).copied()
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
                "dropping unauthenticated client notification"
            );
            return None;
        }

        if let Some(route) = acp_route(&method) {
            return self
                .handle_acp_route(route, connection_id, id, params)
                .await;
        }

        let client_method = ClientMethod::parse(&method)
            .or_else(|| devo_extension_inner_method(&method).and_then(ClientMethod::parse));
        let response = match client_method {
            None if method == "session/start" => {
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
            Some(ClientMethod::SessionMetadataUpdate) => {
                Some(self.handle_session_metadata_update(id?, params).await)
            }
            // update session's permission mode, including auto-approve, default, full-access, readonly
            Some(ClientMethod::SessionPermissionsUpdate) => {
                Some(self.handle_session_permissions_update(id?, params).await)
            }
            // update the session's sandbox profile for spawned commands
            Some(ClientMethod::SessionSandboxProfileUpdate) => Some(
                self.handle_session_sandbox_profile_update(id?, params)
                    .await,
            ),
            // update session title, user may customized session title from ui client
            Some(ClientMethod::SessionTitleUpdate) => {
                Some(self.handle_session_title_update(id?, params).await)
            }
            // resume a history session, server load the jsonl file then replay the events in jsonl
            Some(ClientMethod::SessionResume) => {
                Some(self.handle_session_resume(connection_id, id?, params).await)
            }
            // fork a given session at given user turn index
            Some(ClientMethod::SessionFork) => {
                Some(self.handle_session_fork(connection_id, id?, params).await)
            }
            // rollback session at given point
            Some(ClientMethod::SessionRollback) => Some(
                self.handle_session_rollback(connection_id, id?, params)
                    .await,
            ),
            // compact session context history
            Some(ClientMethod::SessionCompact) => {
                Some(self.handle_session_compact(id?, params).await)
            }
            // list the current skills, including given cwd param
            Some(ClientMethod::SkillsList) => Some(self.handle_skills_list(id?, params).await),
            // TODO: not sure what is the endpoint
            Some(ClientMethod::SkillsChanged) => {
                Some(self.handle_skills_changed(id?, params).await)
            }
            Some(ClientMethod::SkillsSetEnabled) => {
                Some(self.handle_skills_set_enabled(id?, params).await)
            }
            // get the model catalog, aka the configured models list
            Some(ClientMethod::ModelCatalog) => Some(self.handle_model_catalog(id?, params).await),
            Some(ClientMethod::ModelConfig) => Some(self.handle_model_config(id?, params).await),
            Some(ClientMethod::ModelConfigSet) => {
                Some(self.handle_model_config_set(id?, params).await)
            }
            // TODO: not sure, config model from client should be deprecated
            Some(ClientMethod::ModelSaved) => Some(self.handle_model_saved(id?, params).await),
            Some(ClientMethod::CommandExec) => {
                Some(self.handle_command_exec(connection_id, id?, params).await)
            }
            Some(ClientMethod::CommandExecWrite) => Some(
                self.handle_command_exec_write(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::CommandExecResize) => Some(
                self.handle_command_exec_resize(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::CommandExecTerminate) => Some(
                self.handle_command_exec_terminate(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::MessageEditPrevious) => {
                Some(self.handle_message_edit_previous(id?, params).await)
            }
            // TODO: start a new user turn, maybe should change name to "turn/submit"
            Some(ClientMethod::TurnStart) => Some(
                self.handle_turn_start_for_connection(Some(connection_id), id?, params)
                    .await,
            ),
            Some(ClientMethod::TurnShellCommand) => Some(
                self.handle_turn_shell_command_for_connection(Some(connection_id), id?, params)
                    .await,
            ),
            // interupt the current working turn
            Some(ClientMethod::TurnInterrupt) => {
                Some(self.handle_turn_interrupt(id?, params).await)
            }
            Some(ClientMethod::TurnSteer) => {
                Some(self.handle_turn_steer(connection_id, id?, params).await)
            }
            Some(ClientMethod::TurnQueueRemove) => {
                Some(self.handle_turn_queue_remove(id?, params).await)
            }
            Some(ClientMethod::TurnQueueSteer) => Some(
                self.handle_turn_queue_steer(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::WorkspaceChangesRead) => {
                Some(self.handle_workspace_changes_read(id?, params).await)
            }
            Some(ClientMethod::RequestUserInputRespond) => {
                Some(self.handle_request_user_input_respond(id?, params).await)
            }
            Some(ClientMethod::SearchStart) => Some(
                self.handle_reference_search_start(connection_id, id?, params)
                    .await,
            ),
            Some(ClientMethod::SearchUpdate) => {
                Some(self.handle_reference_search_update(id?, params).await)
            }
            Some(ClientMethod::SearchCancel) => {
                Some(self.handle_reference_search_cancel(id?, params).await)
            }
            Some(ClientMethod::EventsSubscribe) => Some(
                self.handle_events_subscribe(connection_id, id?, params)
                    .await,
            ),
            // TODO: the goal design should be simplified
            Some(ClientMethod::GoalCreate) => Some(self.handle_goal_create(id?, params).await),
            Some(ClientMethod::GoalSet) => Some(self.handle_goal_set(id?, params).await),
            Some(ClientMethod::GoalPause) => Some(self.handle_goal_pause(id?, params).await),
            Some(ClientMethod::GoalResume) => Some(self.handle_goal_resume(id?, params).await),
            Some(ClientMethod::GoalComplete) => Some(self.handle_goal_complete(id?, params).await),
            // cancel the current goal loop
            Some(ClientMethod::GoalCancel) => Some(self.handle_goal_cancel(id?, params).await),
            Some(ClientMethod::GoalClear) => Some(self.handle_goal_clear(id?, params).await),
            Some(ClientMethod::GoalStatus) => Some(self.handle_goal_status(id?, params).await),
            Some(ClientMethod::AgentSpawn) => Some(self.handle_agent_spawn(id?, params).await),
            Some(ClientMethod::AgentSendMessage) => {
                Some(self.handle_agent_send_message(id?, params).await)
            }
            Some(ClientMethod::AgentWait) => Some(self.handle_agent_wait(id?, params).await),
            // TODO: list the current sub agents, not sure whther the current agent is right.
            Some(ClientMethod::AgentList) => Some(self.handle_agent_list(id?, params).await),
            // TODO: get the agent status, it is the subagent session status, maybe the design is not right, wait for reviewing.
            Some(ClientMethod::AgentStatus) => Some(self.handle_agent_status(id?, params).await),
            Some(ClientMethod::AgentClose) => Some(self.handle_agent_close(id?, params).await),
            // TODO: list the current provider vender list
            Some(ClientMethod::ProviderVendorList) => {
                Some(self.handle_provider_vendor_list(id?, params).await)
            }
            Some(ClientMethod::ProviderValidate) => {
                Some(self.handle_provider_validate(id?, params).await)
            }
            // TODO: update / add provider vendor to the provider vendor list
            Some(ClientMethod::ProviderVendorUpsert) => {
                Some(self.handle_provider_vendor_upsert(id?, params).await)
            }
            // Paged history reads of the new Native API (canonical types).
            Some(ClientMethod::SessionTurnsList) => {
                Some(self.handle_session_turns_list(id?, params).await)
            }
            Some(ClientMethod::SessionItemsList) => {
                Some(self.handle_session_items_list(id?, params).await)
            }
            // Durable event subscriptions (08 §4).
            Some(ClientMethod::SubscriptionCreate) => {
                Some(self.handle_subscription_create(connection_id, id?, params).await)
            }
            Some(ClientMethod::SubscriptionUpdate) => {
                Some(self.handle_subscription_update(connection_id, id?, params).await)
            }
            Some(ClientMethod::SubscriptionAck) => {
                Some(self.handle_subscription_ack(connection_id, id?, params).await)
            }
            Some(ClientMethod::SubscriptionUnsubscribe) => {
                Some(self.handle_subscription_unsubscribe(connection_id, id?, params).await)
            }
            // TODO: add endpoint to kill background process opened by unified exec command.
            // TODO: add endpoint to list current background processes.
            None => Some(self.error_response(
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
            AcpRoute::CancelSession => {
                self.handle_acp_session_cancel(params).await;
                Some(IncomingResponse::new(crate::acp_success_response(
                    request_id?,
                    crate::AcpEmptyResult::default(),
                )))
            }
            AcpRoute::CloseSession => Some(IncomingResponse::new(
                self.handle_acp_session_close(request_id?, params).await,
            )),
            AcpRoute::DeleteSession => Some(IncomingResponse::new(
                self.handle_acp_session_delete(request_id?, params).await,
            )),
            AcpRoute::ListSession => Some(IncomingResponse::new(
                self.handle_acp_session_list(request_id?, params).await,
            )),
            AcpRoute::LoadSession => {
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
            AcpRoute::NewSession => {
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
            AcpRoute::PromptSession => self
                .handle_acp_session_prompt(connection_id, request_id?, params)
                .await
                .map(IncomingResponse::new),
            AcpRoute::ResumeSession => {
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
            AcpRoute::SetConfigOptionSession => Some(IncomingResponse::new(
                self.handle_acp_session_set_config_option(request_id?, params)
                    .await,
            )),
            AcpRoute::SetModeSession => Some(IncomingResponse::new(
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
            let event_seq = connection.next_seq();
            let event = event.clone().with_seq(event_seq);
            let (method, value) = acp_notification_from_server_event(method, &event);
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
            let event_seq = connection.next_seq();
            let event = event.with_seq(event_seq);
            let (method, value) = acp_notification_from_server_event(method, &event);
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

    pub(super) async fn send_request_to_connection_cancellable(
        &self,
        connection_id: u64,
        method: &str,
        params: serde_json::Value,
        cancel_token: CancellationToken,
    ) -> Result<serde_json::Value, String> {
        self.send_request_to_connection_inner(
            connection_id,
            method,
            params,
            /*timeout_duration*/ None,
            cancel_token,
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
        | ServerEvent::WorkspaceChangesUpdated(_)
        | ServerEvent::ReferenceSearchUpdated(_)
        | ServerEvent::CommandExecOutputDelta(_) => OutboundDeliveryPolicy::BestEffort,
        ServerEvent::SessionStarted(_)
        | ServerEvent::SessionTitleUpdated(_)
        | ServerEvent::SessionCompactionStarted(_)
        | ServerEvent::SessionCompactionCompleted(_)
        | ServerEvent::SessionCompactionFailed(_)
        | ServerEvent::SessionStatusChanged(_)
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
        | ServerEvent::InputQueueUpdated(_)
        | ServerEvent::SteerAccepted(_)
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
    pub(crate) acp_authenticated: bool,
    pub(crate) acp_client_capabilities: crate::AcpClientCapabilities,
    /// Whether the client opted in to native typed `item/*` notifications
    /// via `_meta.devo.typedItems` on ACP initialize (P2).
    pub(crate) typed_items: bool,
    /// Cached union of this connection's new-style (`subscription/*`)
    /// selector sets; rebuilt on every create/update/unsubscribe. Delivery
    /// reads only this cache (the registry is authoritative for ack state).
    pub(crate) event_selectors: Vec<devo_protocol::canonical::event::StreamSelector>,
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
    /// Connection-local notifications bypass session subscription filters.
    pub(super) fn should_deliver_connection_local(&self, method: &str) -> bool {
        !self.opt_out_notification_methods.contains(method)
    }

    /// Routes one server event to its wire notification for this connection:
    /// native typed `item/*` when the connection opted in and the payload
    /// projects, otherwise the legacy ACP-wrapped shape (P2 fallback).
    pub(super) fn notification_for(&self, method: &str, event: &ServerEvent) -> (String, serde_json::Value) {
        if self.typed_items
            && let Some(typed) = typed_item_notification_from_server_event(event)
        {
            return typed;
        }
        acp_notification_from_server_event(method, event)
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
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(NoopProvider);
        let db = Arc::new(
            crate::db::Database::open(data_root.join("connection.db")).expect("open test database"),
        );
        ServerRuntime::new(
            data_root.to_path_buf(),
            ServerRuntimeDependencies::new(
                Arc::clone(&provider),
                Arc::new(SingleProviderRouter::new(provider)),
                Arc::new(ToolRegistry::new()),
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
                    AppConfigStore::load(data_root.to_path_buf(), None)
                        .expect("load app config store"),
                )),
            ),
        )
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
    async fn timed_out_client_request_removes_pending_request() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (outbound_tx, mut receiver) = super::outbound::test_outbound_channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound_tx)
            .await;

        let result = runtime
            .send_request_to_connection_with_timeout(
                connection_id,
                "fs/read_text_file",
                serde_json::json!({}),
                Duration::from_millis(1),
                CancellationToken::new(),
            )
            .await;

        let request = receiver.recv().await.expect("client request");
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "fs/read_text_file",
                "params": {},
            })
        );
        assert!(
            result
                .expect_err("request should time out")
                .contains("timed out")
        );
        let connections = runtime.connections.lock().await;
        let connection = connections.get(&connection_id).expect("connection");
        assert_eq!(connection.pending_client_requests.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn cancelled_client_request_removes_pending_request() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (outbound_tx, mut receiver) = super::outbound::test_outbound_channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound_tx)
            .await;
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let result = runtime
            .send_request_to_connection_with_timeout(
                connection_id,
                "fs/write_text_file",
                serde_json::json!({}),
                Duration::from_secs(30),
                cancel_token,
            )
            .await;

        let request = receiver.recv().await.expect("client request");
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "fs/write_text_file",
                "params": {},
            })
        );
        assert_eq!(
            result.expect_err("request should be cancelled"),
            "client request cancelled"
        );
        let connections = runtime.connections.lock().await;
        let connection = connections.get(&connection_id).expect("connection");
        assert_eq!(connection.pending_client_requests.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn dropped_client_request_removes_pending_request() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let (outbound_tx, mut receiver) = super::outbound::test_outbound_channel(1);
        let connection_id = runtime
            .register_connection(ClientTransportKind::Stdio, outbound_tx)
            .await;
        let runtime_for_request = Arc::clone(&runtime);

        let handle = tokio::spawn(async move {
            runtime_for_request
                .send_request_to_connection_with_timeout(
                    connection_id,
                    "fs/read_text_file",
                    serde_json::json!({}),
                    Duration::from_secs(30),
                    CancellationToken::new(),
                )
                .await
        });

        let request = receiver.recv().await.expect("client request");
        assert_eq!(
            request,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "fs/read_text_file",
                "params": {},
            })
        );
        handle.abort();
        let join_error = handle.await.expect_err("request task should be aborted");
        assert!(join_error.is_cancelled());

        for _ in 0..10 {
            let connections = runtime.connections.lock().await;
            let connection = connections.get(&connection_id).expect("connection");
            if connection.pending_client_requests.is_empty() {
                return Ok(());
            }
            drop(connections);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let connections = runtime.connections.lock().await;
        let connection = connections.get(&connection_id).expect("connection");
        assert_eq!(connection.pending_client_requests.len(), 0);

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
        assert_eq!(owner_message["method"], "_devo/search/completed");
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
    async fn typed_items_opt_in_receives_native_item_notification() -> Result<()> {
        use devo_protocol::TypedItemEventPayload;
        use devo_protocol::canonical::item::{Item, ItemState};

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
            .expect("typed connection")
            .typed_items = true;

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

        Ok(())
    }

    /// Verifies (P2): an opted-in connection falls back to the legacy
    /// ACP-wrapped shape when the payload does not project.
    #[tokio::test]
    async fn typed_items_falls_back_to_legacy_shape_on_unprojectable_payload() -> Result<()> {
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
        runtime
            .connections
            .lock()
            .await
            .get_mut(&connection_id)
            .expect("connection")
            .typed_items = true;

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
            let turn = crate::persistence::build_turn_record(&metadata, None, None, None);
            rollout_store.append_turn(&record, turn).expect("append turn");
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
                rollout_store.append_item(&record, item).expect("append item");
                item_seq += 1;
            }
        }
        record.id
    }

    async fn initialized_connection(runtime: &Arc<ServerRuntime>) -> u64 {
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
        use devo_protocol::canonical::page::Page;
        use devo_protocol::canonical::turn::Turn;

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
            first.data.iter().map(|turn| turn.sequence).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("2"));
        let first_turn = &first.data[0];
        assert_eq!(first_turn.session_id.as_str(), session_id.to_string());
        assert_eq!(
            first_turn.status,
            devo_protocol::canonical::turn::TurnStatus::Completed
        );
        assert_eq!(
            first_turn.kind,
            devo_protocol::canonical::turn::TurnKind::Regular
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
        let first_items: Page<devo_protocol::canonical::item::ItemEnvelope> =
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
            devo_protocol::canonical::item::ItemState::Completed
        );
        assert!(
            matches!(&envelope.item, devo_protocol::canonical::item::Item::AssistantMessage { text, .. } if text == "first-t1")
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
        let second_items: Page<devo_protocol::canonical::item::ItemEnvelope> =
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
        let turns: devo_protocol::canonical::page::Page<devo_protocol::canonical::turn::Turn> =
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
        let items: devo_protocol::canonical::page::Page<
            devo_protocol::canonical::item::ItemEnvelope,
        > = serde_json::from_value(items["result"].clone()).expect("items result");
        assert_eq!(items.data.len(), 2);
        assert!(
            items
                .data
                .iter()
                .all(|item| item.turn_id == turn_two.id)
        );
        assert!(
            matches!(&items.data[0].item, devo_protocol::canonical::item::Item::AssistantMessage { text, .. } if text == "first-t2")
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
        let items: devo_protocol::canonical::page::Page<
            devo_protocol::canonical::item::ItemEnvelope,
        > = serde_json::from_value(items["result"].clone()).expect("items result");
        assert_eq!(items.data.len(), 5);
        assert_eq!(items.next_cursor, None);

        Ok(())
    }

    #[tokio::test]
    async fn history_lists_handle_empty_session_bad_cursor_and_unknown_session() -> Result<()> {
        use devo_protocol::canonical::page::Page;
        use devo_protocol::canonical::turn::Turn;

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        // A session with only its meta line (no turns, no items).
        let rollout_store = crate::persistence::RolloutStore::new(data_root.path().to_path_buf(), None);
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
        let turn = crate::persistence::build_turn_record(&metadata, None, None, None);
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
        use devo_protocol::canonical::event::{SnapshotData, SubscriptionCreateResult};

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let stream_id = devo_core::session_stream_id(
            &devo_protocol::canonical::ids::SessionId::from_string(session_id.to_string()),
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
        assert!(
            matches!(
                &result.replay[0].notification,
                devo_protocol::canonical::event::ServerNotification::SessionCreated { .. }
            )
        );
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
    async fn subscription_create_future_cursor_is_expired() -> Result<()> {
        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let stream_id = devo_core::session_stream_id(
            &devo_protocol::canonical::ids::SessionId::from_string(session_id.to_string()),
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
        assert_eq!(
            frame["method"],
            serde_json::json!(crate::ACP_SESSION_UPDATE_METHOD)
        );
        assert!(
            frame["params"]["_meta"]["devo/originalEvent"]
                .to_string()
                .contains("live"),
            "frame carries the original item event: {frame}"
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
        assert!(second["params"]["_meta"]["devo/originalEvent"]
            .to_string()
            .contains("second"));

        Ok(())
    }

    #[tokio::test]
    async fn subscription_ack_is_monotonic_and_unsubscribe_removes() -> Result<()> {
        use devo_protocol::canonical::event::SubscriptionCreateResult;

        let data_root = TempDir::new()?;
        let runtime = build_runtime(data_root.path());
        let session_id = write_subscribed_rollout(&runtime).await;
        let stream_id = devo_core::session_stream_id(
            &devo_protocol::canonical::ids::SessionId::from_string(session_id.to_string()),
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
}
