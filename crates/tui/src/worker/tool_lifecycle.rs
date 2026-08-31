//! Builds fact-only [`ItemLifecycleEvent`] values from tool payloads.

use std::collections::HashMap;
use std::path::PathBuf;

use devo_protocol::ToolCallPayload;
use devo_protocol::ToolResultPayload;
use devo_protocol::native::item::ExecOrigin;
use devo_protocol::native::item::FileChangeEntry;
use devo_protocol::native::item::FileChangeKind;
use devo_protocol::protocol::ExecCommandSource;
use devo_protocol::protocol::FileChange;

use crate::transcript::lifecycle::ItemLifecycleEvent;
use crate::transcript::tool_state::command_source_from_tool_name;
use crate::transcript::tool_state::shell_command_from_input;

use super::tool_summaries::tool_call_started_actions;
use super::tool_summaries::tool_call_updated_actions;

pub(crate) fn tool_opened_from_call(payload: &ToolCallPayload) -> ItemLifecycleEvent {
    tool_opened_from_call_with_item_seq(payload, None)
}

pub(crate) fn tool_opened_from_call_with_item_seq(
    payload: &ToolCallPayload,
    item_seq: Option<u64>,
) -> ItemLifecycleEvent {
    ItemLifecycleEvent::ToolOpened {
        tool_use_id: payload.tool_call_id.clone(),
        tool_name: payload.tool_name.clone(),
        input: payload.parameters.clone(),
        item_seq,
        command: shell_command_from_input(&payload.parameters),
        command_source: command_source_from_tool_name(&payload.tool_name),
        parsed_commands: tool_call_started_actions(payload),
    }
}

pub(crate) fn tool_opened_refresh_from_call(payload: &ToolCallPayload) -> ItemLifecycleEvent {
    let summary = super::tool_summaries::summarize_tool_call_update(payload);
    ItemLifecycleEvent::ToolOpened {
        tool_use_id: payload.tool_call_id.clone(),
        tool_name: payload.tool_name.clone(),
        input: payload.parameters.clone(),
        item_seq: None,
        command: shell_command_from_input(&payload.parameters),
        command_source: command_source_from_tool_name(&payload.tool_name),
        parsed_commands: tool_call_updated_actions(payload, &summary),
    }
}

pub(crate) fn tool_closed_from_result(payload: &ToolResultPayload) -> ItemLifecycleEvent {
    ItemLifecycleEvent::ToolClosed {
        tool_use_id: payload.tool_call_id.clone(),
        tool_name: payload
            .tool_name
            .clone()
            .unwrap_or_else(|| "tool".to_string()),
        input: payload.input.clone().unwrap_or(serde_json::Value::Null),
        output: Some(payload.content.clone()),
        display_content: payload.display_content.clone(),
        file_changes: None,
        is_error: payload.is_error,
        truncated: false,
    }
}

pub(crate) fn tool_opened_from_command_source(
    call_id: String,
    command: String,
    input: Option<serde_json::Value>,
    source: ExecCommandSource,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
) -> ItemLifecycleEvent {
    tool_opened_from_command_source_with_item_seq(
        call_id,
        command,
        input,
        source,
        command_actions,
        None,
    )
}

pub(crate) fn tool_opened_from_command_source_with_item_seq(
    call_id: String,
    command: String,
    input: Option<serde_json::Value>,
    source: ExecCommandSource,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
    item_seq: Option<u64>,
) -> ItemLifecycleEvent {
    let input = input.unwrap_or_else(|| serde_json::json!({ "command": command }));
    ItemLifecycleEvent::ToolOpened {
        tool_use_id: call_id,
        tool_name: "exec_command".to_string(),
        input,
        item_seq,
        command: Some(command),
        command_source: Some(source),
        parsed_commands: command_actions,
    }
}

pub(crate) fn tool_opened_from_command(
    call_id: String,
    command: String,
    input: Option<serde_json::Value>,
    origin: ExecOrigin,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
) -> ItemLifecycleEvent {
    tool_opened_from_command_with_item_seq(call_id, command, input, origin, command_actions, None)
}

pub(crate) fn tool_opened_from_command_with_item_seq(
    call_id: String,
    command: String,
    input: Option<serde_json::Value>,
    origin: ExecOrigin,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
    item_seq: Option<u64>,
) -> ItemLifecycleEvent {
    let source = match origin {
        ExecOrigin::AgentTool => ExecCommandSource::Agent,
        ExecOrigin::UserShell => ExecCommandSource::UserShell,
    };
    tool_opened_from_command_source_with_item_seq(
        call_id,
        command,
        input,
        source,
        command_actions,
        item_seq,
    )
}

pub(crate) fn tool_closed_from_file_change(
    call_id: String,
    tool_name: Option<String>,
    input: Option<serde_json::Value>,
    changes: HashMap<PathBuf, FileChange>,
) -> ItemLifecycleEvent {
    ItemLifecycleEvent::ToolClosed {
        tool_use_id: call_id,
        tool_name: tool_name.unwrap_or_else(|| "apply_patch".to_string()),
        input: input.unwrap_or(serde_json::Value::Null),
        output: None,
        display_content: None,
        file_changes: Some(changes),
        is_error: false,
        truncated: false,
    }
}

pub(crate) fn tool_closed_from_command(
    call_id: String,
    command: String,
    input: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    is_error: bool,
) -> ItemLifecycleEvent {
    let display_content = output.as_ref().map(|value| value.to_string());
    ItemLifecycleEvent::ToolClosed {
        tool_use_id: call_id,
        tool_name: "exec_command".to_string(),
        input: input.unwrap_or_else(|| serde_json::json!({ "command": command })),
        output,
        display_content,
        file_changes: None,
        is_error,
        truncated: false,
    }
}

pub(crate) fn native_file_changes(changes: &[FileChangeEntry]) -> HashMap<PathBuf, FileChange> {
    changes
        .iter()
        .map(|entry| {
            let change = match &entry.change {
                FileChangeKind::Add { content } => FileChange::Add {
                    content: content.clone(),
                },
                FileChangeKind::Delete { content } => FileChange::Delete {
                    content: content.clone(),
                },
                FileChangeKind::Update {
                    unified_diff,
                    move_path,
                } => FileChange::Update {
                    unified_diff: unified_diff.clone(),
                    old_text: None,
                    new_text: None,
                    move_path: move_path.clone(),
                },
            };
            (entry.path.clone(), change)
        })
        .collect()
}

pub(crate) fn tool_closed_shell(
    tool_use_id: String,
    command: String,
    output: Option<String>,
    is_error: bool,
) -> ItemLifecycleEvent {
    let display = output.clone().unwrap_or_default();
    ItemLifecycleEvent::ToolClosed {
        tool_use_id,
        tool_name: "shell_command".to_string(),
        input: serde_json::json!({ "command": command }),
        output: output.map(serde_json::Value::String),
        display_content: Some(display),
        file_changes: None,
        is_error,
        truncated: false,
    }
}

pub(crate) fn transcript_tool_input_chunk(
    tool_use_id: String,
    chunk: String,
) -> ItemLifecycleEvent {
    ItemLifecycleEvent::ToolInputChunk { tool_use_id, chunk }
}

/// Parses the JSON payload embedded in an `item/toolCall/inputDelta` notification.
pub(crate) fn transcript_tool_input_chunk_from_delta_payload(
    delta_str: &str,
) -> Option<ItemLifecycleEvent> {
    let value = serde_json::from_str::<serde_json::Value>(delta_str).ok()?;
    let tool_use_id = value.get("tool_use_id")?.as_str()?.to_string();
    let partial_json = value.get("partial_json")?.as_str()?.to_string();
    Some(transcript_tool_input_chunk(tool_use_id, partial_json))
}

pub(crate) fn transcript_tool_output_chunk(
    tool_use_id: String,
    chunk: String,
) -> ItemLifecycleEvent {
    ItemLifecycleEvent::ToolOutputChunk { tool_use_id, chunk }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn tool_input_delta_payload_maps_to_transcript_chunk() {
        let delta = serde_json::json!({
            "tool_use_id": "call-1",
            "partial_json": "{\"command\": \"touch foo\"}"
        })
        .to_string();

        assert_eq!(
            transcript_tool_input_chunk_from_delta_payload(&delta),
            Some(ItemLifecycleEvent::ToolInputChunk {
                tool_use_id: "call-1".to_string(),
                chunk: "{\"command\": \"touch foo\"}".to_string(),
            })
        );
    }
}
