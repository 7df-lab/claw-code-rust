use std::{collections::HashMap, pin::Pin};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use reqwest::Client;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use reqwest_eventsource::{Event, EventSource};
use serde_json::{Value, json};
use tracing::debug;

use super::AnthropicAIRole;
use crate::{
    ModelProviderSDK, ModelRequest, ModelResponse, ProviderAdapter, ProviderCapabilities,
    ProviderFamily, RequestContent, ResponseContent, ResponseMetadata, StopReason, StreamEvent,
    Usage,
};

/// <https://platform.claude.com/docs/en/api/messages>
/// Anthropic provider backed by the official HTTP API.
pub struct AnthropicProvider {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

impl AnthropicProvider {
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
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn request_builder(&self, body: &Value) -> reqwest::RequestBuilder {
        let builder = self
            .client
            .post(self.endpoint())
            .header("anthropic-version", "2023-06-01")
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let builder = if let Some(api_key) = &self.api_key {
            builder.header("x-api-key", api_key)
        } else {
            builder
        };
        builder.json(body)
    }
}

#[async_trait]
impl ModelProviderSDK for AnthropicProvider {
    async fn completion(&self, request: ModelRequest) -> Result<ModelResponse> {
        let body = build_request(&request, false);
        debug!(
            provider = "anthropic",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending anthropic completion request"
        );

        let response = self
            .request_builder(&body)
            .send()
            .await
            .context("failed to send anthropic request")?
            .error_for_status()
            .context("anthropic request failed")?;

        let value: Value = response
            .json()
            .await
            .context("failed to decode anthropic response")?;
        parse_response(value)
    }

    async fn completion_stream(
        &self,
        request: ModelRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>> {
        let body = build_request(&request, true);
        debug!(
            provider = "anthropic",
            api_base = %self.base_url,
            model = %request.model,
            messages = request.messages.len(),
            tools = request.tools.as_ref().map_or(0, Vec::len),
            max_tokens = request.max_tokens,
            "sending anthropic streaming request"
        );

        let event_source = EventSource::new(self.request_builder(&body))
            .context("failed to create anthropic event source")?;
        let stream = async_stream::try_stream! {
            let mut message_id = String::new();
            let mut input_tokens = 0usize;
            let mut output_tokens = 0usize;
            let mut stop_reason: Option<StopReason> = None;
            let mut content_blocks: Vec<ResponseContent> = Vec::new();
            let mut tool_json: HashMap<usize, String> = HashMap::new();

            futures::pin_mut!(event_source);
            while let Some(event) = event_source.next().await {
                let event = event.map_err(|error| {
                    anyhow::anyhow!("anthropic stream error for model {}: {error}", request.model)
                })?;

                match event {
                    Event::Open => {}
                    Event::Message(message) => {
                        let data: Value = serde_json::from_str(&message.data)
                            .map_err(|error| anyhow::anyhow!("failed to parse anthropic stream payload: {error}"))?;

                        match message.event.as_str() {
                            "message_start" => {
                                if let Some(id) = data
                                    .get("message")
                                    .and_then(Value::as_object)
                                    .and_then(|message| message.get("id"))
                                    .and_then(Value::as_str)
                                {
                                    message_id = id.to_string();
                                }
                                if let Some(usage) = data.get("usage") {
                                    if let Some(input) =
                                        usage.get("input_tokens").and_then(Value::as_u64)
                                    {
                                        input_tokens = input as usize;
                                    }
                                }
                            }
                            "content_block_start" => {
                                let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                    continue;
                                };
                                let Some(content_block) = data.get("content_block") else {
                                    continue;
                                };
                                let Some(parsed) = parse_content_block(content_block) else {
                                    continue;
                                };
                                while content_blocks.len() <= index as usize {
                                    content_blocks.push(ResponseContent::Text(String::new()));
                                }
                                content_blocks[index as usize] = parsed.clone();
                                if matches!(parsed, ResponseContent::ToolUse { .. }) {
                                    tool_json.insert(index as usize, String::new());
                                }
                                yield StreamEvent::ContentBlockStart {
                                    index: index as usize,
                                    content: parsed,
                                };
                            }
                            "content_block_delta" => {
                                let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                    continue;
                                };
                                let Some(delta) = data.get("delta").and_then(Value::as_object)
                                else {
                                    continue;
                                };
                                match delta.get("type").and_then(Value::as_str) {
                                    Some("text_delta") => {
                                        let text = delta
                                            .get("text")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if let Some(ResponseContent::Text(value)) =
                                            content_blocks.get_mut(index as usize)
                                        {
                                            value.push_str(text);
                                        }
                                        yield StreamEvent::TextDelta {
                                            index: index as usize,
                                            text: text.to_string(),
                                        };
                                    }
                                    Some("input_json_delta") => {
                                        let partial_json = delta
                                            .get("partial_json")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if let Some(acc) = tool_json.get_mut(&(index as usize)) {
                                            acc.push_str(partial_json);
                                        }
                                        yield StreamEvent::InputJsonDelta {
                                            index: index as usize,
                                            partial_json: partial_json.to_string(),
                                        };
                                    }
                                    _ => {}
                                }
                            }
                            "content_block_stop" => {
                                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                                if let Some(json_str) = tool_json.remove(&index) {
                                    if let Ok(parsed) = serde_json::from_str(&json_str) {
                                        if let Some(ResponseContent::ToolUse { input, .. }) =
                                            content_blocks.get_mut(index)
                                        {
                                            *input = parsed;
                                        }
                                    }
                                }
                            }
                            "message_delta" => {
                                if let Some(delta) = data.get("delta").and_then(Value::as_object) {
                                    if let Some(reason) =
                                        delta.get("stop_reason").and_then(Value::as_str)
                                    {
                                        stop_reason = Some(parse_stop_reason(reason));
                                    }
                                }
                                if let Some(usage) = data.get("usage") {
                                    if let Some(output) = usage.get("output_tokens").and_then(Value::as_u64)
                                    {
                                        output_tokens = output as usize;
                                    }
                                    yield StreamEvent::UsageDelta(Usage {
                                        input_tokens,
                                        output_tokens,
                                        cache_creation_input_tokens: None,
                                        cache_read_input_tokens: None,
                                    });
                                }
                            }
                            "message_stop" => {
                                let response = ModelResponse {
                                    id: message_id.clone(),
                                    content: content_blocks.clone(),
                                    stop_reason: stop_reason.clone(),
                                    usage: Usage {
                                        input_tokens,
                                        output_tokens,
                                        cache_creation_input_tokens: None,
                                        cache_read_input_tokens: None,
                                    },
                                    metadata: ResponseMetadata::default(),
                                };
                                yield StreamEvent::MessageDone { response };
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            let response = ModelResponse {
                id: message_id,
                content: content_blocks,
                stop_reason,
                usage: Usage {
                    input_tokens,
                    output_tokens,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                },
                metadata: ResponseMetadata::default(),
            };
            yield StreamEvent::MessageDone { response };
        };

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

#[async_trait]
impl ProviderAdapter for AnthropicProvider {
    fn family(&self) -> ProviderFamily {
        ProviderFamily::Anthropic
    }

    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities::anthropic()
    }
}

fn build_request(request: &ModelRequest, stream: bool) -> Value {
    let mut messages = Vec::new();

    for message in &request.messages {
        let role = message
            .role
            .parse::<AnthropicAIRole>()
            .unwrap_or(AnthropicAIRole::User);
        let mut content = Vec::new();
        for block in &message.content {
            match block {
                RequestContent::Text { text } => {
                    content.push(json!({"type":"text","text":text}));
                }
                RequestContent::ToolUse { id, name, input } => {
                    content.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }));
                }
                RequestContent::ToolResult {
                    tool_use_id,
                    content: result_content,
                    is_error,
                } => {
                    let mut item = json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": result_content,
                    });
                    if let Some(is_error) = is_error {
                        item["is_error"] = json!(is_error);
                    }
                    content.push(item);
                }
            }
        }
        messages.push(json!({
            "role": role,
            "content": content,
        }));
    }

    let mut root = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": stream,
        "messages": messages,
    });

    if let Some(system) = &request.system {
        root["system"] = json!(system);
    }

    if let Some(tools) = &request.tools {
        root["tools"] = Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    })
                })
                .collect(),
        );
    }

    if let Some(thinking) = request.thinking.as_deref().and_then(build_thinking) {
        root["thinking"] = thinking;
    }

    root
}

fn parse_response(value: Value) -> Result<ModelResponse> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let content = value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_content_block_value)
        .collect();
    let stop_reason = value
        .get("stop_reason")
        .and_then(Value::as_str)
        .map(parse_stop_reason);
    let usage = value.get("usage").and_then(parse_usage).unwrap_or_default();

    Ok(ModelResponse {
        id,
        content,
        stop_reason,
        usage,
        metadata: ResponseMetadata::default(),
    })
}

fn parse_content_block_value(value: &Value) -> Option<ResponseContent> {
    let kind = value.get("type")?.as_str()?;
    match kind {
        "text" => Some(ResponseContent::Text(
            value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )),
        "tool_use" => Some(ResponseContent::ToolUse {
            id: value.get("id")?.as_str()?.to_string(),
            name: value.get("name")?.as_str()?.to_string(),
            input: value
                .get("input")
                .cloned()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
        }),
        _ => None,
    }
}

fn parse_content_block(value: &Value) -> Option<ResponseContent> {
    match value.get("type")?.as_str()? {
        "text" => Some(ResponseContent::Text(String::new())),
        "tool_use" => {
            let id = value.get("id")?.as_str()?.to_string();
            let name = value.get("name")?.as_str()?.to_string();
            let input = value
                .get("input")
                .cloned()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
            Some(ResponseContent::ToolUse { id, name, input })
        }
        _ => None,
    }
}

fn parse_usage(value: &Value) -> Option<Usage> {
    Some(Usage {
        input_tokens: value.get("input_tokens")?.as_u64()? as usize,
        output_tokens: value.get("output_tokens")?.as_u64()? as usize,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    })
}

fn parse_stop_reason(value: &str) -> StopReason {
    match value {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}

fn build_thinking(level: &str) -> Option<Value> {
    let budget_tokens = match level.trim().to_ascii_lowercase().as_str() {
        "disabled" => return None,
        "enabled" | "medium" => 4_096,
        "low" => 1_024,
        "high" => 8_192,
        "xhigh" => 16_384,
        _ => 4_096,
    };

    Some(json!({
        "type": "enabled",
        "budget_tokens": budget_tokens,
    }))
}
