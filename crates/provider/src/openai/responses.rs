use std::{collections::HashMap, pin::Pin};

use anyhow::{Context, Result};
use async_trait::async_trait;
use devo_protocol::{
    ModelRequest, ModelResponse, RequestContent, ResponseContent, ResponseExtra, ResponseMetadata,
    StopReason, StreamEvent, Usage,
};
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest_eventsource::{Event, EventSource};
use serde_json::{Value, json};
use tracing::debug;

use crate::error::ProviderError;
use crate::hosted_tools::append_openai_responses_hosted_tools;
use crate::http::invalid_status_error;
use crate::text_normalization::{TaggedTextFragment, TaggedTextParser, split_tagged_text};
use crate::{ModelProviderSDK, ProviderHttpOptions, merge_extra_body};

use super::capabilities::{OpenAITransport, resolve_request_profile};
use super::{
    OpenAIRole,
    shared::{request_role, tool_definitions},
};

/// OpenAI Responses API provider.
/// <https://developers.openai.com/api/reference/resources/responses>
/// This adapter keeps the new Responses wire format isolated from the legacy
/// chat-completions adapter so the transport can evolve independently.
pub struct OpenAIResponsesProvider {
    client: Client,
    streaming_client: Client,
    base_url: String,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
}

impl OpenAIResponsesProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        let http_options = ProviderHttpOptions::default();
        Self {
            client: http_options
                .build_request_client()
                .unwrap_or_else(|_| Client::new()),
            streaming_client: http_options
                .build_streaming_client()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.into(),
            api_key: None,
            http_options,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_http_options(mut self, http_options: ProviderHttpOptions) -> Result<Self> {
        self.client = http_options.build_request_client()?;
        self.streaming_client = http_options.build_streaming_client()?;
        self.http_options = http_options;
        Ok(self)
    }

    fn endpoint(&self) -> String {
        format!("{}/responses", self.base_url.trim_end_matches('/'))
    }

    fn post_builder(&self, client: &Client, body: &Value) -> reqwest::RequestBuilder {
        let builder = client
            .post(self.endpoint())
            .header(CONTENT_TYPE, "application/json");
        let builder = if let Some(api_key) = &self.api_key {
            builder.header(AUTHORIZATION, format!("Bearer {api_key}"))
        } else {
            builder
        };
        self.http_options.apply_custom_headers(builder).json(body)
    }

    fn request_builder(&self, body: &Value) -> reqwest::RequestBuilder {
        self.post_builder(&self.client, body)
    }

    fn streaming_request_builder(&self, body: &Value) -> reqwest::RequestBuilder {
        self.post_builder(&self.streaming_client, body)
    }
}

/// Builds the exact OpenAI Responses request body used by this provider.
fn build_request(request: &ModelRequest, stream: bool) -> Value {
    let profile = resolve_request_profile(&request.model_slug, OpenAITransport::Responses);
    let mut root = json!({
        "model": request.model,
        "input": build_input(request),
        "max_output_tokens": request.max_tokens,
        "stream": stream,
    });

    if let Some(tools) = &request.tools {
        root["tools"] = tool_definitions(tools);
    }

    if profile.supports_temperature
        && let Some(temperature) = request.sampling.temperature
    {
        root["temperature"] = json!(temperature);
    }

    if profile.supports_top_p
        && let Some(top_p) = request.sampling.top_p
    {
        root["top_p"] = json!(top_p);
    }

    if profile.supports_top_k
        && let Some(top_k) = request.sampling.top_k
    {
        root["top_k"] = json!(top_k);
    }

    if let Some(reasoning) = request.reasoning_effort {
        root["reasoning"] = json!({ "effort": reasoning });
    }

    if stream {
        root["stream_options"] = json!({ "include_usage": true });
    }

    append_openai_responses_hosted_tools(&mut root, &request.hosted_tools);

    merge_extra_body(&mut root, request.extra_body.as_ref());

    root
}

fn build_input(request: &ModelRequest) -> Vec<Value> {
    let mut input =
        Vec::with_capacity(request.messages.len() + usize::from(request.system.is_some()));

    if let Some(system) = &request.system {
        input.push(json!({
            "type": "message",
            "role": OpenAIRole::System,
            "content": [{"type": "input_text", "text": system}],
        }));
    }

    for message in &request.messages {
        let role = request_role(&message.role);
        if let Some(message) = build_input_message(role, &message.content) {
            input.push(message);
        }
    }

    input
}

fn build_input_message(role: OpenAIRole, content: &[RequestContent]) -> Option<Value> {
    let mut content_blocks = Vec::with_capacity(content.len());
    for block in content {
        match block {
            RequestContent::Text { text } => content_blocks.push(json!({
                "type": "input_text",
                "text": text,
            })),
            RequestContent::Reasoning { text } => content_blocks.push(json!({
                "type": "reasoning",
                "text": text,
            })),
            RequestContent::ProviderReasoning { .. } | RequestContent::HostedToolUse { .. } => {}
            RequestContent::ToolUse { id, name, input } => content_blocks.push(json!({
                "type": "tool_call",
                "id": id,
                "name": name,
                "input": input,
            })),
            RequestContent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => content_blocks.push(json!({
                "type": "function_call_output",
                "call_id": tool_use_id,
                "output": content,
                "is_error": is_error,
            })),
        }
    }

    (!content_blocks.is_empty()).then(|| {
        json!({
            "type": "message",
            "role": role,
            "content": content_blocks,
        })
    })
}

fn parse_response(value: Value) -> Result<ModelResponse> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut content = Vec::new();
    let mut metadata = ResponseMetadata::default();

    if let Some(output) = value.get("output").and_then(Value::as_array) {
        for item in output {
            if let Some(reasoning_content) = item.get("reasoning_content").and_then(Value::as_str) {
                metadata.extras.push(ResponseExtra::ReasoningText {
                    text: reasoning_content.to_string(),
                });
            }
            if matches!(item.get("type").and_then(Value::as_str), Some("message")) {
                if let Some(items) = item.get("content").and_then(Value::as_array) {
                    for message_item in items {
                        if let Some(text) = message_item.get("text").and_then(Value::as_str) {
                            let (assistant_text, reasoning) = split_tagged_text(text);
                            for text in reasoning {
                                if !text.is_empty() {
                                    metadata.extras.push(ResponseExtra::ReasoningText { text });
                                }
                            }
                            if !assistant_text.is_empty() {
                                content.push(ResponseContent::Text(assistant_text));
                            }
                            continue;
                        }
                        if let Some(parsed) = parse_message_content(message_item) {
                            content.push(parsed);
                        }
                    }
                }
                continue;
            }
            content.extend(parse_output_item(item));
        }
    }

    let stop_reason = value
        .get("status")
        .and_then(Value::as_str)
        .map(parse_status_reason)
        .or_else(|| {
            value
                .get("incomplete")
                .and_then(|item| item.get("reason"))
                .and_then(Value::as_str)
                .map(parse_status_reason)
        });

    let usage = value.get("usage").and_then(parse_usage).unwrap_or_default();

    Ok(ModelResponse {
        id,
        content,
        stop_reason,
        usage,
        metadata,
    })
}

fn parse_output_item(item: &Value) -> Vec<ResponseContent> {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => item
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(parse_message_content)
            .collect(),
        Some("function_call") | Some("tool_call") => {
            let id = item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let input = parse_function_call_arguments(item);
            vec![ResponseContent::ToolUse { id, name, input }]
        }
        Some("web_search_call") => vec![parse_hosted_web_search_call(item)],
        Some("web_fetch_call") => vec![parse_hosted_web_fetch_call(item)],
        Some("reasoning") => Vec::new(),
        _ => Vec::new(),
    }
}

fn parse_message_content(item: &Value) -> Option<ResponseContent> {
    match item.get("type").and_then(Value::as_str) {
        Some("output_text") | Some("text") | Some("input_text") => {
            let assistant_text =
                split_tagged_text(item.get("text").and_then(Value::as_str).unwrap_or_default()).0;
            if assistant_text.is_empty() {
                None
            } else {
                Some(ResponseContent::Text(assistant_text))
            }
        }
        Some("tool_call") | Some("function_call") => Some(ResponseContent::ToolUse {
            id: item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input: parse_function_call_arguments(item),
        }),
        Some("web_search_call") => Some(parse_hosted_web_search_call(item)),
        Some("web_fetch_call") => Some(parse_hosted_web_fetch_call(item)),
        _ => None,
    }
}

fn parse_function_call_arguments(item: &Value) -> Value {
    match item.get("arguments").or_else(|| item.get("input")) {
        Some(Value::String(arguments)) => parse_function_call_arguments_json(arguments),
        Some(Value::Null) | None => Value::Object(serde_json::Map::new()),
        Some(value) => value.clone(),
    }
}

fn function_call_arguments_json(item: &Value) -> String {
    match item.get("arguments").or_else(|| item.get("input")) {
        Some(Value::String(arguments)) => arguments.clone(),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

fn parse_function_call_arguments_json(arguments_json: &str) -> Value {
    if arguments_json.trim().is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(arguments_json).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

fn parse_hosted_web_search_call(item: &Value) -> ResponseContent {
    ResponseContent::HostedToolUse {
        id: item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: "web_search".to_string(),
        input: hosted_web_search_input(item),
        output: hosted_web_search_output(item),
        status: item
            .get("status")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    }
}

fn hosted_web_search_input(item: &Value) -> Value {
    if let Some(query) = item
        .get("action")
        .and_then(|action| action.get("query"))
        .and_then(Value::as_str)
    {
        return json!({ "query": query });
    }
    if let Some(query) = item.get("query").and_then(Value::as_str) {
        return json!({ "query": query });
    }
    item.get("action")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
}

fn hosted_web_search_output(item: &Value) -> Option<Value> {
    item.get("results")
        .or_else(|| item.get("result"))
        .or_else(|| item.get("content"))
        .cloned()
}

fn parse_hosted_web_fetch_call(item: &Value) -> ResponseContent {
    ResponseContent::HostedToolUse {
        id: item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        name: "web_fetch".to_string(),
        input: hosted_web_fetch_input(item),
        output: hosted_web_fetch_output(item),
        status: item
            .get("status")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    }
}

fn hosted_web_fetch_input(item: &Value) -> Value {
    if let Some(url) = item
        .get("action")
        .and_then(|action| action.get("url"))
        .and_then(Value::as_str)
    {
        return json!({ "url": url });
    }
    if let Some(url) = item.get("url").and_then(Value::as_str) {
        return json!({ "url": url });
    }
    if let Some(url) = item
        .get("input")
        .and_then(|input| input.get("url"))
        .and_then(Value::as_str)
    {
        return json!({ "url": url });
    }
    item.get("action")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
}

fn hosted_web_fetch_output(item: &Value) -> Option<Value> {
    item.get("output")
        .or_else(|| item.get("results"))
        .or_else(|| item.get("result"))
        .or_else(|| item.get("content"))
        .cloned()
}

fn stream_error(message: String) -> ProviderError {
    ProviderError::StreamError {
        message,
        bytes_received: None,
    }
}

fn parse_usage(value: &Value) -> Option<Usage> {
    Some(Usage {
        input_tokens: value
            .get("input_tokens")
            .or_else(|| value.get("prompt_tokens"))
            .and_then(Value::as_u64)? as usize,
        output_tokens: value
            .get("output_tokens")
            .or_else(|| value.get("completion_tokens"))
            .and_then(Value::as_u64)? as usize,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        reasoning_output_tokens: value
            .get("output_tokens_details")
            .or_else(|| value.get("completion_tokens_details"))
            .and_then(|details| details.get("reasoning_tokens"))
            .and_then(Value::as_u64)
            .map(|tokens| tokens as usize),
        total_tokens: value
            .get("total_tokens")
            .and_then(Value::as_u64)
            .map(|tokens| tokens as usize),
    })
}

fn parse_status_reason(value: &str) -> StopReason {
    match value {
        "completed" | "stop" | "end_turn" => StopReason::EndTurn,
        "incomplete" | "max_output_tokens" | "length" => StopReason::MaxTokens,
        "tool_use" | "tool_calls" => StopReason::ToolUse,
        "stop_sequence" | "content_filter" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}

#[async_trait]
impl ModelProviderSDK for OpenAIResponsesProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = build_request(&request, false);
        debug!(
            provider = "openai-responses",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending openai responses completion request"
        );

        let response = self
            .request_builder(&body)
            .send()
            .await
            .context("failed to send openai responses request")?;
        let response = match response.error_for_status_ref() {
            Ok(_) => response,
            Err(_) => {
                let status = response.status();
                return Err(invalid_status_error(
                    "openai-responses",
                    &request.model,
                    "request",
                    status,
                    response,
                    &body,
                )
                .await);
            }
        };

        let value: Value = response
            .json()
            .await
            .context("failed to decode openai responses response")?;
        parse_response(value)
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let body = build_request(&request, true);
        debug!(
            provider = "openai-responses",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending openai responses streaming request"
        );

        let event_source = EventSource::new(self.streaming_request_builder(&body))
            .context("failed to create openai responses event source")?;
        let stream = async_stream::try_stream! {
            let mut text_buf = String::new();
            let mut reasoning_buf = String::new();
            let mut text_parser = TaggedTextParser::default();
            let mut response_id = String::new();
            let mut tool_calls: HashMap<String, ResponsesStreamToolCall> = HashMap::new();
            let mut tool_call_keys_by_item_id: HashMap<String, String> = HashMap::new();
            let mut hosted_tool_calls: HashMap<String, ResponsesStreamHostedToolCall> = HashMap::new();
            let mut usage: Option<Usage> = None;
            let mut reasoning_started = false;
            let mut text_started = false;

            futures::pin_mut!(event_source);
            loop {
                let event = match event_source.next().await {
                    Some(event) => event,
                    None => break,
                };
                let event = match event {
                    Ok(event) => event,
                    Err(reqwest_eventsource::Error::InvalidStatusCode(status, response)) => {
                        Err(invalid_status_error(
                            "openai-responses",
                            &request.model,
                            "stream",
                            status,
                            response,
                            &body,
                        )
                        .await)?
                    }
                    Err(error) => Err(stream_error(format!(
                        "openai responses stream error for model {}: {error}",
                        request.model
                    )))?,
                };

                match event {
                    Event::Open => {}
                    Event::Message(message) => {
                        if message.data == "[DONE]" {
                            break;
                        }

                        let chunk: Value = serde_json::from_str(&message.data)
                            .map_err(|error| anyhow::anyhow!("failed to parse openai responses stream chunk: {error}"))?;

                        if response_id.is_empty() {
                            response_id = chunk
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                        }

                        if let Some(parsed_usage) = chunk.get("usage").and_then(parse_usage) {
                            usage = Some(parsed_usage.clone());
                            yield StreamEvent::UsageDelta(parsed_usage);
                        }

                        match message.event.as_str() {
                            "response.output_text.delta" => {
                                let delta = chunk
                                    .get("delta")
                                    .and_then(Value::as_str)
                                    .or_else(|| chunk.get("text").and_then(Value::as_str))
                                    .unwrap_or_default();
                                if !delta.is_empty() {
                                    for fragment in text_parser.consume(delta) {
                                        match fragment {
                                            TaggedTextFragment::Text(text) => {
                                                if text.is_empty() {
                                                    continue;
                                                }
                                                if !text_started {
                                                    text_started = true;
                                                    yield StreamEvent::TextStart { index: 0 };
                                                }
                                                text_buf.push_str(&text);
                                                yield StreamEvent::TextDelta { index: 0, text };
                                            }
                                            TaggedTextFragment::Reasoning(text) => {
                                                if text.is_empty() {
                                                    continue;
                                                }
                                                if !reasoning_started {
                                                    reasoning_started = true;
                                                    yield StreamEvent::ReasoningStart { index: 1 };
                                                }
                                                reasoning_buf.push_str(&text);
                                                yield StreamEvent::ReasoningDelta { index: 1, text };
                                            }
                                        }
                                    }
                                }
                            }
                            "response.output_item.added" => {
                                if let Some(item) = chunk.get("item") {
                                    if let Some(reasoning_content) =
                                        item.get("reasoning_content").and_then(Value::as_str)
                                        && !reasoning_content.is_empty() {
                                            if !reasoning_started {
                                                reasoning_started = true;
                                                yield StreamEvent::ReasoningStart { index: 1 };
                                            }
                                            reasoning_buf.push_str(reasoning_content);
                                            yield StreamEvent::ReasoningDelta {
                                                index: 1,
                                                text: reasoning_content.to_string(),
                                            };
                                        }
                                    if let Some(content) = parse_output_item(item).into_iter().next() {
                                        match content {
                                            ResponseContent::ToolUse { id, name, input } => {
                                                let key = id.clone();
                                                let item_id = item
                                                    .get("id")
                                                    .and_then(Value::as_str)
                                                    .unwrap_or_default();
                                                let index = tool_calls.len() + hosted_tool_calls.len() + 1;
                                                let arguments_json = function_call_arguments_json(item);
                                                tool_calls.insert(
                                                    key.clone(),
                                                    ResponsesStreamToolCall {
                                                        index,
                                                        id: id.clone(),
                                                        name: name.clone(),
                                                        arguments_json,
                                                    },
                                                );
                                                if !item_id.is_empty() {
                                                    tool_call_keys_by_item_id
                                                        .insert(item_id.to_string(), key);
                                                }
                                                yield StreamEvent::ToolCallStart {
                                                    index,
                                                    id,
                                                    name,
                                                    input,
                                                };
                                            }
                                            ResponseContent::HostedToolUse { id, name, input, output, status } => {
                                                let index = tool_calls.len() + hosted_tool_calls.len() + 1;
                                                let key = id.clone();
                                                hosted_tool_calls.insert(
                                                    key,
                                                    (index, id.clone(), name.clone(), input.clone(), output.clone(), status.clone()),
                                                );
                                                yield StreamEvent::HostedToolCallStart {
                                                    index,
                                                    id: id.clone(),
                                                    name: name.clone(),
                                                    input: input.clone(),
                                                };
                                                if output.is_some() {
                                                    yield StreamEvent::HostedToolCallDone {
                                                        index,
                                                        id,
                                                        name,
                                                        input,
                                                        output,
                                                        status,
                                                    };
                                                }
                                            }
                                            ResponseContent::Text(_)
                                            | ResponseContent::ProviderReasoning { .. } => {}
                                        }
                                    }
                                }
                            }
                            "response.output_item.done" => {
                                if let Some(item) = chunk.get("item")
                                    && let Some(ResponseContent::HostedToolUse { id, name, input, output, status }) =
                                        parse_output_item(item).into_iter().next()
                                {
                                    let key = id.clone();
                                    let index = if let Some((
                                        index,
                                        stored_id,
                                        stored_name,
                                        stored_input,
                                        stored_output,
                                        stored_status,
                                    )) = hosted_tool_calls.get_mut(&key) {
                                        *stored_id = id.clone();
                                        *stored_name = name.clone();
                                        *stored_input = input.clone();
                                        if output.is_some() {
                                            *stored_output = output.clone();
                                        }
                                        if status.is_some() {
                                            *stored_status = status.clone();
                                        }
                                        *index
                                    } else {
                                        let index = tool_calls.len() + hosted_tool_calls.len() + 1;
                                        hosted_tool_calls.insert(
                                            key,
                                            (index, id.clone(), name.clone(), input.clone(), output.clone(), status.clone()),
                                        );
                                        index
                                    };
                                    yield StreamEvent::HostedToolCallDone {
                                        index,
                                        id,
                                        name,
                                        input,
                                        output,
                                        status,
                                    };
                                }
                            }
                            "response.function_call_arguments.delta" | "response.output_item.delta" => {
                                let partial_json = chunk
                                    .get("delta")
                                    .or_else(|| chunk.get("arguments_delta"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default();
                                let call_id = chunk
                                    .get("call_id")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned)
                                    .or_else(|| {
                                        chunk
                                            .get("item_id")
                                            .and_then(Value::as_str)
                                            .and_then(|item_id| {
                                                tool_call_keys_by_item_id.get(item_id).cloned()
                                            })
                                    });
                                if !partial_json.is_empty()
                                    && let Some(call_id) = call_id
                                    && let Some(entry) = tool_calls.get_mut(&call_id)
                                {
                                    entry.arguments_json.push_str(partial_json);
                                    yield StreamEvent::ToolCallInputDelta {
                                        index: entry.index,
                                        partial_json: partial_json.to_string(),
                                    };
                                }
                            }
                            "response.completed" | "response.done" => {
                                for fragment in text_parser.finish() {
                                    match fragment {
                                        TaggedTextFragment::Text(text) => {
                                            if text.is_empty() {
                                                continue;
                                            }
                                            if !text_started {
                                                text_started = true;
                                                yield StreamEvent::TextStart { index: 0 };
                                            }
                                            text_buf.push_str(&text);
                                            yield StreamEvent::TextDelta { index: 0, text };
                                        }
                                        TaggedTextFragment::Reasoning(text) => {
                                            if text.is_empty() {
                                                continue;
                                            }
                                            if !reasoning_started {
                                                reasoning_started = true;
                                                yield StreamEvent::ReasoningStart { index: 1 };
                                            }
                                            reasoning_buf.push_str(&text);
                                            yield StreamEvent::ReasoningDelta { index: 1, text };
                                        }
                                    }
                                }
                                if reasoning_started {
                                    yield StreamEvent::ReasoningDone { index: 1 };
                                }
                                let response = if let Some(parsed) = chunk.get("response") {
                                    let mut response = parse_response(parsed.clone())?;
                                    if !reasoning_buf.is_empty()
                                        && !response.metadata.extras.iter().any(|extra| {
                                            matches!(
                                                extra,
                                                ResponseExtra::ReasoningText { text }
                                                    if text == &reasoning_buf
                                            )
                                        })
                                    {
                                        response.metadata.extras.push(ResponseExtra::ReasoningText {
                                            text: reasoning_buf.clone(),
                                        });
                                    }
                                    response
                                } else {
                                    ModelResponse {
                                        id: response_id.clone(),
                                        content: {
                                            let mut content = Vec::new();
                                            if !text_buf.is_empty() {
                                                content.push(ResponseContent::Text(text_buf.clone()));
                                            }
                                            content.extend(responses_stream_tool_content(
                                                &tool_calls,
                                                &hosted_tool_calls,
                                            ));
                                            content
                                        },
                                        stop_reason: Some(StopReason::EndTurn),
                                        usage: usage.unwrap_or_default(),
                                        metadata: if reasoning_buf.is_empty() {
                                            ResponseMetadata::default()
                                        } else {
                                            ResponseMetadata {
                                                extras: vec![ResponseExtra::ReasoningText {
                                                    text: reasoning_buf.clone(),
                                                }],
                                            }
                                        },
                                    }
                                };
                                yield StreamEvent::MessageDone { response };
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            for fragment in text_parser.finish() {
                match fragment {
                    TaggedTextFragment::Text(text) => {
                        if text.is_empty() {
                            continue;
                        }
                        if !text_started {
                            text_started = true;
                            yield StreamEvent::TextStart { index: 0 };
                        }
                        text_buf.push_str(&text);
                        yield StreamEvent::TextDelta { index: 0, text };
                    }
                    TaggedTextFragment::Reasoning(text) => {
                        if text.is_empty() {
                            continue;
                        }
                        if !reasoning_started {
                            reasoning_started = true;
                            yield StreamEvent::ReasoningStart { index: 1 };
                        }
                        reasoning_buf.push_str(&text);
                        yield StreamEvent::ReasoningDelta { index: 1, text };
                    }
                }
            }

            if reasoning_started {
                yield StreamEvent::ReasoningDone { index: 1 };
            }

            let response = ModelResponse {
                id: response_id,
                content: {
                    let mut content = Vec::new();
                    if !text_buf.is_empty() {
                        content.push(ResponseContent::Text(text_buf));
                    }
                    content.extend(responses_stream_tool_content(
                        &tool_calls,
                        &hosted_tool_calls,
                    ));
                    content
                },
                stop_reason: Some(StopReason::EndTurn),
                usage: usage.unwrap_or_default(),
                metadata: if reasoning_buf.is_empty() {
                    ResponseMetadata::default()
                } else {
                    ResponseMetadata {
                        extras: vec![ResponseExtra::ReasoningText {
                            text: reasoning_buf,
                        }],
                    }
                },
            };
            yield StreamEvent::MessageDone { response };
        };

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "openai-responses"
    }
}

#[derive(Debug)]
struct ResponsesStreamToolCall {
    index: usize,
    id: String,
    name: String,
    arguments_json: String,
}

type ResponsesStreamHostedToolCall = (usize, String, String, Value, Option<Value>, Option<String>);

fn responses_stream_tool_content(
    tool_calls: &HashMap<String, ResponsesStreamToolCall>,
    hosted_tool_calls: &HashMap<String, ResponsesStreamHostedToolCall>,
) -> Vec<ResponseContent> {
    let mut indexed_content = Vec::new();
    for call in tool_calls.values() {
        indexed_content.push((
            call.index,
            ResponseContent::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: parse_function_call_arguments_json(&call.arguments_json),
            },
        ));
    }
    for (index, id, name, input, output, status) in hosted_tool_calls.values() {
        indexed_content.push((
            *index,
            ResponseContent::HostedToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                output: output.clone(),
                status: status.clone(),
            },
        ));
    }
    indexed_content.sort_by_key(|(index, _)| *index);
    indexed_content
        .into_iter()
        .map(|(_, content)| content)
        .collect()
}

#[cfg(test)]
mod tests {
    use devo_protocol::{
        ModelRequest, RequestContent, RequestMessage, SamplingControls, ToolDefinition,
    };
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::parse_response;
    use devo_protocol::{ResponseContent, ResponseExtra};

    use crate::openai::responses::build_request;

    #[test]
    fn debug_request_body_includes_reasoning_and_tools() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::CatalogSlug("gpt-5.4".to_string()),
            model: "gpt-5.4".to_string(),
            system: Some("You are helpful.".to_string()),
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "hi".to_string(),
                }],
            }],
            max_tokens: 256,
            tools: Some(vec![ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather by city".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
            }]),
            hosted_tools: Vec::new(),
            sampling: SamplingControls {
                temperature: Some(0.4),
                top_p: Some(0.7),
                top_k: Some(12),
            },
            request_thinking: Some("medium".to_string()),
            reasoning_effort: Some(devo_protocol::ReasoningEffort::Medium),
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["model"], json!("gpt-5.4"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_output_tokens"], json!(256));
        assert_eq!(body["temperature"], json!(0.4));
        assert_eq!(body["top_p"], json!(0.7));
        assert!(body.get("top_k").is_none());
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["input"][0]["role"], json!("system"));
    }

    #[test]
    fn build_request_omits_unsupported_hosted_tool_history() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::CatalogSlug("gpt-5.4".to_string()),
            model: "gpt-5.4".to_string(),
            system: None,
            messages: vec![
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "before".to_string(),
                    }],
                },
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::HostedToolUse {
                            id: "hosted_ws_1".to_string(),
                            name: "web_search".to_string(),
                            input: json!({"query": "Rust docs"}),
                            output: None,
                            status: None,
                        },
                        RequestContent::HostedToolUse {
                            id: "hosted_ws_1".to_string(),
                            name: "web_search".to_string(),
                            input: json!({"query": "Rust docs"}),
                            output: Some(json!([{
                                "title": "Rust documentation",
                                "url": "https://example.test/rust"
                            }])),
                            status: Some("completed".to_string()),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "after".to_string(),
                    }],
                },
            ],
            max_tokens: 256,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["input"].as_array().map(Vec::len), Some(2));
        assert_eq!(body["input"][0]["role"], json!("user"));
        assert_eq!(body["input"][1]["role"], json!("user"));
        let serialized = serde_json::to_string(&body).expect("serialize request body");
        assert!(!serialized.contains("hosted_tool_use"));
        assert!(!serialized.contains("web_search_tool_result"));
    }

    #[test]
    fn parse_response_extracts_text_and_tool_use() {
        let response = parse_response(json!({
            "id": "resp_123",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "content": [
                        {"type": "output_text", "text": "Hello"},
                        {
                            "type": "function_call",
                            "id": "call_1",
                            "name": "get_weather",
                            "arguments": {"city": "Boston"}
                        }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 21,
                "output_tokens_details": {
                    "reasoning_tokens": 6
                }
            }
        }))
        .expect("parse response");

        assert_eq!(response.id, "resp_123");
        assert_eq!(response.content.len(), 2);
        assert_eq!(response.usage.reasoning_output_tokens, Some(6));
        assert_eq!(response.usage.total_tokens, Some(21));
        assert!(matches!(response.content[0], ResponseContent::Text(_)));
        assert!(matches!(
            response.content[1],
            ResponseContent::ToolUse { .. }
        ));
    }

    #[test]
    fn parse_response_parses_string_tool_arguments_as_json() {
        let response = parse_response(json!({
            "id": "resp_args",
            "status": "completed",
            "output": [
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "weather",
                    "arguments": "{\"city\":\"Boston\"}"
                }
            ]
        }))
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![ResponseContent::ToolUse {
                id: "call_1".to_string(),
                name: "weather".to_string(),
                input: json!({"city": "Boston"}),
            }]
        );
    }

    #[test]
    fn parse_response_extracts_web_search_call_as_hosted_tool_use() {
        let response = parse_response(json!({
            "id": "resp_789",
            "status": "completed",
            "output": [
                {
                    "type": "web_search_call",
                    "id": "ws_1",
                    "status": "completed",
                    "action": {
                        "type": "search",
                        "query": "Rust async docs"
                    },
                    "results": [
                        {
                            "title": "Async Programming in Rust",
                            "url": "https://example.test/rust-async"
                        }
                    ]
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        }))
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![ResponseContent::HostedToolUse {
                id: "ws_1".to_string(),
                name: "web_search".to_string(),
                input: json!({"query": "Rust async docs"}),
                output: Some(json!([
                    {
                        "title": "Async Programming in Rust",
                        "url": "https://example.test/rust-async"
                    }
                ])),
                status: Some("completed".to_string()),
            }]
        );
    }

    #[test]
    fn parse_response_extracts_web_fetch_call_as_hosted_tool_use() {
        let response = parse_response(json!({
            "id": "resp_fetch",
            "status": "completed",
            "output": [
                {
                    "type": "web_fetch_call",
                    "id": "wf_1",
                    "status": "completed",
                    "action": {
                        "type": "fetch",
                        "url": "https://example.test/docs"
                    },
                    "output": {
                        "title": "Docs",
                        "url": "https://example.test/docs"
                    }
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        }))
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![ResponseContent::HostedToolUse {
                id: "wf_1".to_string(),
                name: "web_fetch".to_string(),
                input: json!({"url": "https://example.test/docs"}),
                output: Some(json!({
                    "title": "Docs",
                    "url": "https://example.test/docs"
                })),
                status: Some("completed".to_string()),
            }]
        );
    }

    #[test]
    fn parse_response_preserves_reasoning_text_as_metadata() {
        let response = parse_response(json!({
            "id": "resp_456",
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "content": [
                        {"type": "output_text", "text": "final"}
                    ],
                    "reasoning_content": "internal reasoning"
                }
            ],
            "usage": {
                "input_tokens": 3,
                "output_tokens": 1
            }
        }))
        .expect("parse response");

        assert_eq!(response.metadata.extras.len(), 1);
        assert!(matches!(
            response.metadata.extras[0],
            ResponseExtra::ReasoningText { .. }
        ));
    }
}
