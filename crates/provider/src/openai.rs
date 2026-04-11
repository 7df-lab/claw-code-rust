use std::{collections::HashMap, pin::Pin};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest_eventsource::{Event, EventSource};
use serde_json::{Value, json};
use tracing::debug;

use crate::{
    ModelProviderSDK, ModelRequest, ModelResponse, RequestContent, ResponseContent, StopReason,
    StreamEvent, Usage,
};

/// OpenAI-compatible provider backed by the official HTTP API.
///
/// Works with OpenAI and OpenAI-compatible servers by changing the base URL.
pub struct OpenAIProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl OpenAIProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            api_key: None,
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn request_builder(&self, body: &Value) -> reqwest::RequestBuilder {
        let builder = self
            .client
            .post(self.endpoint())
            .header(CONTENT_TYPE, "application/json");
        let builder = if let Some(api_key) = &self.api_key {
            builder.header(AUTHORIZATION, format!("Bearer {api_key}"))
        } else {
            builder
        };
        builder.json(body)
    }
}

/// Builds the exact OpenAI-compatible request body used by this provider.
///
/// This is intended for diagnostics and standalone probes so callers can inspect
/// the serialized request payload when debugging compatibility issues.
pub fn debug_request_body(request: &ModelRequest, stream: bool) -> Value {
    build_request(request, stream)
}

#[async_trait]
impl ModelProviderSDK for OpenAIProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = build_request(&request, false);
        debug!(
            provider = "openai",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending openai completion request"
        );

        let response = self
            .request_builder(&body)
            .send()
            .await
            .context("failed to send openai request")?
            .error_for_status()
            .context("openai request failed")?;

        let value: Value = response
            .json()
            .await
            .context("failed to decode openai response")?;
        parse_response(value)
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let body = build_request(&request, true);
        debug!(
            provider = "openai",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending openai streaming request"
        );

        let event_source = EventSource::new(self.request_builder(&body))
            .context("failed to create openai event source")?;
        let stream = async_stream::try_stream! {
            let mut response_id = String::new();
            let mut text_buf = String::new();
            let mut tool_calls: HashMap<u32, (String, String, String)> = HashMap::new();
            let mut tool_blocks_started: std::collections::HashSet<u32> =
                std::collections::HashSet::new();
            let mut text_block_started = false;
            let mut finish_reason: Option<StopReason> = None;
            let mut stream_usage: Option<Usage> = None;

            futures::pin_mut!(event_source);
            while let Some(event) = event_source.next().await {
                let event = event.map_err(|error| {
                    anyhow::anyhow!("openai stream error for model {}: {error}", request.model)
                })?;

                match event {
                    Event::Open => {}
                    Event::Message(message) => {
                        if message.data == "[DONE]" {
                            break;
                        }

                        let chunk: Value = serde_json::from_str(&message.data)
                            .map_err(|error| anyhow::anyhow!("failed to parse openai stream chunk: {error}"))?;

                        if response_id.is_empty() {
                            response_id = chunk
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                        }

                        if let Some(usage) = chunk.get("usage") {
                            if let Some(parsed) = parse_usage(usage) {
                                stream_usage = Some(parsed.clone());
                                yield StreamEvent::UsageDelta(parsed);
                            }
                        }

                        let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
                            continue;
                        };
                        for choice in choices {
                            let delta = choice.get("delta").unwrap_or(&Value::Null);

                            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                                if !content.is_empty() {
                                    if !text_block_started {
                                        text_block_started = true;
                                        yield StreamEvent::ContentBlockStart {
                                            index: 0,
                                            content: ResponseContent::Text(String::new()),
                                        };
                                    }
                                    text_buf.push_str(content);
                                    yield StreamEvent::TextDelta {
                                        index: 0,
                                        text: content.to_string(),
                                    };
                                }
                            }

                            if let Some(tool_call_deltas) =
                                delta.get("tool_calls").and_then(Value::as_array)
                            {
                                for tool_call_delta in tool_call_deltas {
                                    let index = tool_call_delta
                                        .get("index")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(0) as u32;
                                    let content_idx = (index + 1) as usize;
                                    let entry = tool_calls
                                        .entry(index)
                                        .or_insert_with(|| (String::new(), String::new(), String::new()));

                                    if let Some(id) = tool_call_delta.get("id").and_then(Value::as_str) {
                                        entry.0 = id.to_string();
                                    }
                                    if let Some(function) =
                                        tool_call_delta.get("function").and_then(Value::as_object)
                                    {
                                        if let Some(name) = function.get("name").and_then(Value::as_str) {
                                            entry.1 = name.to_string();
                                        }
                                        if let Some(args) = function.get("arguments").and_then(Value::as_str) {
                                            if !args.is_empty() {
                                                entry.2.push_str(args);
                                                if tool_blocks_started.insert(index) {
                                                    yield StreamEvent::ContentBlockStart {
                                                        index: content_idx,
                                                        content: ResponseContent::ToolUse {
                                                            id: entry.0.clone(),
                                                            name: entry.1.clone(),
                                                            input: Value::Object(serde_json::Map::new()),
                                                        },
                                                    };
                                                }
                                                yield StreamEvent::InputJsonDelta {
                                                    index: content_idx,
                                                    partial_json: args.to_string(),
                                                };
                                            }
                                        }
                                    }
                                }
                            }

                            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                                finish_reason = Some(parse_finish_reason(reason));
                            }
                        }
                    }
                }
            }

            let mut content = Vec::new();
            if !text_buf.is_empty() {
                content.push(ResponseContent::Text(text_buf));
            }
            let mut sorted: Vec<_> = tool_calls.iter().collect();
            sorted.sort_by_key(|(index, _)| *index);
            for (_, (id, name, args)) in sorted {
                let input = serde_json::from_str(args)
                    .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
                content.push(ResponseContent::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input,
                });
            }

            let response = ModelResponse {
                id: response_id,
                content,
                stop_reason: finish_reason,
                usage: stream_usage.unwrap_or_default(),
            };
            yield StreamEvent::MessageDone { response };
        };

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "openai"
    }
}

fn build_request(request: &ModelRequest, stream: bool) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }

    for message in &request.messages {
        match message.role.as_str() {
            "assistant" => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &message.content {
                    match block {
                        RequestContent::Text { text } => text_parts.push(text.clone()),
                        RequestContent::ToolUse { id, name, input } => tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": input.to_string(),
                            }
                        })),
                        RequestContent::ToolResult { .. } => {}
                    }
                }
                let mut entry = json!({ "role": "assistant" });
                entry["content"] = if text_parts.is_empty() {
                    Value::Null
                } else {
                    Value::String(text_parts.join(""))
                };
                if !tool_calls.is_empty() {
                    entry["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(entry);
            }
            _ => {
                for block in &message.content {
                    match block {
                        RequestContent::Text { text } => {
                            messages.push(json!({ "role": "user", "content": text }));
                        }
                        RequestContent::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content,
                            }));
                        }
                        RequestContent::ToolUse { .. } => {}
                    }
                }
            }
        }
    }

    let mut root = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_tokens,
        "stream": stream,
    });

    if let Some(tools) = &request.tools {
        root["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                        }
                    })
                })
                .collect(),
        );
    }

    if let Some(temperature) = request.temperature {
        root["temperature"] = json!(temperature);
    }

    if let Some(reasoning_effort) = map_reasoning_effort(request.thinking.as_deref()) {
        root["reasoning_effort"] = json!(reasoning_effort);
    }

    if stream {
        root["stream_options"] = json!({ "include_usage": true });
    }

    root
}

fn parse_response(value: Value) -> Result<ModelResponse> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut content = Vec::new();
    let mut stop_reason = None;

    if let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    {
        if let Some(message) = choice.get("message") {
            if let Some(text) = message.get("content").and_then(Value::as_str) {
                if !text.is_empty() {
                    content.push(ResponseContent::Text(text.to_string()));
                }
            }
            if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
                for tool_call in tool_calls {
                    if let Some(parsed) = parse_tool_use(tool_call) {
                        content.push(parsed);
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            stop_reason = Some(parse_finish_reason(reason));
        }
    }

    let usage = value.get("usage").and_then(parse_usage).unwrap_or_default();

    Ok(ModelResponse {
        id,
        content,
        stop_reason,
        usage,
    })
}

fn parse_tool_use(value: &Value) -> Option<ResponseContent> {
    let function = value.get("function")?.as_object()?;
    let id = value.get("id")?.as_str()?.to_string();
    let name = function.get("name")?.as_str()?.to_string();
    let args = function
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let input =
        serde_json::from_str(args).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    Some(ResponseContent::ToolUse { id, name, input })
}

fn parse_usage(value: &Value) -> Option<Usage> {
    Some(Usage {
        input_tokens: value.get("prompt_tokens")?.as_u64()? as usize,
        output_tokens: value.get("completion_tokens")?.as_u64()? as usize,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    })
}

fn parse_finish_reason(value: &str) -> StopReason {
    match value {
        "tool_calls" => StopReason::ToolUse,
        "length" => StopReason::MaxTokens,
        "stop" => StopReason::EndTurn,
        "content_filter" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}

fn map_reasoning_effort(thinking: Option<&str>) -> Option<&'static str> {
    let thinking = thinking?.trim();
    match thinking {
        "disabled" => Some("none"),
        "enabled" | "" => Some("medium"),
        "low" | "Low" => Some("low"),
        "medium" | "Medium" => Some("medium"),
        "high" | "High" => Some("high"),
        "xhigh" | "Xhigh" | "XHigh" => Some("xhigh"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::{
        debug_request_body, map_reasoning_effort, parse_finish_reason, parse_response, parse_usage,
    };
    use crate::{
        ModelRequest, RequestContent, RequestMessage, ResponseContent, StopReason, ToolDefinition,
    };

    #[test]
    fn debug_request_body_includes_tools_and_reasoning_effort() {
        let request = ModelRequest {
            model: "gpt-4o-mini".to_string(),
            system: Some("You are helpful.".to_string()),
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::Text {
                            text: "Calling tool".to_string(),
                        },
                        RequestContent::ToolUse {
                            id: "call_123".to_string(),
                            name: "get_weather".to_string(),
                            input: json!({"city": "Boston"}),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::ToolResult {
                        tool_use_id: "call_123".to_string(),
                        content: "{\"temp\":72}".to_string(),
                        is_error: Some(false),
                    }],
                },
            ],
            max_tokens: 256,
            tools: Some(vec![ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather by city".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"]
                }),
            }]),
            temperature: Some(0.2),
            thinking: Some("medium".to_string()),
        };

        let body = debug_request_body(&request, true);

        assert_eq!(body["model"], json!("gpt-4o-mini"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_tokens"], json!(256));
        assert_eq!(body["reasoning_effort"], json!("medium"));
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["messages"][1]["role"], json!("assistant"));
        assert_eq!(
            body["messages"][1]["tool_calls"][0]["function"]["arguments"],
            json!("{\"city\":\"Boston\"}")
        );
        assert_eq!(body["messages"][1]["content"], json!("Calling tool"));
        assert_eq!(body["messages"][2]["role"], json!("tool"));
        assert_eq!(body["messages"][2]["tool_call_id"], json!("call_123"));
    }

    #[test]
    fn parse_response_extracts_text_tool_calls_and_usage() {
        let response = parse_response(json!({
            "id": "chatcmpl-123",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_abc123",
                                "type": "function",
                                "function": {
                                    "name": "get_weather",
                                    "arguments": "{\"location\":\"Boston, MA\"}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ],
            "usage": {
                "prompt_tokens": 82,
                "completion_tokens": 17,
                "total_tokens": 99
            }
        }))
        .expect("parse response");

        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.usage.input_tokens, 82);
        assert_eq!(response.usage.output_tokens, 17);
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::ToolUse { id, name, input } => {
                assert_eq!(id, "call_abc123");
                assert_eq!(name, "get_weather");
                assert_eq!(input, &json!({"location": "Boston, MA"}));
            }
            other => panic!("expected tool use, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_preserves_text_content() {
        let response = parse_response(json!({
            "id": "chatcmpl-456",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello! How can I assist you today?"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 8
            }
        }))
        .expect("parse response");

        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::Text(text) => {
                assert_eq!(text, "Hello! How can I assist you today?");
            }
            other => panic!("expected text response, got {other:?}"),
        }
    }

    #[test]
    fn parse_usage_reads_chat_completion_usage_shape() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 11,
            "completion_tokens": 7,
            "total_tokens": 18
        }))
        .expect("parse usage");

        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, None);
    }

    #[test]
    fn parse_finish_reason_matches_chat_completion_contract() {
        assert_eq!(parse_finish_reason("tool_calls"), StopReason::ToolUse);
        assert_eq!(parse_finish_reason("length"), StopReason::MaxTokens);
        assert_eq!(parse_finish_reason("stop"), StopReason::EndTurn);
        assert_eq!(
            parse_finish_reason("content_filter"),
            StopReason::StopSequence
        );
        assert_eq!(parse_finish_reason("function_call"), StopReason::EndTurn);
    }

    #[test]
    fn map_reasoning_effort_maps_supported_values() {
        assert_eq!(map_reasoning_effort(Some("disabled")), Some("none"));
        assert_eq!(map_reasoning_effort(Some("enabled")), Some("medium"));
        assert_eq!(map_reasoning_effort(Some("low")), Some("low"));
        assert_eq!(map_reasoning_effort(Some("medium")), Some("medium"));
        assert_eq!(map_reasoning_effort(Some("high")), Some("high"));
        assert_eq!(map_reasoning_effort(Some("xhigh")), Some("xhigh"));
        assert_eq!(map_reasoning_effort(Some("unknown")), None);
    }
}
