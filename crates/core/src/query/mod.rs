//! Agent query loop: stream a model turn, run tools, and continue.
//!
//! Submodules isolate the observation surface, provider retry policy, prompt
//! token estimates, stream consumption, and model-turn continuation quirks
//! from the orchestration loop in this file.

mod event;
mod prompt_estimate;
mod provider_retry;
mod stream_consumer;
mod turn_continuation;

pub use event::EventCallback;
pub use event::LiveTurnSettings;
pub use event::ProviderRetryStatus;
pub use event::QueryEvent;
pub use event::QueryOptions;
pub use event::QueryProviderRetryPhase;
pub use event::SharedLastModelRequest;
pub use event::SharedLiveTurnSettings;

pub(crate) use event::emit_query_event;
pub use prompt_estimate::RawContextBreakdown;
pub(crate) use prompt_estimate::estimate_request_context_breakdown;
pub(crate) use provider_retry::ProviderRetryDecision;
pub(crate) use provider_retry::provider_retry_decision;
pub(crate) use provider_retry::wait_for_provider_retry;
pub(crate) use stream_consumer::AssembledModelTurn;
pub(crate) use stream_consumer::ProviderAttemptError;
pub(crate) use stream_consumer::run_provider_attempt;
pub(crate) use turn_continuation::ModelTurnSnapshot;
pub(crate) use turn_continuation::TurnContinuation;
pub(crate) use turn_continuation::TurnContinuationPolicy;
pub(crate) use turn_continuation::assistant_content_has_visible_content;

#[cfg(test)]
pub(crate) use provider_retry::ErrorClass;
#[cfg(test)]
pub(crate) use provider_retry::classify_error;
#[cfg(test)]
pub(crate) use turn_continuation::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT;

use std::collections::HashMap;
use std::sync::Arc;

use devo_protocol::HostedToolDefinition;
use devo_protocol::HostedWebFetchTool;
use devo_protocol::HostedWebSearchTool;
use devo_protocol::ModelRequest;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResolvedReasoningRequest;
use devo_protocol::ResponseContent;
use devo_protocol::SamplingControls;
use devo_protocol::StreamEvent;
use devo_protocol::TruncationPolicy;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::info_span;
use tracing::warn;

use crate::tools::ToolAgentScope;
use crate::tools::ToolContent;
use crate::tools::ToolRegistry;
use crate::tools::ToolRuntime;
use crate::tools::deferred_loading::is_subagent_agent_coordination_tool;
use devo_provider::ModelProviderSDK;

use crate::AgentError;
use crate::ContentBlock;
use crate::Message;
use crate::Model;
use crate::Role;
use crate::SessionState;
use crate::TurnConfig;
use crate::context::AgentsMdDiffFragment;
use crate::context::AgentsMdManager;
use crate::context::ContextualUserFragment;
use crate::context::SessionContext;
use crate::context::TurnContext;
use crate::context::load_workspace_instructions;
use crate::context::turn_aborted::TurnAborted;
use crate::history::ContextView;
use crate::history::History;
use crate::history::TokenInfo;
use crate::history::compaction::CompactAction;
use crate::history::compaction::CompactionConfig;
use crate::history::compaction::CompactionKind;
use crate::history::compaction::compact_history;
use crate::history::summarizer::DefaultHistorySummarizer;
use crate::response_item::ResponseItem;
use crate::response_item::message_to_response_items;

const SUBAGENT_MODE_REMINDER: &str = include_str!("../../prompts/subagent_mode_reminder.md");

fn hosted_tools_for_web_capabilities(
    web_search: &devo_config::ResolvedWebSearchConfig,
    web_fetch: devo_config::ResolvedWebFetchConfig,
) -> Vec<HostedToolDefinition> {
    let mut hosted_tools = Vec::new();
    if matches!(web_search, devo_config::ResolvedWebSearchConfig::Provider) {
        hosted_tools.push(HostedToolDefinition::WebSearch(HostedWebSearchTool::new()));
    }
    if web_fetch.is_provider() {
        hosted_tools.push(HostedToolDefinition::WebFetch(HostedWebFetchTool::new()));
    }
    hosted_tools
}

#[cfg(test)]
fn hosted_tools_for_web_search(
    web_search: &devo_config::ResolvedWebSearchConfig,
) -> Vec<HostedToolDefinition> {
    hosted_tools_for_web_capabilities(web_search, devo_config::ResolvedWebFetchConfig::Disabled)
}

/// Compact session messages using LLM-backed summarization.
///
/// `kind` selects the preserve strategy inside [`compact_history`]:
/// - [`CompactionKind::Auto`]: preventive compaction when the session token
///   budget is high; keeps a tail token window.
/// - [`CompactionKind::Proactive`]: forced compaction after provider
///   `context_too_long`; keeps from the latest user message onward.
struct CompactionModelRequest<'a> {
    provider: &'a Arc<dyn ModelProviderSDK>,
    model_slug: &'a str,
    request_model: &'a str,
    max_tokens: usize,
}

async fn summarize_and_compact(
    session: &mut SessionState,
    on_event: &Option<EventCallback>,
    model: CompactionModelRequest<'_>,
    kind: CompactionKind,
    cancel_token: Option<&CancellationToken>,
) {
    let items: Vec<ResponseItem> = session
        .prompt_source_messages()
        .iter()
        .cloned()
        .flat_map(message_to_response_items)
        .collect();

    let token_info = TokenInfo {
        input_tokens: session.total_input_tokens,
        cached_input_tokens: session.total_cache_read_tokens,
        output_tokens: session.total_output_tokens,
    };

    let config = CompactionConfig {
        budget: session.config.token_budget.clone(),
        kind,
    };

    let summarizer = DefaultHistorySummarizer::with_models(
        Arc::clone(model.provider),
        model.model_slug,
        model.request_model,
        model.max_tokens,
    );

    emit_query_event(on_event, QueryEvent::ContextCompactionStarted).await;
    match compact_history(&items, &token_info, &summarizer, &config, cancel_token).await {
        Ok(CompactAction::Replaced(compacted_items)) => {
            let new_messages: Vec<Message> = compacted_items
                .iter()
                .filter_map(|item| match item {
                    ResponseItem::Message(msg) => Some(msg.clone()),
                    _ => None,
                })
                .collect();
            let removed = session
                .prompt_source_messages()
                .len()
                .saturating_sub(new_messages.len());
            info!("LLM compaction removed {removed} messages");
            session.set_prompt_messages(new_messages);
            emit_query_event(
                on_event,
                QueryEvent::ContextCompactionCompleted { compacted_items },
            )
            .await;
        }
        Ok(CompactAction::Skipped) => {
            debug!("LLM compaction skipped, nothing to compact");
            emit_query_event(
                on_event,
                QueryEvent::ContextCompactionFailed {
                    message: "Context compaction skipped: nothing to compact".to_string(),
                },
            )
            .await;
        }
        Err(e) => {
            warn!("LLM compaction failed: {e}");
            emit_query_event(
                on_event,
                QueryEvent::ContextCompactionFailed {
                    message: e.to_string(),
                },
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Model-visible tool result truncation
// ---------------------------------------------------------------------------

const TOOL_RESULT_TRUNCATION_MARKER: &str = "\n...[truncated]";

/// Tools that store the model-facing payload in Mixed `text` and put UI/protocol
/// metadata in `json` (shell exit/cwd; read preview/truncated). Omit JSON from
/// the prompt so the stream is not duplicated.
fn tool_result_omits_mixed_json_for_model(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("shell_command" | "bash" | "read"))
}

fn serialize_tool_content_for_model(content: ToolContent, tool_name: Option<&str>) -> String {
    if tool_result_omits_mixed_json_for_model(tool_name) {
        content.text_for_model()
    } else {
        content.into_string()
    }
}

fn tool_content_model_bytes(content: &ToolContent, tool_name: Option<&str>) -> usize {
    if tool_result_omits_mixed_json_for_model(tool_name) {
        content.text_for_model_byte_len()
    } else {
        content.into_string_byte_len()
    }
}

fn truncate_tool_result_for_model(
    content: String,
    tool_name: Option<&str>,
    truncation_policy: TruncationPolicy,
) -> String {
    if preserve_full_tool_result(tool_name) {
        return content;
    }

    let byte_budget = truncation_policy.byte_budget();
    if content.len() <= byte_budget {
        return content;
    }

    let marker = if byte_budget > TOOL_RESULT_TRUNCATION_MARKER.len() {
        TOOL_RESULT_TRUNCATION_MARKER
    } else {
        TOOL_RESULT_TRUNCATION_MARKER.trim_start()
    };

    if byte_budget <= marker.len() {
        return marker.to_string();
    }

    let content_budget = byte_budget - marker.len();
    let mut truncate_at = content_budget;
    while truncate_at > 0 && !content.is_char_boundary(truncate_at) {
        truncate_at -= 1;
    }

    let mut truncated = content[..truncate_at].to_string();
    truncated.push_str(marker);
    truncated
}

fn preserve_full_tool_result(tool_name: Option<&str>) -> bool {
    matches!(
        tool_name,
        Some("await_task" | "wait_agent" | "subagent_result")
    )
}

fn insert_subagent_request_reminders(messages: &mut Vec<RequestMessage>) {
    let insert_at = messages
        .iter()
        .rposition(is_user_text_message)
        .unwrap_or(messages.len());
    messages.splice(
        insert_at..insert_at,
        [request_text_message(
            SUBAGENT_MODE_REMINDER.trim_end().to_string(),
        )],
    );
}

fn insert_goal_context_message(messages: &mut Vec<RequestMessage>, goal_context: &str) {
    let insert_at = if messages.last().is_some_and(is_visible_user_text_message) {
        messages.len().saturating_sub(1)
    } else {
        messages.len()
    };
    messages.splice(
        insert_at..insert_at,
        [request_text_message(goal_context.to_string())],
    );
}

fn request_text_message(text: String) -> RequestMessage {
    RequestMessage {
        role: Role::User.as_str().to_string(),
        content: vec![RequestContent::Text { text }],
    }
}

fn is_user_text_message(message: &RequestMessage) -> bool {
    message.role == Role::User.as_str()
        && message
            .content
            .iter()
            .any(|content| matches!(content, RequestContent::Text { .. }))
}

fn is_visible_user_text_message(message: &RequestMessage) -> bool {
    is_user_text_message(message) && !is_injected_context_message(message)
}

fn is_injected_context_message(message: &RequestMessage) -> bool {
    message.role == Role::User.as_str()
        && message.content.iter().any(|content| match content {
            RequestContent::Text { text } => {
                let trimmed = text.trim_start();
                trimmed.starts_with("<environment_context>")
                    || trimmed.starts_with("<available_skills>")
                    || trimmed.starts_with("<language_preference>")
                    || trimmed.starts_with("<context_changes>")
                    || trimmed.starts_with("<user_instructions_updates>")
                    || trimmed.starts_with("<user_instructions>")
            }
            RequestContent::Reasoning { .. }
            | RequestContent::ProviderReasoning { .. }
            | RequestContent::HostedToolUse { .. }
            | RequestContent::ToolUse { .. }
            | RequestContent::ToolResult { .. } => false,
        })
}

/// Agent loop orchestration: build request, stream, continue or run tools.
///
/// Observation (`event`), provider retry (`provider_retry`), prompt estimates
/// (`prompt_estimate`), stream consumption (`stream_consumer`), and model-turn
/// continuation quirks (`turn_continuation`) live in submodules.
///
/// The recursive agent loop is the beating heart of the runtime.
///
/// The implementation refers to Claude Code's `query.ts`. It drives
/// multi-turn conversations by:
///
/// 1. Building the model request from session state
/// 2. Streaming the model response
/// 3. Collecting assistant text and tool_use blocks
/// 4. Executing tool calls via the orchestrator
/// 5. Appending tool_result messages
/// 6. Recursing if the model wants to continue
///
/// The loop terminates when:
/// - The model emits `end_turn` with no tool calls
/// - An unrecoverable error occurs
pub async fn query(
    session: &mut SessionState,
    turn_config: &TurnConfig,
    provider: Arc<dyn ModelProviderSDK>,
    registry: Arc<ToolRegistry>,
    runtime: &ToolRuntime,
    on_event: Option<EventCallback>,
    options: QueryOptions,
) -> Result<(), AgentError> {
    let compaction_provider = options
        .compaction_provider
        .as_ref()
        .unwrap_or(&provider)
        .clone();
    let agents_md_manager = AgentsMdManager::new(session.config.agents_md.clone());
    let current_agents_snapshot = load_workspace_instructions(&session.cwd, &agents_md_manager);
    let agent_scope = runtime.agent_scope();
    let mut request_tools = registry.tool_definitions();
    if agent_scope == ToolAgentScope::Subagent {
        request_tools.retain(|tool| !is_subagent_agent_coordination_tool(&tool.name));
    }
    if !turn_config.web_search.is_local() {
        request_tools.retain(|tool| tool.name != "web_search");
    }
    if !turn_config.web_fetch.is_local() {
        request_tools.retain(|tool| tool.name != "webfetch");
    }
    // Non-OpenAI models often emit malformed apply_patch input, so only expose
    // the tool to OpenAI-channel models (see Model::supports_apply_patch).
    if !turn_config.model.supports_apply_patch() {
        request_tools.retain(|tool| tool.name != "apply_patch");
    }

    if session.session_context.is_none() {
        session.session_context = Some(SessionContext::capture(
            &turn_config.model,
            turn_config.reasoning_effort_selection.as_deref(),
            &session.cwd,
            current_agents_snapshot.clone(),
            session.config.available_skills_instructions.clone(),
        ));
    }
    let current_turn_context =
        TurnContext::capture(session, turn_config, current_agents_snapshot.clone());
    if let Some(context_changes) =
        current_turn_context.context_changes_since(session.latest_turn_context.as_ref())
    {
        session.insert_context_message(context_changes.to_message());
    }
    if let Some(previous_turn_context) = session.latest_turn_context.as_ref()
        && let Some(diff) = AgentsMdManager::diff(
            previous_turn_context.observed_agents_snapshot.as_ref(),
            current_agents_snapshot.as_ref(),
        )
    {
        session.insert_context_message(AgentsMdDiffFragment::new(diff).to_message());
    }
    session.latest_turn_context = Some(current_turn_context.clone());
    let session_context = session
        .session_context
        .clone()
        .expect("session context should be initialized");
    let prefetched_user_inputs = session_context.prefix_user_inputs();

    let mut retry_count: usize = 0;
    let mut context_compacted = false;
    let mut budget_steer_injected = false;
    let mut continuation_policy =
        TurnContinuationPolicy::for_models(&turn_config.model.slug, &turn_config.request_model);
    // Live settings override (L2-DES-CONV-002 Phase 4): the active config
    // starts as the turn-start snapshot and is re-applied per iteration when
    // the shared generation advances.
    let mut active_turn_config = turn_config.clone();
    let mut applied_live_generation = 0u64;

    if session.turn_state.is_none() {
        session.start_turn(devo_protocol::TurnKind::Regular);
    }

    // Explicit interrupted-turn notice for the next user message after a user
    // interrupt. Placed before pending-input processing so it sits just above the
    // latest user text in prompt construction (after any context diff).
    let previous_turn_interrupted = session.take_last_turn_interrupted();
    if previous_turn_interrupted {
        let fragment = TurnAborted::new(TurnAborted::INTERRUPTED_GUIDANCE);
        if let ResponseItem::Message(msg) = fragment.to_response_item() {
            session.insert_context_message(msg);
        }
    }

    loop {
        let pending = session.take_turn_pending_input();

        for item in &pending {
            match &item.kind {
                devo_protocol::PendingInputKind::UserText { text } => {
                    session.push_message(Message::user(text.clone()));
                }
                devo_protocol::PendingInputKind::UserInput {
                    prompt_text,
                    prompt_messages,
                    ..
                } => {
                    if prompt_messages.is_empty() {
                        session.push_message(Message::user(prompt_text.clone()));
                    } else {
                        for prompt_message in prompt_messages {
                            session.push_message(Message::user(prompt_message.clone()));
                        }
                    }
                }
                devo_protocol::PendingInputKind::ToolCallBlockedByHook {
                    tool_use_id,
                    reason,
                } => {
                    session.push_message(Message::user(format!(
                        "[Tool call {} was blocked: {}]",
                        tool_use_id, reason
                    )));
                }
                devo_protocol::PendingInputKind::BudgetLimitSteering => {
                    session.push_message(Message::system(
                        "Note: The conversation is approaching the token budget limit. \
                         Please be concise and consider wrapping up the current task.",
                    ));
                }
            }
        }

        // Check token budget and compact before building the request
        if let Some(live) = &options.live_settings {
            let live = live.lock().expect("live settings mutex poisoned");
            if live.generation != applied_live_generation {
                if let Some(config) = &live.turn_config {
                    active_turn_config = config.clone();
                }
                if let Some(limit) = live.auto_compact_token_limit {
                    session.config.token_budget.context_window = limit;
                    session.config.token_budget.auto_compact_token_limit = Some(limit);
                }
                applied_live_generation = live.generation;
            }
        }
        if session.last_turn_tokens > 0
            && session
                .config
                .token_budget
                .should_compact(session.last_turn_tokens)
        {
            if !budget_steer_injected {
                if let Some(turn) = session.turn_state.as_mut() {
                    turn.push_pending_input(devo_protocol::PendingInputItem::new(
                        devo_protocol::PendingInputKind::BudgetLimitSteering,
                        None,
                        chrono::Utc::now(),
                    ));
                }
                budget_steer_injected = true;
            }
            info!("token budget threshold exceeded, running LLM compaction");
            let live_compaction_model_slug = active_turn_config
                .model
                .resolve_reasoning_effort_selection(
                    active_turn_config.reasoning_effort_selection.as_deref(),
                )
                .request_model;
            let live_compaction_request_model =
                active_turn_config.provider_request_model(&live_compaction_model_slug);
            // Auto: preserve tail items up to COMPACT_USER_MESSAGE_MAX_TOKENS.
            // Example: [user1, asst1, user2, asst2, user3] -> [summary, asst2, user3].
            summarize_and_compact(
                session,
                &on_event,
                CompactionModelRequest {
                    provider: &compaction_provider,
                    model_slug: &live_compaction_model_slug,
                    request_model: &live_compaction_request_model,
                    max_tokens: active_turn_config.model.max_tokens.unwrap_or(4096) as usize,
                },
                CompactionKind::Auto,
                options.cancel_token.as_ref(),
            )
            .await;
        }

        session.turn_count += 1;
        let turn_span = info_span!(
            "turn",
            turn = session.turn_count,
            session_id = %session.id,
            model = %active_turn_config.model.slug,
            cwd = %session.cwd.display()
        );
        let _turn_guard = turn_span.enter();
        info!("starting turn");

        // Build model request from the session-locked prefix.
        let request_system = {
            let mut system = session_context.build_system_prompt();
            if !matches!(
                &turn_config.web_search,
                devo_config::ResolvedWebSearchConfig::Disabled
            ) {
                if !system.trim().is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&crate::tools::websearch_prompt::web_search_prompt());
            }
            Some(system).filter(|system| !system.trim().is_empty())
        };

        // Resolve provider-bound reasoning request parameters from the live
        // config so mid-turn model/effort changes apply at this model call.
        let ResolvedReasoningRequest {
            request_model,
            request_thinking,
            request_reasoning_effort,
            extra_body,
            effective_reasoning_effort: _,
        } = active_turn_config.model.resolve_reasoning_effort_selection(
            active_turn_config.reasoning_effort_selection.as_deref(),
        );
        let catalog_request_model = request_model.clone();
        let provider_request_model =
            active_turn_config.provider_request_model(&catalog_request_model);
        let extra_body = crate::merge_model_request_body(
            active_turn_config
                .provider_request_models
                .request_defaults(),
            extra_body,
        );
        let extra_body = crate::add_model_request_headers(
            extra_body,
            active_turn_config.provider_request_models.request_headers(),
        );

        let prompt_source_message_count = session.prompt_source_messages().len();
        let history_items = session
            .prompt_source_messages()
            .iter()
            .cloned()
            .flat_map(message_to_response_items)
            .collect::<Vec<_>>();
        let prompt_source_item_count = history_items.len();
        let history = History {
            items: history_items,
            token_info: TokenInfo::default(),
            context: ContextView::new(
                std::env::consts::OS,
                session_context.environment.shell.clone(),
                session_context.environment.timezone.clone(),
                session_context.model.slug.clone(),
                session_context
                    .reasoning_effort
                    .map(|effort| effort.label().to_lowercase()),
                Some(session_context.persona.as_str().to_string()),
                session_context.environment.current_date.clone(),
                session_context.environment.cwd.display().to_string(),
            ),
        };
        let mut messages = history.for_prompt_with_prefix(
            &prefetched_user_inputs,
            &active_turn_config.model.input_modalities,
        );
        if let Some(goal_context) = session.goal_context_prompt() {
            insert_goal_context_message(&mut messages, &goal_context);
        }
        if agent_scope == ToolAgentScope::Subagent {
            insert_subagent_request_reminders(&mut messages);
        }

        let hosted_tools =
            hosted_tools_for_web_capabilities(&turn_config.web_search, turn_config.web_fetch);
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::CatalogSlug(catalog_request_model),
            model: provider_request_model,
            system: request_system,
            messages,
            max_tokens: active_turn_config
                .model
                .max_tokens
                .map_or(session.config.token_budget.max_output_tokens, |value| {
                    value as usize
                }),
            tools: Some(request_tools.clone()),
            hosted_tools: hosted_tools.clone(),
            sampling: SamplingControls {
                temperature: active_turn_config.model.temperature,
                top_p: active_turn_config.model.top_p,
                top_k: active_turn_config.model.top_k.map(|value| value as u32),
            },
            request_thinking,
            reasoning_effort: request_reasoning_effort,
            extra_body,
        };
        let breakdown = estimate_request_context_breakdown(&request);
        session.prompt_token_estimate = breakdown.total().try_into().unwrap_or(usize::MAX);
        session.raw_context_breakdown = Some(breakdown);
        emit_query_event(&on_event, QueryEvent::ContextEstimate { breakdown }).await;
        debug!(
            prompt_source_messages = prompt_source_message_count,
            prompt_source_items = prompt_source_item_count,
            prefix_user_inputs = prefetched_user_inputs.len(),
            request_messages = request.messages.len(),
            exposed_tools = request.tools.as_ref().map_or(0, Vec::len),
            prompt_token_estimate = session.prompt_token_estimate,
            max_tokens = request.max_tokens,
            has_system = request.system.is_some(),
            "built model request"
        );
        if let Some(slot) = &options.last_model_request
            && let Ok(mut last) = slot.lock()
        {
            *last = Some(request.clone());
        }

        let assembled = match run_provider_attempt(
            provider.as_ref(),
            request,
            session,
            &on_event,
            options.cancel_token.as_ref(),
            &turn_config.model.slug,
        )
        .await
        {
            Ok(assembled) => {
                retry_count = 0;
                context_compacted = false;
                assembled
            }
            Err(ProviderAttemptError::Fatal(error)) => {
                return Err(AgentError::Provider(error));
            }
            Err(error) => {
                let (is_create, retry_error) = match error {
                    ProviderAttemptError::Create(error) => (true, error),
                    ProviderAttemptError::Retryable(error) => (false, error),
                    ProviderAttemptError::Fatal(_) => {
                        unreachable!("fatal errors handled above")
                    }
                };
                match provider_retry_decision(
                    &retry_error,
                    &mut retry_count,
                    &mut context_compacted,
                ) {
                    ProviderRetryDecision::CompactAndRetry => {
                        warn!("context_too_long - compacting and retrying");
                        // Proactive: must compact even if token estimates disagree
                        // with the provider; preserve from latest user only.
                        let retry_compaction_model_slug = active_turn_config
                            .model
                            .resolve_reasoning_effort_selection(
                                active_turn_config.reasoning_effort_selection.as_deref(),
                            )
                            .request_model;
                        let retry_compaction_request_model =
                            active_turn_config.provider_request_model(&retry_compaction_model_slug);
                        summarize_and_compact(
                            session,
                            &on_event,
                            CompactionModelRequest {
                                provider: &compaction_provider,
                                model_slug: &retry_compaction_model_slug,
                                request_model: &retry_compaction_request_model,
                                max_tokens: active_turn_config.model.max_tokens.unwrap_or(4096)
                                    as usize,
                            },
                            CompactionKind::Proactive,
                            options.cancel_token.as_ref(),
                        )
                        .await;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::RetryAfter(backoff) => {
                        if is_create {
                            warn!(
                                attempt = retry_count,
                                backoff_ms = backoff.as_millis(),
                                "transient provider error - retrying with exponential backoff"
                            );
                        } else {
                            warn!(
                                attempt = retry_count,
                                backoff_ms = backoff.as_millis(),
                                "transient provider stream error - retrying with exponential backoff"
                            );
                        }
                        wait_for_provider_retry(
                            &on_event,
                            options.cancel_token.as_ref(),
                            provider.name(),
                            &turn_config.model.slug,
                            retry_count,
                            backoff,
                            &retry_error.to_string(),
                        )
                        .await?;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::Fail => {
                        return Err(AgentError::Provider(retry_error));
                    }
                }
            }
        };

        let AssembledModelTurn {
            assistant_content,
            tool_calls,
            stop_reason,
            has_hosted_tool_uses,
            has_provider_reasoning,
            has_visible_assistant_text,
        } = assembled;

        let continuation = continuation_policy.decide(ModelTurnSnapshot {
            stop_reason,
            assistant_content: &assistant_content,
            has_visible_assistant_text,
            has_local_tool_calls: !tool_calls.is_empty(),
            has_hosted_tool_uses,
            has_provider_reasoning,
            request_tools: &request_tools,
            hosted_tools: &hosted_tools,
        });

        if assistant_content_has_visible_content(&assistant_content) {
            session.push_message(Message {
                role: Role::Assistant,
                content: assistant_content,
            });
        }

        match continuation {
            TurnContinuation::RunTools => {}
            TurnContinuation::Continue => continue,
            TurnContinuation::ContinueWithMessage(message) => {
                session.push_message(message);
                continue;
            }
            TurnContinuation::Complete { stop_reason } => {
                if let Some(sr) = stop_reason {
                    emit_query_event(&on_event, QueryEvent::TurnComplete { stop_reason: sr }).await;
                }
                debug!("no tool calls, ending query loop");
                session.end_turn();
                if options
                    .cancel_token
                    .as_ref()
                    .is_some_and(|ct| ct.is_cancelled())
                {
                    return Err(AgentError::Aborted);
                }
                return Ok(());
            }
            TurnContinuation::Fail(error) => return Err(error),
        }

        // If the turn was cancelled (e.g. mid-stream interrupt with partial
        // tool calls), save whatever partial assistant content was already
        // committed above, skip tool execution, and end the turn.
        if options
            .cancel_token
            .as_ref()
            .is_some_and(|ct| ct.is_cancelled())
        {
            session.end_turn();
            return Ok(());
        }

        let tool_result_metadata: HashMap<String, (String, serde_json::Value, String)> = tool_calls
            .iter()
            .map(|call| {
                (
                    call.id.clone(),
                    (
                        call.name.clone(),
                        call.input.clone(),
                        crate::tools::tool_summary::tool_summary(
                            &call.name,
                            &call.input,
                            &session.cwd,
                        ),
                    ),
                )
            })
            .collect();

        // Execute tool calls. When a caller is observing query events, wire
        // tool progress and per-call completion into the same event stream so
        // long-running and parallel tools can render before the whole batch ends.
        let results = if let Some(progress_events) = on_event.clone() {
            let completion_events = Arc::clone(&progress_events);
            let metadata = Arc::new(tool_result_metadata.clone());
            runtime
                .execute_batch_streaming_with_completion(
                    &tool_calls,
                    move |tool_use_id, progress| {
                        let progress_events = Arc::clone(&progress_events);
                        Box::pin(async move {
                            progress_events(QueryEvent::ToolProgress {
                                tool_use_id,
                                progress,
                            })
                            .await;
                        })
                    },
                    move |result| {
                        let completion_events = Arc::clone(&completion_events);
                        let metadata = Arc::clone(&metadata);
                        Box::pin(async move {
                            let (tool_name, input, summary) = metadata
                                .get(result.tool_use_id.as_str())
                                .cloned()
                                .unwrap_or_else(|| {
                                    (String::new(), serde_json::Value::Null, String::new())
                                });
                            completion_events(QueryEvent::ToolResult {
                                tool_use_id: result.tool_use_id,
                                tool_name,
                                input,
                                content: result.content,
                                display_content: result.display_content,
                                is_error: result.is_error,
                                summary,
                            })
                            .await;
                        })
                    },
                )
                .await
        } else {
            runtime.execute_batch(&tool_calls).await
        };
        let tool_result_count = results.len();
        let tool_error_count = results.iter().filter(|result| result.is_error).count();
        let tool_output_bytes = results
            .iter()
            .map(|result| {
                let tool_name = tool_result_metadata
                    .get(result.tool_use_id.as_str())
                    .map(|(tool_name, _, _)| tool_name.as_str());
                tool_content_model_bytes(&result.content, tool_name)
            })
            .sum::<usize>();
        debug!(
            tool_calls = tool_calls.len(),
            tool_results = tool_result_count,
            tool_errors = tool_error_count,
            tool_output_bytes,
            "tool batch completed"
        );

        // Build tool result message (user role, per Anthropic API convention)
        let truncation_policy = TruncationPolicy::from(turn_config.model.truncation_policy);
        let result_content: Vec<ContentBlock> = results
            .into_iter()
            .map(|r| {
                let tool_name = tool_result_metadata
                    .get(r.tool_use_id.as_str())
                    .map(|(tool_name, _, _)| tool_name.as_str());
                let content_str = serialize_tool_content_for_model(r.content, tool_name);
                let content =
                    truncate_tool_result_for_model(content_str, tool_name, truncation_policy);
                ContentBlock::ToolResult {
                    tool_use_id: r.tool_use_id,
                    content,
                    is_error: r.is_error,
                }
            })
            .collect();

        session.push_message(Message {
            role: Role::User,
            content: result_content,
        });

        // If the turn was cancelled while tools were running, keep the
        // interrupted tool results above and stop without another model call.
        if options
            .cancel_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            session.end_turn();
            return Err(AgentError::Aborted);
        }
    }
}

/// Sends a minimal provider probe request used by onboarding and configuration checks.
pub async fn test_model_connection(
    provider: &dyn ModelProviderSDK,
    model: &Model,
    model_profile: devo_protocol::ModelProfileKey,
    request_model: &str,
    prompt: &str,
) -> Result<String, AgentError> {
    let ResolvedReasoningRequest {
        request_model: _,
        request_thinking,
        request_reasoning_effort,
        extra_body,
        effective_reasoning_effort: _,
    } = model.resolve_reasoning_effort_selection(None);
    let request = ModelRequest {
        model_slug: model_profile,
        model: request_model.to_string(),
        system: None,
        messages: vec![devo_protocol::RequestMessage {
            role: "user".to_string(),
            content: vec![devo_protocol::RequestContent::Text {
                text: prompt.to_string(),
            }],
        }],
        max_tokens: model.max_tokens.map_or(64, |value| value as usize),
        tools: None,
        hosted_tools: Vec::new(),
        sampling: SamplingControls {
            temperature: model.temperature,
            top_p: model.top_p,
            top_k: model.top_k.map(|value| value as u32),
        },
        request_thinking,
        reasoning_effort: request_reasoning_effort,
        extra_body,
    };
    let mut stream = provider.completion_stream(request).await?;
    let mut reply_preview = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { text, .. } => reply_preview.push_str(&text),
            StreamEvent::MessageDone { response } => {
                if reply_preview.trim().is_empty() {
                    reply_preview = response
                        .content
                        .into_iter()
                        .find_map(|content| match content {
                            ResponseContent::Text(text) => Some(text),
                            _ => None,
                        })
                        .unwrap_or_default();
                }
                break;
            }
            _ => {}
        }
    }
    let preview = reply_preview.trim();
    if preview.is_empty() {
        return Err(AgentError::Provider(anyhow::anyhow!(
            "provider validation completed without a model reply"
        )));
    }
    Ok(preview.to_string())
}

#[cfg(test)]
mod tests;
