//! Derives projector tool state from structured tool facts (name + input).

use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;

use super::model::ToolPhase;

pub(crate) fn is_streaming_param_tool(tool_name: &str) -> bool {
    matches!(tool_name, "write" | "edit" | "apply_patch")
}

pub(crate) fn input_is_incomplete(input: &serde_json::Value) -> bool {
    input.is_null() || matches!(input, serde_json::Value::Object(map) if map.is_empty())
}

pub(crate) fn initial_phase(tool_name: &str, input: &serde_json::Value) -> ToolPhase {
    if is_streaming_param_tool(tool_name) && input_is_incomplete(input) {
        ToolPhase::Preparing
    } else {
        ToolPhase::Running
    }
}

pub(crate) fn is_exec_like(parsed_commands: &[ParsedCommand]) -> bool {
    !parsed_commands.is_empty()
        && parsed_commands
            .iter()
            .all(|parsed| !matches!(parsed, ParsedCommand::Unknown { .. }))
}

pub(crate) fn shell_command_from_input(input: &serde_json::Value) -> Option<String> {
    input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub(crate) fn shell_description_from_input(input: Option<&serde_json::Value>) -> Option<String> {
    input
        .and_then(|value| value.get("description"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

pub(crate) fn is_shell_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "bash" | "shell_command" | "exec_command" | "write_stdin" | "shell"
    )
}

pub(crate) fn command_source_from_tool_name(tool_name: &str) -> Option<ExecCommandSource> {
    match tool_name {
        "bash" | "shell_command" | "exec_command" | "write_stdin" => Some(ExecCommandSource::Agent),
        _ => None,
    }
}
