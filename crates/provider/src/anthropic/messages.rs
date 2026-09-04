use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error as StdError;
use std::pin::Pin;

use anyhow::Context;
use anyhow::Result;
use async_trait::async_trait;
use devo_protocol::ModelRequest;
use devo_protocol::ModelResponse;
use devo_protocol::ProviderWireApi;
use devo_protocol::ReasoningEffort;
use devo_protocol::RequestContent;
use devo_protocol::RequestMessage;
use devo_protocol::ResponseContent;
use devo_protocol::ResponseExtra;
use devo_protocol::ResponseMetadata;
use devo_protocol::StopReason;
use devo_protocol::StreamEvent;
use devo_protocol::Usage;
use devo_protocol::normalize_tool_result_messages;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::header::ACCEPT_ENCODING;
use reqwest::header::CACHE_CONTROL;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderValue;
use reqwest_eventsource::Event;
use reqwest_eventsource::EventSource;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use tracing::debug;

use super::AnthropicAIRole;
use super::stream_usage::AnthropicStreamUsage;
use crate::ModelProviderSDK;
use crate::ProviderAdapter;
use crate::ProviderCapabilities;
use crate::ProviderHttpOptions;
use crate::dsml::DsmlToolCallHealer;
use crate::error::ProviderError;
use crate::hosted_tools::append_anthropic_hosted_tools;
use crate::http::invalid_status_error;
use crate::merge_extra_body;

/// <https://platform.claude.com/docs/en/api/messages>
/// Anthropic provider backed by the official HTTP API.
pub struct AnthropicProvider {
    client: Client,
    streaming_client: Client,
    base_url: String,
    api_key: Option<String>,
    http_options: ProviderHttpOptions,
}

impl AnthropicProvider {
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
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn post_builder(
        &self,
        client: &Client,
        body: &Value,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        let builder = client
            .post(self.endpoint())
            .header("anthropic-version", "2023-06-01")
            .header(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let builder = self
            .http_options
            .apply_request_headers(self.http_options.apply_custom_headers(builder), headers);

        let builder = if let Some(api_key) = &self.api_key {
            builder
                .header("x-api-key", api_key)
                .header("Authorization", format!("Bearer {}", api_key))
        } else {
            builder
        };
        builder.json(body)
    }

    fn request_builder(
        &self,
        body: &Value,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.client, body, headers)
    }

    fn streaming_request_builder(
        &self,
        body: &Value,
        headers: &BTreeMap<String, String>,
    ) -> reqwest::RequestBuilder {
        self.post_builder(&self.streaming_client, body, headers)
            .header(CACHE_CONTROL, HeaderValue::from_static("no-cache"))
            .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
    }
}

#[derive(Debug, Serialize)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: usize,
    stream: bool,
    messages: Vec<AnthropicInputMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AnthropicInputMessage {
    role: AnthropicAIRole,
    content: Vec<AnthropicInputContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicInputContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "web_search_tool_result")]
    WebSearchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    #[serde(rename = "web_fetch_tool_result")]
    WebFetchToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Serialize)]
struct AnthropicToolDefinition {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct AnthropicThinkingConfig {
    r#type: &'static str,
}

#[derive(Debug, Serialize)]
struct AnthropicOutputConfig {
    effort: &'static str,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicMessageResponse {
    id: String,
    #[serde(default)]
    container: Option<AnthropicContainer>,
    #[serde(default)]
    content: Vec<AnthropicResponseContentBlock>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    stop_details: Option<AnthropicStopDetails>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_sequence: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicContainer {
    id: String,
    expires_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicStopDetails {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicUsage {
    input_tokens: usize,
    output_tokens: usize,
    #[serde(default)]
    cache_creation_input_tokens: Option<usize>,
    #[serde(default)]
    cache_read_input_tokens: Option<usize>,
    #[serde(default)]
    cache_creation: Option<AnthropicCacheCreation>,
    #[serde(default)]
    inference_geo: Option<String>,
    #[serde(default)]
    server_tool_use: Option<AnthropicServerToolUsage>,
    #[serde(default)]
    service_tier: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicCacheCreation {
    #[serde(default)]
    ephemeral_1h_input_tokens: Option<usize>,
    #[serde(default)]
    ephemeral_5m_input_tokens: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicServerToolUsage {
    #[serde(default)]
    web_fetch_requests: Option<usize>,
    #[serde(default)]
    web_search_requests: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnthropicResponseContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    citations: Option<Value>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    caller: Option<Value>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
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
            .request_builder(&body, &crate::request_headers(request.extra_body.as_ref()))
            .send()
            .await
            .context("failed to send anthropic request")?;
        let response = match response.error_for_status_ref() {
            Ok(_) => response,
            Err(_) => {
                let status = response.status();
                return Err(invalid_status_error(
                    "anthropic",
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
            .context("failed to decode anthropic response")?;
        parse_response(value, &DsmlToolCallHealer::for_request(&request))
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

        let dsml_healer = DsmlToolCallHealer::for_request(&request);
        let event_source = EventSource::new(self.streaming_request_builder(
            &body,
            &crate::request_headers(request.extra_body.as_ref()),
        ))
        .context("failed to create anthropic event source")?;
        let stream = async_stream::try_stream! {
            let mut message_id = String::new();
            let mut stream_usage = AnthropicStreamUsage::default();
            let mut stop_reason: Option<StopReason> = None;
            let mut content_blocks: BTreeMap<usize, ResponseContent> = BTreeMap::new();
            let mut reasoning_blocks: BTreeMap<usize, String> = BTreeMap::new();
            let mut provider_reasoning_blocks: BTreeMap<usize, Map<String, Value>> = BTreeMap::new();
            let mut completed_reasoning_blocks: HashSet<usize> = HashSet::new();
            let mut tool_json: HashMap<usize, String> = HashMap::new();
            let mut hosted_tool_inputs: HashMap<String, Value> = HashMap::new();
            let mut dsml_text_filters = BTreeMap::new();

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
                            "anthropic",
                            &request.model,
                            "stream",
                            status,
                            response,
                            &body,
                        )
                        .await)?
                    }
                    Err(error) => Err(stream_error(
                        format!(
                            "anthropic stream error for model {}: {}",
                            request.model,
                            format_eventsource_error(&error)
                        )
                    ))?,
                };

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
                                if let Some(usage) = stream_usage.update_from_message_start(&data) {
                                    yield StreamEvent::UsageDelta(usage);
                                }
                            }
                            "content_block_start" => {
                                let Some(index) = data.get("index").and_then(Value::as_u64) else {
                                    continue;
                                };
                                let Some(content_block) = data.get("content_block") else {
                                    continue;
                                };
                                let block: AnthropicResponseContentBlock =
                                    serde_json::from_value(content_block.clone()).map_err(|error| {
                                        anyhow::anyhow!(
                                            "failed to parse anthropic content block start: {error}"
                                        )
                                    })?;
                                match block.kind.as_str() {
                                    "text" => {
                                        if let Some(filter) = dsml_healer.text_stream_filter() {
                                            dsml_text_filters.insert(index as usize, filter);
                                        }
                                        content_blocks.insert(
                                            index as usize,
                                            ResponseContent::Text(String::new()),
                                        );
                                        yield StreamEvent::TextStart {
                                            index: index as usize,
                                        };
                                    }
                                    "tool_use" => {
                                        let Some(id) = block.id.clone() else {
                                            continue;
                                        };
                                        let Some(name) = block.name.clone() else {
                                            continue;
                                        };
                                        let input = block
                                            .input
                                            .clone()
                                            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                        content_blocks.insert(
                                            index as usize,
                                            ResponseContent::ToolUse {
                                                id: id.clone(),
                                                name: name.clone(),
                                                input: input.clone(),
                                            },
                                        );
                                        tool_json.insert(index as usize, String::new());
                                        yield StreamEvent::ToolCallStart {
                                            index: index as usize,
                                            id,
                                            name,
                                            input,
                                        };
                                    }
                                    "server_tool_use" => {
                                        let Some(id) = block.id.clone() else {
                                            continue;
                                        };
                                        let name = block
                                            .name
                                            .clone()
                                            .unwrap_or_else(|| "web_search".to_string());
                                        let input = block
                                            .input
                                            .clone()
                                            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                        content_blocks.insert(
                                            index as usize,
                                            ResponseContent::HostedToolUse {
                                                id: id.clone(),
                                                name: name.clone(),
                                                input: input.clone(),
                                                output: None,
                                                status: None,
                                            },
                                        );
                                        tool_json.insert(index as usize, String::new());
                                    }
                                    "web_search_tool_result" | "web_fetch_tool_result" => {
                                        let id = block
                                            .tool_use_id
                                            .clone()
                                            .or_else(|| block.id.clone())
                                            .unwrap_or_default();
                                        let name = hosted_result_tool_name(&block.kind);
                                        let input = hosted_tool_inputs
                                            .get(&id)
                                            .cloned()
                                            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                                        let output = block.content.clone();
                                        let status = Some(
                                            block
                                                .status
                                                .clone()
                                                .unwrap_or_else(|| "completed".to_string()),
                                        );
                                        content_blocks.insert(
                                            index as usize,
                                            ResponseContent::HostedToolUse {
                                                id: id.clone(),
                                                name: name.clone(),
                                                input: input.clone(),
                                                output: output.clone(),
                                                status: status.clone(),
                                            },
                                        );
                                        yield StreamEvent::HostedToolCallDone {
                                            index: index as usize,
                                            id,
                                            name,
                                            input,
                                            output,
                                            status,
                                        };
                                    }
                                    "thinking" => {
                                        reasoning_blocks.insert(index as usize, String::new());
                                        provider_reasoning_blocks.insert(
                                            index as usize,
                                            anthropic_thinking_payload_map(&block),
                                        );
                                        yield StreamEvent::ReasoningStart {
                                            index: index as usize,
                                        };
                                    }
                                    _ => {}
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
                                            content_blocks.get_mut(&(index as usize))
                                        {
                                            value.push_str(text);
                                        }
                                        let index = index as usize;
                                        let text_chunks = if let Some(filter) =
                                            dsml_text_filters.get_mut(&index)
                                        {
                                            filter.consume(text)
                                        } else {
                                            vec![text.to_string()]
                                        };
                                        for text in text_chunks {
                                            yield StreamEvent::TextDelta {
                                                index,
                                                text,
                                            };
                                        }
                                    }
                                    Some("thinking_delta") => {
                                        let text = delta
                                            .get("thinking")
                                            .or_else(|| delta.get("text"))
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if let Some(value) = reasoning_blocks.get_mut(&(index as usize))
                                        {
                                            value.push_str(text);
                                        }
                                        append_json_string_field(
                                            provider_reasoning_blocks
                                                .entry(index as usize)
                                                .or_insert_with(|| {
                                                    let mut payload = Map::new();
                                                    payload
                                                        .insert("type".to_string(), json!("thinking"));
                                                    payload
                                                }),
                                            "thinking",
                                            text,
                                        );
                                        yield StreamEvent::ReasoningDelta {
                                            index: index as usize,
                                            text: text.to_string(),
                                        };
                                    }
                                    Some("signature_delta") => {
                                        let signature = delta
                                            .get("signature")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        append_json_string_field(
                                            provider_reasoning_blocks
                                                .entry(index as usize)
                                                .or_insert_with(|| {
                                                    let mut payload = Map::new();
                                                    payload
                                                        .insert("type".to_string(), json!("thinking"));
                                                    payload
                                                }),
                                            "signature",
                                            signature,
                                        );
                                    }
                                    Some("input_json_delta") => {
                                        let partial_json = delta
                                            .get("partial_json")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default();
                                        if let Some(acc) = tool_json.get_mut(&(index as usize)) {
                                            acc.push_str(partial_json);
                                        }
                                        yield StreamEvent::ToolCallInputDelta {
                                            index: index as usize,
                                            partial_json: partial_json.to_string(),
                                        };
                                    }
                                    _ => {}
                                }
                            }
                            "content_block_stop" => {
                                let index = data.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                                if let Some(mut filter) = dsml_text_filters.remove(&index) {
                                    for text in filter.finish() {
                                        yield StreamEvent::TextDelta {
                                            index,
                                            text,
                                        };
                                    }
                                }
                                if let Some(json_str) = tool_json.remove(&index)
                                    && !json_str.is_empty()
                                    && let Ok(parsed) = serde_json::from_str(&json_str)
                                    && let Some(block) = content_blocks.get_mut(&index)
                                {
                                    match block {
                                        ResponseContent::ToolUse { input, .. }
                                        | ResponseContent::HostedToolUse { input, .. } => {
                                            *input = parsed;
                                        }
                                        ResponseContent::Text(_)
                                        | ResponseContent::ProviderReasoning { .. } => {}
                                    }
                                }
                                if let Some(ResponseContent::HostedToolUse {
                                    id,
                                    name,
                                    input,
                                    output,
                                    status,
                                }) = content_blocks.get(&index)
                                    && output.is_none()
                                    && status.is_none()
                                {
                                    hosted_tool_inputs.insert(id.clone(), input.clone());
                                    yield StreamEvent::HostedToolCallStart {
                                        index,
                                        id: id.clone(),
                                        name: name.clone(),
                                        input: input.clone(),
                                    };
                                }
                                if reasoning_blocks.contains_key(&index)
                                    && completed_reasoning_blocks.insert(index)
                                {
                                    yield StreamEvent::ReasoningDone { index };
                                }
                            }
                            "message_delta" => {
                                if let Some(delta) = data.get("delta").and_then(Value::as_object)
                                    && let Some(reason) =
                                        delta.get("stop_reason").and_then(Value::as_str)
                                    {
                                        stop_reason = Some(parse_stop_reason(reason));
                                    }
                                if let Some(usage) = stream_usage.update_from_message_delta(&data) {
                                    yield StreamEvent::UsageDelta(usage);
                                }
                            }
                            "message_stop" => {
                                for (index, mut filter) in std::mem::take(&mut dsml_text_filters) {
                                    for text in filter.finish() {
                                        yield StreamEvent::TextDelta {
                                            index,
                                            text,
                                        };
                                    }
                                }
                                for index in reasoning_blocks.keys().copied().collect::<Vec<_>>() {
                                    if completed_reasoning_blocks.insert(index) {
                                        yield StreamEvent::ReasoningDone { index };
                                    }
                                }
                                insert_provider_reasoning_blocks(
                                    &mut content_blocks,
                                    std::mem::take(&mut provider_reasoning_blocks),
                                );
                                let response = ModelResponse {
                                    id: message_id.clone(),
                                    content: dsml_healer
                                        .heal_response_content(content_blocks.into_values().collect()),
                                    stop_reason: stop_reason.clone(),
                                    usage: stream_usage.snapshot(),
                                    metadata: ResponseMetadata {
                                        extras: reasoning_blocks
                                            .values()
                                            .filter(|text| !text.is_empty())
                                            .cloned()
                                            .map(|text| ResponseExtra::ReasoningText { text })
                                            .collect(),
                                    },
                                };
                                yield StreamEvent::MessageDone { response };
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            for (index, mut filter) in std::mem::take(&mut dsml_text_filters) {
                for text in filter.finish() {
                    yield StreamEvent::TextDelta {
                        index,
                        text,
                    };
                }
            }
            for index in reasoning_blocks.keys().copied().collect::<Vec<_>>() {
                if completed_reasoning_blocks.insert(index) {
                    yield StreamEvent::ReasoningDone { index };
                }
            }
            insert_provider_reasoning_blocks(
                &mut content_blocks,
                provider_reasoning_blocks,
            );
            let response = ModelResponse {
                id: message_id,
                content: dsml_healer.heal_response_content(content_blocks.into_values().collect()),
                stop_reason,
                usage: stream_usage.snapshot(),
                metadata: ResponseMetadata {
                    extras: reasoning_blocks
                        .into_values()
                        .filter(|text| !text.is_empty())
                        .map(|text| ResponseExtra::ReasoningText { text })
                        .collect(),
                },
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
    fn family(&self) -> ProviderWireApi {
        ProviderWireApi::AnthropicMessages
    }

    fn capabilities(&self, _model: &str) -> ProviderCapabilities {
        ProviderCapabilities::anthropic()
    }
}

/// Here is the documentation of the Anthropic Messages API request body.
///
/// Official reference:
/// <https://platform.claude.com/docs/en/api/messages>
///
/// Create
/// - `POST /v1/messages`
///   Send a structured list of input messages with text and/or image content,
///   and the model will generate the next message in the conversation.
///   The Messages API can be used for either single queries or stateless
///   multi-turn conversations.
///
/// Body Parameters
/// - `max_tokens: number`
///   The maximum number of tokens to generate before stopping.
/// - `messages: array of MessageParam`
///   Input messages. Anthropic models operate on alternating `user` and
///   `assistant` turns, and consecutive turns with the same role are combined.
///   Each input message has:
///   - `role: "user" or "assistant"`
///   - `content: string or array of ContentBlockParam`
///     A string is shorthand for a single text block.
///     Supported content block families documented by Anthropic include:
///     - `TextBlockParam`
///     - `ImageBlockParam`
///     - `DocumentBlockParam`
///     - `SearchResultBlockParam`
///     - `ThinkingBlockParam`
///     - `RedactedThinkingBlockParam`
///     - `ToolUseBlockParam`
///     - `ToolResultBlockParam`
///     - `ServerToolUseBlockParam`
///     - `WebSearchToolResultBlockParam`
///     - `WebFetchToolResultBlockParam`
///     - `CodeExecutionToolResultBlockParam`
///     - `BashCodeExecutionToolResultBlockParam`
///     - `TextEditorCodeExecutionToolResultBlockParam`
///     - `ToolSearchToolResultBlockParam`
///     - `ContainerUploadBlockParam`
/// - `model: Model`
///   The model slug/id that will complete the prompt.
/// - `cache_control: optional CacheControlEphemeral`
///   Top-level cache control that automatically applies a cache marker to the
///   last cacheable block in the request.
/// - `container: optional string`
///   Container identifier for reuse across requests.
/// - `inference_geo: optional string`
///   Geographic region for inference processing.
/// - `metadata: optional Metadata`
///   Request metadata such as `user_id`.
/// - `output_config: optional OutputConfig`
///   Output configuration such as response format and effort.
/// - `service_tier: optional "auto" or "standard_only"`
///   Determines whether priority or standard capacity is used.
/// - `stop_sequences: optional array of string`
///   Custom sequences that stop generation.
/// - `stream: optional boolean`
///   Whether to stream the response using server-sent events.
/// - `system: optional string or array of TextBlockParam`
///   System prompt. Anthropic Messages uses a top-level `system` field rather
///   than a `"system"` message role.
/// - `temperature: optional number`
///   Amount of randomness injected into the response.
/// - `thinking: optional ThinkingConfigParam`
///   Extended-thinking configuration.
///   One of the following:
///   - `ThinkingConfigEnabled = object { budget_tokens, type, display }`
///   - `ThinkingConfigDisabled = object { type }`
///   - `ThinkingConfigAdaptive = object { type, display }`
///     This implementation sends `type: "enabled"` for enabled thinking and
///     uses `output_config.effort` for effort control.
/// - `tool_choice: optional ToolChoice`
///   Controls how the model uses available tools.
/// - `tools: optional array of ToolUnion`
///   Tool definitions that the model may use.
/// - `top_k: optional number`
///   Limits sampling to the top K options.
/// - `top_p: optional number`
///   Uses nucleus sampling.
///
/// Notes about this implementation:
/// - This builder currently emits the subset represented by the crate's
///   `ModelRequest`: `model`, `max_tokens`, `stream`, `messages`, optional
///   `system`, optional `tools`, optional `thinking`, optional
///   `output_config.effort`, and any merged `extra_body`.
/// - Request message content is currently serialized from shared IR blocks into
///   Anthropic `text`, `tool_use`, and `tool_result` blocks.
/// - More advanced Anthropic request features documented above, such as image
///   blocks, document blocks, `tool_choice`, `cache_control`,
///   `metadata`, `service_tier`, `stop_sequences`, `top_k`, and `top_p`, are
///   not constructed directly here unless supplied through `extra_body`.
fn build_request(request: &ModelRequest, stream: bool) -> Value {
    let mut messages = request.messages.clone();
    normalize_tool_result_messages(&mut messages);
    let body = AnthropicMessagesRequest {
        model: request.model.clone(),
        max_tokens: request.max_tokens,
        stream,
        messages: messages
            .iter()
            .filter_map(build_message)
            .collect::<Vec<_>>(),
        system: request.system.clone(),
        tools: request.tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|tool| AnthropicToolDefinition {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                })
                .collect::<Vec<_>>()
        }),
        thinking: request.request_thinking.as_deref().and_then(build_thinking),
        output_config: build_output_config(
            request.request_thinking.as_deref(),
            request.reasoning_effort,
        ),
        temperature: request.sampling.temperature,
        top_p: request.sampling.top_p,
        top_k: request.sampling.top_k,
    };
    let mut root =
        serde_json::to_value(body).expect("anthropic request body serialization should succeed");

    append_anthropic_hosted_tools(&mut root, &request.hosted_tools);

    merge_extra_body(&mut root, request.extra_body.as_ref());

    root
}

/// Here is the documentation of the Anthropic Messages API response body.
///
/// Returns
/// - `Message = object`
///   Generated assistant message returned by the Messages API.
/// - `id: string`
///   Unique object identifier.
/// - `container: Container`
///   Information about the container used in the request.
///   - `id: string`
///   - `expires_at: string`
/// - `content: array of ContentBlock`
///   Content generated by the model.
///   Anthropic documents a large union of possible response block types,
///   including:
///   - `TextBlock`
///   - `ThinkingBlock`
///   - `RedactedThinkingBlock`
///   - `ToolUseBlock`
///   - `ServerToolUseBlock`
///   - `WebSearchToolResultBlock`
///   - `WebFetchToolResultBlock`
///   - `CodeExecutionToolResultBlock`
///   - `BashCodeExecutionToolResultBlock`
///   - `TextEditorCodeExecutionToolResultBlock`
///   - `ToolSearchToolResultBlock`
///   - `ContainerUploadBlock`
/// - `model: Model`
///   The model that completed the prompt.
/// - `role: "assistant"`
///   The generated message role.
/// - `stop_details: RefusalStopDetails`
///   Structured refusal information when applicable.
/// - `stop_reason: StopReason`
///   One of:
///   - `"end_turn"`
///   - `"max_tokens"`
///   - `"stop_sequence"`
///   - `"tool_use"`
///   - `"pause_turn"`
///   - `"refusal"`
/// - `stop_sequence: string`
///   The matched custom stop sequence, if any.
/// - `type: "message"`
///   Object type for Messages API responses.
/// - `usage: Usage`
///   Billing and rate-limit usage.
///   Includes fields such as:
///   - `input_tokens`
///   - `output_tokens`
///   - `cache_creation_input_tokens`
///   - `cache_read_input_tokens`
///   - `cache_creation`
///   - `inference_geo`
///   - `server_tool_use`
///   - `service_tier`
///
/// Notes about this implementation:
/// - `parse_response` currently reads `id`, `content`, `stop_reason`, and
///   `usage`.
/// - Response content maps Anthropic `text`, local `tool_use`, hosted tool,
///   and signed `thinking` blocks.
/// - Other documented response fields such as `container`, `model`, `role`,
///   `stop_details`, `stop_sequence`, `type`, citations, and the richer usage
///   breakdown are not currently projected into shared `ModelResponse` fields.
///   -------------------------- Below is an example -------------------------
/// ```json
/// {
///  "id": "msg_013Zva2CMHLNnXjNJJKqJ2EF",
///  "container": {
///    "id": "id",
///    "expires_at": "2019-12-27T18:11:19.117Z"
///  },
///  "content": [
///    {
///      "citations": [
///        {
///          "cited_text": "cited_text",
///          "document_index": 0,
///          "document_title": "document_title",
///          "end_char_index": 0,
///          "file_id": "file_id",
///          "start_char_index": 0,
///          "type": "char_location"
///        }
///      ],
///      "text": "Hi! My name is Claude.",
///      "type": "text"
///    }
///  ],
///  "model": "claude-opus-4-6",
///  "role": "assistant",
///  "stop_details": {
///    "category": "cyber",
///    "explanation": "explanation",
///    "type": "refusal"
///  },
///  "stop_reason": "end_turn",
///  "stop_sequence": null,
///  "type": "message",
///  "usage": {
///    "cache_creation": {
///      "ephemeral_1h_input_tokens": 0,
///      "ephemeral_5m_input_tokens": 0
///    },
///    "cache_creation_input_tokens": 2051,
///    "cache_read_input_tokens": 2051,
///    "inference_geo": "inference_geo",
///    "input_tokens": 2095,
///    "output_tokens": 503,
///    "server_tool_use": {
///      "web_fetch_requests": 2,
///      "web_search_requests": 0
///    },
///    "service_tier": "standard"
///  }
///}
/// ```
fn parse_response(value: Value, dsml_healer: &DsmlToolCallHealer) -> Result<ModelResponse> {
    let response: AnthropicMessageResponse = serde_json::from_value(value.clone())
        .context("failed to deserialize anthropic messages response")?;
    let mut content = Vec::new();
    let mut metadata = ResponseMetadata::default();

    let mut hosted_tool_inputs: HashMap<String, Value> = HashMap::new();
    for block in &response.content {
        if let Some(mut parsed) = parse_response_content_block(block, &mut metadata) {
            if let ResponseContent::HostedToolUse {
                id,
                input,
                output,
                status,
                ..
            } = &mut parsed
            {
                if output.is_none() && status.is_none() {
                    hosted_tool_inputs.insert(id.clone(), input.clone());
                } else if matches!(input, Value::Object(map) if map.is_empty())
                    && let Some(previous_input) = hosted_tool_inputs.get(id)
                {
                    *input = previous_input.clone();
                }
            }
            content.push(parsed);
        }
    }
    let content = dsml_healer.heal_response_content(content);
    let stop_reason = response.stop_reason.as_deref().map(parse_stop_reason);
    let usage = response.usage.as_ref().map(map_usage).unwrap_or_default();

    if let Some(provider_payload) = build_provider_specific_response_payload(&response) {
        metadata.extras.push(ResponseExtra::ProviderSpecific {
            provider: "anthropic".to_string(),
            payload: provider_payload,
        });
    }

    Ok(ModelResponse {
        id: response.id,
        content,
        stop_reason,
        usage,
        metadata,
    })
}

fn build_message(message: &RequestMessage) -> Option<AnthropicInputMessage> {
    let role = message
        .role
        .parse::<AnthropicAIRole>()
        .unwrap_or(AnthropicAIRole::User);
    let content = message
        .content
        .iter()
        .filter_map(build_content_block)
        .collect::<Vec<_>>();

    (!content.is_empty()).then_some(AnthropicInputMessage { role, content })
}

fn build_content_block(block: &RequestContent) -> Option<AnthropicInputContentBlock> {
    match block {
        RequestContent::Text { text } => {
            Some(AnthropicInputContentBlock::Text { text: text.clone() })
        }
        RequestContent::Reasoning { .. } => None,
        RequestContent::ProviderReasoning { provider, payload } if provider == "anthropic" => {
            Some(AnthropicInputContentBlock::Thinking {
                thinking: payload
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                signature: payload
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            })
        }
        RequestContent::ProviderReasoning { .. } => None,
        RequestContent::ToolUse { id, name, input } => Some(AnthropicInputContentBlock::ToolUse {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        }),
        RequestContent::HostedToolUse {
            id,
            name,
            input,
            output,
            status,
        } => Some(build_hosted_content_block(id, name, input, output, status)),
        RequestContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => Some(AnthropicInputContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: content.clone(),
            is_error: *is_error,
        }),
    }
}

fn build_hosted_content_block(
    id: &str,
    name: &str,
    input: &Value,
    output: &Option<Value>,
    status: &Option<String>,
) -> AnthropicInputContentBlock {
    match output {
        None => AnthropicInputContentBlock::ServerToolUse {
            id: id.to_string(),
            name: name.to_string(),
            input: input.clone(),
        },
        Some(content) if name == "web_fetch" => AnthropicInputContentBlock::WebFetchToolResult {
            tool_use_id: id.to_string(),
            content: content.clone(),
            status: status.clone(),
        },
        Some(content) => AnthropicInputContentBlock::WebSearchToolResult {
            tool_use_id: id.to_string(),
            content: content.clone(),
            status: status.clone(),
        },
    }
}

fn hosted_result_tool_name(kind: &str) -> String {
    match kind {
        "web_fetch_tool_result" => "web_fetch".to_string(),
        "web_search_tool_result" => "web_search".to_string(),
        _ => "web_search".to_string(),
    }
}

fn parse_response_content_block(
    block: &AnthropicResponseContentBlock,
    metadata: &mut ResponseMetadata,
) -> Option<ResponseContent> {
    match block.kind.as_str() {
        "text" => Some(ResponseContent::Text(
            block.text.clone().unwrap_or_default(),
        )),
        "tool_use" => Some(ResponseContent::ToolUse {
            id: block.id.clone()?,
            name: block.name.clone()?,
            input: block
                .input
                .clone()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
        }),
        "server_tool_use" => Some(ResponseContent::HostedToolUse {
            id: block.id.clone()?,
            name: block
                .name
                .clone()
                .unwrap_or_else(|| "web_search".to_string()),
            input: block
                .input
                .clone()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
            output: None,
            status: None,
        }),
        "web_search_tool_result" | "web_fetch_tool_result" => {
            Some(ResponseContent::HostedToolUse {
                id: block.tool_use_id.clone().or_else(|| block.id.clone())?,
                name: hosted_result_tool_name(&block.kind),
                input: Value::Object(serde_json::Map::new()),
                output: block.content.clone(),
                status: Some(
                    block
                        .status
                        .clone()
                        .unwrap_or_else(|| "completed".to_string()),
                ),
            })
        }
        "thinking" => {
            if let Some(thinking) = &block.thinking
                && !thinking.is_empty()
            {
                metadata.extras.push(ResponseExtra::ReasoningText {
                    text: thinking.clone(),
                });
            }
            Some(ResponseContent::ProviderReasoning {
                provider: "anthropic".to_string(),
                payload: anthropic_thinking_payload(block),
            })
        }
        _ => None,
    }
}

fn anthropic_thinking_payload(block: &AnthropicResponseContentBlock) -> Value {
    Value::Object(anthropic_thinking_payload_map(block))
}

fn anthropic_thinking_payload_map(block: &AnthropicResponseContentBlock) -> Map<String, Value> {
    let mut payload = block.extra.clone();
    payload.insert("type".to_string(), json!("thinking"));
    payload.insert(
        "thinking".to_string(),
        json!(block.thinking.clone().unwrap_or_default()),
    );
    if let Some(signature) = &block.signature {
        payload.insert("signature".to_string(), json!(signature));
    }
    payload
}

fn append_json_string_field(payload: &mut Map<String, Value>, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    match payload.get_mut(key) {
        Some(Value::String(existing)) => existing.push_str(value),
        _ => {
            payload.insert(key.to_string(), json!(value));
        }
    }
}

fn insert_provider_reasoning_blocks(
    content_blocks: &mut BTreeMap<usize, ResponseContent>,
    provider_reasoning_blocks: BTreeMap<usize, Map<String, Value>>,
) {
    for (index, payload) in provider_reasoning_blocks {
        content_blocks.insert(
            index,
            ResponseContent::ProviderReasoning {
                provider: "anthropic".to_string(),
                payload: Value::Object(payload),
            },
        );
    }
}

fn map_usage(usage: &AnthropicUsage) -> Usage {
    let cache_creation_input_tokens = usage.cache_creation_input_tokens.unwrap_or(0);
    let cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or(0);
    Usage {
        input_tokens: usage
            .input_tokens
            .saturating_add(cache_creation_input_tokens)
            .saturating_add(cache_read_input_tokens),
        output_tokens: usage.output_tokens,
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        reasoning_output_tokens: None,
        total_tokens: None,
    }
}

fn build_provider_specific_response_payload(response: &AnthropicMessageResponse) -> Option<Value> {
    let mut payload = serde_json::Map::new();

    if let Some(container) = &response.container {
        payload.insert("container".to_string(), json!(container));
    }
    if !response.content.is_empty() {
        payload.insert("content".to_string(), json!(response.content));
    }
    if let Some(model) = &response.model {
        payload.insert("model".to_string(), json!(model));
    }
    if let Some(role) = &response.role {
        payload.insert("role".to_string(), json!(role));
    }
    if let Some(stop_details) = &response.stop_details {
        payload.insert("stop_details".to_string(), json!(stop_details));
    }
    if let Some(stop_sequence) = &response.stop_sequence {
        payload.insert("stop_sequence".to_string(), json!(stop_sequence));
    }
    if let Some(kind) = &response.kind {
        payload.insert("type".to_string(), json!(kind));
    }
    if let Some(usage) = &response.usage {
        payload.insert("usage".to_string(), json!(usage));
    }

    if payload.is_empty() {
        None
    } else {
        Some(Value::Object(payload))
    }
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

fn stream_error(message: String) -> ProviderError {
    ProviderError::StreamError {
        message,
        bytes_received: None,
    }
}

fn format_eventsource_error(error: &reqwest_eventsource::Error) -> String {
    let mut message = format!("{error}; debug={error:?}");
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str("; source=");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

fn build_thinking(level: &str) -> Option<AnthropicThinkingConfig> {
    match level.trim().to_ascii_lowercase().as_str() {
        "" | "default" => None,
        "disabled" => Some(AnthropicThinkingConfig { r#type: "disabled" }),
        "enabled" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Some(AnthropicThinkingConfig { r#type: "enabled" })
        }
        _ => None,
    }
}

fn build_output_config(
    thinking: Option<&str>,
    reasoning_effort: Option<ReasoningEffort>,
) -> Option<AnthropicOutputConfig> {
    if matches!(
        thinking
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "" | "default" | "disabled"
    ) {
        return None;
    }

    let effort = match reasoning_effort {
        Some(ReasoningEffort::Low) => "low",
        Some(ReasoningEffort::Medium) => "medium",
        Some(ReasoningEffort::High) => "high",
        Some(ReasoningEffort::XHigh) => "xhigh",
        Some(ReasoningEffort::Max) => "max",
        Some(ReasoningEffort::None | ReasoningEffort::Minimal) => return None,
        None => match thinking?.trim().to_ascii_lowercase().as_str() {
            "enabled" => "high",
            "low" => "low",
            "medium" => "medium",
            "high" => "high",
            "xhigh" => "xhigh",
            "max" => "max",
            _ => return None,
        },
    };

    Some(AnthropicOutputConfig { effort })
}

#[cfg(test)]
mod tests {
    use crate::dsml::DsmlToolCallHealer;
    use devo_protocol::ModelRequest;
    use devo_protocol::ReasoningEffort;
    use devo_protocol::RequestContent;
    use devo_protocol::RequestMessage;
    use devo_protocol::SamplingControls;
    use devo_protocol::ToolDefinition;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::build_request;
    use super::parse_response;
    use super::parse_stop_reason;
    use devo_protocol::ResponseContent;
    use devo_protocol::ResponseExtra;
    use devo_protocol::StopReason;

    #[test]
    fn build_request_includes_sampling_tools_and_thinking() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "claude-sonnet-4-6".to_string(),
            system: Some("You are helpful.".to_string()),
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![
                        RequestContent::Text {
                            text: "Calling tool".to_string(),
                        },
                        RequestContent::ToolUse {
                            id: "toolu_123".to_string(),
                            name: "get_weather".to_string(),
                            input: json!({"city": "Boston"}),
                        },
                    ],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::ToolResult {
                        tool_use_id: "toolu_123".to_string(),
                        content: "{\"temp\":72}".to_string(),
                        is_error: Some(false),
                    }],
                },
            ],
            max_tokens: 1024,
            tools: Some(vec![ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather by city".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"]
                }),
                output_schema: None,
            }]),
            hosted_tools: Vec::new(),
            sampling: SamplingControls {
                temperature: Some(0.2),
                top_p: Some(0.9),
                top_k: Some(32),
            },
            request_thinking: Some("medium".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["model"], json!("claude-sonnet-4-6"));
        assert_eq!(body["max_tokens"], json!(1024));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["system"], json!("You are helpful."));
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["top_p"], json!(0.9));
        assert_eq!(body["top_k"], json!(32));
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["output_config"]["effort"], json!("medium"));
        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(body["messages"][0]["content"][1]["type"], json!("tool_use"));
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("tool_result")
        );
        assert_eq!(body["tools"][0]["name"], json!("get_weather"));
    }

    #[test]
    fn build_request_skips_unsigned_reasoning_blocks() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-flash".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::Reasoning {
                        text: "Need to inspect the file first.".to_string(),
                    },
                    RequestContent::Text {
                        text: "I'll read the README.".to_string(),
                    },
                    RequestContent::ToolUse {
                        id: "toolu_123".to_string(),
                        name: "read".to_string(),
                        input: json!({"path": "README.md"}),
                    },
                ],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {
                    "type": "text",
                    "text": "I'll read the README."
                },
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "read",
                    "input": { "path": "README.md" }
                }
            ])
        );
    }

    #[test]
    fn build_request_omits_messages_with_no_anthropic_content() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "claude-sonnet-4-6".to_string(),
            system: None,
            messages: vec![
                RequestMessage {
                    role: "assistant".to_string(),
                    content: vec![RequestContent::Reasoning {
                        text: "unsigned reasoning".to_string(),
                    }],
                },
                RequestMessage {
                    role: "user".to_string(),
                    content: vec![RequestContent::Text {
                        text: "continue".to_string(),
                    }],
                },
            ],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, false);

        assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][0]["content"][0]["text"], json!("continue"));
    }

    #[test]
    fn build_request_serializes_provider_reasoning_with_signature() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![RequestContent::ProviderReasoning {
                    provider: "anthropic".to_string(),
                    payload: json!({
                        "type": "thinking",
                        "thinking": "Need to inspect the file first.",
                        "signature": "sig_123"
                    }),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(
            body["messages"][0]["content"],
            json!([{
                "type": "thinking",
                "thinking": "Need to inspect the file first.",
                "signature": "sig_123"
            }])
        );
    }

    #[test]
    fn build_request_serializes_hosted_tool_use_blocks() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "assistant".to_string(),
                content: vec![
                    RequestContent::HostedToolUse {
                        id: "srvtool_1".to_string(),
                        name: "web_search".to_string(),
                        input: json!({"query": "Rust docs"}),
                        output: None,
                        status: None,
                    },
                    RequestContent::HostedToolUse {
                        id: "srvtool_1".to_string(),
                        name: "web_search".to_string(),
                        input: json!({"query": "Rust docs"}),
                        output: Some(json!([{
                            "title": "Rust documentation",
                            "url": "https://example.test/rust"
                        }])),
                        status: Some("completed".to_string()),
                    },
                ],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: None,
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(
            body["messages"][0]["content"],
            json!([
                {
                    "type": "server_tool_use",
                    "id": "srvtool_1",
                    "name": "web_search",
                    "input": { "query": "Rust docs" }
                },
                {
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtool_1",
                    "content": [{
                        "title": "Rust documentation",
                        "url": "https://example.test/rust"
                    }],
                    "status": "completed"
                }
            ])
        );
    }

    #[test]
    fn build_request_sends_disabled_thinking_without_output_config() {
        let mut request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-flash".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("disabled".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let disabled = build_request(&request, true);
        request.request_thinking = Some("bogus".to_string());
        let unknown = build_request(&request, true);

        assert_eq!(disabled["thinking"]["type"], json!("disabled"));
        assert_eq!(disabled.get("output_config"), None);
        assert_eq!(unknown.get("thinking"), None);
        assert_eq!(unknown.get("output_config"), None);
    }

    #[test]
    fn build_request_maps_max_thinking_to_output_config_effort() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("max".to_string()),
            reasoning_effort: None,
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["output_config"]["effort"], json!("max"));
    }

    #[test]
    fn build_request_prefers_reasoning_effort_for_output_config_effort() {
        let request = ModelRequest {
            model_slug: devo_protocol::ModelProfileKey::Generic,
            model: "deepseek-v4-pro".to_string(),
            system: None,
            messages: vec![RequestMessage {
                role: "user".to_string(),
                content: vec![RequestContent::Text {
                    text: "Reply with OK only.".to_string(),
                }],
            }],
            max_tokens: 1024,
            tools: None,
            hosted_tools: Vec::new(),
            sampling: SamplingControls::default(),
            request_thinking: Some("enabled".to_string()),
            reasoning_effort: Some(ReasoningEffort::Max),
            extra_body: None,
        };

        let body = build_request(&request, true);

        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["output_config"]["effort"], json!("max"));
    }

    #[test]
    fn parse_response_extracts_text_tool_use_reasoning_and_usage() {
        let response = parse_response(
            json!({
                "id": "msg_123",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "thinking",
                        "thinking": "Need to call the weather tool first.",
                        "signature": "sig_123"
                    },
                    {
                        "type": "text",
                        "text": "Let me check that."
                    },
                    {
                        "type": "server_tool_use",
                        "id": "srvtool_1",
                        "name": "web_search",
                        "input": { "query": "Boston weather" },
                        "caller": { "type": "direct" }
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 11,
                    "output_tokens": 7,
                    "cache_creation_input_tokens": 3,
                    "cache_read_input_tokens": 5,
                    "service_tier": "standard",
                    "inference_geo": "us"
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(response.id, "msg_123");
        assert_eq!(response.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(response.usage.input_tokens, 19);
        assert_eq!(response.usage.output_tokens, 7);
        assert_eq!(response.usage.cache_creation_input_tokens, Some(3));
        assert_eq!(response.usage.cache_read_input_tokens, Some(5));
        assert_eq!(response.content.len(), 3);
        match &response.content[0] {
            ResponseContent::ProviderReasoning { provider, payload } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(
                    payload,
                    &json!({
                        "type": "thinking",
                        "thinking": "Need to call the weather tool first.",
                        "signature": "sig_123"
                    })
                );
            }
            other => panic!("expected provider reasoning block, got {other:?}"),
        }
        match &response.content[1] {
            ResponseContent::Text(text) => {
                assert_eq!(text, "Let me check that.");
            }
            other => panic!("expected text block, got {other:?}"),
        }
        match &response.content[2] {
            ResponseContent::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => {
                assert_eq!(id, "srvtool_1");
                assert_eq!(name, "web_search");
                assert_eq!(input, &json!({"query": "Boston weather"}));
                assert_eq!(output, &None);
                assert_eq!(status, &None);
            }
            other => panic!("expected hosted tool use block, got {other:?}"),
        }
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ReasoningText { text }
            if text == "Need to call the weather tool first."
        )));
        assert!(response.metadata.extras.iter().any(|extra| matches!(
            extra,
            ResponseExtra::ProviderSpecific { provider, .. } if provider == "anthropic"
        )));
    }

    #[test]
    fn parse_response_heals_deepseek_v4_dsml_text_tool_calls() {
        let response = parse_response(
            json!({
                "id": "msg_dsml",
                "type": "message",
                "role": "assistant",
                "model": "deepseek-v4-pro",
                "content": [
                    {
                        "type": "text",
                        "text": "<｜｜DSML｜｜tool_calls>\n<｜｜DSML｜｜invoke name=\"web_search\">\n<｜｜DSML｜｜parameter name=\"query\" string=\"true\">electron-vite npm package 2026</｜｜DSML｜｜parameter>\n<｜｜DSML｜｜parameter name=\"limit\" string=\"false\">3</｜｜DSML｜｜parameter>\n</｜｜DSML｜｜invoke>\n</｜｜DSML｜｜tool_calls>"
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("deepseek-v4-pro"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![ResponseContent::ToolUse {
                id: "dsml_0_0".to_string(),
                name: "web_search".to_string(),
                input: json!({
                    "query": "electron-vite npm package 2026",
                    "limit": 3
                }),
            }]
        );
    }

    #[test]
    fn parse_response_extracts_web_search_tool_result_as_hosted_completion() {
        let response = parse_response(
            json!({
                "id": "msg_456",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvtool_1",
                        "content": [
                            {
                                "type": "web_search_result",
                                "title": "Boston weather",
                                "url": "https://example.test/weather"
                            }
                        ],
                        "status": "completed"
                    }
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(response.content.len(), 1);
        match &response.content[0] {
            ResponseContent::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => {
                assert_eq!(id, "srvtool_1");
                assert_eq!(name, "web_search");
                assert_eq!(input, &json!({}));
                assert_eq!(
                    output,
                    &Some(json!([
                        {
                            "type": "web_search_result",
                            "title": "Boston weather",
                            "url": "https://example.test/weather"
                        }
                    ]))
                );
                assert_eq!(status.as_deref(), Some("completed"));
            }
            other => panic!("expected hosted tool completion, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_carries_hosted_web_search_input_into_completion() {
        let response = parse_response(
            json!({
                "id": "msg_789",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "server_tool_use",
                        "id": "srvtool_1",
                        "name": "web_search",
                        "input": { "query": "DeepSeek official website" }
                    },
                    {
                        "type": "web_search_tool_result",
                        "tool_use_id": "srvtool_1",
                        "content": [
                            {
                                "type": "web_search_result",
                                "title": "DeepSeek",
                                "url": "https://www.deepseek.com/"
                            }
                        ],
                        "status": "completed"
                    }
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![
                ResponseContent::HostedToolUse {
                    id: "srvtool_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "DeepSeek official website"}),
                    output: None,
                    status: None,
                },
                ResponseContent::HostedToolUse {
                    id: "srvtool_1".to_string(),
                    name: "web_search".to_string(),
                    input: json!({"query": "DeepSeek official website"}),
                    output: Some(json!([
                        {
                            "type": "web_search_result",
                            "title": "DeepSeek",
                            "url": "https://www.deepseek.com/"
                        }
                    ])),
                    status: Some("completed".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parse_response_extracts_web_fetch_tool_result_as_hosted_completion() {
        let response = parse_response(
            json!({
                "id": "msg_fetch",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [
                    {
                        "type": "server_tool_use",
                        "id": "srvtool_fetch",
                        "name": "web_fetch",
                        "input": { "url": "https://example.test/docs" }
                    },
                    {
                        "type": "web_fetch_tool_result",
                        "tool_use_id": "srvtool_fetch",
                        "content": {
                            "type": "web_fetch_result",
                            "url": "https://example.test/docs",
                            "title": "Docs"
                        },
                        "status": "completed"
                    }
                ],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 4,
                    "output_tokens": 2
                }
            }),
            &DsmlToolCallHealer::for_model("claude-sonnet-4-6"),
        )
        .expect("parse response");

        assert_eq!(
            response.content,
            vec![
                ResponseContent::HostedToolUse {
                    id: "srvtool_fetch".to_string(),
                    name: "web_fetch".to_string(),
                    input: json!({"url": "https://example.test/docs"}),
                    output: None,
                    status: None,
                },
                ResponseContent::HostedToolUse {
                    id: "srvtool_fetch".to_string(),
                    name: "web_fetch".to_string(),
                    input: json!({"url": "https://example.test/docs"}),
                    output: Some(json!({
                        "type": "web_fetch_result",
                        "url": "https://example.test/docs",
                        "title": "Docs"
                    })),
                    status: Some("completed".to_string()),
                },
            ]
        );
    }

    #[test]
    fn parse_stop_reason_matches_messages_contract() {
        assert_eq!(parse_stop_reason("end_turn"), StopReason::EndTurn);
        assert_eq!(parse_stop_reason("tool_use"), StopReason::ToolUse);
        assert_eq!(parse_stop_reason("max_tokens"), StopReason::MaxTokens);
        assert_eq!(parse_stop_reason("stop_sequence"), StopReason::StopSequence);
        assert_eq!(parse_stop_reason("pause_turn"), StopReason::EndTurn);
        assert_eq!(parse_stop_reason("refusal"), StopReason::EndTurn);
    }
}
