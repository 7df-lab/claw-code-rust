//! Normalization utilities for `ResponseItem` sequences.
//!
//! This module provides:
//! - ToolCall / ToolCallOutput pairing: when an item is removed, its
//!   counterpart is also removed.
//! - Modality-based filtering: items whose content types are not supported
//!   by the model are removed.
//! - Reason-item stripping: `Reason` items can be removed before compaction.

use std::collections::HashSet;

use devo_protocol::InputModality;

use crate::response_item::ResponseItem;

/// Removes items at `index` and also removes the paired counterpart
/// (ToolCall ↔ ToolCallOutput) if one exists.
///
/// When the removed item is a `ToolCall`, the corresponding
/// `ToolCallOutput` (matched by `id` ↔ `tool_use_id`) is also
/// removed. Similarly, when the removed item is a `ToolCallOutput`,
/// the corresponding `ToolCall` is removed.
///
/// Returns the removed item(s). The primary item is always first;
/// the paired item, if found and removed, is second.
pub fn remove_paired(items: &mut Vec<ResponseItem>, index: usize) -> Vec<ResponseItem> {
    if index >= items.len() {
        return Vec::new();
    }

    let removed = items.remove(index);

    // Try to find and remove the paired item.
    let paired_idx = find_pair_index(items, &removed);

    let mut result = vec![removed];
    if let Some(pidx) = paired_idx {
        result.push(items.remove(pidx));
    }

    result
}

/// Finds the index of the paired item for the given removed item.
fn find_pair_index(items: &[ResponseItem], removed: &ResponseItem) -> Option<usize> {
    match removed {
        ResponseItem::ToolCall { id, .. } => items.iter().position(|item| match item {
            ResponseItem::ToolCallOutput { tool_use_id, .. } => tool_use_id == id,
            _ => false,
        }),
        ResponseItem::ToolCallOutput { tool_use_id, .. } => {
            items.iter().position(|item| match item {
                ResponseItem::ToolCall { id, .. } => id == tool_use_id,
                _ => false,
            })
        }
        _ => None,
    }
}

/// Filters a slice of `ResponseItem`s, keeping only content types that match
/// the model's supported modalities.
///
/// For `Message` items, content blocks whose types are not in the supported
/// modalities are removed. If a `Message` becomes empty after filtering, the
/// entire message is removed.
///
/// Currently supported modalities:
/// - `InputModality::Text` — keeps `Text` content blocks
/// - `InputModality::Image` — keeps `Image` content blocks (when added)
///
/// `Reason`, `ToolCall`, and `ToolCallOutput` items are always preserved
/// regardless of modality.
pub fn filter_by_modality(
    items: &[ResponseItem],
    modalities: &[InputModality],
) -> Vec<ResponseItem> {
    let supports_text = modalities.contains(&InputModality::Text);
    if supports_text && text_modality_keeps_all_items(items) {
        return items.to_vec();
    }

    items
        .iter()
        .filter_map(|item| match item {
            ResponseItem::Message(msg) => {
                // Filter content blocks within the message based on modality.
                let filtered_content: Vec<_> = msg
                    .content
                    .iter()
                    .filter(|block| match block {
                        devo_protocol::ContentBlock::Text { .. } => supports_text,
                        devo_protocol::ContentBlock::Reasoning { .. } => supports_text,
                        devo_protocol::ContentBlock::ProviderReasoning { .. } => true,
                        devo_protocol::ContentBlock::ToolUse { .. } => true,
                        devo_protocol::ContentBlock::HostedToolUse { .. } => true,
                        devo_protocol::ContentBlock::ToolResult { .. } => true,
                    })
                    .cloned()
                    .collect();

                if filtered_content.is_empty() {
                    None // Remove empty messages
                } else {
                    Some(ResponseItem::Message(devo_protocol::Message {
                        role: msg.role,
                        content: filtered_content,
                    }))
                }
            }
            // Non-message items are preserved as-is.
            other => Some(other.clone()),
        })
        .collect()
}

pub(crate) fn text_modality_keeps_all_items(items: &[ResponseItem]) -> bool {
    items.iter().all(|item| match item {
        ResponseItem::Message(msg) => msg.content.iter().all(|block| match block {
            devo_protocol::ContentBlock::Text { .. }
            | devo_protocol::ContentBlock::Reasoning { .. }
            | devo_protocol::ContentBlock::ProviderReasoning { .. }
            | devo_protocol::ContentBlock::ToolUse { .. }
            | devo_protocol::ContentBlock::HostedToolUse { .. }
            | devo_protocol::ContentBlock::ToolResult { .. } => true,
        }),
        ResponseItem::Reason { .. }
        | ResponseItem::ToolCall { .. }
        | ResponseItem::ToolCallOutput { .. } => true,
    })
}

/// Ensures tool-call / tool-call-output items are properly paired.
///
/// Any `ToolCall` without a matching `ToolCallOutput` (and vice versa)
/// is removed from the sequence. This operates on a **mutable** slice
/// since it is typically called before prompt building.
pub fn pair_tool_call_items(items: &mut Vec<ResponseItem>) {
    let mut tool_call_ids = None::<HashSet<&str>>;
    let mut tool_output_ids = None::<HashSet<&str>>;
    for item in items.iter() {
        match item {
            ResponseItem::ToolCall { id, .. } => {
                tool_call_ids
                    .get_or_insert_with(|| HashSet::with_capacity(items.len() / 2))
                    .insert(id.as_str());
            }
            ResponseItem::ToolCallOutput { tool_use_id, .. } => {
                tool_output_ids
                    .get_or_insert_with(|| HashSet::with_capacity(items.len() / 2))
                    .insert(tool_use_id.as_str());
            }
            _ => {}
        }
    }
    let Some(tool_call_ids) = tool_call_ids else {
        let has_tool_outputs = tool_output_ids.is_some();
        drop(tool_output_ids);
        if has_tool_outputs {
            items.retain(|item| !matches!(item, ResponseItem::ToolCallOutput { .. }));
        }
        return;
    };
    let Some(tool_output_ids) = tool_output_ids else {
        drop(tool_call_ids);
        items.retain(|item| !matches!(item, ResponseItem::ToolCall { .. }));
        return;
    };

    if items
        .iter()
        .all(|item| item_has_required_tool_pair(item, &tool_call_ids, &tool_output_ids))
    {
        return;
    }

    // Compute keep decisions while the ID sets can borrow from `items`, then
    // drop those borrows before mutating the vector with `retain`.
    let retain_flags = items
        .iter()
        .map(|item| item_has_required_tool_pair(item, &tool_call_ids, &tool_output_ids))
        .collect::<Vec<_>>();
    drop(tool_call_ids);
    drop(tool_output_ids);

    let mut index = 0;
    items.retain(|_| {
        let keep = retain_flags[index];
        index += 1;
        keep
    });
}

fn item_has_required_tool_pair(
    item: &ResponseItem,
    tool_call_ids: &HashSet<&str>,
    tool_output_ids: &HashSet<&str>,
) -> bool {
    match item {
        ResponseItem::ToolCall { id, .. } => tool_output_ids.contains(id.as_str()),
        ResponseItem::ToolCallOutput { tool_use_id, .. } => {
            tool_call_ids.contains(tool_use_id.as_str())
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::response_item::ResponseItem;
    use devo_protocol::Message;

    #[test]
    fn remove_paired_removes_tool_call_and_output() {
        let mut items = vec![
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::ToolCallOutput {
                tool_use_id: "tc-1".into(),
                content: "ok".into(),
                is_error: false,
            },
        ];

        let removed = remove_paired(&mut items, 0);
        assert_eq!(removed.len(), 2);
        assert!(removed[0].is_tool_call());
        assert!(removed[1].is_tool_call_output());
        assert_eq!(items.len(), 1);
        assert!(items[0].is_message());
    }

    #[test]
    fn remove_paired_removes_output_and_tool_call() {
        let mut items = vec![
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "tc-1".into(),
                content: "ok".into(),
                is_error: false,
            },
        ];

        let removed = remove_paired(&mut items, 2);
        assert_eq!(removed.len(), 2);
        assert!(removed[0].is_tool_call_output());
        assert!(removed[1].is_tool_call());
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn remove_paired_no_pair_for_message() {
        let mut items = vec![
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::Message(Message::assistant_text("world")),
        ];

        let removed = remove_paired(&mut items, 0);
        assert_eq!(removed.len(), 1);
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn remove_paired_out_of_bounds() {
        let mut items: Vec<ResponseItem> = Vec::new();
        let removed = remove_paired(&mut items, 0);
        assert!(removed.is_empty());
    }

    #[test]
    fn filter_by_modality_keeps_text() {
        let items = vec![
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::Message(Message::assistant_text("world")),
        ];

        let filtered = filter_by_modality(&items, &[InputModality::Text]);
        assert_eq!(filtered, items);
    }

    #[test]
    fn filter_by_modality_keeps_all_non_message() {
        let items = vec![
            ResponseItem::Reason {
                text: "thinking".into(),
            },
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
        ];

        let filtered = filter_by_modality(&items, &[InputModality::Text]);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn pair_tool_call_items_removes_orphan_tool_call() {
        let mut items = vec![
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            ResponseItem::Message(Message::user("hello")),
        ];

        pair_tool_call_items(&mut items);
        assert_eq!(items.len(), 1);
        assert!(items[0].is_message());
    }

    #[test]
    fn pair_tool_call_items_removes_orphan_output() {
        let mut items = vec![
            ResponseItem::Message(Message::user("hello")),
            ResponseItem::ToolCallOutput {
                tool_use_id: "tc-1".into(),
                content: "ok".into(),
                is_error: false,
            },
        ];

        pair_tool_call_items(&mut items);
        assert_eq!(items.len(), 1);
        assert!(items[0].is_message());
    }

    #[test]
    fn pair_tool_call_items_keeps_paired() {
        let mut items = vec![
            ResponseItem::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            ResponseItem::ToolCallOutput {
                tool_use_id: "tc-1".into(),
                content: "ok".into(),
                is_error: false,
            },
        ];

        pair_tool_call_items(&mut items);
        assert_eq!(items.len(), 2);
    }

    #[test]
    #[ignore]
    fn bench_pair_tool_call_items_without_tools() {
        let mut items = (0..2_000)
            .map(|index| ResponseItem::Message(Message::user(format!("plain message {index}"))))
            .collect::<Vec<_>>();

        let started = Instant::now();
        let mut total_items = 0;
        for _ in 0..10_000 {
            pair_tool_call_items(black_box(&mut items));
            total_items += black_box(items.len());
        }
        let elapsed = started.elapsed();

        assert_eq!(total_items, 20_000_000);
        println!(
            "pair_tool_call_items_without_tools iterations=10000 items=2000 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 10_000.0
        );
    }

    #[test]
    #[ignore]
    fn bench_pair_tool_call_items_with_paired_tools() {
        let template = (0..500)
            .flat_map(|index| {
                [
                    ResponseItem::Message(Message::assistant_text(format!("message {index}"))),
                    ResponseItem::ToolCall {
                        id: format!("tc-{index}"),
                        name: "bash".into(),
                        input: serde_json::json!({ "cmd": "date" }),
                    },
                    ResponseItem::ToolCallOutput {
                        tool_use_id: format!("tc-{index}"),
                        content: "ok".into(),
                        is_error: false,
                    },
                ]
            })
            .collect::<Vec<_>>();

        let started = Instant::now();
        let mut total_items = 0;
        for _ in 0..2_000 {
            let mut items = black_box(template.clone());
            pair_tool_call_items(black_box(&mut items));
            total_items += black_box(items.len());
        }
        let elapsed = started.elapsed();

        assert_eq!(total_items, 3_000_000);
        println!(
            "pair_tool_call_items_with_paired_tools iterations=2000 items=1500 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 2_000.0
        );
    }

    #[test]
    #[ignore]
    fn bench_filter_by_modality_text_preserves_messages() {
        let items = (0..2_000)
            .map(|index| {
                ResponseItem::Message(Message {
                    role: devo_protocol::Role::Assistant,
                    content: vec![
                        devo_protocol::ContentBlock::Text {
                            text: format!("assistant text {index}"),
                        },
                        devo_protocol::ContentBlock::Reasoning {
                            text: format!("reasoning {index}"),
                        },
                    ],
                })
            })
            .collect::<Vec<_>>();

        let started = Instant::now();
        let mut total_items = 0;
        for _ in 0..5_000 {
            total_items += black_box(filter_by_modality(
                black_box(&items),
                black_box(&[InputModality::Text]),
            ))
            .len();
        }
        let elapsed = started.elapsed();

        assert_eq!(total_items, 10_000_000);
        println!(
            "filter_by_modality_text_preserves_messages iterations=5000 items=2000 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / 5_000.0
        );
    }
}
