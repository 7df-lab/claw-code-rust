//! Consume one provider completion stream into an assembled model turn.
//!
//! Hides `StreamEvent` matching, hosted-tool normalization, and assistant
//! content assembly so the query loop only sees a structured outcome (or a
//! retryable / fatal stream error).

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::pin::Pin;

use futures::Stream;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::ContentBlock;
use crate::SessionState;
use crate::tools::ToolCall;
use crate::tools::ToolContent;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_provider::ModelProviderSDK;

use super::event::EventCallback;
use super::event::QueryEvent;
use super::event::emit_query_event;

/// Structured result of one successful provider stream attempt.
pub(crate) struct AssembledModelTurn {
    pub assistant_content: Vec<ContentBlock>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: Option<StopReason>,
    pub has_hosted_tool_uses: bool,
    pub has_provider_reasoning: bool,
    pub has_visible_assistant_text: bool,
}

/// Failure while creating or consuming a provider stream.
pub(crate) enum ProviderAttemptError {
    /// Stream creation failed before any events.
    Create(anyhow::Error),
    /// Stream failed before any useful content; safe to classify for retry.
    Retryable(anyhow::Error),
    /// Stream failed after partial content was observed.
    Fatal(anyhow::Error),
}

type ProviderEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>;

/// Create a provider stream and assemble the model turn.
pub(crate) async fn run_provider_attempt(
    provider: &dyn ModelProviderSDK,
    request: ModelRequest,
    session: &mut SessionState,
    on_event: &Option<EventCallback>,
    cancel_token: Option<&CancellationToken>,
    model_slug: &str,
) -> Result<AssembledModelTurn, ProviderAttemptError> {
    let stream = match provider.completion_stream(request).await {
        Ok(stream) => stream,
        Err(error) => {
            warn!(
                provider = provider.name(),
                model = %model_slug,
                turn = session.turn_count,
                error = ?error,
                "failed to create provider stream"
            );
            return Err(ProviderAttemptError::Create(error));
        }
    };

    consume_provider_stream(
        stream,
        session,
        on_event,
        cancel_token,
        provider.name(),
        model_slug,
    )
    .await
}

struct StreamAccumulation {
    assistant_text: String,
    reasoning_text: String,
    tool_uses: Vec<(usize, String, String, serde_json::Value, String, bool)>,
    hosted_tool_inputs: HashMap<String, (usize, String, serde_json::Value)>,
    emitted_tool_use_starts: HashSet<String>,
    emitted_hosted_tool_starts: HashSet<String>,
    emitted_hosted_tool_results: HashSet<String>,
    final_response: Option<ModelResponse>,
    stop_reason: Option<StopReason>,
}

async fn consume_provider_stream(
    mut stream: ProviderEventStream,
    session: &mut SessionState,
    on_event: &Option<EventCallback>,
    cancel_token: Option<&CancellationToken>,
    provider_name: &str,
    model_slug: &str,
) -> Result<AssembledModelTurn, ProviderAttemptError> {
    let mut acc = StreamAccumulation {
        assistant_text: String::new(),
        reasoning_text: String::new(),
        tool_uses: Vec::new(),
        hosted_tool_inputs: HashMap::new(),
        emitted_tool_use_starts: HashSet::new(),
        emitted_hosted_tool_starts: HashSet::new(),
        emitted_hosted_tool_results: HashSet::new(),
        final_response: None,
        stop_reason: None,
    };

    loop {
        tokio::select! {
            biased;
            _ = async {
                if let Some(ct) = cancel_token {
                    ct.cancelled().await
                } else {
                    std::future::pending::<()>().await
                }
            } => {
                break;
            }
            event = stream.next() => {
                let Some(event) = event else { break; };
                match event {
                    Ok(StreamEvent::TextStart { .. }) => {}
                    Ok(StreamEvent::TextDelta { text, .. }) => {
                        acc.assistant_text.push_str(&text);
                        emit_query_event(on_event, QueryEvent::TextDelta(text)).await;
                    }
                    Ok(StreamEvent::ReasoningStart { .. }) => {}
                    Ok(StreamEvent::ReasoningDelta { text, .. }) => {
                        acc.reasoning_text.push_str(&text);
                        emit_query_event(on_event, QueryEvent::ReasoningDelta(text)).await;
                    }
                    Ok(StreamEvent::ReasoningDone { .. }) => {
                        emit_query_event(on_event, QueryEvent::ReasoningCompleted).await;
                    }
                    Ok(StreamEvent::ToolCallStart {
                        index,
                        id,
                        name,
                        input,
                    }) => {
                        acc.tool_uses.push((index, id, name, input, String::new(), false));
                    }
                    Ok(StreamEvent::HostedToolCallStart {
                        index,
                        id,
                        name,
                        input,
                    }) => {
                        let id = normalize_hosted_tool_id(index, id, &name);
                        let name = normalize_hosted_tool_name(name);
                        acc.hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                        emit_hosted_tool_start(
                            on_event,
                            &mut acc.emitted_hosted_tool_starts,
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
                        let previous_input = acc.hosted_tool_inputs
                            .get(&id)
                            .map(|(_, _, previous_input)| previous_input);
                        let input = hosted_tool_input_or_previous(input, previous_input);
                        acc.hosted_tool_inputs.insert(id.clone(), (index, name.clone(), input.clone()));
                        emit_hosted_tool_start(
                            on_event,
                            &mut acc.emitted_hosted_tool_starts,
                            &id,
                            &name,
                            &input,
                        )
                        .await;
                        emit_hosted_tool_result(
                            on_event,
                            &mut acc.emitted_hosted_tool_results,
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
                        if let Some(tool_use) = acc.tool_uses
                            .iter_mut()
                            .rev()
                            .find(|(tool_index, ..)| *tool_index == index)
                        {
                            tool_use.4.push_str(&partial_json);
                            tool_use.5 = true;
                        }
                    }
                    Ok(StreamEvent::MessageDone { response }) => {
                        acc.stop_reason = response.stop_reason.clone();
                        acc.final_response = Some(response.clone());

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
                            on_event,
                            QueryEvent::Usage {
                                usage: response.usage.clone(),
                            },
                        )
                        .await;
                    }
                    Ok(StreamEvent::UsageDelta(usage)) => {
                        emit_query_event(on_event, QueryEvent::UsageDelta { usage }).await;
                    }
                    Err(error) => {
                        warn!(
                            provider = provider_name,
                            model = %model_slug,
                            turn = session.turn_count,
                            error = ?error,
                            "stream error"
                        );
                        if !acc.assistant_text.is_empty()
                            || !acc.reasoning_text.is_empty()
                            || !acc.tool_uses.is_empty()
                            || !acc.hosted_tool_inputs.is_empty()
                            || acc.final_response.is_some()
                        {
                            return Err(ProviderAttemptError::Fatal(error));
                        }
                        return Err(ProviderAttemptError::Retryable(error));
                    }
                }
            }
        }
    }

    assemble_model_turn(session, on_event, acc).await
}

async fn assemble_model_turn(
    session: &SessionState,
    on_event: &Option<EventCallback>,
    acc: StreamAccumulation,
) -> Result<AssembledModelTurn, ProviderAttemptError> {
    let StreamAccumulation {
        mut assistant_text,
        mut reasoning_text,
        mut tool_uses,
        mut hosted_tool_inputs,
        mut emitted_tool_use_starts,
        mut emitted_hosted_tool_starts,
        mut emitted_hosted_tool_results,
        final_response,
        stop_reason,
    } = acc;
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
                        response_assistant_content.push(ContentBlock::Text { text: text.clone() });
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
                        on_event,
                        &mut emitted_hosted_tool_starts,
                        &id,
                        &name,
                        &input,
                    )
                    .await;
                    if output.is_some() || status.is_some() {
                        emit_hosted_tool_result(
                            on_event,
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
                    on_event,
                    QueryEvent::ReasoningDelta(final_reasoning.clone()),
                )
                .await;
                emit_query_event(on_event, QueryEvent::ReasoningCompleted).await;
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
                    on_event,
                    QueryEvent::ReasoningDelta(final_reasoning.clone()),
                )
                .await;
                emit_query_event(on_event, QueryEvent::ReasoningCompleted).await;
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
            on_event,
            &mut emitted_hosted_tool_starts,
            &id,
            &name,
            &input,
        )
        .await;
        emit_hosted_tool_result(
            on_event,
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

    let mut assistant_content: Vec<ContentBlock> = response_assistant_content;

    if !reasoning_text.trim().is_empty() && !has_provider_reasoning_content {
        assistant_content.insert(
            0,
            ContentBlock::Reasoning {
                text: reasoning_text,
            },
        );
    }

    let has_visible_assistant_text = !assistant_text.trim().is_empty();

    if assistant_content.is_empty() && !assistant_text.is_empty() {
        assistant_content.push(ContentBlock::Text {
            text: assistant_text,
        });
    }

    let final_tool_inputs: HashMap<String, serde_json::Value> = final_response
        .as_ref()
        .map(|response| {
            response
                .content
                .iter()
                .filter_map(|block| match block {
                    ResponseContent::ToolUse { id, input, .. } => Some((id.clone(), input.clone())),
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
            serde_json::from_str(&json_str)
                .unwrap_or_else(|_| final_tool_inputs.get(&id).cloned().unwrap_or(initial_input))
        } else {
            final_tool_inputs.get(&id).cloned().unwrap_or(initial_input)
        };
        if emitted_tool_use_starts.insert(id.clone()) {
            emit_query_event(
                on_event,
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

    Ok(AssembledModelTurn {
        assistant_content,
        tool_calls,
        stop_reason,
        has_hosted_tool_uses,
        has_provider_reasoning: has_provider_reasoning_content,
        has_visible_assistant_text,
    })
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
    session_cwd: &Path,
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

    let text = hosted_tool_result_text(status.as_deref());
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

fn hosted_tool_result_text(status: Option<&str>) -> String {
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
