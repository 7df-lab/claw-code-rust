//! Renders transcript cell models into history cells and styled lines.

use std::path::Path;

use ratatui::text::Line;

use crate::agent_tool_cell::AgentToolCell;
use crate::agent_tool_cell::is_agent_task_tool_name;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::tool_io_cell::FileChangeToolIoCell;
use crate::tool_io_cell::ToolIoCell;
use crate::tool_io_cell::ToolIoCellOptions;
use crate::tool_result_cell::ToolResultCell;
use crate::transcript::model::CommittedCellModel;
use crate::transcript::model::ToolCellModel;
use crate::transcript::model::ToolPhase;
use crate::transcript::presentation::tool_title_line;
use crate::transcript::presentation::tool_title_parts;

use super::model::TextCellModel;

fn tool_title_for_cell(tool: &ToolCellModel) -> Line<'static> {
    let change_is_add = tool.file_changes.as_ref().is_some_and(|changes| {
        changes
            .values()
            .any(|change| matches!(change, devo_protocol::protocol::FileChange::Add { .. }))
    });
    let parts = tool_title_parts(
        tool.phase,
        tool.tool_name.as_deref(),
        tool.input.as_ref(),
        &tool.parsed_commands,
        change_is_add,
        &tool.summary,
    );
    tool_title_line(tool.phase, &parts)
}

/// Converts a committed cell model into a renderable history cell.
pub(crate) fn committed_cell_to_history(
    cell: &CommittedCellModel,
    cwd: &Path,
    _ran_tool_line: impl Fn(&str) -> Line<'static>,
    tool_dot_prefix: Line<'static>,
    tool_text_style: ratatui::style::Style,
) -> Box<dyn HistoryCell> {
    match cell {
        CommittedCellModel::Tool(tool) => {
            tool_cell_to_history(tool, cwd, tool_dot_prefix, tool_text_style)
        }
        CommittedCellModel::Text(text) => text_cell_to_history(text),
    }
}

fn tool_cell_to_history(
    tool: &ToolCellModel,
    cwd: &Path,
    tool_dot_prefix: Line<'static>,
    tool_text_style: ratatui::style::Style,
) -> Box<dyn HistoryCell> {
    if let Some(changes) = &tool.file_changes {
        if let (Some(tool_name), Some(input)) = (&tool.tool_name, &tool.input) {
            return Box::new(FileChangeToolIoCell::new(
                None,
                tool_name.clone(),
                input.clone(),
                changes.clone(),
                cwd.to_path_buf(),
            ));
        }
        return Box::new(history_cell::new_patch_event(changes.clone(), cwd));
    }

    if let (Some(tool_name), Some(input)) = (&tool.tool_name, &tool.input) {
        if is_agent_task_tool_name(tool_name) {
            return Box::new(AgentToolCell::new(
                tool_name.clone(),
                tool.phase,
                Some(input.clone()),
                tool.tool_output.clone(),
                tool.tool_display_content
                    .clone()
                    .unwrap_or_else(|| tool.output_preview.clone()),
                tool_dot_prefix.clone(),
            ));
        }
        let title_line = Some(tool_title_for_cell(tool));
        return Box::new(ToolIoCell::new(
            ToolIoCellOptions {
                title_line,
                dot_prefix: tool_dot_prefix.clone(),
                subsequent_prefix: Line::from("  "),
                output_style: tool_text_style,
                show_empty_ellipsis: tool.truncated,
            },
            tool_name.clone(),
            input.clone(),
            tool.tool_output.clone(),
            tool.tool_display_content.clone(),
        ));
    }

    let title_line = Some(tool_title_for_cell(tool));
    Box::new(ToolResultCell::new(
        title_line,
        tool.output_preview.clone(),
        tool_dot_prefix,
        Line::from("  "),
        tool_text_style,
        tool.truncated,
    ))
}

fn text_cell_to_history(text: &TextCellModel) -> Box<dyn HistoryCell> {
    let _ = text;
    Box::new(history_cell::PlainHistoryCell::new(Vec::new()))
}

/// Live tool row for the inline viewport.
pub(crate) fn live_tool_display_lines(
    tool: &ToolCellModel,
    width: u16,
    pending_dot_prefix: Line<'static>,
    tool_text_style: ratatui::style::Style,
) -> Vec<Line<'static>> {
    let title_line = tool_title_for_cell(tool);
    if tool.phase == ToolPhase::Preparing {
        return vec![title_line];
    }
    match (&tool.tool_name, &tool.input) {
        (Some(tool_name), Some(input)) => ToolIoCell::from_text_output(
            ToolIoCellOptions {
                title_line: Some(title_line),
                dot_prefix: pending_dot_prefix,
                subsequent_prefix: "  ".into(),
                output_style: tool_text_style,
                show_empty_ellipsis: false,
            },
            tool_name.clone(),
            input.clone(),
            tool.output_preview.clone(),
        )
        .display_lines(width),
        _ => history_cell::AgentMessageCell::new_with_prefix(
            vec![title_line],
            pending_dot_prefix,
            "  ",
            false,
        )
        .display_lines(width),
    }
}
