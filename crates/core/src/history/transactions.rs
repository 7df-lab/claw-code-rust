//! Transaction boundaries and lossless prompt reconstruction shared by compaction.

use std::collections::HashSet;

use devo_protocol::{ContentBlock, Message, Role};

use crate::ResponseItem;

/// Move a proposed split back to the start of an outstanding tool batch.
/// Unfinished calls always remain in the preserved suffix.
pub(super) fn preserve_boundary(items: &[ResponseItem], proposed: usize) -> usize {
    let mut pending = HashSet::new();
    let mut batch_start = None;
    let mut boundary = proposed.min(items.len());
    for (index, item) in items.iter().enumerate() {
        match item {
            ResponseItem::ToolCall { id, .. } => {
                batch_start.get_or_insert(index);
                pending.insert(id);
            }
            ResponseItem::ToolCallOutput { tool_use_id, .. } => {
                pending.remove(tool_use_id);
            }
            ResponseItem::Reason { .. } | ResponseItem::Message(_) => {}
        }
        if let Some(start) = batch_start {
            if start < boundary && index >= boundary {
                boundary = start;
            }
            if pending.is_empty() {
                batch_start = None;
            }
        }
    }
    if let Some(start) = batch_start {
        boundary = boundary.min(start);
    }
    boundary
}

/// Reconstitute all response variants, grouping adjacent assistant/tool messages.
/// This conversion never removes an intent merely because its result is pending.
pub fn response_items_to_messages(items: &[ResponseItem]) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();
    for item in items {
        let message = match item {
            ResponseItem::Message(message) => message.clone(),
            ResponseItem::Reason { text } => Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Reasoning { text: text.clone() }],
            },
            ResponseItem::ToolCall { id, name, input } => Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                }],
            },
            ResponseItem::ToolCallOutput {
                tool_use_id,
                content,
                is_error,
            } => Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }],
            },
        };
        if let Some(last) = messages.last_mut()
            && last.role == message.role
            && (message.role == Role::Assistant
                || (last
                    .content
                    .iter()
                    .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
                    && message
                        .content
                        .iter()
                        .all(|block| matches!(block, ContentBlock::ToolResult { .. }))))
        {
            last.content.extend(message.content);
        } else {
            messages.push(message);
        }
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn batch() -> Vec<ResponseItem> {
        vec![
            ResponseItem::Message(Message::user("old")),
            ResponseItem::ToolCall {
                id: "a".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCall {
                id: "b".into(),
                name: "read".into(),
                input: serde_json::json!({}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "b".into(),
                content: "B".into(),
                is_error: false,
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "a".into(),
                content: "A".into(),
                is_error: false,
            },
        ]
    }

    /// Trace: L2-DES-CONTEXT-004
    #[test]
    fn parallel_batch_is_indivisible() {
        let items = batch();
        assert_eq!(
            (0..=5)
                .map(|split| preserve_boundary(&items, split))
                .collect::<Vec<_>>(),
            vec![0, 1, 1, 1, 1, 5]
        );
        assert_eq!(preserve_boundary(&items[..4], 4), 1);
    }

    /// Trace: L2-DES-CONTEXT-004
    #[test]
    fn messages_round_trip_all_tool_items() {
        let items = batch();
        let restored = response_items_to_messages(&items)
            .into_iter()
            .flat_map(crate::response_item::message_to_response_items)
            .collect::<Vec<_>>();
        assert_eq!(restored, items);
    }
}
