//! Native `Item` → [`ItemLifecycleEvent`] projection (P0: no legacy `ItemKind` shim).

use devo_core::ItemId;
use devo_protocol::ToolCallPayload;
use devo_protocol::ToolResultPayload;
use devo_protocol::native::item::Item;

use crate::events::TextItemKind;
use crate::transcript::lifecycle::ItemLifecycleEvent;

use super::tool_lifecycle::{
    native_file_changes, tool_closed_from_command, tool_closed_from_file_change,
    tool_closed_from_result, tool_opened_from_call, tool_opened_from_command,
    tool_opened_refresh_from_call,
};

/// Projects a native item `item/started` notification into lifecycle events.
pub(crate) fn started_events(item: &Item, item_id: ItemId) -> Vec<ItemLifecycleEvent> {
    match item {
        Item::AssistantMessage { .. } => vec![ItemLifecycleEvent::TextStarted {
            item_id,
            kind: TextItemKind::Assistant,
        }],
        Item::Reasoning { .. } => vec![ItemLifecycleEvent::TextStarted {
            item_id,
            kind: TextItemKind::Reasoning,
        }],
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => {
            let payload = ToolCallPayload {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                parameters: input.clone().unwrap_or(serde_json::Value::Null),
                command_actions: Vec::new(),
            };
            vec![tool_opened_from_call(&payload)]
        }
        Item::CommandExecution {
            call_id,
            command,
            input,
            origin,
            ..
        } => vec![tool_opened_from_command(
            call_id.clone(),
            command.clone(),
            input.clone(),
            *origin,
            Vec::new(),
        )],
        _ => Vec::new(),
    }
}

/// Projects a native item `item/completed` notification into lifecycle events.
pub(crate) fn completed_events(item: &Item, item_id: ItemId) -> Vec<ItemLifecycleEvent> {
    match item {
        Item::AssistantMessage { text, .. } => {
            let final_text = text.trim().to_string();
            if final_text.is_empty() {
                Vec::new()
            } else {
                vec![ItemLifecycleEvent::TextCompleted {
                    item_id,
                    kind: TextItemKind::Assistant,
                    final_text,
                }]
            }
        }
        Item::Reasoning { text, .. } => {
            let final_text = text.trim().to_string();
            if final_text.is_empty() {
                Vec::new()
            } else {
                vec![ItemLifecycleEvent::TextCompleted {
                    item_id,
                    kind: TextItemKind::Reasoning,
                    final_text,
                }]
            }
        }
        Item::ToolCall {
            call_id,
            tool_name,
            input,
            ..
        } => {
            let payload = ToolCallPayload {
                tool_call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                parameters: input.clone().unwrap_or(serde_json::Value::Null),
                command_actions: Vec::new(),
            };
            vec![tool_opened_refresh_from_call(&payload)]
        }
        Item::FileChange {
            call_id, changes, ..
        } => vec![tool_closed_from_file_change(
            call_id.clone(),
            None,
            None,
            native_file_changes(changes),
        )],
        Item::ToolResult {
            call_id,
            output,
            display_content,
            is_error,
            truncated,
            ..
        } => {
            let payload = ToolResultPayload {
                tool_call_id: call_id.clone(),
                tool_name: None,
                input: None,
                content: output.clone(),
                display_content: display_content.clone(),
                is_error: *is_error,
                summary: String::new(),
            };
            let mut event = tool_closed_from_result(&payload);
            if let ItemLifecycleEvent::ToolClosed {
                truncated: truncated_slot,
                ..
            } = &mut event
            {
                *truncated_slot = *truncated;
            }
            vec![event]
        }
        Item::CommandExecution {
            call_id,
            command,
            input,
            output,
            is_error,
            ..
        } => vec![tool_closed_from_command(
            call_id.clone(),
            command.clone(),
            input.clone(),
            output.clone(),
            *is_error,
        )],
        _ => Vec::new(),
    }
}
