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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::transcript::model::CommittedCellModel;
    use crate::transcript::model::ToolPhase;
    use crate::transcript::presentation::tool_title_parts;
    use crate::transcript::projector::TranscriptProjector;
    use devo_core::ItemId;
    use devo_protocol::native::item::ToolSource;

    fn tool_call_item(input: Option<serde_json::Value>) -> Item {
        Item::ToolCall {
            call_id: "bash-1".to_string(),
            tool_name: "bash".to_string(),
            source: ToolSource::Builtin,
            server_name: None,
            input,
        }
    }

    /// A streamed tool call opens with empty parameters, the server then
    /// re-broadcasts `item/started` with the complete input, and the result
    /// closes the row. The running row must render the command as soon as the
    /// refresh lands, and the completed row must keep it.
    #[test]
    fn refreshed_started_updates_running_tool_parameters() {
        let item_id = ItemId::new();
        let mut projector = TranscriptProjector::default();

        for event in started_events(&tool_call_item(Some(serde_json::json!({}))), item_id) {
            projector.apply(event);
        }

        let live = projector.live_tool("bash-1").expect("running tool row");
        assert_eq!(live.phase, ToolPhase::Running);
        assert!(live.command.is_none());

        for event in started_events(
            &tool_call_item(Some(serde_json::json!({ "command": "cargo test" }))),
            item_id,
        ) {
            projector.apply(event);
        }

        let live = projector
            .live_tool("bash-1")
            .expect("refreshed running tool row");
        assert_eq!(
            live.phase,
            ToolPhase::Running,
            "refresh must not flip the running phase"
        );
        assert_eq!(live.command.as_deref(), Some("cargo test"));

        let parts = tool_title_parts(
            live.phase,
            live.tool_name.as_deref(),
            live.input.as_ref(),
            &live.parsed_commands,
            false,
            live.summary.as_str(),
        );
        assert_eq!(parts.verb, "Running");
        assert_eq!(parts.detail, "cargo test");

        for event in completed_events(
            &Item::ToolResult {
                call_id: "bash-1".to_string(),
                output: serde_json::Value::String("ok".to_string()),
                display_content: Some("ok".to_string()),
                is_error: false,
                truncated: false,
            },
            item_id,
        ) {
            projector.apply(event);
        }

        let completed_live = projector.live_tool("bash-1").expect("completed live row");
        assert_eq!(completed_live.phase, ToolPhase::Completed);
        assert!(projector.drain_unsynced_committed().is_empty());
        projector.apply(ItemLifecycleEvent::TurnLiveToolsCleared {
            outcome: crate::transcript::lifecycle::TurnToolOutcome::Completed,
        });
        let committed = projector.drain_unsynced_committed();
        let CommittedCellModel::Tool(tool) = &committed[0] else {
            panic!("expected committed tool cell");
        };
        assert_eq!(tool.phase, ToolPhase::Completed);
        let parts = tool_title_parts(
            tool.phase,
            tool.tool_name.as_deref(),
            tool.input.as_ref(),
            &tool.parsed_commands,
            false,
            tool.summary.as_str(),
        );
        assert_eq!(parts.verb, "Ran");
        assert_eq!(parts.detail, "cargo test");
    }
}
