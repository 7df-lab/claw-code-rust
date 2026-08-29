//! Canonical history commit path for finished tool rows (live session + resume).

use std::time::Duration;

use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;
use ratatui::text::Line;
use serde_json::Value;

use crate::exec_cell::CommandOutput;
use crate::exec_cell::ExecCell;
use crate::exec_cell::new_active_exec_command;
use crate::tool_result_cell::ToolResultCell;
use crate::transcript::model::CommittedCellModel;
use crate::transcript::model::ToolCellModel;
use crate::transcript::model::ToolPhase;
use crate::transcript::presentation::tool_title_line;
use crate::transcript::presentation::tool_title_parts;
use crate::transcript::tool_state::is_exec_like;

use super::ChatWidget;

pub(crate) fn is_exploration_tool(tool: &ToolCellModel) -> bool {
    is_exec_like(&tool.parsed_commands)
        && !matches!(tool.command_source, Some(ExecCommandSource::UserShell))
}

/// Where a finished tool row should land in the transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolCommitTarget {
    /// Keep exec/explore tools in the live overlay until turn finish or compaction.
    LiveOverlay,
    /// Append directly to scrollback history (session resume rebuild).
    ScrollbackHistory,
}

impl ChatWidget {
    /// Commits a finished tool row to scrollback history using resume semantics.
    pub(crate) fn commit_committed_tool_to_history(&mut self, tool: ToolCellModel) {
        self.commit_committed_tool_to_history_with_target(
            tool,
            ToolCommitTarget::ScrollbackHistory,
        );
    }

    /// Commits a finished tool row during an active live turn.
    pub(crate) fn commit_committed_tool_to_live_turn(&mut self, tool: ToolCellModel) {
        self.commit_committed_tool_to_history_with_target(tool, ToolCommitTarget::LiveOverlay);
    }

    fn commit_committed_tool_to_history_with_target(
        &mut self,
        tool: ToolCellModel,
        target: ToolCommitTarget,
    ) {
        if tool
            .file_changes
            .as_ref()
            .is_some_and(|changes| !changes.is_empty())
        {
            self.append_committed_tool_io_cell(tool);
            return;
        }

        if is_exploration_tool(&tool) {
            self.commit_exploration_tool(tool, target);
            return;
        }

        if tool.exec_like {
            self.commit_exec_tool(tool, target);
            return;
        }

        if tool.tool_name.is_some() && tool.input.is_some() {
            self.append_committed_tool_io_cell(tool);
            return;
        }

        self.commit_tool_fallback_to_history(tool);
    }

    fn append_committed_tool_io_cell(&mut self, tool: ToolCellModel) {
        let dot_prefix = if tool.is_error {
            Self::failed_dot_prefix()
        } else {
            Self::tool_dot_prefix()
        };
        let history_cell = crate::transcript::render::committed_cell_to_history(
            &CommittedCellModel::Tool(tool),
            &self.session.cwd,
            |title| Self::ran_tool_line(title),
            dot_prefix,
            Self::tool_text_style(),
        );
        self.add_history_entry_without_redraw(history_cell);
    }

    pub(crate) fn commit_exploration_tool_from_history_item(
        &mut self,
        tool_use_id: String,
        command: String,
        actions: Vec<ParsedCommand>,
        tool_name: Option<String>,
        input: Option<Value>,
        output: Option<Value>,
        display_content: Option<String>,
        is_error: bool,
    ) {
        let tool = ToolCellModel {
            tool_use_id,
            seq: 0,
            phase: if is_error {
                ToolPhase::Failed
            } else {
                ToolPhase::Completed
            },
            summary: command.clone(),
            tool_name,
            input,
            input_partial_json: String::new(),
            parsed_commands: actions,
            exec_like: true,
            start_time: None,
            output_preview: display_content.clone().unwrap_or_default(),
            output_delta_lines: Vec::new(),
            file_changes: None,
            command: Some(command),
            command_source: Some(ExecCommandSource::Agent),
            command_output: None,
            command_duration: None,
            tool_output: output,
            tool_display_content: display_content,
            is_error,
            truncated: false,
        };
        self.commit_exploration_tool(tool, ToolCommitTarget::ScrollbackHistory);
    }

    fn commit_exec_tool(&mut self, tool: ToolCellModel, target: ToolCommitTarget) {
        if self.complete_exec_tool_from_committed(&tool) {
            return;
        }

        if target == ToolCommitTarget::LiveOverlay {
            return;
        }

        let exec = exec_cell_from_tool(&tool, &self.session.cwd);
        self.add_history_entry_without_redraw(Box::new(exec));
        self.apply_tool_io_to_history_exec(&tool);
    }

    fn commit_exploration_tool(&mut self, tool: ToolCellModel, target: ToolCommitTarget) {
        match target {
            ToolCommitTarget::LiveOverlay => self.commit_exploration_tool_to_live_overlay(tool),
            ToolCommitTarget::ScrollbackHistory => {
                self.commit_exploration_tool_to_scrollback_history(tool)
            }
        }
    }

    fn commit_exploration_tool_to_live_overlay(&mut self, tool: ToolCellModel) {
        self.apply_tool_io_to_active_exec(&tool);

        if self.active_exec_has_call(&tool.tool_use_id) {
            return;
        }

        if self.try_merge_exploration_into_active_exec(&tool) {
            return;
        }

        let exec = exec_cell_from_tool(&tool, &self.session.cwd);
        if let Some(active) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && active.is_exploring_cell()
        {
            let mut actions = tool.parsed_commands.clone();
            crate::read_display::normalize_read_actions(&mut actions, &self.session.cwd);
            let command = tool
                .command
                .clone()
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| tool.summary.clone());
            let command_tokens = crate::exec_command::split_command_string(&command);
            if let Some(grouped) = active.with_added_call(
                tool.tool_use_id.clone(),
                command_tokens,
                actions,
                tool.command_source.unwrap_or(ExecCommandSource::Agent),
                None,
            ) {
                *active = grouped;
                self.apply_tool_io_to_active_exec(&tool);
            }
            return;
        }

        self.active_cell = Some(Box::new(exec));
        self.apply_tool_io_to_active_exec(&tool);
    }

    fn commit_exploration_tool_to_scrollback_history(&mut self, tool: ToolCellModel) {
        self.apply_tool_io_to_active_exec(&tool);

        if self.try_merge_exploration_into_history_exec(&tool) {
            self.clear_active_exec_call_if_present(&tool.tool_use_id);
            return;
        }

        if self.should_flush_active_exploring_cell() {
            self.flush_active_cell();
            return;
        }

        let exec = exec_cell_from_tool(&tool, &self.session.cwd);
        self.add_history_entry_without_redraw(Box::new(exec));
        self.apply_tool_io_to_history_exec(&tool);
    }

    fn active_exec_has_call(&self, call_id: &str) -> bool {
        self.active_cell
            .as_ref()
            .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
            .is_some_and(|cell| cell.contains_call(call_id))
    }

    fn try_merge_exploration_into_active_exec(&mut self, tool: &ToolCellModel) -> bool {
        let mut actions = tool.parsed_commands.clone();
        crate::read_display::normalize_read_actions(&mut actions, &self.session.cwd);
        let command = tool
            .command
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| tool.summary.clone());
        let command_tokens = crate::exec_command::split_command_string(&command);
        let call_id = tool.tool_use_id.clone();

        let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            .filter(|cell| cell.is_exploring_cell())
        else {
            return false;
        };
        let Some(grouped) = cell.with_added_call(
            call_id,
            command_tokens,
            actions,
            tool.command_source.unwrap_or(ExecCommandSource::Agent),
            None,
        ) else {
            return false;
        };
        *cell = grouped;
        self.apply_tool_io_to_active_exec(tool);
        true
    }

    fn try_merge_exploration_into_history_exec(&mut self, tool: &ToolCellModel) -> bool {
        let mut actions = tool.parsed_commands.clone();
        crate::read_display::normalize_read_actions(&mut actions, &self.session.cwd);
        let command = tool
            .command
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| tool.summary.clone());
        let command_tokens = crate::exec_command::split_command_string(&command);
        let call_id = tool.tool_use_id.clone();

        let Some(cell) = self
            .history
            .last_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        else {
            return false;
        };
        let Some(grouped) = cell.with_added_call(
            call_id,
            command_tokens,
            actions,
            tool.command_source.unwrap_or(ExecCommandSource::Agent),
            None,
        ) else {
            return false;
        };
        *cell = grouped;
        self.apply_tool_io_to_history_exec(tool);
        true
    }

    fn should_flush_active_exploring_cell(&self) -> bool {
        self.active_cell
            .as_ref()
            .and_then(|cell| cell.as_any().downcast_ref::<ExecCell>())
            .is_some_and(|cell| {
                cell.is_exploring_cell() && cell.calls.iter().all(|call| call.output.is_some())
            })
    }

    fn clear_active_exec_call_if_present(&mut self, call_id: &str) {
        let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        else {
            return;
        };
        if cell.calls.len() == 1 && cell.calls[0].call_id == call_id {
            self.active_cell = None;
            return;
        }
        cell.calls.retain(|call| call.call_id != call_id);
        if cell.calls.is_empty() {
            self.active_cell = None;
        }
    }

    fn apply_tool_io_to_active_exec(&mut self, tool: &ToolCellModel) {
        let (Some(tool_name), Some(input)) = (&tool.tool_name, &tool.input) else {
            if let Some(output) = tool_output_for_commit(tool) {
                self.complete_active_exec_call(tool, output);
            }
            return;
        };
        let tool_use_id = tool.tool_use_id.as_str();
        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && cell.set_tool_io_input(tool_use_id, tool_name.clone(), input.clone())
        {
            if let Some(output) = tool_output_for_commit(tool) {
                let display_content = tool.tool_display_content.clone();
                let output_text = display_content
                    .clone()
                    .unwrap_or_else(|| value_text(&output));
                cell.complete_tool_io(tool_use_id, output, display_content.clone());
                cell.complete_call(
                    tool_use_id,
                    CommandOutput {
                        exit_code: if tool.is_error { 1 } else { 0 },
                        aggregated_output: output_text.clone(),
                        formatted_output: output_text.clone(),
                    },
                    Duration::from_millis(0),
                );
            }
        }
    }

    fn apply_tool_io_to_history_exec(&mut self, tool: &ToolCellModel) {
        let (Some(tool_name), Some(input)) = (&tool.tool_name, &tool.input) else {
            if let Some(output) = tool_output_for_commit(tool) {
                self.complete_history_exec_call(tool, output);
            }
            return;
        };
        let tool_use_id = tool.tool_use_id.as_str();
        for cell in self
            .history
            .iter_mut()
            .rev()
            .filter_map(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        {
            if !cell.set_tool_io_input(tool_use_id, tool_name.clone(), input.clone()) {
                continue;
            }
            if let Some(output) = tool_output_for_commit(tool) {
                let display_content = tool.tool_display_content.clone();
                let output_text = display_content
                    .clone()
                    .unwrap_or_else(|| value_text(&output));
                cell.complete_tool_io(tool_use_id, output, display_content.clone());
                cell.complete_call(
                    tool_use_id,
                    CommandOutput {
                        exit_code: if tool.is_error { 1 } else { 0 },
                        aggregated_output: output_text.clone(),
                        formatted_output: output_text.clone(),
                    },
                    Duration::from_millis(0),
                );
            }
            return;
        }
    }

    fn complete_active_exec_call(&mut self, tool: &ToolCellModel, output: Value) {
        let tool_use_id = tool.tool_use_id.as_str();
        let output_text = tool
            .tool_display_content
            .clone()
            .unwrap_or_else(|| value_text(&output));
        if let Some(cell) = self
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
            && cell.complete_call(
                tool_use_id,
                CommandOutput {
                    exit_code: if tool.is_error { 1 } else { 0 },
                    aggregated_output: output_text.clone(),
                    formatted_output: output_text,
                },
                Duration::from_millis(0),
            )
        {
            let _ = cell;
        }
    }

    fn complete_history_exec_call(&mut self, tool: &ToolCellModel, output: Value) {
        let tool_use_id = tool.tool_use_id.as_str();
        let output_text = tool
            .tool_display_content
            .clone()
            .unwrap_or_else(|| value_text(&output));
        for cell in self
            .history
            .iter_mut()
            .rev()
            .filter_map(|cell| cell.as_any_mut().downcast_mut::<ExecCell>())
        {
            if cell.complete_call(
                tool_use_id,
                CommandOutput {
                    exit_code: if tool.is_error { 1 } else { 0 },
                    aggregated_output: output_text.clone(),
                    formatted_output: output_text.clone(),
                },
                Duration::from_millis(0),
            ) {
                return;
            }
        }
    }

    fn commit_tool_fallback_to_history(&mut self, tool: ToolCellModel) {
        let dot_prefix = if tool.is_error {
            Self::failed_dot_prefix()
        } else {
            Self::tool_dot_prefix()
        };
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
        let title_line = tool_title_line(tool.phase, &parts);
        let preview = tool
            .tool_display_content
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| tool.output_preview.clone());
        self.add_history_entry_without_redraw(Box::new(ToolResultCell::new(
            Some(title_line),
            preview,
            dot_prefix,
            Line::from("  "),
            Self::tool_text_style(),
            tool.truncated,
        )));
    }
}

fn tool_output_for_commit(tool: &ToolCellModel) -> Option<Value> {
    tool.tool_output.clone().or_else(|| {
        (!tool.output_preview.is_empty()).then(|| Value::String(tool.output_preview.clone()))
    })
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn exec_cell_from_tool(tool: &ToolCellModel, cwd: &std::path::Path) -> ExecCell {
    let mut actions = tool.parsed_commands.clone();
    crate::read_display::normalize_read_actions(&mut actions, cwd);
    let command = tool
        .command
        .clone()
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| tool.summary.clone());
    let command_tokens = crate::exec_command::split_command_string(&command);
    new_active_exec_command(
        tool.tool_use_id.clone(),
        command_tokens,
        actions,
        tool.command_source.unwrap_or(ExecCommandSource::Agent),
        None,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::tool_state::is_exec_like;

    #[test]
    fn exploration_tool_detects_parsed_search_actions() {
        let tool = ToolCellModel {
            tool_use_id: "grep-1".into(),
            seq: 0,
            phase: ToolPhase::Completed,
            summary: String::new(),
            tool_name: Some("grep".into()),
            input: Some(serde_json::json!({"pattern": "plan"})),
            input_partial_json: String::new(),
            parsed_commands: vec![ParsedCommand::Search {
                cmd: "grep plan".into(),
                query: Some("plan".into()),
                path: None,
            }],
            exec_like: true,
            start_time: None,
            output_preview: String::new(),
            output_delta_lines: Vec::new(),
            file_changes: None,
            command: None,
            command_source: None,
            command_output: None,
            command_duration: None,
            tool_output: None,
            tool_display_content: None,
            is_error: false,
            truncated: false,
        };
        assert!(is_exploration_tool(&tool));
        assert!(is_exec_like(&tool.parsed_commands));
    }
}
