//! Session history → committed cell models (shared by live completion and restore).

use std::collections::HashMap;

use devo_protocol::SessionHistoryItem;
use devo_protocol::SessionHistoryItemKind;
use devo_protocol::SessionHistoryMetadata;
use devo_protocol::protocol::ExecCommandSource;
use devo_protocol::protocol::FileChange;

use crate::events::TextItemKind;
use crate::transcript::model::CommittedCellModel;
use crate::transcript::model::TextCellModel;
use crate::transcript::model::ToolCellModel;
use crate::transcript::model::ToolPhase;
use crate::transcript::tool_state::is_shell_tool_name;
use crate::transcript::tool_state::shell_command_from_input;

pub(crate) fn finalize_restored_tool_cell(mut tool: ToolCellModel) -> ToolCellModel {
    if tool.tool_name.as_deref().is_some_and(is_shell_tool_name) {
        tool.exec_like = true;
        if tool.command.is_none() {
            tool.command = tool
                .input
                .as_ref()
                .and_then(shell_command_from_input)
                .or_else(|| (!tool.summary.is_empty()).then(|| tool.summary.clone()));
        }
        tool.command_source = Some(ExecCommandSource::Agent);
    }
    tool
}

pub(crate) fn committed_cells_from_history(
    items: &[SessionHistoryItem],
) -> Vec<CommittedCellModel> {
    let mut paired_result_by_call_id = HashMap::new();
    let mut consumed = std::collections::HashSet::new();

    for (index, item) in items.iter().enumerate() {
        if matches!(
            item.kind,
            SessionHistoryItemKind::ToolResult | SessionHistoryItemKind::Error
        ) && let Some(tool_call_id) = item.tool_call_id.as_deref()
        {
            paired_result_by_call_id
                .entry(tool_call_id.to_string())
                .or_insert(index);
        }
    }

    let mut committed = Vec::new();
    let mut seq = 0u64;

    for (index, item) in items.iter().enumerate() {
        if consumed.contains(&index) {
            continue;
        }

        if let Some(SessionHistoryMetadata::Edited { changes }) = &item.metadata {
            committed.push(CommittedCellModel::Tool(completed_tool_from_edit(
                item,
                changes.clone(),
                seq,
            )));
            seq = seq.wrapping_add(1);
            continue;
        }

        if item.kind == SessionHistoryItemKind::ToolCall
            && let Some(tool_call_id) = item.tool_call_id.as_deref()
            && let Some(result_index) = paired_result_by_call_id.get(tool_call_id).copied()
            && result_index != index
        {
            consumed.insert(result_index);
            let result_item = &items[result_index];
            if let Some(tool_cell) = paired_tool_cell(item, result_item, seq) {
                committed.push(CommittedCellModel::Tool(tool_cell));
                seq = seq.wrapping_add(1);
            }
            continue;
        }

        if let Some(cell) = restore_item_to_committed(item, seq) {
            committed.push(cell);
            seq = seq.wrapping_add(1);
        }
    }

    committed
}

pub(crate) fn restore_item_to_committed(
    item: &SessionHistoryItem,
    seq: u64,
) -> Option<CommittedCellModel> {
    match item.kind {
        SessionHistoryItemKind::Assistant => Some(CommittedCellModel::Text(TextCellModel {
            item_id: devo_core::ItemId::new(),
            kind: TextItemKind::Assistant,
            text: item.body.clone(),
        })),
        SessionHistoryItemKind::Reasoning => Some(CommittedCellModel::Text(TextCellModel {
            item_id: devo_core::ItemId::new(),
            kind: TextItemKind::Reasoning,
            text: item.body.clone(),
        })),
        SessionHistoryItemKind::Error => Some(CommittedCellModel::Tool(
            finalize_restored_tool_cell(ToolCellModel {
                tool_use_id: item.tool_call_id.clone().unwrap_or_default(),
                seq,
                phase: ToolPhase::Failed,
                summary: item.title.clone(),
                tool_name: item.tool_io.as_ref().map(|io| io.tool_name.clone()),
                input: item.tool_io.as_ref().map(|io| io.input.clone()),
                input_partial_json: String::new(),
                parsed_commands: Vec::new(),
                exec_like: false,
                start_time: None,
                output_preview: item.body.clone(),
                output_delta_lines: Vec::new(),
                file_changes: edited_changes_from_history_item(item),
                command: None,
                command_source: None,
                command_output: None,
                command_duration: None,
                tool_output: item.tool_io.as_ref().and_then(|io| io.output.clone()),
                tool_display_content: item
                    .tool_io
                    .as_ref()
                    .and_then(|io| io.display_content.clone()),
                is_error: true,
                truncated: false,
            }),
        )),
        SessionHistoryItemKind::ToolResult | SessionHistoryItemKind::CommandExecution => Some(
            CommittedCellModel::Tool(finalize_restored_tool_cell(ToolCellModel {
                tool_use_id: item.tool_call_id.clone().unwrap_or_default(),
                seq,
                phase: ToolPhase::Completed,
                summary: item.title.clone(),
                tool_name: item.tool_io.as_ref().map(|io| io.tool_name.clone()),
                input: item.tool_io.as_ref().map(|io| io.input.clone()),
                input_partial_json: String::new(),
                parsed_commands: Vec::new(),
                exec_like: false,
                start_time: None,
                output_preview: item.body.clone(),
                output_delta_lines: Vec::new(),
                file_changes: edited_changes_from_history_item(item),
                command: None,
                command_source: None,
                command_output: None,
                command_duration: None,
                tool_output: item.tool_io.as_ref().and_then(|io| io.output.clone()),
                tool_display_content: item
                    .tool_io
                    .as_ref()
                    .and_then(|io| io.display_content.clone()),
                is_error: false,
                truncated: false,
            })),
        ),
        _ => None,
    }
}

pub(crate) fn paired_tool_cell(
    call_item: &SessionHistoryItem,
    result_item: &SessionHistoryItem,
    seq: u64,
) -> Option<ToolCellModel> {
    let changes = edited_changes_from_history_item(result_item);
    Some(finalize_restored_tool_cell(ToolCellModel {
        tool_use_id: call_item.tool_call_id.clone().unwrap_or_default(),
        seq,
        phase: if result_item.kind == SessionHistoryItemKind::Error {
            ToolPhase::Failed
        } else {
            ToolPhase::Completed
        },
        summary: call_item.title.clone(),
        tool_name: call_item
            .tool_io
            .as_ref()
            .map(|io| io.tool_name.clone())
            .or_else(|| result_item.tool_io.as_ref().map(|io| io.tool_name.clone())),
        input: call_item
            .tool_io
            .as_ref()
            .map(|io| io.input.clone())
            .or_else(|| result_item.tool_io.as_ref().map(|io| io.input.clone())),
        input_partial_json: String::new(),
        parsed_commands: Vec::new(),
        exec_like: false,
        start_time: None,
        output_preview: result_item.body.clone(),
        output_delta_lines: Vec::new(),
        file_changes: changes,
        command: None,
        command_source: None,
        command_output: None,
        command_duration: None,
        tool_output: result_item
            .tool_io
            .as_ref()
            .and_then(|io| io.output.clone()),
        tool_display_content: result_item
            .tool_io
            .as_ref()
            .and_then(|io| io.display_content.clone()),
        is_error: result_item.kind == SessionHistoryItemKind::Error,
        truncated: false,
    }))
}

pub(crate) fn completed_tool_from_edit(
    item: &SessionHistoryItem,
    changes: HashMap<std::path::PathBuf, FileChange>,
    seq: u64,
) -> ToolCellModel {
    ToolCellModel {
        tool_use_id: item.tool_call_id.clone().unwrap_or_default(),
        seq,
        phase: ToolPhase::Completed,
        summary: item.title.clone(),
        tool_name: item.tool_io.as_ref().map(|io| io.tool_name.clone()),
        input: item.tool_io.as_ref().map(|io| io.input.clone()),
        input_partial_json: String::new(),
        parsed_commands: Vec::new(),
        exec_like: false,
        start_time: None,
        output_preview: String::new(),
        output_delta_lines: Vec::new(),
        file_changes: Some(changes),
        command: None,
        command_source: None,
        command_output: None,
        command_duration: None,
        tool_output: None,
        tool_display_content: None,
        is_error: false,
        truncated: false,
    }
}

fn edited_changes_from_history_item(
    item: &SessionHistoryItem,
) -> Option<HashMap<std::path::PathBuf, FileChange>> {
    if let Some(SessionHistoryMetadata::Edited { changes }) = &item.metadata {
        return Some(changes.clone());
    }
    item.tool_io.as_ref().and_then(|io| {
        io.output
            .as_ref()
            .and_then(|output| parse_file_changes_from_json(output))
    })
}

fn parse_file_changes_from_json(
    output: &serde_json::Value,
) -> Option<HashMap<std::path::PathBuf, FileChange>> {
    let files = output.get("files")?.as_array()?;
    let mut changes = HashMap::new();
    for file in files {
        let path = std::path::PathBuf::from(file.get("path")?.as_str()?);
        let kind = file.get("kind")?.as_str()?;
        let change = match kind {
            "add" => FileChange::Add {
                content: file
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
            "delete" => FileChange::Delete {
                content: file
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            },
            "update" | "move" => FileChange::Update {
                unified_diff: file
                    .get("diff")
                    .or_else(|| file.get("patch"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                old_text: None,
                new_text: None,
                move_path: file
                    .get("move_path")
                    .and_then(serde_json::Value::as_str)
                    .map(std::path::PathBuf::from),
            },
            _ => continue,
        };
        changes.insert(path, change);
    }
    (!changes.is_empty()).then_some(changes)
}
