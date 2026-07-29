use std::collections::HashMap;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;

use devo_protocol::HostedToolDefinition;
use devo_protocol::HostedWebFetchTool;
use devo_protocol::HostedWebSearchTool;
use devo_protocol::ModelRequest;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResolvedReasoningRequest;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::SamplingControls;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::ToolDefinition;
use devo_protocol::TruncationPolicy;
use futures::StreamExt;
use futures::future::BoxFuture;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::info;
use tracing::info_span;
use tracing::warn;

use crate::tools::ToolAgentScope;
use crate::tools::ToolCall;
use crate::tools::ToolContent;
use crate::tools::ToolRegistry;
use crate::tools::ToolRuntime;
use crate::tools::deferred_loading::is_subagent_agent_coordination_tool;
use devo_provider::ModelProviderSDK;
use devo_provider::error::ProviderError;

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

const SUBAGENT_MODE_REMINDER: &str = "<system-reminder>\nYou are running as a sub-agent. Complete the delegated task using the available non-agent tools. Do not call agent coordination tools such as spawn_agent, send_message, await_task, list_tasks, or cancel_task; report progress and final results through assistant output.\n</system-reminder>";
const DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT: &str = "Your previous response contained only hidden reasoning and no user-visible answer. Provide the final answer to the user's original request now. Do not reveal or summarize hidden reasoning; return only user-visible content.";
const MAX_DSML_TEXT_TOOL_CALL_CONTINUATIONS: usize = 3;
const DSML_TEXT_TOOL_CALL_CONTINUATION_REMINDER: &str = "Your previous assistant message contained DSML tagged tool-call text. Those tags were emitted as ordinary text and no tool was executed. Do not repeat the DSML block. Continue now by using the provider's native hosted tool interface when you need a hosted tool, by invoking one of the available local tools when appropriate, or by producing normal prose if no tool is needed.";
const DSML_TOOL_CALL_MARKERS: [&str; 4] = [
    "<｜DSML｜tool_calls>",
    "<｜｜DSML｜｜tool_calls>",
    "<|DSML|tool_calls>",
    "<||DSML||tool_calls>",
];

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

fn hosted_tool_name_for_reminder(tool: &HostedToolDefinition) -> &'static str {
    match tool {
        HostedToolDefinition::WebSearch(_) => "web_search",
        HostedToolDefinition::WebFetch(_) => "web_fetch",
    }
}

#[cfg(test)]
fn hosted_tools_for_web_search(
    web_search: &devo_config::ResolvedWebSearchConfig,
) -> Vec<HostedToolDefinition> {
    hosted_tools_for_web_capabilities(web_search, devo_config::ResolvedWebFetchConfig::Disabled)
}

fn estimate_request_prompt_tokens(request: &ModelRequest) -> usize {
    let system_bytes = request.system.as_ref().map_or(0, String::len);
    let message_bytes = request
        .messages
        .iter()
        .map(|message| serde_json::to_string(message).map_or(0, |json| json.len()))
        .sum::<usize>();
    let tool_bytes = request
        .tools
        .as_ref()
        .map(|tools| serde_json::to_string(tools).map_or(0, |json| json.len()))
        .unwrap_or(0);
    let hosted_tool_bytes =
        serde_json::to_string(&request.hosted_tools).map_or(0, |json| json.len());
    (system_bytes + message_bytes + tool_bytes + hosted_tool_bytes).div_ceil(4)
}

/// Events emitted during a query for the caller (CLI/UI) to observe.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// Provider request retry status.
    ProviderRetryStatus(ProviderRetryStatus),
    /// Context compaction is about to begin.
    ContextCompactionStarted,
    /// Context compaction replaced the current prompt history.
    ContextCompactionCompleted,
    /// Context compaction did not replace the current prompt history.
    ContextCompactionFailed {
        /// Human-readable reason the compaction did not complete.
        message: String,
    },
    /// Incremental text from the assistant.
    TextDelta(String),
    /// Incremental reasoning text from the assistant.
    ReasoningDelta(String),
    /// Current reasoning block completed.
    ReasoningCompleted,
    /// Incremental token usage update from the provider stream.
    /// TODO: Review the mechanism from the OpenAI API / Anthropic API documentation.
    UsageDelta { usage: devo_protocol::Usage },
    /// The assistant started a tool call.
    ToolUseStart {
        /// Stable provider-issued tool use identifier.
        id: String,
        /// Tool name selected by the model.
        name: String,
        /// Fully decoded tool input payload, when available.
        input: serde_json::Value,
    },
    /// A locally executed tool has passed permission checks and started running.
    ToolExecutionStart {
        /// Stable provider-issued tool use identifier.
        id: String,
    },
    /// Incremental output delta from a running tool.
    ToolProgress {
        tool_use_id: String,
        progress: crate::tools::ToolProgress,
    },
    /// A tool call completed.
    ToolResult {
        tool_use_id: String,
        tool_name: String,
        input: serde_json::Value,
        content: ToolContent,
        display_content: Option<String>,
        is_error: bool,
        /// Human-readable summary for client-side rendering (e.g. "bash: npm run dev").
        summary: String,
    },
    /// A turn is complete (model stopped generating).
    TurnComplete { stop_reason: StopReason },
    /// Token usage update.
    Usage { usage: devo_protocol::Usage },
}

/// Async sink for streaming `QueryEvent`s out of the core query loop.
///
/// The type is intentionally erased so `query()` can accept callbacks from tests, the server
/// runtime, and tool-progress plumbing without knowing their concrete future types:
///
/// - `Arc`: shared, cheap-to-clone ownership. The same callback is cloned into model-stream and
///   tool-progress paths that may outlive the immediate stack frame.
/// - `dyn Fn(QueryEvent)`: dynamic callback interface. Callers provide any closure that accepts one
///   event and can be invoked repeatedly.
/// - `BoxFuture<'static, ()>`: boxed async work returned by the callback. Boxing hides the
///   closure's concrete future type behind one trait-object shape; `'static` prevents borrowed
///   stack data from escaping into spawned or delayed event paths.
/// - `Send + Sync`: the callback can be shared and awaited across Tokio tasks and worker threads.
///
/// Awaiting this future is what lets callers use bounded async channels for backpressure instead of
/// the old synchronous callback bridge.
pub type EventCallback = Arc<dyn Fn(QueryEvent) -> BoxFuture<'static, ()> + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRetryStatus {
    pub provider: String,
    pub model: String,
    pub attempt: usize,
    pub backoff_ms: u64,
    pub phase: QueryProviderRetryPhase,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryProviderRetryPhase {
    Scheduled,
    Resumed,
}

#[derive(Clone, Default)]
pub struct QueryOptions {
    pub cancel_token: Option<CancellationToken>,
    /// Optional provider used only for compaction summaries. Servers use this
    /// seam to attach Compaction metering without misclassifying the main
    /// streaming query as compaction overhead.
    pub compaction_provider: Option<Arc<dyn ModelProviderSDK>>,
}

impl std::fmt::Debug for QueryOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueryOptions")
            .field("cancel_token", &self.cancel_token)
            .field(
                "compaction_provider",
                &self
                    .compaction_provider
                    .as_ref()
                    .map(|provider| provider.name()),
            )
            .finish()
    }
}

async fn emit_query_event(on_event: &Option<EventCallback>, event: QueryEvent) {
    if let Some(callback) = on_event {
        callback(event).await;
    }
}

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum ErrorClass {
    ContextTooLong,
    ParameterError,
    FileContentAnomaly,
    AuthenticationFailure,
    FeatureUnavailable,
    TaskNotFound,
    RateLimit,
    NoApiPermission,
    FileTooLarge,
    ServerError,
    NetworkError,
    Unretryable,
}

enum ProviderRetryDecision {
    RetryAfter(Duration),
    CompactAndRetry,
    Fail,
}

fn classify_error(e: &anyhow::Error) -> ErrorClass {
    for cause in e.chain() {
        let Some(provider_error) = cause.downcast_ref::<ProviderError>() else {
            continue;
        };
        match provider_error {
            ProviderError::AuthenticationError { .. } => return ErrorClass::AuthenticationFailure,
            ProviderError::RateLimitError { .. } => return ErrorClass::RateLimit,
            ProviderError::ProviderServerError {
                status_code: Some(429),
                ..
            } => return ErrorClass::RateLimit,
            ProviderError::ProviderServerError {
                status_code: Some(408),
                ..
            }
            | ProviderError::ProviderTimeoutError { .. }
            | ProviderError::StreamError { .. } => return ErrorClass::NetworkError,
            ProviderError::ProviderServerError { .. } => return ErrorClass::ServerError,
            ProviderError::ContextLimitError { .. } => return ErrorClass::ContextTooLong,
            ProviderError::ModelNotFoundError { .. } => return ErrorClass::TaskNotFound,
            ProviderError::InvalidRequestError { .. } => return ErrorClass::ParameterError,
            ProviderError::QuotaExceededError { .. }
            | ProviderError::ContentFilteredError { .. } => {
                return ErrorClass::Unretryable;
            }
            ProviderError::UnknownError {
                status_code: Some(429),
                ..
            } => return ErrorClass::RateLimit,
            ProviderError::UnknownError {
                status_code: Some(408),
                ..
            } => return ErrorClass::NetworkError,
            ProviderError::UnknownError {
                status_code: Some(500..=599),
                ..
            } => return ErrorClass::ServerError,
            ProviderError::UnknownError { .. } => {}
        }
    }

    if e.chain().any(|cause| {
        cause
            .downcast_ref::<devo_provider::timeout::StreamIdleTimeoutError>()
            .is_some()
    }) {
        return ErrorClass::NetworkError;
    }

    if e.chain().any(|cause| {
        cause.downcast_ref::<reqwest::Error>().is_some_and(|error| {
            error.is_timeout()
                || error.is_connect()
                || error.status() == Some(reqwest::StatusCode::REQUEST_TIMEOUT)
        })
    }) {
        return ErrorClass::NetworkError;
    }

    if e.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                ErrorKind::TimedOut
                    | ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::NotConnected
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
            )
        })
    }) {
        return ErrorClass::NetworkError;
    }

    let msg = e.to_string().to_lowercase();
    // TODO: Expand the error of ContextTooLong
    if msg.contains("context_too_long") {
        ErrorClass::ContextTooLong
    } else if msg.contains("401")
        || msg.contains("authentication failure")
        || msg.contains("token timeout")
        || msg.contains("unauthorized")
        || msg.contains("api key")
    {
        ErrorClass::AuthenticationFailure
    } else if msg.contains("404")
        && (msg.contains("feature not available")
            || msg.contains("fine-tuning feature not available"))
    {
        ErrorClass::FeatureUnavailable
    } else if msg.contains("404")
        && (msg.contains("task does not exist")
            || msg.contains("does not exist")
            || msg.contains("not found"))
    {
        ErrorClass::TaskNotFound
    } else if msg.contains("429") || msg.contains("rate limit") {
        ErrorClass::RateLimit
    } else if msg.contains("434") || msg.contains("no api permission") || msg.contains("beta phase")
    {
        ErrorClass::NoApiPermission
    } else if msg.contains("435")
        || msg.contains("file size exceeds 100mb")
        || msg.contains("smaller than 100mb")
    {
        ErrorClass::FileTooLarge
    } else if msg.contains("400")
        && (msg.contains("file content anomaly")
            || msg.contains("jsonl file content")
            || msg.contains("jsonl"))
    {
        ErrorClass::FileContentAnomaly
    } else if msg.contains("408")
        || msg.contains("request timeout")
        || msg.contains("request timed out")
        || msg.contains("operation timed out")
        || msg.contains("timed out")
        || msg.contains("deadline has elapsed")
        || msg.contains("deadline exceeded")
        || msg.contains("provider timeout")
        || msg.contains("stream idle timeout")
        || msg.contains("network error")
        || msg.contains("network is unreachable")
        || msg.contains("network unreachable")
        || msg.contains("host unreachable")
        || msg.contains("destination unreachable")
        || msg.contains("unreachable host")
        || msg.contains("no route to host")
        || msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("connection closed")
        || msg.contains("connection aborted")
        || msg.contains("connection timed out")
        || msg.contains("connection failure")
        || msg.contains("connection failed")
        || msg.contains("failed to connect")
        || msg.contains("connect error")
        || msg.contains("error trying to connect")
        || msg.contains("error sending request")
        || msg.contains("dns error")
        || msg.contains("failed to lookup address information")
        || msg.contains("temporary failure in name resolution")
        || msg.contains("name or service not known")
        || msg.contains("nodename nor servname")
        || msg.contains("could not resolve host")
        || msg.contains("unexpected eof")
        || msg.contains("invalidcontenttype")
        || msg.contains("invalid content-type")
        || msg.contains("invalid header value")
        || msg.contains("text/event-stream")
        // Stream-level decode/decrypt errors (e.g. TLS decrypt failure, chunk
        // deserialization).  These are typically transient — the proxy or
        // TLS-terminator that sits between us and the provider may have had a
        // hiccup; retrying usually succeeds.
        || msg.contains("error decoding")
        || msg.contains("decoding response")
        || msg.contains("cannot decrypt")
        || msg.contains("decrypt error")
        || msg.contains("decrypterror")
        || msg.contains("stream error")
        || msg.contains("failed to decode")
    {
        ErrorClass::NetworkError
    } else if msg.contains("400")
        || msg.contains("parameter error")
        || msg.contains("invalid parameter")
        || msg.contains("bad request")
    {
        ErrorClass::ParameterError
    } else if msg.starts_with('5')
        || msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("internal server error")
        || msg.contains("server error occurred while processing the request")
    {
        ErrorClass::ServerError
    } else {
        ErrorClass::Unretryable
    }
}

fn provider_retry_decision(
    error: &anyhow::Error,
    retry_count: &mut usize,
    context_compacted: &mut bool,
) -> ProviderRetryDecision {
    match classify_error(error) {
        ErrorClass::ContextTooLong => {
            if *context_compacted {
                ProviderRetryDecision::Fail
            } else {
                *context_compacted = true;
                ProviderRetryDecision::CompactAndRetry
            }
        }
        ErrorClass::RateLimit => {
            if *retry_count >= MAX_RETRIES {
                ProviderRetryDecision::Fail
            } else {
                *retry_count += 1;
                ProviderRetryDecision::RetryAfter(RATE_LIMIT_RETRY_DELAY)
            }
        }
        ErrorClass::ServerError | ErrorClass::NetworkError => {
            if *retry_count >= MAX_RETRIES {
                ProviderRetryDecision::Fail
            } else {
                *retry_count += 1;
                ProviderRetryDecision::RetryAfter(retry_backoff_duration(*retry_count))
            }
        }
        ErrorClass::ParameterError
        | ErrorClass::FileContentAnomaly
        | ErrorClass::AuthenticationFailure
        | ErrorClass::FeatureUnavailable
        | ErrorClass::TaskNotFound
        | ErrorClass::NoApiPermission
        | ErrorClass::FileTooLarge
        | ErrorClass::Unretryable => ProviderRetryDecision::Fail,
    }
}

// ---------------------------------------------------------------------------
// Session compaction
// ---------------------------------------------------------------------------

/// Compact session messages using LLM-backed summarization.
///
/// `kind` selects the preserve strategy inside [`compact_history`]:
/// - [`CompactionKind::Auto`]: preventive compaction when the session token
///   budget is high; keeps a tail token window.
/// - [`CompactionKind::Proactive`]: forced compaction after provider
///   `context_too_long`; keeps from the latest user message onward.
async fn summarize_and_compact(
    session: &mut SessionState,
    on_event: &Option<EventCallback>,
    provider: &Arc<dyn ModelProviderSDK>,
    model_slug: &str,
    request_model: &str,
    max_tokens: usize,
    kind: CompactionKind,
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
        Arc::clone(provider),
        model_slug,
        request_model,
        max_tokens,
    );

    emit_query_event(on_event, QueryEvent::ContextCompactionStarted).await;
    match compact_history(&items, &token_info, &summarizer, &config).await {
        Ok(CompactAction::Replaced(compacted_items)) => {
            let new_messages: Vec<Message> = compacted_items
                .into_iter()
                .filter_map(|item| match item {
                    ResponseItem::Message(msg) => Some(msg),
                    _ => None,
                })
                .collect();
            let removed = session
                .prompt_source_messages()
                .len()
                .saturating_sub(new_messages.len());
            info!("LLM compaction removed {removed} messages");
            session.set_prompt_messages(new_messages);
            emit_query_event(on_event, QueryEvent::ContextCompactionCompleted).await;
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
        [request_text_message(SUBAGENT_MODE_REMINDER.to_string())],
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

fn tool_content_model_bytes(content: &ToolContent) -> usize {
    match content {
        ToolContent::Text(text) => text.len(),
        ToolContent::Json(json) => json.to_string().len(),
        ToolContent::Mixed { text, json } => {
            text.as_ref().map_or(0, String::len)
                + json.as_ref().map_or(0, |json| json.to_string().len())
        }
    }
}

fn normalize_hosted_tool_id(index: usize, id: String, name: &str) -> String {
    if id.is_empty() {
        format!("hosted_{}_{index}", name.replace('-', "_"))
    } else {
        id
    }
}

fn normalize_hosted_tool_name(name: String) -> String {
    if name.is_empty() {
        "web_search".to_string()
    } else {
        name
    }
}

fn hosted_tool_input_or_previous(
    input: serde_json::Value,
    previous: Option<&serde_json::Value>,
) -> serde_json::Value {
    if matches!(&input, serde_json::Value::Object(map) if map.is_empty()) {
        previous.cloned().unwrap_or(input)
    } else {
        input
    }
}

async fn emit_hosted_tool_start(
    on_event: &Option<EventCallback>,
    emitted_tool_use_starts: &mut HashSet<String>,
    id: &str,
    name: &str,
    input: &serde_json::Value,
) {
    if emitted_tool_use_starts.insert(id.to_string()) {
        emit_query_event(
            on_event,
            QueryEvent::ToolUseStart {
                id: id.to_string(),
                name: name.to_string(),
                input: input.clone(),
            },
        )
        .await;
    }
}

struct HostedToolResultEvent<'a> {
    id: &'a str,
    name: &'a str,
    input: &'a serde_json::Value,
    output: Option<serde_json::Value>,
    status: Option<String>,
}

async fn emit_hosted_tool_result(
    on_event: &Option<EventCallback>,
    emitted_tool_results: &mut HashSet<String>,
    session_cwd: &std::path::Path,
    event: HostedToolResultEvent<'_>,
) {
    let HostedToolResultEvent {
        id,
        name,
        input,
        output,
        status,
    } = event;
    if !emitted_tool_results.insert(id.to_string()) {
        return;
    }

    let text = hosted_tool_result_text(name, input, output.as_ref(), status.as_deref());
    let content = if output.is_some() {
        ToolContent::Mixed {
            text: Some(text.clone()),
            json: output.clone(),
        }
    } else {
        ToolContent::Text(text.clone())
    };
    let summary = crate::tools::tool_summary::tool_summary(name, input, session_cwd);
    emit_query_event(
        on_event,
        QueryEvent::ToolResult {
            tool_use_id: id.to_string(),
            tool_name: name.to_string(),
            input: input.clone(),
            content,
            display_content: Some(text),
            is_error: hosted_tool_status_is_error(status.as_deref()),
            summary,
        },
    )
    .await;
}

fn hosted_tool_result_text(
    _name: &str,
    _input: &serde_json::Value,
    _output: Option<&serde_json::Value>,
    status: Option<&str>,
) -> String {
    let status = status
        .filter(|status| !status.is_empty())
        .unwrap_or("completed");
    format!("status: {status}")
}
fn hosted_tool_status_is_error(status: Option<&str>) -> bool {
    status
        .map(str::to_ascii_lowercase)
        .is_some_and(|status| matches!(status.as_str(), "error" | "errored" | "failed"))
}

fn assistant_content_contains_dsml_tool_call_text(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| match block {
        ContentBlock::Text { text } => DSML_TOOL_CALL_MARKERS
            .iter()
            .any(|marker| text.contains(marker)),
        ContentBlock::Reasoning { .. }
        | ContentBlock::ProviderReasoning { .. }
        | ContentBlock::ToolUse { .. }
        | ContentBlock::HostedToolUse { .. }
        | ContentBlock::ToolResult { .. } => false,
    })
}

fn assistant_content_has_visible_content(content: &[ContentBlock]) -> bool {
    content.iter().any(|block| match block {
        ContentBlock::Text { text }
        | ContentBlock::Reasoning { text }
        | ContentBlock::ToolResult { content: text, .. } => !text.trim().is_empty(),
        ContentBlock::ProviderReasoning { .. }
        | ContentBlock::ToolUse { .. }
        | ContentBlock::HostedToolUse { .. } => true,
    })
}

fn dsml_text_tool_call_continuation_message(
    request_tools: &[ToolDefinition],
    hosted_tools: &[HostedToolDefinition],
) -> Message {
    let mut reminder = String::from("<system-reminder>\n");
    reminder.push_str(DSML_TEXT_TOOL_CALL_CONTINUATION_REMINDER);
    let local_tool_names = request_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    if !local_tool_names.is_empty() {
        reminder.push_str("\n\nAvailable local tools: ");
        reminder.push_str(&local_tool_names.join(", "));
        reminder.push('.');
    }
    let hosted_tool_names = hosted_tools
        .iter()
        .map(hosted_tool_name_for_reminder)
        .collect::<Vec<_>>();
    if !hosted_tool_names.is_empty() {
        reminder.push_str("\nAvailable hosted tools: ");
        reminder.push_str(&hosted_tool_names.join(", "));
        reminder.push_str(". Hosted tools must be invoked through provider-native server tool calls, not by writing DSML tags in text.");
    }
    if local_tool_names.contains(&"spawn_agent") && local_tool_names.contains(&"await_task") {
        reminder.push_str("\nFor research work with separable subtasks, prefer spawning independent agents first and then waiting for their results.");
    }
    reminder.push_str("\n</system-reminder>");
    Message::user(reminder)
}

// ---------------------------------------------------------------------------
// Main query loop
// ---------------------------------------------------------------------------

const MAX_RETRIES: usize = 5;
const INITIAL_RETRY_BACKOFF_MS: u64 = 250;
const RATE_LIMIT_RETRY_DELAY: Duration = Duration::from_secs(60);

/// TODO: The body of `query` is too lengthy, we should move out `stream lop` out, I am
/// not sure whether we should do this.
/// The recursive agent loop the beating heart of the runtime.
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
) -> Result<(), AgentError> {
    query_with_options(
        session,
        turn_config,
        provider,
        registry,
        runtime,
        on_event,
        QueryOptions::default(),
    )
    .await
}

pub async fn query_with_options(
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
    let deepseek_v4_thinking_only_continuation_enabled =
        turn_config.model.slug.starts_with("deepseek-v4-")
            || turn_config.request_model.starts_with("deepseek-v4-");
    let mut deepseek_v4_thinking_only_continuation_used = false;
    let mut dsml_text_tool_call_continuations = 0usize;

    if session.turn_state.is_none() {
        session.start_turn(devo_protocol::TurnKind::Regular);
    }

    let compaction_model_slug = turn_config
        .model
        .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref())
        .request_model;
    let compaction_request_model = turn_config.provider_request_model(&compaction_model_slug);

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

    'query_loop: loop {
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

        // 1.3 + 1.7: Check token budget and compact before building the request
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
            // Auto: preserve tail items up to COMPACT_USER_MESSAGE_MAX_TOKENS.
            // Example: [user1, asst1, user2, asst2, user3] -> [summary, asst2, user3].
            summarize_and_compact(
                session,
                &on_event,
                &compaction_provider,
                &compaction_model_slug,
                &compaction_request_model,
                turn_config.model.max_tokens.unwrap_or(4096) as usize,
                CompactionKind::Auto,
            )
            .await;
        }

        session.turn_count += 1;
        let turn_span = info_span!(
            "turn",
            turn = session.turn_count,
            session_id = %session.id,
            model = %turn_config.model.slug,
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

        // Resolve provider-bound reasoning request parameters.
        let ResolvedReasoningRequest {
            request_model,
            request_thinking,
            request_reasoning_effort,
            extra_body,
            effective_reasoning_effort: _,
        } = turn_config
            .model
            .resolve_reasoning_effort_selection(turn_config.reasoning_effort_selection.as_deref());
        let catalog_request_model = request_model.clone();
        let provider_request_model = turn_config.provider_request_model(&catalog_request_model);

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
        let mut messages = history
            .for_prompt_with_prefix(&prefetched_user_inputs, &turn_config.model.input_modalities);
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
            max_tokens: turn_config
                .model
                .max_tokens
                .map_or(session.config.token_budget.max_output_tokens, |value| {
                    value as usize
                }),
            tools: Some(request_tools.clone()),
            hosted_tools: hosted_tools.clone(),
            sampling: SamplingControls {
                temperature: turn_config.model.temperature,
                top_p: turn_config.model.top_p,
                top_k: turn_config.model.top_k.map(|value| value as u32),
            },
            request_thinking,
            reasoning_effort: request_reasoning_effort,
            extra_body,
        };
        session.prompt_token_estimate = estimate_request_prompt_tokens(&request);
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

        // Stream with error classification
        let stream_result = provider.completion_stream(request).await;

        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    provider = provider.name(),
                    model = %turn_config.model.slug,
                    turn = session.turn_count,
                    error = ?e,
                    "failed to create provider stream"
                );
                match provider_retry_decision(&e, &mut retry_count, &mut context_compacted) {
                    ProviderRetryDecision::CompactAndRetry => {
                        warn!("context_too_long - compacting and retrying");
                        // Proactive: must compact even if token estimates disagree
                        // with the provider; preserve from latest user only.
                        summarize_and_compact(
                            session,
                            &on_event,
                            &compaction_provider,
                            &compaction_model_slug,
                            &compaction_request_model,
                            turn_config.model.max_tokens.unwrap_or(4096) as usize,
                            CompactionKind::Proactive,
                        )
                        .await;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::RetryAfter(backoff) => {
                        warn!(
                            attempt = retry_count,
                            backoff_ms = backoff.as_millis(),
                            "transient provider error - retrying with exponential backoff"
                        );
                        wait_for_provider_retry(
                            &on_event,
                            options.cancel_token.as_ref(),
                            provider.name(),
                            &turn_config.model.slug,
                            retry_count,
                            backoff,
                        )
                        .await?;
                        session.turn_count -= 1;
                        continue;
                    }
                    ProviderRetryDecision::Fail => {
                        return Err(AgentError::Provider(e));
                    }
                }
            }
        };

        // HTTP return ok, then processing Server Sent Event

        let mut assistant_text = String::new();
        let mut reasoning_text = String::new();
        let mut tool_uses: Vec<(usize, String, String, serde_json::Value, String, bool)> =
            Vec::new();
        let mut hosted_tool_inputs: HashMap<String, (usize, String, serde_json::Value)> =
            HashMap::new();
        let mut emitted_tool_use_starts: HashSet<String> = HashSet::new();
        let mut emitted_hosted_tool_starts: HashSet<String> = HashSet::new();
        let mut emitted_hosted_tool_results: HashSet<String> = HashSet::new();
        let mut final_response = None;
        let mut stop_reason = None;

        loop {
            tokio::select! {
                biased;
                _ = async {
                    if let Some(ct) = options.cancel_token.as_ref() {
                        ct.cancelled().await
                    } else {
                        std::future::pending::<()>().await
                    }
                } =>
                {
                    break;
                }
                event = stream.next() => {
                    let Some(event) = event else { break; };
                    match event {
                        Ok(StreamEvent::TextStart { .. }) => {}
                        Ok(StreamEvent::TextDelta { text, .. }) => {
                            assistant_text.push_str(&text);
                            emit_query_event(&on_event, QueryEvent::TextDelta(text)).await;
                        }
                        Ok(StreamEvent::ReasoningStart { .. }) => {}
                        Ok(StreamEvent::ReasoningDelta { text, .. }) => {
                            reasoning_text.push_str(&text);
                            emit_query_event(&on_event, QueryEvent::ReasoningDelta(text)).await;
                        }
                        Ok(StreamEvent::ReasoningDone { .. }) => {
                            emit_query_event(&on_event, QueryEvent::ReasoningCompleted).await;
                        }
                        Ok(StreamEvent::ToolCallStart {
                            index,
                            id,
                            name,
                            input,
                        }) => {
                            tool_uses.push((index, id, name, input, String::new(), false));
                        }
                        Ok(StreamEvent::HostedToolCallStart {
                            index,
                            id,
                            name,
                            input,
                        }) => {
                            let id = normalize_hosted_tool_id(index, id, &name);
                            let name = normalize_hosted_tool_name(name);
                            hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                            emit_hosted_tool_start(
                                &on_event,
                                &mut emitted_hosted_tool_starts,
                                &id,
                                &name,
                                &input,
                            )
                            .await;
                        }
                        Ok(StreamEvent::HostedToolCallDone {
                            index,
                            id,
                            name,
                            input,
                            output,
                            status,
                        }) => {
                            let id = normalize_hosted_tool_id(index, id, &name);
                            let name = normalize_hosted_tool_name(name);
                            let previous_input = hosted_tool_inputs
                                .get(&id)
                                .map(|(_, _, previous_input)| previous_input);
                            let input = hosted_tool_input_or_previous(input, previous_input);
                            hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                            emit_hosted_tool_start(
                                &on_event,
                                &mut emitted_hosted_tool_starts,
                                &id,
                                &name,
                                &input,
                            )
                            .await;
                            emit_hosted_tool_result(
                                &on_event,
                                &mut emitted_hosted_tool_results,
                                &session.cwd,
                                HostedToolResultEvent {
                                    id: &id,
                                    name: &name,
                                    input: &input,
                                    output,
                                    status,
                                },
                            )
                            .await;
                        }
                        Ok(StreamEvent::ToolCallInputDelta {
                            index,
                            partial_json,
                        }) => {
                            if let Some(tool_use) = tool_uses
                                .iter_mut()
                                .rev()
                                .find(|(tool_index, ..)| *tool_index == index)
                            {
                                tool_use.4.push_str(&partial_json);
                                tool_use.5 = true;
                            }
                        }
                        Ok(StreamEvent::MessageDone { response }) => {
                            stop_reason = response.stop_reason.clone();
                            final_response = Some(response.clone());

                            // Accumulate all usage counters at completion time.
                            session.total_input_tokens += response.usage.input_tokens;
                            session.total_output_tokens += response.usage.output_tokens;
                            session.total_tokens += response.usage.display_total_tokens();
                            session.total_cache_creation_tokens +=
                                response.usage.cache_creation_input_tokens.unwrap_or(0);
                            session.total_cache_read_tokens +=
                                response.usage.cache_read_input_tokens.unwrap_or(0);
                            session.last_input_tokens = response.usage.input_tokens;
                            session.last_turn_tokens = response.usage.display_total_tokens();

                            emit_query_event(
                                &on_event,
                                QueryEvent::Usage {
                                    usage: response.usage.clone(),
                                },
                            )
                            .await;
                        }
                        Ok(StreamEvent::UsageDelta(usage)) => {
                            emit_query_event(&on_event, QueryEvent::UsageDelta { usage }).await;
                        }
                        Err(e) => {
                            warn!(
                                provider = provider.name(),
                                model = %turn_config.model.slug,
                                turn = session.turn_count,
                                error = ?e,
                                "stream error"
                            );
                            if !assistant_text.is_empty()
                                || !reasoning_text.is_empty()
                                || !tool_uses.is_empty()
                                || !hosted_tool_inputs.is_empty()
                                || final_response.is_some()
                            {
                                return Err(AgentError::Provider(e));
                            }

                            match provider_retry_decision(&e, &mut retry_count, &mut context_compacted) {
                                ProviderRetryDecision::CompactAndRetry => {
                                    warn!("context_too_long - compacting and retrying");
                                    // Proactive: must compact even if token estimates disagree
                                    // with the provider; preserve from latest user only.
                                    summarize_and_compact(
                                        session,
                                        &on_event,
                                        &compaction_provider,
                                        &compaction_model_slug,
                                        &compaction_request_model,
                                        turn_config.model.max_tokens.unwrap_or(4096) as usize,
                                        CompactionKind::Proactive,
                                    )
                                    .await;
                                    session.turn_count -= 1;
                                    continue 'query_loop;
                                }
                                ProviderRetryDecision::RetryAfter(backoff) => {
                                    warn!(
                                        attempt = retry_count,
                                        backoff_ms = backoff.as_millis(),
                                        "transient provider stream error - retrying with exponential backoff"
                                    );
                                    wait_for_provider_retry(
                                        &on_event,
                                        options.cancel_token.as_ref(),
                                        provider.name(),
                                        &turn_config.model.slug,
                                        retry_count,
                                        backoff,
                                    )
                                    .await?;
                                    session.turn_count -= 1;
                                    continue 'query_loop;
                                }
                                ProviderRetryDecision::Fail => {
                                    return Err(AgentError::Provider(e));
                                }
                            }
                        }
                    }
                }
            }
        }

        retry_count = 0;
        context_compacted = false;

        let mut response_assistant_content = Vec::new();
        let mut final_response_tool_use_ids = HashSet::new();
        let mut has_provider_reasoning_content = false;
        let mut has_hosted_tool_uses = false;
        if let Some(response) = &final_response {
            let has_provider_reasoning = response
                .content
                .iter()
                .any(|block| matches!(block, ResponseContent::ProviderReasoning { .. }));
            if assistant_text.is_empty() {
                assistant_text = response
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ResponseContent::Text(text) => Some(text.as_str()),
                        ResponseContent::ToolUse { .. }
                        | ResponseContent::HostedToolUse { .. }
                        | ResponseContent::ProviderReasoning { .. } => None,
                    })
                    .collect();
            }
            if tool_uses.is_empty() {
                tool_uses = response
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(|(index, block)| match block {
                        ResponseContent::ToolUse { id, name, input } => Some((
                            index,
                            id.clone(),
                            name.clone(),
                            input.clone(),
                            String::new(),
                            false,
                        )),
                        ResponseContent::Text(_)
                        | ResponseContent::HostedToolUse { .. }
                        | ResponseContent::ProviderReasoning { .. } => None,
                    })
                    .collect();
            }
            for (index, block) in response.content.iter().enumerate() {
                match block {
                    ResponseContent::Text(text) => {
                        if !text.is_empty() {
                            response_assistant_content
                                .push(ContentBlock::Text { text: text.clone() });
                        }
                    }
                    ResponseContent::ToolUse { id, name, input } => {
                        final_response_tool_use_ids.insert(id.clone());
                        response_assistant_content.push(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });
                    }
                    ResponseContent::HostedToolUse {
                        id,
                        name,
                        input,
                        output,
                        status,
                    } => {
                        let id = normalize_hosted_tool_id(index, id.clone(), name);
                        let name = normalize_hosted_tool_name(name.clone());
                        let previous_input = hosted_tool_inputs
                            .get(&id)
                            .map(|(_, _, previous_input)| previous_input);
                        let input = hosted_tool_input_or_previous(input.clone(), previous_input);
                        has_hosted_tool_uses = true;
                        response_assistant_content.push(ContentBlock::HostedToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            output: output.clone(),
                            status: status.clone(),
                        });
                        hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                        emit_hosted_tool_start(
                            &on_event,
                            &mut emitted_hosted_tool_starts,
                            &id,
                            &name,
                            &input,
                        )
                        .await;
                        if output.is_some() || status.is_some() {
                            emit_hosted_tool_result(
                                &on_event,
                                &mut emitted_hosted_tool_results,
                                &session.cwd,
                                HostedToolResultEvent {
                                    id: &id,
                                    name: &name,
                                    input: &input,
                                    output: output.clone(),
                                    status: status.clone(),
                                },
                            )
                            .await;
                        }
                    }
                    ResponseContent::ProviderReasoning { provider, payload } => {
                        has_provider_reasoning_content = true;
                        response_assistant_content.push(ContentBlock::ProviderReasoning {
                            provider: provider.clone(),
                            payload: payload.clone(),
                        });
                    }
                }
            }
            if reasoning_text.is_empty() && has_provider_reasoning {
                let final_reasoning = response_assistant_content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ProviderReasoning { payload, .. } => {
                            payload.get("thinking").and_then(serde_json::Value::as_str)
                        }
                        ContentBlock::Text { .. }
                        | ContentBlock::Reasoning { .. }
                        | ContentBlock::ToolUse { .. }
                        | ContentBlock::HostedToolUse { .. }
                        | ContentBlock::ToolResult { .. } => None,
                    })
                    .collect::<String>();
                if !final_reasoning.is_empty() {
                    emit_query_event(
                        &on_event,
                        QueryEvent::ReasoningDelta(final_reasoning.clone()),
                    )
                    .await;
                    emit_query_event(&on_event, QueryEvent::ReasoningCompleted).await;
                    reasoning_text = final_reasoning;
                }
            }
            if reasoning_text.is_empty() && !has_provider_reasoning {
                let final_reasoning = response
                    .metadata
                    .extras
                    .iter()
                    .filter_map(|extra| match extra {
                        ResponseExtra::ReasoningText { text } => Some(text.as_str()),
                        ResponseExtra::ProviderSpecific { .. } => None,
                    })
                    .collect::<String>();
                if !final_reasoning.is_empty() {
                    emit_query_event(
                        &on_event,
                        QueryEvent::ReasoningDelta(final_reasoning.clone()),
                    )
                    .await;
                    emit_query_event(&on_event, QueryEvent::ReasoningCompleted).await;
                    reasoning_text = final_reasoning;
                }
            }
        }

        let pending_hosted_tools = hosted_tool_inputs
            .iter()
            .map(|(id, (_index, name, input))| (id.clone(), name.clone(), input.clone()))
            .collect::<Vec<_>>();
        for (id, name, input) in pending_hosted_tools {
            emit_hosted_tool_start(
                &on_event,
                &mut emitted_hosted_tool_starts,
                &id,
                &name,
                &input,
            )
            .await;
            emit_hosted_tool_result(
                &on_event,
                &mut emitted_hosted_tool_results,
                &session.cwd,
                HostedToolResultEvent {
                    id: &id,
                    name: &name,
                    input: &input,
                    output: None,
                    status: Some("completed".to_string()),
                },
            )
            .await;
        }

        // Build assistant message
        let mut assistant_content: Vec<ContentBlock> = response_assistant_content;

        if !reasoning_text.trim().is_empty() && !has_provider_reasoning_content {
            assistant_content.insert(
                0,
                ContentBlock::Reasoning {
                    text: reasoning_text,
                },
            );
        }

        let assistant_text_has_visible_content = !assistant_text.trim().is_empty();

        if assistant_content.is_empty() && !assistant_text.is_empty() {
            assistant_content.push(ContentBlock::Text {
                text: assistant_text,
            });
        }

        let deepseek_v4_thinking_only_end_turn = deepseek_v4_thinking_only_continuation_enabled
            && stop_reason == Some(StopReason::EndTurn)
            && !assistant_text_has_visible_content
            && tool_uses.is_empty()
            && !has_hosted_tool_uses
            && has_provider_reasoning_content;

        let final_tool_inputs: HashMap<String, serde_json::Value> = final_response
            .as_ref()
            .map(|response| {
                response
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ResponseContent::ToolUse { id, input, .. } => {
                            Some((id.clone(), input.clone()))
                        }
                        ResponseContent::Text(_)
                        | ResponseContent::HostedToolUse { .. }
                        | ResponseContent::ProviderReasoning { .. } => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut tool_calls = Vec::with_capacity(tool_uses.len());
        for (_index, id, name, initial_input, json_str, saw_delta) in tool_uses {
            let input = if saw_delta {
                serde_json::from_str(&json_str).unwrap_or_else(|_| {
                    final_tool_inputs.get(&id).cloned().unwrap_or(initial_input)
                })
            } else {
                final_tool_inputs.get(&id).cloned().unwrap_or(initial_input)
            };
            if emitted_tool_use_starts.insert(id.clone()) {
                emit_query_event(
                    &on_event,
                    QueryEvent::ToolUseStart {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                )
                .await;
            }
            if !final_response_tool_use_ids.contains(&id) {
                assistant_content.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
            }
            tool_calls.push(ToolCall { id, name, input });
        }

        let assistant_content_contains_dsml_tool_call =
            assistant_content_contains_dsml_tool_call_text(&assistant_content);
        if assistant_content_has_visible_content(&assistant_content) {
            session.push_message(Message {
                role: Role::Assistant,
                content: assistant_content,
            });
        }

        if deepseek_v4_thinking_only_end_turn {
            if deepseek_v4_thinking_only_continuation_used {
                return Err(AgentError::Provider(anyhow::anyhow!(
                    "deepseek-v4 returned thinking-only end_turn after continuation; no user-visible text was produced"
                )));
            }
            debug!("deepseek-v4 returned thinking-only end_turn; injecting continuation prompt");
            deepseek_v4_thinking_only_continuation_used = true;
            session.push_message(Message::user(DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT));
            continue;
        }

        // If no tool calls, check stop reason
        if tool_calls.is_empty() {
            if has_hosted_tool_uses && stop_reason == Some(StopReason::ToolUse) {
                debug!("hosted tool use returned without local calls, continuing query loop");
                continue;
            }

            if assistant_content_contains_dsml_tool_call {
                if dsml_text_tool_call_continuations >= MAX_DSML_TEXT_TOOL_CALL_CONTINUATIONS {
                    return Err(AgentError::Provider(anyhow::anyhow!(
                        "provider returned DSML text tool calls {MAX_DSML_TEXT_TOOL_CALL_CONTINUATIONS} times without structured or hosted tool results"
                    )));
                }
                dsml_text_tool_call_continuations += 1;
                debug!(
                    "DSML text tool call returned without structured tool result; continuing query loop"
                );
                session.push_message(dsml_text_tool_call_continuation_message(
                    &request_tools,
                    &hosted_tools,
                ));
                continue;
            }

            // MaxOutputTokens auto-continue
            if stop_reason == Some(StopReason::MaxTokens) {
                debug!("max_tokens reached injecting continuation prompt");
                session.push_message(Message::user("Please continue from where you left off."));
                continue;
            }

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
            .map(|result| tool_content_model_bytes(&result.content))
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
                let content_str = r.content.into_string();
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

async fn wait_for_provider_retry(
    on_event: &Option<EventCallback>,
    cancel_token: Option<&CancellationToken>,
    provider: &str,
    model: &str,
    attempt: usize,
    backoff: Duration,
) -> Result<(), AgentError> {
    let backoff_ms = backoff.as_millis().min(u128::from(u64::MAX)) as u64;
    emit_query_event(
        on_event,
        QueryEvent::ProviderRetryStatus(ProviderRetryStatus {
            provider: provider.to_string(),
            model: model.to_string(),
            attempt,
            backoff_ms,
            phase: QueryProviderRetryPhase::Scheduled,
            message: format!("Retrying provider request in {:.1}s", backoff.as_secs_f64()),
        }),
    )
    .await;

    if let Some(cancel_token) = cancel_token {
        tokio::select! {
            biased;
            () = cancel_token.cancelled() => return Err(AgentError::Aborted),
            () = sleep(backoff) => {}
        }
    } else {
        sleep(backoff).await;
    }

    emit_query_event(
        on_event,
        QueryEvent::ProviderRetryStatus(ProviderRetryStatus {
            provider: provider.to_string(),
            model: model.to_string(),
            attempt,
            backoff_ms: 0,
            phase: QueryProviderRetryPhase::Resumed,
            message: "Retrying provider request now".to_string(),
        }),
    )
    .await;

    Ok(())
}

fn retry_backoff_duration(attempt: usize) -> Duration {
    let exponent = attempt.saturating_sub(1).min(10) as u32;
    let multiplier = 2u64.pow(exponent);
    Duration::from_millis(INITIAL_RETRY_BACKOFF_MS.saturating_mul(multiplier))
}

#[cfg(test)]
mod tests {
    use devo_protocol::Usage;
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use crate::EventCallback;
    use crate::ProviderRetryStatus;
    use crate::QueryProviderRetryPhase;
    use crate::tools::ToolAgentScope;
    use crate::tools::ToolContent;
    use crate::tools::ToolPreparationFeedback;
    use crate::tools::ToolRegistry;
    use crate::tools::ToolRuntime;
    use crate::tools::ToolRuntimeContext;
    use crate::tools::json_schema::JsonSchema;
    use crate::tools::registry::ToolExposure;
    use crate::tools::registry::ToolRegistryBuilder;
    use crate::tools::router::PermissionChecker;
    use crate::tools::router::ToolExecutionOptions;
    use crate::tools::tool_handler::ToolHandler;
    use crate::tools::tool_spec::ToolExecutionMode;
    use crate::tools::tool_spec::ToolOutputMode;
    use crate::tools::tool_spec::ToolSpec;
    use anyhow::Result;
    use async_trait::async_trait;
    use devo_protocol::CollaborationMode;
    use devo_protocol::ModelRequest;
    use devo_protocol::ModelResponse;
    use devo_protocol::RequestContent;
    use devo_protocol::RequestMessage;
    use devo_protocol::ResponseContent;
    use devo_protocol::ResponseExtra;
    use devo_protocol::ResponseMetadata;
    use devo_protocol::StopReason;
    use devo_protocol::StreamEvent;
    use devo_protocol::ThreadGoal;
    use devo_protocol::ThreadGoalStatus;
    use devo_provider::ModelProviderSDK;
    use devo_safety::PermissionMode;
    use futures::Stream;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::QueryEvent;
    use super::QueryOptions;
    use super::hosted_tools_for_web_search;
    use super::insert_subagent_request_reminders;
    use super::query;
    use super::query_with_options;
    use super::test_model_connection;
    use super::truncate_tool_result_for_model;
    use crate::AgentError;
    use crate::ContentBlock;
    use crate::Message;
    use crate::Model;
    use crate::ReasoningEffort;
    use crate::Role;
    use crate::context::ContextualUserFragment;
    use crate::context::compaction_summary::CompactionSummary;
    use crate::history::compaction::CompactionKind;
    use crate::response_item::ResponseItem;

    #[test]
    fn assistant_content_visibility_requires_visible_content() {
        assert!(!super::assistant_content_has_visible_content(&[]));
        assert!(!super::assistant_content_has_visible_content(&[
            ContentBlock::Text {
                text: " \n\t".to_string(),
            },
        ]));
        assert!(!super::assistant_content_has_visible_content(&[
            ContentBlock::ToolResult {
                tool_use_id: "call-1".to_string(),
                content: String::new(),
                is_error: false,
            },
        ]));

        for content in [
            vec![ContentBlock::Text {
                text: "visible".to_string(),
            }],
            vec![ContentBlock::Reasoning {
                text: "reasoning".to_string(),
            }],
            vec![ContentBlock::ProviderReasoning {
                provider: "test".to_string(),
                payload: serde_json::json!({"thinking":"hidden"}),
            }],
            vec![ContentBlock::ToolUse {
                id: "call-1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"filePath":"README.md"}),
            }],
            vec![ContentBlock::HostedToolUse {
                id: "hosted-1".to_string(),
                name: "web_search".to_string(),
                input: serde_json::json!({"query":"docs"}),
                output: None,
                status: None,
            }],
            vec![ContentBlock::ToolResult {
                tool_use_id: "call-1".to_string(),
                content: "result".to_string(),
                is_error: false,
            }],
        ] {
            assert!(super::assistant_content_has_visible_content(&content));
        }
    }

    #[test]
    fn hosted_tools_follow_resolved_web_search_mode() {
        let hosted = hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Provider);
        assert_eq!(hosted.len(), 1);
        assert!(matches!(
            hosted.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));

        assert_eq!(
            hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Disabled),
            Vec::new()
        );
        assert_eq!(
            hosted_tools_for_web_search(&devo_config::ResolvedWebSearchConfig::Local(
                devo_config::ResolvedLocalWebSearchConfig {
                    provider_id: "test".to_string(),
                    kind: devo_config::LocalWebSearchProviderKind::Exa,
                    api_key: "secret".to_string(),
                    base_url: None,
                    max_results: None,
                },
            )),
            Vec::new()
        );
    }
    use crate::ReasoningCapability;
    use crate::ReasoningImplementation;

    #[test]
    fn network_errors_are_retryable() {
        let cases = [
            anyhow::anyhow!("request timed out while connecting"),
            anyhow::anyhow!(
                "error sending request for url (https://api.example.test): connection refused"
            ),
            anyhow::anyhow!("dns error: failed to lookup address information"),
            anyhow::anyhow!("network is unreachable"),
            anyhow::anyhow!(
                "anthropic stream error for model deepseek-v4-flash: invalid header value: \"text/html; charset=utf-8\"; debug=InvalidContentType(\"text/html; charset=utf-8\")"
            ),
            anyhow::anyhow!("Invalid status code: 408 Request Timeout"),
            anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "socket timed out",
            )),
            anyhow::Error::new(devo_provider::error::ProviderError::ProviderTimeoutError {
                message: "provider request timed out".into(),
                provider_name: Some("test-provider".into()),
            }),
            anyhow::Error::new(devo_provider::timeout::stream_idle_timeout_provider_error(
                "openai",
                "gpt-test",
                devo_provider::timeout::StreamIdleTimeoutError {
                    idle_timeout: std::time::Duration::from_secs(60),
                },
            )),
            anyhow::Error::new(devo_provider::timeout::StreamIdleTimeoutError {
                idle_timeout: std::time::Duration::from_secs(60),
            }),
            anyhow::anyhow!(
                "openai stream idle timeout for model gpt-test: provider stream idle timeout after 60s without receiving data"
            ),
        ];

        for error in cases {
            assert_eq!(
                super::classify_error(&error),
                super::ErrorClass::NetworkError
            );

            let mut retry_count = 0;
            let mut context_compacted = false;
            assert!(matches!(
                super::provider_retry_decision(&error, &mut retry_count, &mut context_compacted),
                super::ProviderRetryDecision::RetryAfter(_)
            ));
            assert_eq!(retry_count, 1);
            assert!(!context_compacted);
        }
    }

    #[test]
    fn token_timeout_remains_authentication_failure() {
        let error = anyhow::anyhow!("token timeout");

        assert_eq!(
            super::classify_error(&error),
            super::ErrorClass::AuthenticationFailure
        );

        let mut retry_count = 0;
        let mut context_compacted = false;
        assert!(matches!(
            super::provider_retry_decision(&error, &mut retry_count, &mut context_compacted),
            super::ProviderRetryDecision::Fail
        ));
        assert_eq!(retry_count, 0);
        assert!(!context_compacted);
    }
    use crate::ReasoningVariant;
    use crate::ReasoningVariantConfig;
    use crate::SessionConfig;
    use crate::SessionState;
    use crate::TruncationMode;
    use crate::TruncationPolicyConfig;
    use crate::TurnConfig;

    #[test]
    fn model_tool_result_truncation_preserves_content_within_budget() {
        assert_eq!(
            truncate_tool_result_for_model(
                "short".to_string(),
                Some("read"),
                TruncationPolicyConfig::bytes(100).into(),
            ),
            "short"
        );
    }

    #[test]
    fn model_tool_result_truncation_uses_byte_policy() {
        assert_eq!(
            truncate_tool_result_for_model(
                "abcdefghijklmnopqrstuvwxyz".to_string(),
                Some("read"),
                TruncationPolicyConfig::bytes(20).into(),
            ),
            "abcde\n...[truncated]"
        );
    }

    #[test]
    fn model_tool_result_truncation_uses_token_policy_byte_budget() {
        assert_eq!(
            truncate_tool_result_for_model(
                "abcdefghijklmnopqrstuvwxyz".to_string(),
                Some("read"),
                TruncationPolicyConfig::tokens(5).into(),
            ),
            "abcde\n...[truncated]"
        );
    }

    #[test]
    fn model_tool_result_truncation_preserves_utf8_boundaries() {
        let truncated = truncate_tool_result_for_model(
            "éééééabcdefghij".to_string(),
            Some("read"),
            TruncationPolicyConfig::bytes(18).into(),
        );

        assert_eq!(truncated, "é\n...[truncated]");
        assert!(truncated.len() <= 18);
    }

    #[test]
    fn model_tool_result_truncation_preserves_agent_coordination_results() {
        let content = "abcdefghijklmnopqrstuvwxyz".to_string();

        for tool_name in [
            Some("await_task"),
            Some("wait_agent"),
            Some("subagent_result"),
        ] {
            assert_eq!(
                truncate_tool_result_for_model(
                    content.clone(),
                    tool_name,
                    TruncationPolicyConfig::bytes(20).into(),
                ),
                content
            );
        }
    }

    const HOSTED_DSML_TEXT: &str = "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"web_search\">\n<｜｜DSML｜｜parameter name=\"query\" string=\"true\">current Rust docs</｜｜DSML｜｜parameter>\n</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>";

    struct SingleToolUseProvider {
        requests: AtomicUsize,
    }

    struct CapturingToolUseProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        calls: AtomicUsize,
    }

    struct InterleavedToolUseProvider {
        requests: AtomicUsize,
    }

    struct ParallelToolUseProvider {
        requests: AtomicUsize,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for SingleToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "tool-1".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 0,
                        partial_json: r#"{"value":1}"#.into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![ResponseContent::ToolUse {
                                id: "tool-1".into(),
                                name: "mutating_tool".into(),
                                input: json!({ "value": 1 }),
                            }],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![
                    Ok(StreamEvent::TextDelta {
                        index: 0,
                        text: "done".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-2".into(),
                            content: vec![ResponseContent::Text("done".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "test-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for CapturingToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            let request_number = self.calls.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "tool-1".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![ResponseContent::ToolUse {
                                id: "tool-1".into(),
                                name: "mutating_tool".into(),
                                input: json!({}),
                            }],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                })]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "capturing-tool-use-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for InterleavedToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "tool-1".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::ToolCallStart {
                        index: 1,
                        id: "tool-2".into(),
                        name: "mutating_tool".into(),
                        input: json!({}),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 0,
                        partial_json: r#"{"value":1}"#.into(),
                    }),
                    Ok(StreamEvent::ToolCallInputDelta {
                        index: 1,
                        partial_json: r#"{"value":2}"#.into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![
                                ResponseContent::ToolUse {
                                    id: "tool-1".into(),
                                    name: "mutating_tool".into(),
                                    input: json!({}),
                                },
                                ResponseContent::ToolUse {
                                    id: "tool-2".into(),
                                    name: "mutating_tool".into(),
                                    input: json!({}),
                                },
                            ],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![
                    Ok(StreamEvent::TextDelta {
                        index: 0,
                        text: "done".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-2".into(),
                            content: vec![ResponseContent::Text("done".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "interleaved-test-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for ParallelToolUseProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_number = self.requests.fetch_add(1, Ordering::SeqCst);

            let events = if request_number == 0 {
                vec![
                    Ok(StreamEvent::ToolCallStart {
                        index: 0,
                        id: "slow".into(),
                        name: "parallel_tool".into(),
                        input: json!({
                            "delay_ms": 50,
                            "output": "slow complete",
                        }),
                    }),
                    Ok(StreamEvent::ToolCallStart {
                        index: 1,
                        id: "fast".into(),
                        name: "parallel_tool".into(),
                        input: json!({
                            "delay_ms": 5,
                            "output": "fast complete",
                        }),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-1".into(),
                            content: vec![
                                ResponseContent::ToolUse {
                                    id: "slow".into(),
                                    name: "parallel_tool".into(),
                                    input: json!({
                                        "delay_ms": 50,
                                        "output": "slow complete",
                                    }),
                                },
                                ResponseContent::ToolUse {
                                    id: "fast".into(),
                                    name: "parallel_tool".into(),
                                    input: json!({
                                        "delay_ms": 5,
                                        "output": "fast complete",
                                    }),
                                },
                            ],
                            stop_reason: Some(StopReason::ToolUse),
                            usage: Usage::default(),
                            metadata: Default::default(),
                        },
                    }),
                ]
            } else {
                vec![Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-2".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                })]
            };

            Ok(Box::pin(futures::stream::iter(events)))
        }

        fn name(&self) -> &str {
            "parallel-tool-provider"
        }
    }

    struct MutatingTool;

    struct CapturingProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct OpenAiCapturingProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct HostedWebSearchProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct HostedDsmlTextProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    struct HostedWebFetchProvider {
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    }

    fn final_text_stream(text: &str) -> Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> {
        Box::pin(futures::stream::iter(vec![Ok(StreamEvent::MessageDone {
            response: ModelResponse {
                id: "resp-final".into(),
                content: vec![ResponseContent::Text(text.to_string())],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
                metadata: Default::default(),
            },
        })]))
    }

    struct TransientStreamCreateProvider {
        attempts: AtomicUsize,
    }

    struct TransientStreamEventProvider {
        attempts: AtomicUsize,
    }

    struct RateLimitedStreamCreateProvider {
        attempts: AtomicUsize,
    }

    enum CompactionProviderOutcome {
        Summary,
        Error,
    }

    struct CompactionProvider {
        completion_calls: AtomicUsize,
        outcome: CompactionProviderOutcome,
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for CapturingProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "capturing-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for OpenAiCapturingProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            self.requests.lock().expect("lock requests").push(request);
            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "openai"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for HostedWebSearchProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_count = {
                let mut requests = self.requests.lock().expect("lock requests");
                requests.push(request);
                requests.len()
            };
            if request_count > 1 {
                return Ok(final_text_stream("done"));
            }
            let input = json!({ "query": "current Rust docs" });
            let output = Some(json!({
                "results": [
                    {
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }
                ]
            }));
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::HostedToolCallStart {
                    index: 0,
                    id: "hosted_ws_1".into(),
                    name: "web_search".into(),
                    input: input.clone(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![
                            ResponseContent::HostedToolUse {
                                id: "hosted_ws_1".into(),
                                name: "web_search".into(),
                                input: input.clone(),
                                output: None,
                                status: None,
                            },
                            ResponseContent::HostedToolUse {
                                id: "hosted_ws_1".into(),
                                name: "web_search".into(),
                                input,
                                output,
                                status: Some("completed".into()),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ])))
        }

        fn name(&self) -> &str {
            "hosted-web-search-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for HostedDsmlTextProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_count = {
                let mut requests = self.requests.lock().expect("lock requests");
                requests.push(request);
                requests.len()
            };
            if request_count > 1 {
                return Ok(final_text_stream("done"));
            }
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::TextDelta {
                    index: 0,
                    text: HOSTED_DSML_TEXT.to_string(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp-dsml".into(),
                        content: vec![ResponseContent::Text(HOSTED_DSML_TEXT.to_string())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ])))
        }

        fn name(&self) -> &str {
            "hosted-dsml-text-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for HostedWebFetchProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let request_count = {
                let mut requests = self.requests.lock().expect("lock requests");
                requests.push(request);
                requests.len()
            };
            if request_count > 1 {
                return Ok(final_text_stream("done"));
            }
            let input = json!({ "url": "https://example.test/docs" });
            let output = Some(json!({
                "title": "Docs",
                "url": "https://example.test/docs"
            }));
            Ok(Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::HostedToolCallStart {
                    index: 0,
                    id: "hosted_wf_1".into(),
                    name: "web_fetch".into(),
                    input: input.clone(),
                }),
                Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![
                            ResponseContent::HostedToolUse {
                                id: "hosted_wf_1".into(),
                                name: "web_fetch".into(),
                                input: input.clone(),
                                output: None,
                                status: None,
                            },
                            ResponseContent::HostedToolUse {
                                id: "hosted_wf_1".into(),
                                name: "web_fetch".into(),
                                input,
                                output,
                                status: Some("completed".into()),
                            },
                        ],
                        stop_reason: Some(StopReason::ToolUse),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                }),
            ])))
        }

        fn name(&self) -> &str {
            "hosted-web-fetch-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for TransientStreamCreateProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(anyhow::anyhow!("503 service unavailable"));
            }

            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "transient-stream-create-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for TransientStreamEventProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Ok(Box::pin(futures::stream::iter(vec![Err(anyhow::anyhow!(
                    "500 internal server error"
                ))])));
            }

            Ok(Box::pin(futures::stream::iter(vec![Ok(
                StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: "resp".into(),
                        content: vec![ResponseContent::Text("done".into())],
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: Default::default(),
                    },
                },
            )])))
        }

        fn name(&self) -> &str {
            "transient-stream-event-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for RateLimitedStreamCreateProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            unreachable!("tests stream responses only")
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt < 2 {
                return Err(anyhow::anyhow!("429 rate limit exceeded"));
            }

            Ok(final_text_stream("done"))
        }

        fn name(&self) -> &str {
            "rate-limited-stream-create-provider"
        }
    }

    #[async_trait]
    impl devo_provider::ModelProviderSDK for CompactionProvider {
        async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
            self.completion_calls.fetch_add(1, Ordering::SeqCst);
            match &self.outcome {
                CompactionProviderOutcome::Summary => Ok(ModelResponse {
                    id: "compaction-response".to_string(),
                    content: vec![ResponseContent::Text("summary".to_string())],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage::default(),
                    metadata: Default::default(),
                }),
                CompactionProviderOutcome::Error => {
                    Err(anyhow::anyhow!("compaction provider failed"))
                }
            }
        }

        async fn completion_stream(
            &self,
            _request: ModelRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
            unreachable!("tests call the non-streaming compaction path only")
        }

        fn name(&self) -> &str {
            "compaction-provider"
        }
    }

    #[async_trait]
    impl ToolHandler for MutatingTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            // Leak a static spec for test purposes
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "write",
                "write tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("ok".into()),
                "ok",
            ))
        }
    }

    struct DisplayContentTool;

    struct LargeToolResultTool {
        content: String,
        display_content: Option<String>,
    }

    struct CountingWebSearchTool {
        executions: Arc<AtomicUsize>,
    }

    struct CountingWebFetchTool {
        executions: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolHandler for CountingWebSearchTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "web_search",
                "Search the web.",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("local search".into()),
                "local search",
            ))
        }
    }

    #[async_trait]
    impl ToolHandler for CountingWebFetchTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "webfetch",
                "Fetch a URL.",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("local fetch".into()),
                "local fetch",
            ))
        }
    }

    #[async_trait]
    impl ToolHandler for DisplayContentTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "read",
                "read tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            let mut result = crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("canonical".into()),
                "done",
            );
            result.display_content = Some("display".to_string());
            Ok(result)
        }
    }

    #[async_trait]
    impl ToolHandler for LargeToolResultTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "read",
                "read tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            let mut result = crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text(self.content.clone()),
                "done",
            );
            result.display_content = self.display_content.clone();
            Ok(result)
        }
    }

    struct StreamingMutatingTool;

    struct ParallelDelayTool;

    #[async_trait]
    impl ToolHandler for StreamingMutatingTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "write",
                "write tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            _input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text("stream complete".into()),
                "done",
            ))
        }
    }

    #[async_trait]
    impl ToolHandler for ParallelDelayTool {
        fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
            Box::leak(Box::new(crate::tools::tool_spec::ToolSpec::new(
                "read",
                "read tool",
                crate::tools::JsonSchema::object(Default::default(), None, None),
            )))
        }

        async fn handle(
            &self,
            _ctx: crate::tools::contracts::ToolContext,
            input: serde_json::Value,
            _progress: Option<crate::tools::contracts::ToolProgressSender>,
        ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
        {
            let delay_ms = input
                .get("delay_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            let output = input
                .get("output")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            Ok(crate::tools::contracts::ToolResult::success(
                crate::tools::contracts::ToolResultContent::Text(output.to_string()),
                "done",
            ))
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum RecordedCompactionEvent {
        Started,
        Completed,
        Failed { message: String },
    }

    fn recorded_compaction_events(events: &[QueryEvent]) -> Vec<RecordedCompactionEvent> {
        events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ContextCompactionStarted => Some(RecordedCompactionEvent::Started),
                QueryEvent::ContextCompactionCompleted => Some(RecordedCompactionEvent::Completed),
                QueryEvent::ContextCompactionFailed { message } => {
                    Some(RecordedCompactionEvent::Failed {
                        message: message.clone(),
                    })
                }
                QueryEvent::ProviderRetryStatus(_)
                | QueryEvent::TextDelta(_)
                | QueryEvent::ReasoningDelta(_)
                | QueryEvent::ReasoningCompleted
                | QueryEvent::UsageDelta { .. }
                | QueryEvent::ToolUseStart { .. }
                | QueryEvent::ToolExecutionStart { .. }
                | QueryEvent::ToolProgress { .. }
                | QueryEvent::ToolResult { .. }
                | QueryEvent::TurnComplete { .. }
                | QueryEvent::Usage { .. } => None,
            })
            .collect()
    }

    fn recording_callback(events: &Arc<Mutex<Vec<QueryEvent>>>) -> EventCallback {
        let captured_events = Arc::clone(events);
        Arc::new(move |event| {
            let captured_events = Arc::clone(&captured_events);
            Box::pin(async move {
                captured_events.lock().expect("lock events").push(event);
            })
        })
    }

    fn compaction_test_session(total_input_tokens: usize) -> SessionState {
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("x".repeat(80_004)));
        session.push_message(Message::user("latest"));
        session.total_input_tokens = total_input_tokens;
        session
    }

    #[tokio::test]
    async fn automatic_compaction_emits_started_then_completed_when_history_is_replaced() {
        let provider = Arc::new(CompactionProvider {
            completion_calls: AtomicUsize::new(0),
            outcome: CompactionProviderOutcome::Summary,
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let on_event = Some(recording_callback(&events));
        let mut session = compaction_test_session(/*total_input_tokens*/ 200_000);

        super::summarize_and_compact(
            &mut session,
            &on_event,
            &provider_sdk,
            "compaction-model",
            "compaction-request-model",
            /*max_tokens*/ 4096,
            CompactionKind::Auto,
        )
        .await;

        assert_eq!(
            recorded_compaction_events(&events.lock().expect("lock events")),
            vec![
                RecordedCompactionEvent::Started,
                RecordedCompactionEvent::Completed,
            ]
        );
        assert_eq!(provider.completion_calls.load(Ordering::SeqCst), 1);
        let ResponseItem::Message(expected_summary) =
            CompactionSummary::new("summary").to_response_item()
        else {
            unreachable!("compaction summaries are messages");
        };
        assert_eq!(
            session.prompt_source_messages(),
            &[expected_summary, Message::user("latest")]
        );
    }

    #[tokio::test]
    async fn automatic_compaction_emits_failed_when_compaction_is_skipped() {
        let provider = Arc::new(CompactionProvider {
            completion_calls: AtomicUsize::new(0),
            outcome: CompactionProviderOutcome::Summary,
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let on_event = Some(recording_callback(&events));
        let mut session = compaction_test_session(/*total_input_tokens*/ 0);
        let original_messages = session.prompt_source_messages().to_vec();

        super::summarize_and_compact(
            &mut session,
            &on_event,
            &provider_sdk,
            "compaction-model",
            "compaction-request-model",
            /*max_tokens*/ 4096,
            CompactionKind::Auto,
        )
        .await;

        assert_eq!(
            recorded_compaction_events(&events.lock().expect("lock events")),
            vec![
                RecordedCompactionEvent::Started,
                RecordedCompactionEvent::Failed {
                    message: "Context compaction skipped: nothing to compact".to_string(),
                },
            ]
        );
        assert_eq!(provider.completion_calls.load(Ordering::SeqCst), 0);
        assert_eq!(session.prompt_source_messages(), original_messages);
    }

    #[tokio::test(start_paused = true)]
    async fn proactive_compaction_emits_failed_when_compaction_errors() {
        let provider = Arc::new(CompactionProvider {
            completion_calls: AtomicUsize::new(0),
            outcome: CompactionProviderOutcome::Error,
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let on_event = Some(recording_callback(&events));
        let mut session = compaction_test_session(/*total_input_tokens*/ 0);
        let original_messages = session.prompt_source_messages().to_vec();

        super::summarize_and_compact(
            &mut session,
            &on_event,
            &provider_sdk,
            "compaction-model",
            "compaction-request-model",
            /*max_tokens*/ 4096,
            CompactionKind::Proactive,
        )
        .await;

        assert_eq!(
            recorded_compaction_events(&events.lock().expect("lock events")),
            vec![
                RecordedCompactionEvent::Started,
                RecordedCompactionEvent::Failed {
                    message: "summarization failed: compaction provider failed".to_string(),
                },
            ]
        );
        assert_eq!(provider.completion_calls.load(Ordering::SeqCst), 5);
        assert_eq!(session.prompt_source_messages(), original_messages);
    }

    #[tokio::test]
    async fn query_retries_transient_stream_creation_errors() {
        let provider = Arc::new(TransientStreamCreateProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider_sdk,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should retry and succeed");

        assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            session.messages.last(),
            Some(&Message::assistant_text("done"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn query_retries_transient_stream_event_errors_before_content() {
        let provider = Arc::new(TransientStreamEventProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let turn_config = TurnConfig::new(Model::default(), None);
        let model = turn_config.model.slug.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = Arc::clone(&events);
        let callback: EventCallback = Arc::new(move |event| {
            let captured_events = Arc::clone(&captured_events);
            Box::pin(async move {
                captured_events.lock().expect("lock events").push(event);
            })
        });

        query(
            &mut session,
            &turn_config,
            provider_sdk,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should retry and succeed");

        let retry_statuses = events
            .lock()
            .expect("lock events")
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ProviderRetryStatus(status) => Some(status.clone()),
                QueryEvent::ContextCompactionStarted
                | QueryEvent::ContextCompactionCompleted
                | QueryEvent::ContextCompactionFailed { .. }
                | QueryEvent::TextDelta(_)
                | QueryEvent::ReasoningDelta(_)
                | QueryEvent::ReasoningCompleted
                | QueryEvent::UsageDelta { .. }
                | QueryEvent::ToolUseStart { .. }
                | QueryEvent::ToolExecutionStart { .. }
                | QueryEvent::ToolProgress { .. }
                | QueryEvent::ToolResult { .. }
                | QueryEvent::TurnComplete { .. }
                | QueryEvent::Usage { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            retry_statuses,
            vec![
                ProviderRetryStatus {
                    provider: "transient-stream-event-provider".to_string(),
                    model: model.clone(),
                    attempt: 1,
                    backoff_ms: 250,
                    phase: QueryProviderRetryPhase::Scheduled,
                    message: "Retrying provider request in 0.2s".to_string(),
                },
                ProviderRetryStatus {
                    provider: "transient-stream-event-provider".to_string(),
                    model,
                    attempt: 1,
                    backoff_ms: 0,
                    phase: QueryProviderRetryPhase::Resumed,
                    message: "Retrying provider request now".to_string(),
                },
            ]
        );
        assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
        let assistant_messages = session
            .messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(assistant_messages, vec![Message::assistant_text("done")]);
    }

    #[tokio::test(start_paused = true)]
    async fn query_waits_sixty_seconds_for_each_rate_limit_retry() {
        let provider = Arc::new(RateLimitedStreamCreateProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let turn_config = TurnConfig::new(Model::default(), None);
        let model = turn_config.model.slug.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured_events = Arc::clone(&events);
        let callback: EventCallback = Arc::new(move |event| {
            let captured_events = Arc::clone(&captured_events);
            Box::pin(async move {
                captured_events.lock().expect("lock events").push(event);
            })
        });
        let started_at = tokio::time::Instant::now();

        query(
            &mut session,
            &turn_config,
            provider_sdk,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should retry and succeed");

        assert_eq!(
            tokio::time::Instant::now().duration_since(started_at),
            std::time::Duration::from_secs(120)
        );
        let retry_statuses = events
            .lock()
            .expect("lock events")
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ProviderRetryStatus(status) => Some(status.clone()),
                QueryEvent::ContextCompactionStarted
                | QueryEvent::ContextCompactionCompleted
                | QueryEvent::ContextCompactionFailed { .. }
                | QueryEvent::TextDelta(_)
                | QueryEvent::ReasoningDelta(_)
                | QueryEvent::ReasoningCompleted
                | QueryEvent::UsageDelta { .. }
                | QueryEvent::ToolUseStart { .. }
                | QueryEvent::ToolExecutionStart { .. }
                | QueryEvent::ToolProgress { .. }
                | QueryEvent::ToolResult { .. }
                | QueryEvent::TurnComplete { .. }
                | QueryEvent::Usage { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            retry_statuses,
            vec![
                ProviderRetryStatus {
                    provider: "rate-limited-stream-create-provider".to_string(),
                    model: model.clone(),
                    attempt: 1,
                    backoff_ms: 60_000,
                    phase: QueryProviderRetryPhase::Scheduled,
                    message: "Retrying provider request in 60.0s".to_string(),
                },
                ProviderRetryStatus {
                    provider: "rate-limited-stream-create-provider".to_string(),
                    model: model.clone(),
                    attempt: 1,
                    backoff_ms: 0,
                    phase: QueryProviderRetryPhase::Resumed,
                    message: "Retrying provider request now".to_string(),
                },
                ProviderRetryStatus {
                    provider: "rate-limited-stream-create-provider".to_string(),
                    model: model.clone(),
                    attempt: 2,
                    backoff_ms: 60_000,
                    phase: QueryProviderRetryPhase::Scheduled,
                    message: "Retrying provider request in 60.0s".to_string(),
                },
                ProviderRetryStatus {
                    provider: "rate-limited-stream-create-provider".to_string(),
                    model,
                    attempt: 2,
                    backoff_ms: 0,
                    phase: QueryProviderRetryPhase::Resumed,
                    message: "Retrying provider request now".to_string(),
                },
            ]
        );
        assert_eq!(provider.attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn query_cancels_stream_creation_retry_backoff() {
        let provider = Arc::new(TransientStreamCreateProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let result = query_with_options(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider_sdk,
            registry,
            &runtime,
            None,
            QueryOptions {
                cancel_token: Some(cancel_token),
                ..QueryOptions::default()
            },
        )
        .await;

        assert!(matches!(result, Err(AgentError::Aborted)));
        assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn query_cancels_stream_event_retry_backoff() {
        let provider = Arc::new(TransientStreamEventProvider {
            attempts: AtomicUsize::new(0),
        });
        let provider_sdk: Arc<dyn ModelProviderSDK> = provider.clone();
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();

        let result = query_with_options(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider_sdk,
            registry,
            &runtime,
            None,
            QueryOptions {
                cancel_token: Some(cancel_token),
                ..QueryOptions::default()
            },
        )
        .await;

        assert!(matches!(result, Err(AgentError::Aborted)));
        assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn query_exposes_stable_tools_and_appends_subagent_warning() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let mut builder = ToolRegistryBuilder::new();
        builder.push_spec_with_exposure(
            ToolSpec::new(
                "ToolSearch",
                "Search available tools.",
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
        builder.push_spec_with_exposure(
            ToolSpec::new(
                "web_search",
                "Search the web.",
                JsonSchema::object(Default::default(), None, None),
            ),
            ToolExposure::Direct,
        );
        for (name, description) in [
            ("spawn_agent", "Create a child agent."),
            ("send_message", "Send input to a child agent."),
            ("await_task", "Wait for task completion."),
            ("list_tasks", "List child tasks."),
            ("cancel_task", "Cancel a child task."),
        ] {
            builder.push_spec_with_exposure(
                ToolSpec::new(
                    name,
                    description,
                    JsonSchema::object(Default::default(), None, None),
                ),
                ToolExposure::Direct,
            );
        }
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_with_context(
            Arc::clone(&registry),
            PermissionChecker::always_allow(),
            ToolRuntimeContext {
                agent_scope: ToolAgentScope::Subagent,
                ..ToolRuntimeContext::default()
            },
        );
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("work on the delegated task"));
        let mut turn_config = TurnConfig::new(
            Model {
                base_instructions: "base system".to_string(),
                ..Model::default()
            },
            None,
        );
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Local(
            devo_config::ResolvedLocalWebSearchConfig {
                provider_id: "test".to_string(),
                kind: devo_config::LocalWebSearchProviderKind::Exa,
                api_key: "secret".to_string(),
                base_url: None,
                max_results: None,
            },
        );

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should complete");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let request = &captured[0];
        let tool_names = request
            .tools
            .as_ref()
            .expect("tools should be present")
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tool_names, vec!["ToolSearch", "web_search"]);
        let system = request.system.as_deref().expect("system prompt");
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        assert!(system.contains("base system"));
        assert!(system.contains(&mode_prompt));
        assert!(system.contains("Sources:"));
        assert!(
            !request
                .system
                .as_deref()
                .unwrap_or_default()
                .contains("web_search")
        );
        assert!(
            !request
                .system
                .as_deref()
                .unwrap_or_default()
                .contains("spawn_agent")
        );

        assert!(
            request
                .messages
                .iter()
                .all(|message| !message_contains(message, "web_search: Search the web."))
        );
        let subagent_reminder_index =
            request_message_index_containing(request, "You are running as a sub-agent");
        let task_index = request_message_index_containing(request, "work on the delegated task");
        assert!(subagent_reminder_index < task_index);
        assert!(
            request
                .messages
                .iter()
                .any(|message| message_contains(message, "<context_changes>"))
        );
    }

    #[tokio::test]
    async fn query_adds_web_search_prompt_for_provider_hosted_search() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("search current docs"));
        let mut turn_config = TurnConfig::new(
            Model {
                base_instructions: "base system".to_string(),
                ..Model::default()
            },
            None,
        );
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should complete");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let request = &captured[0];
        let system = request.system.as_deref().expect("system prompt");

        assert!(system.contains("base system"));
        assert!(system.contains("Sources:"));
        assert!(system.contains("The current month is "));
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().all(|tool| tool.name != "web_search"))
        );
    }

    /// Trace: L2-DES-RESEARCH-001
    /// Verifies: provider-hosted web_search emits normal tool events with hosted output.
    #[tokio::test]
    async fn provider_hosted_web_search_emits_tool_events_without_local_execution() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedWebSearchProvider {
            requests: Arc::clone(&requests),
        });
        let executions = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "web_search",
            Arc::new(CountingWebSearchTool {
                executions: Arc::clone(&executions),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "web_search".into(),
            description: "Search the web.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("search current docs"));
        let mut turn_config = TurnConfig::new(Model::default(), None);
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                seen_clone.lock().unwrap().push(event);
            })
        });

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let request = &captured[0];
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().all(|tool| tool.name != "web_search"))
        );
        let continuation = &captured[1];
        assert!(continuation.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    RequestContent::HostedToolUse {
                        id,
                        name,
                        input,
                        output: Some(_),
                        status,
                    } if id == "hosted_ws_1"
                        && name == "web_search"
                        && input == &json!({ "query": "current Rust docs" })
                        && status.as_deref() == Some("completed")
                )
            })
        }));

        let events = seen.lock().unwrap();
        let starts = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolUseStart { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec![(
                "hosted_ws_1",
                "web_search",
                json!({ "query": "current Rust docs" })
            )]
        );
        let results = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolResult {
                    tool_use_id,
                    tool_name,
                    input,
                    content,
                    is_error,
                    ..
                } => Some((
                    tool_use_id.as_str(),
                    tool_name.as_str(),
                    input.clone(),
                    content,
                    *is_error,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        let (tool_use_id, tool_name, input, content, is_error) = &results[0];
        assert_eq!(*tool_use_id, "hosted_ws_1");
        assert_eq!(*tool_name, "web_search");
        assert_eq!(input, &json!({ "query": "current Rust docs" }));
        assert!(!*is_error);
        assert!(matches!(
            *content,
            ToolContent::Mixed {
                text: Some(text),
                json: Some(json),
            } if text == "status: completed"
                && json == &json!({
                    "results": [
                        {
                            "title": "Rust documentation",
                            "url": "https://example.test/rust"
                        }
                    ]
                })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            QueryEvent::TurnComplete {
                stop_reason: StopReason::EndTurn
            }
        )));
        assert!(session.messages.iter().all(|message| {
            message.content.iter().all(|block| {
                !matches!(
                    block,
                    ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
                )
            })
        }));
    }

    /// Trace: L2-DES-RESEARCH-001
    /// Verifies: DSML text that represents a provider-hosted web_search does not end the query loop.
    #[tokio::test]
    async fn provider_hosted_dsml_text_tool_call_continues_query_loop() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedDsmlTextProvider {
            requests: Arc::clone(&requests),
        });
        let mut builder = ToolRegistryBuilder::new();
        for (name, description) in [
            ("spawn_agent", "Create a child agent."),
            ("await_task", "Wait for task completion."),
        ] {
            builder.push_spec_with_exposure(
                ToolSpec::new(
                    name,
                    description,
                    JsonSchema::object(Default::default(), None, None),
                ),
                ToolExposure::Direct,
            );
        }
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("search current docs"));
        let mut turn_config = TurnConfig::new(Model::default(), None);
        turn_config.web_search = devo_config::ResolvedWebSearchConfig::Provider;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                seen_clone.lock().unwrap().push(event);
            })
        });

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should continue after DSML text and complete");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let request = &captured[0];
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebSearch(_)]
        ));
        let continuation = &captured[1];
        assert!(continuation.messages.iter().any(|message| {
            message_contains(message, "DSML tagged tool-call text")
                && message_contains(message, "spawn_agent")
                && message_contains(message, "await_task")
                && message_contains(message, "web_search")
        }));

        let assistant_messages = session
            .messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            assistant_messages,
            vec![
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: HOSTED_DSML_TEXT.to_string(),
                    }],
                },
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: "done".to_string(),
                    }],
                },
            ]
        );

        let turn_completes = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                QueryEvent::TurnComplete { stop_reason } => Some(stop_reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(turn_completes, vec![StopReason::EndTurn]);
    }

    /// Trace: L2-DES-RESEARCH-001
    /// Verifies: provider-hosted web_fetch emits normal tool events with hosted output.
    #[tokio::test]
    async fn provider_hosted_web_fetch_emits_tool_events_without_local_execution() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(HostedWebFetchProvider {
            requests: Arc::clone(&requests),
        });
        let executions = Arc::new(AtomicUsize::new(0));
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "webfetch",
            Arc::new(CountingWebFetchTool {
                executions: Arc::clone(&executions),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "webfetch".into(),
            description: "Fetch a URL.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Mixed,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("fetch docs"));
        let mut turn_config = TurnConfig::new(Model::default(), None);
        turn_config.web_fetch = devo_config::ResolvedWebFetchConfig::Provider;

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                seen_clone.lock().unwrap().push(event);
            })
        });

        query(
            &mut session,
            &turn_config,
            provider,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(executions.load(Ordering::SeqCst), 0);
        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let request = &captured[0];
        assert!(matches!(
            request.hosted_tools.as_slice(),
            [devo_protocol::HostedToolDefinition::WebFetch(_)]
        ));
        assert!(
            request
                .tools
                .as_ref()
                .is_none_or(|tools| tools.iter().all(|tool| tool.name != "webfetch"))
        );
        let continuation = &captured[1];
        assert!(continuation.messages.iter().any(|message| {
            message.content.iter().any(|content| {
                matches!(
                    content,
                    RequestContent::HostedToolUse {
                        id,
                        name,
                        input,
                        output: Some(_),
                        status,
                    } if id == "hosted_wf_1"
                        && name == "web_fetch"
                        && input == &json!({ "url": "https://example.test/docs" })
                        && status.as_deref() == Some("completed")
                )
            })
        }));

        let events = seen.lock().unwrap();
        let starts = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolUseStart { id, name, input } => {
                    Some((id.as_str(), name.as_str(), input.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            starts,
            vec![(
                "hosted_wf_1",
                "web_fetch",
                json!({ "url": "https://example.test/docs" })
            )]
        );
        let results = events
            .iter()
            .filter_map(|event| match event {
                QueryEvent::ToolResult {
                    tool_use_id,
                    tool_name,
                    input,
                    content,
                    is_error,
                    ..
                } => Some((
                    tool_use_id.as_str(),
                    tool_name.as_str(),
                    input.clone(),
                    content,
                    *is_error,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        let (tool_use_id, tool_name, input, content, is_error) = &results[0];
        assert_eq!(*tool_use_id, "hosted_wf_1");
        assert_eq!(*tool_name, "web_fetch");
        assert_eq!(input, &json!({ "url": "https://example.test/docs" }));
        assert!(!*is_error);
        assert!(matches!(
            *content,
            ToolContent::Mixed {
                text: Some(text),
                json: Some(json),
            } if text == "status: completed"
                && json == &json!({
                    "title": "Docs",
                    "url": "https://example.test/docs"
                })
        ));
    }

    #[test]
    fn subagent_reminder_insertion_preserves_tool_result_adjacency() {
        let mut messages = vec![
            RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::Text {
                    text: "child task input".to_string(),
                }],
            },
            RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::ToolUse {
                    id: "tool-1".to_string(),
                    name: "read".to_string(),
                    input: json!({}),
                }],
            },
            RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content: "tool output".to_string(),
                    is_error: None,
                }],
            },
        ];

        insert_subagent_request_reminders(&mut messages);

        assert!(message_contains(
            &messages[0],
            "You are running as a sub-agent"
        ));
        assert!(message_contains(&messages[1], "child task input"));
        assert!(
            matches!(messages[2].content.as_slice(), [RequestContent::ToolUse { id, .. }] if id == "tool-1")
        );
        assert!(
            matches!(messages[3].content.as_slice(), [RequestContent::ToolResult { tool_use_id, .. }] if tool_use_id == "tool-1")
        );
    }

    fn request_message_index_containing(request: &ModelRequest, needle: &str) -> usize {
        request
            .messages
            .iter()
            .position(|message| message_contains(message, needle))
            .unwrap_or_else(|| {
                panic!("expected request message containing {needle:?}: {request:?}")
            })
    }

    fn message_contains(message: &RequestMessage, needle: &str) -> bool {
        message.content.iter().any(
            |content| matches!(content, RequestContent::Text { text } if text.contains(needle)),
        )
    }

    fn active_goal(objective: &str) -> ThreadGoal {
        ThreadGoal {
            thread_id: devo_protocol::SessionId::new(),
            objective: objective.to_string(),
            status: ThreadGoalStatus::Active,
            token_budget: Some(10_000),
            tokens_used: 250,
            time_used_seconds: 0,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[tokio::test]
    async fn query_uses_session_permission_mode_for_mutating_tools() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(MutatingTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: "A test-only mutating tool.".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let deny_checker = PermissionChecker::new(|request| {
            let n = request.tool_name;
            Box::pin(async move { Err(format!("{n} denied")) })
        });
        let runtime = ToolRuntime::new(Arc::clone(&registry), deny_checker);

        let mut session = SessionState::new(
            SessionConfig {
                permission_mode: PermissionMode::Deny,
                ..Default::default()
            },
            std::env::temp_dir(),
        );
        session.push_message(Message::user("run the tool"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should complete and append a tool_result");

        let tool_result_message = session
            .messages
            .iter()
            .find(|message| {
                message
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
            })
            .expect("tool_result message should be appended");
        let ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } = &tool_result_message.content[0]
        else {
            panic!("expected tool_result content block");
        };

        assert_eq!(tool_use_id, "tool-1");
        assert!(
            *is_error,
            "denied permission should surface as a tool error"
        );
        assert!(
            content.contains("permission denied"),
            "expected tool_result to mention permission denial, got: {content}"
        );
    }

    #[tokio::test]
    async fn query_resolves_reasoning_model_variant_before_building_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "kimi-k2.5".into(),
            display_name: "Kimi K2.5".into(),
            provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
            description: None,
            reasoning_capability: ReasoningCapability::Toggle,
            default_reasoning_effort: Some(ReasoningEffort::Medium),
            reasoning_implementation: Some(ReasoningImplementation::ModelVariant(
                ReasoningVariantConfig {
                    variants: vec![
                        ReasoningVariant {
                            selection_value: "disabled".into(),
                            model_slug: "kimi-k2.5".into(),
                            reasoning_effort: None,
                            label: "Off".into(),
                            description: "Use the standard model".into(),
                            extra_body: None,
                        },
                        ReasoningVariant {
                            selection_value: "enabled".into(),
                            model_slug: "kimi-k2.5-thinking".into(),
                            reasoning_effort: Some(ReasoningEffort::Medium),
                            label: "On".into(),
                            description: "Use the reasoning model".into(),
                            extra_body: None,
                        },
                    ],
                },
            )),
            base_instructions: String::new(),
            context_window: 200_000,
            effective_context_window_percent: None,
            truncation_policy: TruncationPolicyConfig {
                mode: TruncationMode::Tokens,
                limit: 10_000,
            },
            input_modalities: vec![],
            supports_image_detail_original: false,
            channel: None,
            temperature: None,
            top_p: None,
            top_k: None,
            max_tokens: None,
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::with_request_model(
                model,
                "vendor/kimi-k2.5".into(),
                HashMap::from([(
                    "kimi-k2.5-thinking".into(),
                    "vendor/kimi-k2.5-thinking".into(),
                )])
                .into(),
                Some("enabled".into()),
            ),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "vendor/kimi-k2.5-thinking");
        assert_eq!(captured[0].request_thinking, None);
    }

    #[tokio::test]
    async fn query_sends_turn_config_request_model_to_provider() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "catalog-slug".into(),
            display_name: "Catalog Model".into(),
            base_instructions: "catalog instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::with_request_model(
                model,
                "vendor/model-name".into(),
                HashMap::new().into(),
                /*reasoning_effort_selection*/ None,
            ),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].model, "vendor/model-name");
        assert_eq!(
            session
                .session_context
                .as_ref()
                .expect("session context")
                .model
                .slug,
            "catalog-slug"
        );
    }

    /// Trace: L2-DES-CONTEXT-001
    /// Verifies: Plan turns append the active Plan collaboration prompt to the provider system prompt.
    #[tokio::test]
    async fn query_appends_plan_mode_reminder_to_system_prompt() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.collaboration_mode = CollaborationMode::Plan;
        session.push_message(Message::user("plan this"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let system = captured[0].system.as_deref().expect("system prompt");
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        assert!(system.contains("base instructions"));
        assert!(system.contains(&mode_prompt));
        let mode_index = request_message_index_containing(&captured[0], "<collaboration_mode>");
        assert!(message_contains(
            &captured[0].messages[mode_index],
            "<current>plan</current>"
        ));
    }

    /// Trace: L2-DES-CONTEXT-001
    /// Verifies: Returning from Plan to Build uses Build system prompt and a lightweight mode diff.
    #[tokio::test]
    async fn query_inserts_mode_change_prompt_when_returning_to_build_mode() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.collaboration_mode = CollaborationMode::Plan;
        session.push_message(Message::user("plan this"));

        query(
            &mut session,
            &TurnConfig::new(model.clone(), None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("plan query should succeed");

        session.collaboration_mode = CollaborationMode::Build;
        session.push_message(Message::user("implement this"));
        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("build query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].system, captured[1].system);
        let system = captured[1].system.as_deref().expect("system prompt");
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        assert!(system.contains("base instructions"));
        assert!(system.contains(&mode_prompt));

        let mode_change_index = request_message_index_containing(
            &captured[1],
            "<transition>plan -> build</transition>",
        );
        let request_index = request_message_index_containing(&captured[1], "implement this");
        assert!(mode_change_index < request_index);
        assert!(message_contains(
            &captured[1].messages[mode_change_index],
            "<previous>plan</previous>"
        ));
        assert!(message_contains(
            &captured[1].messages[mode_change_index],
            "<current>build</current>"
        ));
        assert!(message_contains(
            &captured[1].messages[mode_change_index],
            "<note>any previous instructions for other modes (e.g. Plan mode) are no longer active.</note>"
        ));
        assert!(!message_contains(
            &captured[1].messages[mode_change_index],
            "<collaboration_mode_build>"
        ));
        assert!(!message_contains(
            &captured[1].messages[mode_change_index],
            "<collaboration_mode_plan>"
        ));
    }

    #[tokio::test]
    async fn query_inserts_goal_context_before_latest_user_request() {
        // Trace: L2-DES-GOAL-001
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.set_active_goal(active_goal("ship /goal"));
        session.push_message(Message::user("finish implementation"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert!(
            !captured[0]
                .system
                .as_deref()
                .unwrap_or_default()
                .contains("ship /goal")
        );
        let messages = &captured[0].messages;
        let goal_index = messages
            .iter()
            .position(|message| message_contains(message, "ship /goal"))
            .expect("goal context message");
        let request_index = messages
            .iter()
            .position(|message| message_contains(message, "finish implementation"))
            .expect("latest user request message");
        assert!(goal_index < request_index);
    }

    #[tokio::test]
    async fn autonomous_goal_context_is_latest_request_after_completed_turn() {
        // Trace: L2-DES-GOAL-001
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            base_instructions: "base instructions".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.set_active_goal(active_goal("continue the active goal"));
        session.push_message(Message::user("older user prompt"));
        session.push_message(Message::assistant_text("older assistant reply"));

        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        let messages = &captured[0].messages;
        let goal_index = messages
            .iter()
            .position(|message| message_contains(message, "continue the active goal"))
            .expect("goal context message");
        let assistant_index = messages
            .iter()
            .position(|message| message_contains(message, "older assistant reply"))
            .expect("assistant history message");
        assert!(goal_index > assistant_index);
        assert_eq!(goal_index, messages.len() - 1);
    }

    #[tokio::test]
    async fn query_locks_system_prompt_and_environment_prefix_per_session() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let temp_root =
            std::env::temp_dir().join(format!("devo-query-lock-{}", uuid::Uuid::new_v4()));
        let second_cwd = temp_root.join("nested");
        let first_model = Model {
            slug: "model-a".into(),
            base_instructions: "base-a".into(),
            ..Model::default()
        };
        let second_model = Model {
            slug: "model-b".into(),
            base_instructions: "base-b".into(),
            ..Model::default()
        };

        let mut session = SessionState::new(SessionConfig::default(), temp_root.clone());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(first_model, None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("first query should succeed");

        session.cwd = second_cwd;
        session.push_message(Message::user("follow up"));

        query(
            &mut session,
            &TurnConfig::new(second_model, Some("enabled".into())),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let mode_prompt = crate::collaboration_mode_prompts::mode_introductions_prompt();
        let expected_system = format!("base-a\n\n{mode_prompt}");
        assert_eq!(
            captured[0].system.as_deref(),
            Some(expected_system.as_str())
        );
        assert_eq!(
            captured[1].system.as_deref(),
            Some(expected_system.as_str())
        );

        let first_prefix = &captured[0].messages[0];
        let second_prefix = &captured[1].messages[0];
        assert_eq!(first_prefix.role, second_prefix.role);
        let devo_protocol::RequestContent::Text { text: first_text } = &first_prefix.content[0]
        else {
            panic!("expected text prefix");
        };
        let devo_protocol::RequestContent::Text { text: second_text } = &second_prefix.content[0]
        else {
            panic!("expected text prefix");
        };
        assert_eq!(first_text, second_text);
    }

    #[tokio::test]
    async fn query_inserts_context_diff_before_changed_turn_input() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        let first_model = Model {
            slug: "model-a".into(),
            ..Model::default()
        };
        let second_model = Model {
            slug: "model-b".into(),
            ..Model::default()
        };

        session.push_message(Message::user("hello"));
        query(
            &mut session,
            &TurnConfig::new(first_model, None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("first query should succeed");

        session.push_message(Message::user("follow up"));
        query(
            &mut session,
            &TurnConfig::new(second_model, Some("enabled".into())),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let diff_message = &session.messages[session.messages.len() - 3];
        let user_message = &session.messages[session.messages.len() - 2];
        assert_eq!(user_message, &Message::user("follow up"));
        let ContentBlock::Text { text } = &diff_message.content[0] else {
            panic!("expected text diff message");
        };
        assert!(text.contains("<context_changes>"));
        assert!(text.contains("<metadata>"));
        assert!(text.contains("<name>model</name>"));
        assert!(text.contains("<previous>model-a</previous>"));
        assert!(text.contains("<current>model-b</current>"));
    }

    #[tokio::test]
    async fn query_skips_context_diff_when_turn_metadata_unchanged() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "model-a".into(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());

        session.push_message(Message::user("hello"));
        query(
            &mut session,
            &TurnConfig::new(model.clone(), None),
            Arc::clone(&provider),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("first query should succeed");

        session.push_message(Message::user("follow up"));
        query(
            &mut session,
            &TurnConfig::new(model, None),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let follow_up_index = request_message_index_containing(&captured[1], "follow up");
        assert!(
            follow_up_index > 0,
            "follow-up user message should not be the first prompt message"
        );
        assert!(
            !message_contains(
                &captured[1].messages[follow_up_index - 1],
                "<context_changes>"
            ),
            "unchanged metadata after a completed turn should not insert a new context_changes before the next user message"
        );
    }

    #[tokio::test]
    async fn query_inserts_interrupted_notice_before_next_user_message() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        session.push_message(Message::assistant_text("partial"));
        session.mark_last_turn_interrupted();
        session.push_message(Message::user("continue please"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let abort_index = session
            .messages
            .iter()
            .position(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { text } if text.contains("<turn_aborted>")
                    )
                })
            })
            .expect("interrupted notice should be inserted");
        let continue_index = session
            .messages
            .iter()
            .position(|message| {
                message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { text } if text.contains("continue please")
                    )
                })
            })
            .expect("user message should remain");
        assert!(abort_index < continue_index);
        assert!(!session.last_turn_interrupted);
    }

    #[tokio::test]
    async fn query_pairs_interrupted_tool_result_when_cancel_fires_during_tool() {
        #[derive(Debug)]
        struct HangingMutatingTool;

        #[async_trait]
        impl ToolHandler for HangingMutatingTool {
            fn spec(&self) -> &crate::tools::tool_spec::ToolSpec {
                Box::leak(Box::new(crate::tools::tool_spec::ToolSpec {
                    name: "mutating_tool".into(),
                    description: "hangs until cancelled".into(),
                    input_schema: JsonSchema::object(Default::default(), None, None),
                    output_mode: ToolOutputMode::Text,
                    execution_mode: ToolExecutionMode::Mutating,
                    capability_tags: vec![],
                    supports_parallel: false,
                    preparation_feedback: ToolPreparationFeedback::None,
                    display_name: None,
                    supports_cancellation: None,
                    supports_streaming: None,
                }))
            }

            async fn handle(
                &self,
                _ctx: crate::tools::contracts::ToolContext,
                _input: serde_json::Value,
                _progress: Option<crate::tools::contracts::ToolProgressSender>,
            ) -> Result<crate::tools::contracts::ToolResult, crate::tools::contracts::ToolCallError>
            {
                std::future::pending::<()>().await;
                unreachable!("tool should be cancelled")
            }
        }

        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(HangingMutatingTool));
        builder.push_spec(crate::tools::tool_spec::ToolSpec {
            name: "mutating_tool".into(),
            description: "hangs until cancelled".into(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let cancel_token = CancellationToken::new();
        let cancel_for_task = cancel_token.clone();
        let runtime = ToolRuntime::new_with_context_and_options(
            Arc::clone(&registry),
            PermissionChecker::always_allow(),
            ToolRuntimeContext::default(),
            ToolExecutionOptions {
                cancel_token: cancel_token.clone(),
                ..ToolExecutionOptions::default()
            },
        );
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));
        let turn_config = TurnConfig::new(Model::default(), None);
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(SingleToolUseProvider {
            requests: AtomicUsize::new(0),
        });

        let result = {
            let mut query_future = std::pin::pin!(query_with_options(
                &mut session,
                &turn_config,
                provider,
                registry,
                &runtime,
                None,
                QueryOptions {
                    cancel_token: Some(cancel_token),
                    ..QueryOptions::default()
                },
            ));
            tokio::select! {
                result = &mut query_future => {
                    panic!("query completed before cancel: {result:?}");
                }
                () = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                    cancel_for_task.cancel();
                }
            }
            query_future.await
        };
        assert!(matches!(result, Err(AgentError::Aborted)));

        let tool_use = session.messages.iter().find(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { id, .. } if id == "tool-1"))
        });
        assert!(tool_use.is_some(), "tool call should be retained");

        let tool_result = session.messages.iter().find_map(|message| {
            message.content.iter().find_map(|block| match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if tool_use_id == "tool-1" => Some((content.clone(), *is_error)),
                _ => None,
            })
        });
        let (content, is_error) = tool_result.expect("interrupted tool result should exist");
        assert!(is_error);
        assert_eq!(content, crate::tools::INTERRUPTED_TOOL_RESULT_MESSAGE);
    }

    #[tokio::test]
    async fn query_drops_orphaned_tool_calls_from_prompt_history() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(CapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());

        session.push_message(Message::user("first"));
        session.push_message(Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Calling tool".into(),
                },
                ContentBlock::ToolUse {
                    id: "call-1".into(),
                    name: "bash".into(),
                    input: json!({ "cmd": "pwd" }),
                },
            ],
        });
        session.push_message(Message::user("follow up"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert!(
            captured[0]
                .messages
                .iter()
                .flat_map(|message| message.content.iter())
                .all(|content| !matches!(content, devo_protocol::RequestContent::ToolUse { .. })),
            "expected orphaned tool calls to be removed from prompt history"
        );
    }

    #[tokio::test]
    async fn test_model_connection_sends_minimal_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = CapturingProvider {
            requests: Arc::clone(&requests),
        };
        let model = Model {
            slug: "glm-4.5".into(),
            reasoning_capability: devo_protocol::ReasoningCapability::Toggle,
            top_p: Some(0.95),
            ..Model::default()
        };
        let preview = test_model_connection(
            &provider,
            &model,
            devo_protocol::ModelProfileKey::CatalogSlug(model.slug.clone()),
            "renamed-provider-model",
            "Reply with OK only.",
        )
        .await
        .expect("probe request should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(preview, "done");
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].model_slug,
            devo_protocol::ModelProfileKey::CatalogSlug("glm-4.5".to_string())
        );
        assert_eq!(captured[0].model, "renamed-provider-model");
        assert_eq!(captured[0].request_thinking.as_deref(), Some("enabled"));
        assert_eq!(captured[0].system, None);
        assert!(captured[0].tools.is_none());
        assert_eq!(captured[0].messages.len(), 1);
        assert_eq!(captured[0].sampling.top_p, Some(0.95));
    }

    #[tokio::test]
    async fn query_persists_streamed_reasoning_for_follow_up_request() {
        struct ReasoningProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
        }

        #[async_trait]
        impl devo_provider::ModelProviderSDK for ReasoningProvider {
            async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
                unreachable!("tests stream responses only")
            }

            async fn completion_stream(
                &self,
                request: ModelRequest,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
                self.requests.lock().expect("lock requests").push(request);
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(StreamEvent::ReasoningStart { index: 0 }),
                    Ok(StreamEvent::ReasoningDelta {
                        index: 0,
                        text: "plan".into(),
                    }),
                    Ok(StreamEvent::TextStart { index: 1 }),
                    Ok(StreamEvent::TextDelta {
                        index: 1,
                        text: "final".into(),
                    }),
                    Ok(StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: "resp-3".into(),
                            content: vec![ResponseContent::Text("final".into())],
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata {
                                extras: vec![ResponseExtra::ReasoningText {
                                    text: "plan".into(),
                                }],
                            },
                        },
                    }),
                ])))
            }

            fn name(&self) -> &str {
                "reasoning-provider"
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(ReasoningProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let seen_events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&seen_events);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let callback_events = Arc::clone(&callback_events);
            Box::pin(async move {
                callback_events.lock().expect("lock callback").push(event);
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider.clone(),
            Arc::clone(&registry),
            &runtime,
            Some(callback),
        )
        .await
        .expect("first query should succeed");

        {
            let events = seen_events.lock().expect("lock events");
            assert!(events.iter().any(|event| matches!(
                event,
                QueryEvent::ReasoningDelta(text) if text == "plan"
            )));
        }

        let assistant_message = session
            .messages
            .iter()
            .find(|message| matches!(message.role, Role::Assistant))
            .expect("assistant message");
        assert_eq!(
            assistant_message,
            &Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Reasoning {
                        text: "plan".into(),
                    },
                    ContentBlock::Text {
                        text: "final".into(),
                    },
                ],
            }
        );

        session.push_message(Message::user("follow up"));
        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let replayed_assistant = captured[1]
            .messages
            .iter()
            .find(|message| message.role == "assistant")
            .expect("assistant replay");
        assert_eq!(
            serde_json::to_value(replayed_assistant).expect("serialize assistant replay"),
            json!({
                "role": "assistant",
                "content": [
                    { "type": "reasoning", "text": "plan" },
                    { "type": "text", "text": "final" }
                ]
            })
        );
    }

    #[tokio::test]
    async fn query_round_trips_provider_reasoning_without_plain_reasoning() {
        struct SignedReasoningProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            calls: AtomicUsize,
        }

        #[async_trait]
        impl devo_provider::ModelProviderSDK for SignedReasoningProvider {
            async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
                unreachable!("tests stream responses only")
            }

            async fn completion_stream(
                &self,
                request: ModelRequest,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
                self.requests.lock().expect("lock requests").push(request);
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let content = if call == 0 {
                    vec![
                        ResponseContent::ProviderReasoning {
                            provider: "anthropic".into(),
                            payload: json!({
                                "type": "thinking",
                                "thinking": "signed plan",
                                "signature": "sig_123"
                            }),
                        },
                        ResponseContent::Text("first".into()),
                    ]
                } else {
                    vec![ResponseContent::Text("second".into())]
                };
                Ok(Box::pin(futures::stream::iter(vec![Ok(
                    StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: format!("resp-{call}"),
                            content,
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata::default(),
                        },
                    },
                )])))
            }

            fn name(&self) -> &str {
                "signed-reasoning-provider"
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(SignedReasoningProvider {
            requests: Arc::clone(&requests),
            calls: AtomicUsize::new(0),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let seen_events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&seen_events);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let callback_events = Arc::clone(&callback_events);
            Box::pin(async move {
                callback_events.lock().expect("lock callback").push(event);
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider.clone(),
            Arc::clone(&registry),
            &runtime,
            Some(callback),
        )
        .await
        .expect("first query should succeed");

        {
            let events = seen_events.lock().expect("lock events");
            assert!(events.iter().any(|event| matches!(
                event,
                QueryEvent::ReasoningDelta(text) if text == "signed plan"
            )));
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, QueryEvent::ReasoningCompleted))
            );
        }

        let assistant_message = session
            .messages
            .iter()
            .find(|message| matches!(message.role, Role::Assistant))
            .expect("assistant message");
        assert_eq!(
            assistant_message,
            &Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: json!({
                            "type": "thinking",
                            "thinking": "signed plan",
                            "signature": "sig_123"
                        }),
                    },
                    ContentBlock::Text {
                        text: "first".into(),
                    },
                ],
            }
        );

        session.push_message(Message::user("follow up"));
        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let second_request_content = captured[1]
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .collect::<Vec<_>>();
        assert!(second_request_content.iter().any(|content| matches!(
            content,
            RequestContent::ProviderReasoning { provider, payload }
            if provider == "anthropic"
                && payload["thinking"] == json!("signed plan")
                && payload["signature"] == json!("sig_123")
        )));
        assert!(
            second_request_content
                .iter()
                .all(|content| !matches!(content, RequestContent::Reasoning { .. }))
        );
    }

    #[tokio::test]
    async fn query_continues_deepseek_v4_thinking_only_end_turn_once() {
        struct ThinkingOnlyThenTextProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            calls: AtomicUsize,
        }

        #[async_trait]
        impl devo_provider::ModelProviderSDK for ThinkingOnlyThenTextProvider {
            async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
                unreachable!("tests stream responses only")
            }

            async fn completion_stream(
                &self,
                request: ModelRequest,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
                self.requests.lock().expect("lock requests").push(request);
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let content = if call == 0 {
                    vec![ResponseContent::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: json!({
                            "type": "thinking",
                            "thinking": "internal plan",
                            "signature": "sig_plan"
                        }),
                    }]
                } else {
                    vec![ResponseContent::Text("visible answer".into())]
                };
                let message_done = Ok(StreamEvent::MessageDone {
                    response: ModelResponse {
                        id: format!("resp-{call}"),
                        content,
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Usage::default(),
                        metadata: ResponseMetadata::default(),
                    },
                });
                let events = if call == 0 {
                    vec![message_done]
                } else {
                    vec![
                        Ok(StreamEvent::TextDelta {
                            index: 0,
                            text: "visible answer".into(),
                        }),
                        message_done,
                    ]
                };
                Ok(Box::pin(futures::stream::iter(events)))
            }

            fn name(&self) -> &str {
                "thinking-only-then-text-provider"
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(ThinkingOnlyThenTextProvider {
            requests: Arc::clone(&requests),
            calls: AtomicUsize::new(0),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "deepseek-v4-pro".into(),
            provider: devo_protocol::ProviderWireApi::AnthropicMessages,
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));
        let seen_events = Arc::new(Mutex::new(Vec::new()));
        let callback_events = Arc::clone(&seen_events);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let callback_events = Arc::clone(&callback_events);
            Box::pin(async move {
                callback_events.lock().expect("lock callback").push(event);
            })
        });

        query(
            &mut session,
            &TurnConfig::new(model, None),
            provider,
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should continue once and finish with text");

        let session_message_tail = session.messages[session.messages.len() - 4..].to_vec();
        assert_eq!(
            session_message_tail,
            vec![
                Message::user("hello"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ProviderReasoning {
                        provider: "anthropic".into(),
                        payload: json!({
                            "type": "thinking",
                            "thinking": "internal plan",
                            "signature": "sig_plan"
                        }),
                    }],
                },
                Message::user(super::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT),
                Message::assistant_text("visible answer"),
            ]
        );

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let second_request_messages = &captured[1].messages;
        let second_request_tail = &second_request_messages[second_request_messages.len() - 3..];
        assert_eq!(
            serde_json::to_value(second_request_tail).expect("serialize second request messages"),
            json!([
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": "hello"
                    }]
                },
                {
                    "role": "assistant",
                    "content": [{
                        "type": "provider_reasoning",
                        "provider": "anthropic",
                        "payload": {
                            "type": "thinking",
                            "thinking": "internal plan",
                            "signature": "sig_plan"
                        }
                    }]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": super::DEEPSEEK_THINKING_ONLY_CONTINUATION_PROMPT
                    }]
                }
            ])
        );

        let events = seen_events.lock().expect("lock events");
        let turn_complete_count = events
            .iter()
            .filter(|event| matches!(event, QueryEvent::TurnComplete { .. }))
            .count();
        assert_eq!(turn_complete_count, 1);
        assert!(events.iter().any(|event| matches!(
            event,
            QueryEvent::TextDelta(text) if text == "visible answer"
        )));
    }

    #[tokio::test]
    async fn query_preserves_provider_reasoning_and_hosted_tool_order() {
        struct OrderedHostedProvider {
            requests: Arc<Mutex<Vec<ModelRequest>>>,
            calls: AtomicUsize,
        }

        #[async_trait]
        impl devo_provider::ModelProviderSDK for OrderedHostedProvider {
            async fn completion(&self, _request: ModelRequest) -> Result<ModelResponse> {
                unreachable!("tests stream responses only")
            }

            async fn completion_stream(
                &self,
                request: ModelRequest,
            ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
                self.requests.lock().expect("lock requests").push(request);
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let content = if call == 0 {
                    vec![
                        ResponseContent::ProviderReasoning {
                            provider: "anthropic".into(),
                            payload: json!({
                                "type": "thinking",
                                "thinking": "before tool",
                                "signature": "sig_before"
                            }),
                        },
                        ResponseContent::HostedToolUse {
                            id: "srvtool_1".into(),
                            name: "web_search".into(),
                            input: json!({"query": "desktop gui 2026"}),
                            output: None,
                            status: None,
                        },
                        ResponseContent::HostedToolUse {
                            id: "srvtool_1".into(),
                            name: "web_search".into(),
                            input: json!({}),
                            output: Some(json!([{"title": "result"}])),
                            status: Some("completed".into()),
                        },
                        ResponseContent::ProviderReasoning {
                            provider: "anthropic".into(),
                            payload: json!({
                                "type": "thinking",
                                "thinking": "after tool",
                                "signature": "sig_after"
                            }),
                        },
                        ResponseContent::Text("final".into()),
                    ]
                } else {
                    vec![ResponseContent::Text("second".into())]
                };
                Ok(Box::pin(futures::stream::iter(vec![Ok(
                    StreamEvent::MessageDone {
                        response: ModelResponse {
                            id: format!("resp-{call}"),
                            content,
                            stop_reason: Some(StopReason::EndTurn),
                            usage: Usage::default(),
                            metadata: ResponseMetadata::default(),
                        },
                    },
                )])))
            }

            fn name(&self) -> &str {
                "ordered-hosted-provider"
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(OrderedHostedProvider {
            requests: Arc::clone(&requests),
            calls: AtomicUsize::new(0),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("hello"));

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider.clone(),
            Arc::clone(&registry),
            &runtime,
            None,
        )
        .await
        .expect("first query should succeed");

        let assistant_message = session
            .messages
            .iter()
            .find(|message| matches!(message.role, Role::Assistant))
            .expect("assistant message");
        assert_eq!(
            assistant_message.content,
            vec![
                ContentBlock::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "before tool",
                        "signature": "sig_before"
                    }),
                },
                ContentBlock::HostedToolUse {
                    id: "srvtool_1".into(),
                    name: "web_search".into(),
                    input: json!({"query": "desktop gui 2026"}),
                    output: None,
                    status: None,
                },
                ContentBlock::HostedToolUse {
                    id: "srvtool_1".into(),
                    name: "web_search".into(),
                    input: json!({"query": "desktop gui 2026"}),
                    output: Some(json!([{"title": "result"}])),
                    status: Some("completed".into()),
                },
                ContentBlock::ProviderReasoning {
                    provider: "anthropic".into(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "after tool",
                        "signature": "sig_after"
                    }),
                },
                ContentBlock::Text {
                    text: "final".into(),
                },
            ]
        );

        session.push_message(Message::user("follow up"));
        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            provider,
            registry,
            &runtime,
            None,
        )
        .await
        .expect("second query should succeed");

        let captured = requests.lock().expect("lock requests");
        let replayed_content = captured[1]
            .messages
            .iter()
            .find(|message| message.role == "assistant")
            .expect("assistant replay")
            .content
            .clone();
        assert_eq!(
            serde_json::to_value(&replayed_content).expect("serialize replayed content"),
            json!([
                {
                    "type": "provider_reasoning",
                    "provider": "anthropic",
                    "payload": {
                        "type": "thinking",
                        "thinking": "before tool",
                        "signature": "sig_before"
                    }
                },
                {
                    "type": "hosted_tool_use",
                    "id": "srvtool_1",
                    "name": "web_search",
                    "input": { "query": "desktop gui 2026" }
                },
                {
                    "type": "hosted_tool_use",
                    "id": "srvtool_1",
                    "name": "web_search",
                    "input": { "query": "desktop gui 2026" },
                    "output": [{ "title": "result" }],
                    "status": "completed"
                },
                {
                    "type": "provider_reasoning",
                    "provider": "anthropic",
                    "payload": {
                        "type": "thinking",
                        "thinking": "after tool",
                        "signature": "sig_after"
                    }
                },
                {
                    "type": "text",
                    "text": "final"
                }
            ])
        );
    }

    #[tokio::test]
    async fn query_disables_openai_thinking_when_reasoning_context_is_missing() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider: Arc<dyn ModelProviderSDK> = Arc::new(OpenAiCapturingProvider {
            requests: Arc::clone(&requests),
        });
        let registry = Arc::new(ToolRegistry::new());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let model = Model {
            slug: "deepseek-v4-flash".into(),
            provider: devo_protocol::ProviderWireApi::OpenAIChatCompletions,
            reasoning_capability: ReasoningCapability::Toggle,
            base_instructions: String::new(),
            ..Model::default()
        };
        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::assistant_text("legacy assistant reply"));
        session.push_message(Message::user("follow up"));

        query(
            &mut session,
            &TurnConfig::new(model, Some("enabled".into())),
            Arc::clone(&provider),
            registry,
            &runtime,
            None,
        )
        .await
        .expect("query should succeed");

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].model_slug,
            devo_protocol::ModelProfileKey::CatalogSlug("deepseek-v4-flash".to_string())
        );
        assert_eq!(captured[0].request_thinking.as_deref(), Some("enabled"));
        // Toggle capability does not set reasoning_effort on the request.
        assert_eq!(captured[0].reasoning_effort, None);
    }

    #[tokio::test]
    async fn query_tool_result_summary_is_set() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(MutatingTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                if let QueryEvent::ToolResult { summary, .. } = event {
                    seen_clone.lock().unwrap().push(summary);
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        let summaries = seen.lock().unwrap();
        assert!(
            !summaries.is_empty(),
            "should have at least one ToolResult summary"
        );
        for summary in summaries.iter() {
            assert!(!summary.is_empty(), "summary should not be empty");
        }
    }

    #[tokio::test]
    async fn query_tool_result_event_includes_final_tool_input() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                if let QueryEvent::ToolResult {
                    tool_name, input, ..
                } = event
                {
                    seen_clone.lock().unwrap().push((tool_name, input));
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[(String::from("mutating_tool"), json!({ "value": 1 }))]
        );
    }

    #[tokio::test]
    async fn query_tool_result_event_matches_input_delta_by_tool_index() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tools"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                if let QueryEvent::ToolResult {
                    tool_use_id, input, ..
                } = event
                {
                    seen_clone.lock().unwrap().push((tool_use_id, input));
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(InterleavedToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[
                (String::from("tool-1"), json!({ "value": 1 })),
                (String::from("tool-2"), json!({ "value": 2 })),
            ]
        );
    }

    #[tokio::test]
    async fn query_truncates_model_visible_tool_results_but_emits_raw_tool_result_events() {
        let full_content = "abcdefghijklmnopqrstuvwxyz".to_string();
        let display_content = "raw display abcdefghijklmnopqrstuvwxyz".to_string();
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler(
            "mutating_tool",
            Arc::new(LargeToolResultTool {
                content: full_content.clone(),
                display_content: Some(display_content.clone()),
            }),
        );
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));
        let requests = Arc::new(Mutex::new(Vec::new()));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                if let QueryEvent::ToolResult {
                    content,
                    display_content,
                    ..
                } = event
                {
                    seen_clone
                        .lock()
                        .expect("lock seen events")
                        .push((content.into_string(), display_content));
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(
                Model {
                    truncation_policy: TruncationPolicyConfig::bytes(20),
                    ..Model::default()
                },
                None,
            ),
            Arc::new(CapturingToolUseProvider {
                requests: Arc::clone(&requests),
                calls: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().expect("lock seen events").as_slice(),
            &[(full_content.clone(), Some(display_content))]
        );

        let captured = requests.lock().expect("lock requests");
        assert_eq!(captured.len(), 2);
        let model_visible_tool_result = captured[1]
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .find_map(|content| match content {
                RequestContent::ToolResult { content, .. } => Some(content.as_str()),
                RequestContent::Text { .. }
                | RequestContent::Reasoning { .. }
                | RequestContent::ProviderReasoning { .. }
                | RequestContent::HostedToolUse { .. }
                | RequestContent::ToolUse { .. } => None,
            })
            .expect("continuation request should include tool result");
        assert_eq!(model_visible_tool_result, "abcde\n...[truncated]");
    }

    #[tokio::test]
    async fn query_tool_start_event_includes_final_tool_input() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tools"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                if let QueryEvent::ToolUseStart { id, input, .. } = event {
                    seen_clone.lock().unwrap().push((id, input));
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(InterleavedToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[
                (String::from("tool-1"), json!({ "value": 1 })),
                (String::from("tool-2"), json!({ "value": 2 })),
            ]
        );
    }

    #[tokio::test]
    #[ignore = "legacy progress mechanism replaced by L3 contracts"]
    async fn query_emits_tool_result_display_content() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(DisplayContentTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                if let QueryEvent::ToolResult {
                    content,
                    display_content,
                    ..
                } = event
                {
                    seen_clone.lock().unwrap().push((content, display_content));
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(matches!(
            &seen[0],
            (crate::tools::ToolContent::Text(text), Some(display))
                if text == "canonical" && display == "display"
        ));
    }

    #[tokio::test]
    #[ignore = "legacy progress mechanism replaced by L3 contracts"]
    async fn query_emits_tool_progress_before_tool_result() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("mutating_tool", Arc::new(StreamingMutatingTool));
        builder.push_spec(ToolSpec {
            name: "mutating_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::Mutating,
            capability_tags: vec![],
            supports_parallel: false,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tool"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                seen_clone.lock().unwrap().push(event);
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(SingleToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        let events = seen.lock().unwrap();
        let progress_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    QueryEvent::ToolProgress {
                        tool_use_id,
                        progress: crate::tools::ToolProgress::OutputDelta { delta },
                    } if tool_use_id == "tool-1" && delta == "stream chunk\n"
                )
            })
            .expect("tool progress event should be emitted");
        let result_index = events
            .iter()
            .position(|event| {
                matches!(
                    event,
                    QueryEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                        ..
                    } if tool_use_id == "tool-1"
                        && matches!(content, crate::tools::ToolContent::Text(text) if text == "stream complete")
                        && !is_error
                )
            })
            .expect("tool result event should be emitted");

        assert!(
            progress_index < result_index,
            "tool progress should arrive before final result"
        );
    }

    #[tokio::test]
    async fn query_emits_parallel_tool_results_as_each_tool_finishes() {
        let mut builder = ToolRegistryBuilder::new();
        builder.register_handler("parallel_tool", Arc::new(ParallelDelayTool));
        builder.push_spec(ToolSpec {
            name: "parallel_tool".into(),
            description: String::new(),
            input_schema: JsonSchema::object(Default::default(), None, None),
            output_mode: ToolOutputMode::Text,
            execution_mode: ToolExecutionMode::ReadOnly,
            capability_tags: vec![],
            supports_parallel: true,
            preparation_feedback: ToolPreparationFeedback::None,
            display_name: None,
            supports_cancellation: None,
            supports_streaming: None,
        });
        let registry = Arc::new(builder.build());
        let runtime = ToolRuntime::new_without_permissions(Arc::clone(&registry));

        let mut session = SessionState::new(SessionConfig::default(), std::env::temp_dir());
        session.push_message(Message::user("run the tools"));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = Arc::clone(&seen);
        let callback: EventCallback = Arc::new(move |event: QueryEvent| {
            let seen_clone = Arc::clone(&seen_clone);
            Box::pin(async move {
                match event {
                    QueryEvent::ToolUseStart { id, .. } => {
                        seen_clone
                            .lock()
                            .expect("lock events")
                            .push(format!("start:{id}"));
                    }
                    QueryEvent::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        let content = content.into_string();
                        seen_clone
                            .lock()
                            .expect("lock events")
                            .push(format!("result:{tool_use_id}:{content}"));
                    }
                    _ => {}
                }
            })
        });

        query(
            &mut session,
            &TurnConfig::new(Model::default(), None),
            Arc::new(ParallelToolUseProvider {
                requests: AtomicUsize::new(0),
            }),
            registry,
            &runtime,
            Some(callback),
        )
        .await
        .expect("query should complete");

        assert_eq!(
            seen.lock().expect("lock events").as_slice(),
            &[
                "start:slow".to_string(),
                "start:fast".to_string(),
                "result:fast:fast complete".to_string(),
                "result:slow:slow complete".to_string(),
            ]
        );

        let tool_result_ids = session
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| match block {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_result_ids, vec!["slow", "fast"]);
    }
}
