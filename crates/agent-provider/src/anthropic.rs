use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use tracing::{debug, warn};

use crate::{ModelProvider, ModelRequest, ModelResponse, ResponseContent, StopReason, StreamEvent, Usage};

pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    fn build_body(&self, request: &ModelRequest, stream: bool) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
        });
        if stream {
            body["stream"] = serde_json::json!(true);
        }
        if let Some(ref system) = request.system {
            body["system"] = serde_json::json!(system);
        }
        if let Some(ref tools) = request.tools {
            if !tools.is_empty() {
                body["tools"] = serde_json::to_value(tools).unwrap();
            }
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = serde_json::json!(temp);
        }
        body["messages"] = serde_json::to_value(&request.messages).unwrap();
        body
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    async fn complete(&self, request: ModelRequest) -> anyhow::Result<ModelResponse> {
        let body = self.build_body(&request, false);
        debug!(model = %request.model, "complete request");

        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status, text);
        }

        let raw: serde_json::Value = resp.json().await?;
        parse_complete_response(&raw)
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = anyhow::Result<StreamEvent>> + Send>>> {
        let body = self.build_body(&request, true);
        debug!(model = %request.model, "stream request");

        let resp = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API error {}: {}", status, text);
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<anyhow::Result<StreamEvent>>(64);
        let byte_stream = resp.bytes_stream();

        tokio::spawn(async move {
            if let Err(e) = process_sse_stream(byte_stream, &tx).await {
                let _ = tx.send(Err(e)).await;
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

// ---------------------------------------------------------------------------
// SSE stream processing
// ---------------------------------------------------------------------------

struct SseState {
    message_id: String,
    input_tokens: usize,
    output_tokens: usize,
    stop_reason: Option<StopReason>,
    content_blocks: Vec<ResponseContent>,
    tool_json: HashMap<usize, String>,
}

impl SseState {
    fn new() -> Self {
        Self {
            message_id: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: None,
            content_blocks: Vec::new(),
            tool_json: HashMap::new(),
        }
    }
}

async fn process_sse_stream(
    mut byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    tx: &tokio::sync::mpsc::Sender<anyhow::Result<StreamEvent>>,
) -> anyhow::Result<()> {
    let mut buffer = String::new();
    let mut event_type = String::new();
    let mut event_data = String::new();
    let mut state = SseState::new();

    while let Some(chunk) = byte_stream.next().await {
        let bytes = chunk?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim_end_matches('\r').to_string();
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() {
                // Empty line → dispatch the accumulated event
                if !event_data.is_empty() {
                    let events = handle_sse_event(&event_type, &event_data, &mut state);
                    for evt in events {
                        if tx.send(evt).await.is_err() {
                            return Ok(());
                        }
                    }
                }
                event_type.clear();
                event_data.clear();
            } else if let Some(val) = line.strip_prefix("event: ") {
                event_type = val.to_string();
            } else if let Some(val) = line.strip_prefix("data: ") {
                if event_data.is_empty() {
                    event_data = val.to_string();
                } else {
                    event_data.push('\n');
                    event_data.push_str(val);
                }
            }
        }
    }

    Ok(())
}

fn handle_sse_event(
    event_type: &str,
    data: &str,
    state: &mut SseState,
) -> Vec<anyhow::Result<StreamEvent>> {
    let json: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            warn!(event_type, "failed to parse SSE data: {}", e);
            return vec![];
        }
    };

    let mut out = Vec::new();

    match event_type {
        "message_start" => {
            if let Some(msg) = json.get("message") {
                state.message_id = msg["id"].as_str().unwrap_or("").to_string();
                if let Some(u) = msg.get("usage") {
                    state.input_tokens = u["input_tokens"].as_u64().unwrap_or(0) as usize;
                }
            }
        }

        "content_block_start" => {
            let index = json["index"].as_u64().unwrap_or(0) as usize;
            let block = &json["content_block"];
            let btype = block["type"].as_str().unwrap_or("");

            let content = match btype {
                "text" => ResponseContent::Text(String::new()),
                "tool_use" => {
                    state.tool_json.insert(index, String::new());
                    ResponseContent::ToolUse {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        input: serde_json::Value::Object(serde_json::Map::new()),
                    }
                }
                _ => return out,
            };

            while state.content_blocks.len() <= index {
                state.content_blocks.push(ResponseContent::Text(String::new()));
            }
            state.content_blocks[index] = content.clone();
            out.push(Ok(StreamEvent::ContentBlockStart { index, content }));
        }

        "content_block_delta" => {
            let index = json["index"].as_u64().unwrap_or(0) as usize;
            let delta = &json["delta"];
            let dtype = delta["type"].as_str().unwrap_or("");

            match dtype {
                "text_delta" => {
                    let text = delta["text"].as_str().unwrap_or("").to_string();
                    if let Some(ResponseContent::Text(ref mut t)) = state.content_blocks.get_mut(index) {
                        t.push_str(&text);
                    }
                    out.push(Ok(StreamEvent::TextDelta { index, text }));
                }
                "input_json_delta" => {
                    let partial = delta["partial_json"].as_str().unwrap_or("").to_string();
                    if let Some(accum) = state.tool_json.get_mut(&index) {
                        accum.push_str(&partial);
                    }
                    out.push(Ok(StreamEvent::InputJsonDelta {
                        index,
                        partial_json: partial,
                    }));
                }
                _ => {}
            }
        }

        "content_block_stop" => {
            let index = json["index"].as_u64().unwrap_or(0) as usize;

            // Finalize tool_use input from accumulated JSON
            if let Some(json_str) = state.tool_json.remove(&index) {
                if !json_str.is_empty() {
                    if let Ok(parsed) = serde_json::from_str(&json_str) {
                        if let Some(ResponseContent::ToolUse { ref mut input, .. }) =
                            state.content_blocks.get_mut(index)
                        {
                            *input = parsed;
                        }
                    }
                }
            }

            out.push(Ok(StreamEvent::ContentBlockStop { index }));
        }

        "message_delta" => {
            if let Some(delta) = json.get("delta") {
                if let Some(sr) = delta["stop_reason"].as_str() {
                    state.stop_reason = Some(parse_stop_reason(sr));
                }
            }
            if let Some(u) = json.get("usage") {
                state.output_tokens = u["output_tokens"].as_u64().unwrap_or(0) as usize;
            }
        }

        "message_stop" => {
            let response = ModelResponse {
                id: state.message_id.clone(),
                content: state.content_blocks.clone(),
                stop_reason: state.stop_reason.clone(),
                usage: Usage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    ..Default::default()
                },
            };
            out.push(Ok(StreamEvent::MessageDone { response }));
        }

        "ping" | "error" => {
            if event_type == "error" {
                warn!(data, "anthropic stream error event");
            }
        }

        _ => {
            debug!(event_type, "unhandled SSE event type");
        }
    }

    out
}

fn parse_stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        _ => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// Non-streaming response parsing
// ---------------------------------------------------------------------------

fn parse_complete_response(raw: &serde_json::Value) -> anyhow::Result<ModelResponse> {
    let id = raw["id"].as_str().unwrap_or("").to_string();

    let mut content = Vec::new();
    if let Some(blocks) = raw["content"].as_array() {
        for block in blocks {
            let btype = block["type"].as_str().unwrap_or("");
            match btype {
                "text" => {
                    content.push(ResponseContent::Text(
                        block["text"].as_str().unwrap_or("").to_string(),
                    ));
                }
                "tool_use" => {
                    content.push(ResponseContent::ToolUse {
                        id: block["id"].as_str().unwrap_or("").to_string(),
                        name: block["name"].as_str().unwrap_or("").to_string(),
                        input: block["input"].clone(),
                    });
                }
                _ => {}
            }
        }
    }

    let stop_reason = raw["stop_reason"]
        .as_str()
        .map(parse_stop_reason);

    let usage = Usage {
        input_tokens: raw["usage"]["input_tokens"].as_u64().unwrap_or(0) as usize,
        output_tokens: raw["usage"]["output_tokens"].as_u64().unwrap_or(0) as usize,
        ..Default::default()
    };

    Ok(ModelResponse {
        id,
        content,
        stop_reason,
        usage,
    })
}
