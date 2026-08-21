use std::sync::Arc;

use devo_core::tools::{
    AgentToolCoordinator, ClientFilesystem, ToolAgentScope, ToolCall, ToolExecutionOptions,
    ToolRuntime, ToolRuntimeContext,
};
use devo_core::{Message, QueryEvent, QueryOptions, TurnConfig, query};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::*;
use super::event_stream::enqueue_query_event;
use super::tool_display::without_agent_coordination_tools;
use super::types::TurnQueryOutcome;

pub(crate) struct TurnModelQueryParams<'a> {
    pub state: &'a mut SessionActorState,
    pub turn_id: devo_core::TurnId,
    pub turn_config: &'a TurnConfig,
    pub input: &'a str,
    pub input_messages: &'a [String],
    pub collaboration_mode: devo_protocol::CollaborationMode,
    pub input_mode: super::super::TurnInputMode,
    pub usage_parent_session_id: Option<devo_core::SessionId>,
    pub event_tx: mpsc::Sender<QueryEvent>,
}

impl ServerRuntime {
    pub(crate) async fn run_turn_model_query(
        self: &Arc<Self>,
        params: TurnModelQueryParams<'_>,
    ) -> TurnQueryOutcome {
        let TurnModelQueryParams {
            state,
            turn_id,
            turn_config,
            input,
            input_messages,
            collaboration_mode,
            input_mode,
            usage_parent_session_id,
            event_tx,
        } = params;
        let session_id = state.session_id();
        let agent_scope = if state.summary.parent_session_id.is_some() {
            ToolAgentScope::Subagent
        } else {
            ToolAgentScope::Parent
        };
        let agent_tool_policy = state.agent_tool_policy;
        let session_tool_registry = self.tool_registry_for_actor_state(state);
        let runtime_context = Arc::clone(&state.runtime_context);
        let turn_goal = match &input_mode {
            super::super::TurnInputMode::VisibleUserMessage => {
                let stores = self.goal_stores.lock().await;
                stores
                    .get(&session_id)
                    .and_then(crate::GoalStore::get)
                    .map(crate::goal::Goal::to_thread_goal)
            }
            super::super::TurnInputMode::HiddenGoalContinuation { goal } => Some(goal.clone()),
        };
        state.core.config.token_budget = turn_config
            .token_budget_for_session(state.core.config.effective_context_window_override);
        state.core.collaboration_mode = collaboration_mode;
        state.summary.collaboration_mode = collaboration_mode;
        if let Some(goal) = turn_goal {
            state.core.set_active_goal(goal);
        } else {
            state.core.clear_active_goal();
        }
        if input_mode.emits_user_message() && input_messages.is_empty() {
            state.core.push_message(Message::user(input.to_string()));
        } else if input_mode.emits_user_message() {
            for input_message in input_messages {
                state
                    .core
                    .push_message(Message::user(input_message.clone()));
            }
        }
        let event_callback_tx = event_tx.clone();
        let callback: devo_core::EventCallback = std::sync::Arc::new(move |event: QueryEvent| {
            let event_callback_tx = event_callback_tx.clone();
            Box::pin(async move {
                enqueue_query_event(&event_callback_tx, event).await;
            })
        });
        let tool_execution_start_tx = event_tx.clone();
        let registry = match agent_tool_policy {
            devo_protocol::AgentToolPolicy::Inherit if usage_parent_session_id.is_some() => {
                Arc::new(without_agent_coordination_tools(&session_tool_registry))
            }
            devo_protocol::AgentToolPolicy::Inherit => session_tool_registry,
            devo_protocol::AgentToolPolicy::DenyAll => {
                Arc::new(devo_core::tools::ToolRegistry::new())
            }
        };
        let permission_mode = state.core.config.permission_mode;
        let permission_profile = state.core.config.permission_profile.clone();
        let hook_context = Self::hook_context_from_actor_state(state, session_id);
        // Share the turn's live sandbox handle with tool execution so a
        // mid-turn settings override applies at the next spawn (Phase 3).
        let sandbox_profile_live = if let Some(stream) = self.active_stream_state(session_id).await
        {
            let stream = stream.lock().await;
            stream
                .turn_inline
                .as_ref()
                .map(|inline| Arc::clone(&inline.sandbox_profile_live))
        } else {
            None
        };
        // Same for the live turn-settings channel (Phase 4): seed it with the
        // turn-start config so a mid-turn model/effort overlay has a base to
        // modify; an overlay that already landed (generation > 0) wins.
        let live_turn_settings = if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            stream
                .turn_inline
                .as_ref()
                .map(|inline| Arc::clone(&inline.live_turn_settings))
        } else {
            None
        };
        if let Some(live) = &live_turn_settings {
            let mut live = live.lock().expect("live settings mutex poisoned");
            if live.generation == 0 && live.turn_config.is_none() {
                live.turn_config = Some(turn_config.clone());
            }
        }
        let turn_cancel_token = self
            .active_turns
            .cancel_token(session_id)
            .await
            .unwrap_or_else(CancellationToken::new);
        let query_cancel_token = turn_cancel_token.clone();
        let provider_http = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .provider_http
            .clone();
        let runtime = ToolRuntime::new_with_context_and_options(
            Arc::clone(&registry),
            self.build_permission_checker(session_id, turn_id, permission_mode, permission_profile),
            ToolRuntimeContext {
                session_id: session_id.to_string(),
                turn_id: Some(turn_id.to_string()),
                cwd: state.core.cwd.clone(),
                agent_scope,
                collaboration_mode,
                agent_coordinator: Some(Arc::clone(self) as Arc<dyn AgentToolCoordinator>),
                client_filesystem: Some(Arc::clone(self) as Arc<dyn ClientFilesystem>),
                file_read_ledger: Arc::clone(&state.file_read_ledger),
                local_web_search: match &turn_config.web_search {
                    devo_core::ResolvedWebSearchConfig::Local(config) => Some(config.clone()),
                    devo_core::ResolvedWebSearchConfig::Disabled
                    | devo_core::ResolvedWebSearchConfig::Provider => None,
                },
                hooks: hook_context,
                network_proxy: provider_http.proxy_url,
                network_no_proxy: provider_http.no_proxy,
                sandbox_profile: state.core.config.sandbox_profile.clone(),
                sandbox_profile_live,
            },
            ToolExecutionOptions {
                cancel_token: turn_cancel_token,
                on_tool_execution_start: Some(Arc::new(move |call: ToolCall| {
                    let tool_execution_start_tx = tool_execution_start_tx.clone();
                    Box::pin(async move {
                        enqueue_query_event(
                            &tool_execution_start_tx,
                            QueryEvent::ToolExecutionStart { id: call.id },
                        )
                        .await;
                    })
                })),
                ..ToolExecutionOptions::default()
            },
        );
        // Turns execute inline on the session actor's own task rather than as a
        // separately spawned task, so an external `JoinHandle::abort()` can no
        // longer stop an in-flight query: it only cancels the caller waiting on
        // the actor's reply, not the actor itself. Race the query against the
        // turn's cancellation token so interrupting a turn actually unblocks the
        // actor's mailbox instead of hanging it forever.
        //
        // The stream-loop's biased select! checks the cancel token before every
        // chunk.  On interrupt it breaks immediately, the post-processing stage
        // commits partial assistant/reasoning text to session, then the cancel
        // guard before tool execution skips incomplete tool calls and ends the
        // turn cleanly.  The tool-execution cancel guard already handles the
        // "cancel during tools" case (line ~1827).
        let result = {
            let provider = self.usage_ledger.instrumented_provider(
                runtime_context.provider_for_route(turn_config.provider_route.clone()),
                session_id,
                Some(turn_id),
                devo_protocol::native::usage::UsagePurpose::TurnQuery,
            );
            let compaction_provider = self.usage_ledger.instrumented_provider(
                runtime_context.provider_for_route(turn_config.provider_route.clone()),
                session_id,
                Some(turn_id),
                devo_protocol::native::usage::UsagePurpose::Compaction,
            );
            let mut query_future = std::pin::pin!(query(
                &mut state.core,
                turn_config,
                provider,
                registry,
                &runtime,
                Some(callback),
                QueryOptions {
                    cancel_token: Some(query_cancel_token.clone()),
                    compaction_provider: Some(compaction_provider),
                    live_settings: live_turn_settings.clone(),
                },
            ));
            tokio::select! {
                biased;
                () = query_cancel_token.cancelled() => {
                    // Give the query a brief window so the stream-loop cancel
                    // check and post-processing commit can complete, then map
                    // any completion (including Aborted from the tool-execution
                    // guard) to the Interrupted status.
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(100),
                        &mut query_future,
                    )
                    .await
                    {
                        Ok(Ok(())) | Ok(Err(devo_core::AgentError::Aborted)) | Err(_) => {
                            Err(devo_core::AgentError::Aborted)
                        }
                        Ok(Err(error)) => Err(error),
                    }
                }
                result = &mut query_future => result,
            }
        };
        TurnQueryOutcome {
            result,
            session_total_input_tokens: state.core.total_input_tokens,
            session_total_output_tokens: state.core.total_output_tokens,
            session_total_tokens: state.core.total_tokens,
            session_total_cache_creation_tokens: state.core.total_cache_creation_tokens,
            session_total_cache_read_tokens: state.core.total_cache_read_tokens,
            session_last_input_tokens: state.core.last_input_tokens,
            session_prompt_token_estimate: state.core.prompt_token_estimate,
        }
    }
}
