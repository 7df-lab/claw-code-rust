//! Pure transcript cell models. Renderers consume these without mutating them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use devo_core::ItemId;
use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;
use devo_protocol::protocol::FileChange;

use crate::events::TextItemKind;
use crate::exec_cell::CommandOutput;

/// Lifecycle phase for any tool row in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolPhase {
    Preparing,
    Running,
    Completed,
    Failed,
}

/// Unified tool representation: write/edit, exec, and generic tools share one model.
#[derive(Debug, Clone)]
pub(crate) struct ToolCellModel {
    pub(crate) tool_use_id: String,
    pub(crate) seq: u64,
    pub(crate) phase: ToolPhase,
    pub(crate) summary: String,
    pub(crate) tool_name: Option<String>,
    pub(crate) input: Option<serde_json::Value>,
    pub(crate) input_partial_json: String,
    pub(crate) parsed_commands: Vec<ParsedCommand>,
    pub(crate) exec_like: bool,
    pub(crate) start_time: Option<Instant>,
    pub(crate) output_preview: String,
    pub(crate) output_delta_lines: Vec<String>,
    pub(crate) file_changes: Option<HashMap<PathBuf, FileChange>>,
    pub(crate) command: Option<String>,
    pub(crate) command_source: Option<ExecCommandSource>,
    pub(crate) command_output: Option<CommandOutput>,
    pub(crate) command_duration: Option<Duration>,
    pub(crate) tool_output: Option<serde_json::Value>,
    pub(crate) tool_display_content: Option<String>,
    pub(crate) is_error: bool,
    pub(crate) truncated: bool,
}

impl ToolCellModel {
    pub(crate) fn new_opened(
        tool_use_id: String,
        seq: u64,
        tool_name: String,
        input: serde_json::Value,
        command: Option<String>,
        command_source: Option<ExecCommandSource>,
        parsed_commands: Vec<ParsedCommand>,
    ) -> Self {
        use super::tool_state::{initial_phase, is_exec_like};

        let exec_like = is_exec_like(&parsed_commands)
            || command.is_some()
            || matches!(
                tool_name.as_str(),
                "exec_command" | "shell_command" | "bash" | "shell" | "write_stdin"
            );
        let phase = initial_phase(&tool_name, &input);
        Self {
            tool_use_id,
            seq,
            phase,
            summary: String::new(),
            tool_name: Some(tool_name),
            input: Some(input),
            input_partial_json: String::new(),
            parsed_commands,
            exec_like,
            start_time: if phase == ToolPhase::Preparing {
                Some(Instant::now())
            } else {
                None
            },
            output_preview: String::new(),
            output_delta_lines: Vec::new(),
            file_changes: None,
            command,
            command_source,
            command_output: None,
            command_duration: None,
            tool_output: None,
            tool_display_content: None,
            is_error: false,
            truncated: false,
        }
    }

    pub(crate) fn refresh_opened(
        &mut self,
        tool_name: String,
        input: serde_json::Value,
        command: Option<String>,
        command_source: Option<ExecCommandSource>,
        parsed_commands: Vec<ParsedCommand>,
    ) {
        use super::tool_state::{initial_phase, is_exec_like};

        self.tool_name = Some(tool_name.clone());
        self.input = Some(input.clone());
        self.command = command;
        self.command_source = command_source;
        self.parsed_commands = parsed_commands;
        self.exec_like = is_exec_like(&self.parsed_commands)
            || self.command.is_some()
            || matches!(
                tool_name.as_str(),
                "exec_command" | "shell_command" | "bash" | "shell" | "write_stdin"
            );
        if self.phase == ToolPhase::Preparing && !super::tool_state::input_is_incomplete(&input) {
            self.phase = ToolPhase::Running;
        } else if self.phase == ToolPhase::Preparing {
            self.phase = initial_phase(&tool_name, &input);
        }
    }

    pub(crate) fn new_running(
        tool_use_id: String,
        seq: u64,
        summary: String,
        preparing: bool,
        parsed_commands: Option<Vec<ParsedCommand>>,
    ) -> Self {
        let exec_like = parsed_commands.as_ref().is_some_and(|parsed| {
            !parsed.is_empty()
                && parsed.iter().all(|parsed| {
                    !matches!(
                        parsed,
                        devo_protocol::parse_command::ParsedCommand::Unknown { .. }
                    )
                })
        });
        Self {
            tool_use_id,
            seq,
            phase: if preparing {
                ToolPhase::Preparing
            } else {
                ToolPhase::Running
            },
            summary,
            tool_name: None,
            input: None,
            input_partial_json: String::new(),
            parsed_commands: parsed_commands.unwrap_or_default(),
            exec_like,
            start_time: if preparing {
                Some(Instant::now())
            } else {
                None
            },
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
        }
    }

    pub(crate) fn is_live(&self) -> bool {
        matches!(self.phase, ToolPhase::Preparing | ToolPhase::Running)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TextCellModel {
    pub(crate) item_id: ItemId,
    pub(crate) kind: TextItemKind,
    pub(crate) text: String,
}

/// In-flight assistant or reasoning stream owned by the transcript projector.
#[derive(Debug, Clone)]
pub(crate) struct LiveTextCellModel {
    pub(crate) item_id: ItemId,
    pub(crate) kind: TextItemKind,
    pub(crate) seq: u64,
    pub(crate) text: String,
}

/// One committed transcript entry produced by the projector.
#[derive(Debug, Clone)]
pub(crate) enum CommittedCellModel {
    Tool(ToolCellModel),
    Text(TextCellModel),
}
