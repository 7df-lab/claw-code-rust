use serde::{Deserialize, Serialize};

use devo_protocol::{ContentBlock, Message, RequestContent, RequestMessage, Role};

/// Unified representation of LLM conversation items.
///
/// This is the core IR that the history management system operates on,
/// bridging provider-agnostic protocol types with normalization and
/// compaction workflows.
///
/// # Why `ResponseItem` is separate from [`Message`]
///
/// [`Message`] is the session/protocol shape: one role turn that may bundle
/// several [`ContentBlock`]s (text, reasoning, tool use, tool result) together.
/// That is the natural unit for "append what the assistant just said."
///
/// [`ResponseItem`] is a flatter working representation for history algorithms
/// that need to operate on atomic records—pairing tool calls with outputs,
/// dropping reasoning before summarization, estimating tokens per item, or
/// trimming a turn from the tail. Mixed-content messages are therefore split
/// by [`message_to_response_items`]; see that function for a concrete example.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResponseItem {
    /// Model reasoning / thinking output.
    /// Some models produce this, others do not.
    Reason { text: String },
    /// A user-sent message or model reply, containing text (and image in future).
    Message(Message),
    /// A model tool-call request.
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// The result/output of a tool call.
    ToolCallOutput {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

impl ResponseItem {
    /// Returns `true` if this item is a `Reason` variant.
    pub fn is_reason(&self) -> bool {
        matches!(self, Self::Reason { .. })
    }

    /// Returns `true` if this item is a `Message` variant.
    pub fn is_message(&self) -> bool {
        matches!(self, Self::Message(..))
    }

    /// Returns `true` if this item is a `ToolCall` variant.
    pub fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall { .. })
    }

    /// Returns the tool-use id if this is a `ToolCall`.
    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            Self::ToolCall { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }

    /// Returns `true` if this item is a `ToolCallOutput` variant.
    pub fn is_tool_call_output(&self) -> bool {
        matches!(self, Self::ToolCallOutput { .. })
    }

    /// Returns the tool-use id if this is a `ToolCallOutput`.
    pub fn tool_call_output_id(&self) -> Option<&str> {
        match self {
            Self::ToolCallOutput { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions: from ContentBlock -> ResponseItem (partial)
// ---------------------------------------------------------------------------

impl From<ContentBlock> for ResponseItem {
    fn from(block: ContentBlock) -> Self {
        match block {
            ContentBlock::Text { text } => Self::Message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text }],
            }),
            ContentBlock::ProviderReasoning { provider, payload } => Self::Message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ProviderReasoning { provider, payload }],
            }),
            ContentBlock::Reasoning { text } => Self::Reason { text },
            ContentBlock::ToolUse { id, name, input } => Self::ToolCall { id, name, input },
            ContentBlock::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => Self::Message(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::HostedToolUse {
                    id,
                    name,
                    input,
                    output,
                    status,
                }],
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Self::ToolCallOutput {
                tool_use_id,
                content,
                is_error,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Conversions: from ResponseItem -> RequestMessage (for LLM prompt building)
// ---------------------------------------------------------------------------

impl From<ResponseItem> for RequestMessage {
    fn from(item: ResponseItem) -> Self {
        match item {
            ResponseItem::Reason { text } => RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::Reasoning { text }],
            },
            ResponseItem::Message(msg) => msg.to_request_message(),
            ResponseItem::ToolCall { id, name, input } => RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::ToolUse { id, name, input }],
            },
            ResponseItem::ToolCallOutput {
                tool_use_id,
                content,
                is_error,
            } => RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::ToolResult {
                    tool_use_id,
                    content,
                    is_error: if is_error { Some(true) } else { None },
                }],
            },
        }
    }
}

impl From<&ResponseItem> for RequestMessage {
    fn from(item: &ResponseItem) -> Self {
        match item {
            ResponseItem::Reason { text } => RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::Reasoning { text: text.clone() }],
            },
            ResponseItem::Message(msg) => msg.to_request_message(),
            ResponseItem::ToolCall { id, name, input } => RequestMessage {
                role: Role::Assistant.as_str().to_string(),
                content: vec![RequestContent::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }],
            },
            ResponseItem::ToolCallOutput {
                tool_use_id,
                content,
                is_error,
            } => RequestMessage {
                role: Role::User.as_str().to_string(),
                content: vec![RequestContent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: if *is_error { Some(true) } else { None },
                }],
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Converting a full Message into ResponseItem(s)
// ---------------------------------------------------------------------------

/// Converts a `Message` into one or more `ResponseItem`s.
///
/// A `Message` can contain multiple content blocks of different types.
/// This split representation is useful for normalization (e.g. pairing
/// tool calls with their outputs) and modality-based filtering.
///
/// # Example: splitting a mixed assistant turn
///
/// Session storage keeps one assistant [`Message`] for a single model turn:
///
/// ```text
/// Message {
///   role: Assistant,
///   content: [
///     Reasoning { "hmm" },
///     Text      { "hello" },
///     ToolUse   { id: "tu-1", name: "bash", input: { "cmd": "ls" } },
///   ]
/// }
/// ```
///
/// History algorithms need each of those blocks as a standalone record, so
/// this function yields three items:
///
/// ```text
/// [
///   ResponseItem::Reason   { text: "hmm" },
///   ResponseItem::Message  { role: Assistant, content: [Text("hello")] },
///   ResponseItem::ToolCall { id: "tu-1", name: "bash", input: { "cmd": "ls" } },
/// ]
/// ```
///
/// A following user message that only carries `ToolResult { tool_use_id: "tu-1", ... }`
/// becomes `ResponseItem::ToolCallOutput { tool_use_id: "tu-1", ... }`. The history
/// is then a flat sequence where tool calls and outputs are adjacent and easy to
/// pair, filter, or drop independently of the original message boundaries.
///
/// When converting back to provider `RequestMessage`s, consecutive assistant
/// fragments produced by this split are merged again (see
/// `merge_consecutive_assistant_messages` in `history`) so the wire format still
/// satisfies provider adjacency rules.
pub fn message_to_response_items(msg: Message) -> Vec<ResponseItem> {
    let role = msg.role;
    let content = msg.content;
    if matches!(
        content.as_slice(),
        [ContentBlock::Text { .. }
            | ContentBlock::ProviderReasoning { .. }
            | ContentBlock::HostedToolUse { .. }]
    ) {
        return vec![ResponseItem::Message(Message { role, content })];
    }

    let mut items = Vec::with_capacity(content.len());
    for block in content {
        match block {
            ContentBlock::Text { text } => {
                // Preserve the historical split shape for mixed-content messages:
                // each text-like block becomes its own message item so tool calls
                // can still be reasoned about as adjacent standalone records.
                items.push(ResponseItem::Message(Message {
                    role,
                    content: vec![ContentBlock::Text { text }],
                }));
            }
            ContentBlock::Reasoning { text } => {
                items.push(ResponseItem::Reason { text });
            }
            ContentBlock::ProviderReasoning { provider, payload } => {
                items.push(ResponseItem::Message(Message {
                    role,
                    content: vec![ContentBlock::ProviderReasoning { provider, payload }],
                }));
            }
            ContentBlock::ToolUse { id, name, input } => {
                items.push(ResponseItem::ToolCall { id, name, input });
            }
            ContentBlock::HostedToolUse {
                id,
                name,
                input,
                output,
                status,
            } => {
                items.push(ResponseItem::Message(Message {
                    role,
                    content: vec![ContentBlock::HostedToolUse {
                        id,
                        name,
                        input,
                        output,
                        status,
                    }],
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                items.push(ResponseItem::ToolCallOutput {
                    tool_use_id,
                    content,
                    is_error,
                });
            }
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::hint::black_box;
    use std::time::Instant;

    use super::*;
    use devo_protocol::{ContentBlock, Message, Role};

    #[test]
    fn response_item_reason_variant() {
        let item = ResponseItem::Reason {
            text: "thinking".into(),
        };
        assert!(item.is_reason());
        assert!(!item.is_message());
        assert!(!item.is_tool_call());
        assert!(!item.is_tool_call_output());
    }

    #[test]
    fn response_item_message_variant() {
        let msg = Message::user("hello");
        let item = ResponseItem::Message(msg);
        assert!(item.is_message());
        assert!(!item.is_reason());
    }

    #[test]
    fn response_item_tool_call_variant() {
        let item = ResponseItem::ToolCall {
            id: "call-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        };
        assert!(item.is_tool_call());
        assert_eq!(item.tool_call_id(), Some("call-1"));
    }

    #[test]
    fn response_item_tool_call_output_variant() {
        let item = ResponseItem::ToolCallOutput {
            tool_use_id: "call-1".into(),
            content: "ok".into(),
            is_error: false,
        };
        assert!(item.is_tool_call_output());
        assert_eq!(item.tool_call_output_id(), Some("call-1"));
    }

    #[test]
    fn from_content_block_reasoning() {
        let block = ContentBlock::Reasoning {
            text: "deep thought".into(),
        };
        let item: ResponseItem = block.into();
        assert!(item.is_reason());
    }

    #[test]
    fn from_content_block_tool_use() {
        let block = ContentBlock::ToolUse {
            id: "tu-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "pwd"}),
        };
        let item: ResponseItem = block.into();
        assert!(item.is_tool_call());
        assert_eq!(item.tool_call_id(), Some("tu-1"));
    }

    #[test]
    fn from_content_block_tool_result() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "tu-1".into(),
            content: "result".into(),
            is_error: false,
        };
        let item: ResponseItem = block.into();
        assert!(item.is_tool_call_output());
        assert_eq!(item.tool_call_output_id(), Some("tu-1"));
    }

    #[test]
    fn response_item_to_request_message_reason() {
        let item = ResponseItem::Reason {
            text: "thinking".into(),
        };
        let req: RequestMessage = item.into();
        assert_eq!(req.role, "assistant");
        assert_eq!(req.content.len(), 1);
    }

    #[test]
    fn response_item_to_request_message_message() {
        let item = ResponseItem::Message(Message::user("hello"));
        let req: RequestMessage = item.into();
        assert_eq!(req.role, "user");
    }

    #[test]
    fn response_item_to_request_message_tool_call() {
        let item = ResponseItem::ToolCall {
            id: "tc-1".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "ls"}),
        };
        let req: RequestMessage = item.into();
        assert_eq!(req.role, "assistant");
    }

    #[test]
    fn response_item_to_request_message_tool_output() {
        let item = ResponseItem::ToolCallOutput {
            tool_use_id: "tc-1".into(),
            content: "done".into(),
            is_error: false,
        };
        let req: RequestMessage = item.into();
        assert_eq!(
            serde_json::to_value(req).expect("request message should serialize"),
            serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "tc-1",
                    "content": "done",
                }],
            })
        );
    }

    #[test]
    fn response_item_borrowed_and_owned_request_messages_match() {
        let items = vec![
            ResponseItem::Reason {
                text: "thinking".into(),
            },
            ResponseItem::Message(Message::assistant_text("hello")),
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "tc-1".into(),
                content: "done".into(),
                is_error: true,
            },
        ];

        let owned = items
            .clone()
            .into_iter()
            .map(RequestMessage::from)
            .map(|message| serde_json::to_value(message).expect("request message should serialize"))
            .collect::<Vec<_>>();
        let borrowed = items
            .iter()
            .map(RequestMessage::from)
            .map(|message| serde_json::to_value(message).expect("request message should serialize"))
            .collect::<Vec<_>>();

        assert_eq!(owned, borrowed);
    }

    #[test]
    fn message_to_response_items_splits_mixed_content() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning { text: "hmm".into() },
                ContentBlock::Text {
                    text: "hello".into(),
                },
                ContentBlock::ToolUse {
                    id: "tu-1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"cmd": "ls"}),
                },
            ],
        };

        let items = message_to_response_items(msg);
        assert_eq!(items.len(), 3);
        assert!(items[0].is_reason());
        assert!(items[1].is_message());
        assert!(items[2].is_tool_call());
    }

    #[test]
    fn response_item_serde_roundtrip() {
        let items = vec![
            ResponseItem::Reason {
                text: "thinking".into(),
            },
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "tc-1".into(),
                content: "done".into(),
                is_error: false,
            },
        ];

        for item in &items {
            let json = serde_json::to_string(item).expect("serialize");
            let restored: ResponseItem = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(*item, restored);
        }
    }

    #[test]
    #[ignore]
    fn bench_message_to_response_items_many_blocks() {
        let content = (0..1_000)
            .flat_map(|index| {
                [
                    ContentBlock::Reasoning {
                        text: format!("thinking {index}"),
                    },
                    ContentBlock::Text {
                        text: format!("assistant text {index}"),
                    },
                    ContentBlock::ToolUse {
                        id: format!("tool-{index}"),
                        name: "bash".to_string(),
                        input: serde_json::json!({"cmd": format!("echo {index}")}),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id: format!("tool-{index}"),
                        content: format!("output {index}"),
                        is_error: false,
                    },
                ]
            })
            .collect::<Vec<_>>();
        let expected_len = content.len();
        let iterations = 200;
        let messages = (0..iterations)
            .map(|_| Message {
                role: Role::Assistant,
                content: content.clone(),
            })
            .collect::<Vec<_>>();
        let started = Instant::now();
        let mut total_items = 0usize;

        for message in messages {
            let items = message_to_response_items(black_box(message));
            total_items += items.len();
            black_box(items);
        }

        let elapsed = started.elapsed();
        assert_eq!(total_items, expected_len * iterations);
        println!(
            "message_to_response_items_many_blocks iterations={iterations} blocks={expected_len} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
