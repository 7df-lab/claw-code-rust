use std::collections::HashMap;

use devo_core::ItemId;
use devo_core::tools::tool_spec::ToolPreparationFeedback;

use super::types::{PendingToolCall, ToolDisplayKind, ToolStartItem};
use crate::{CommandExecutionPayload, FileChangePayload, ItemKind, ToolCallPayload};

pub(super) fn is_unified_exec_tool(name: &str) -> bool {
    matches!(name, "exec_command" | "write_stdin")
}

pub(super) fn is_file_change_tool(name: &str) -> bool {
    matches!(name, "apply_patch" | "write" | "edit")
}

pub(super) fn is_plan_tool(name: &str) -> bool {
    matches!(name, "update_plan")
}

fn tool_start_item_kind(
    tool_name: &str,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
) -> ItemKind {
    if preparation_feedback == ToolPreparationFeedback::LiveOnly {
        ItemKind::ToolCall
    } else if is_file_change_tool(tool_name) {
        ItemKind::FileChange
    } else if display_kind.is_command_execution() {
        ItemKind::CommandExecution
    } else if is_plan_tool(tool_name) {
        ItemKind::Plan
    } else {
        ItemKind::ToolCall
    }
}

fn tool_start_item(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
) -> ToolStartItem {
    let item_kind = tool_start_item_kind(tool_name, display_kind, preparation_feedback);
    let payload = match item_kind {
        ItemKind::ToolCall => serde_json::to_value(ToolCallPayload {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            parameters: input.clone(),
            command_actions,
        })
        .expect("serialize tool call payload"),
        ItemKind::FileChange => serde_json::to_value(FileChangePayload {
            tool_call_id: tool_call_id.to_string(),
            tool_name: Some(tool_name.to_string()),
            input: Some(input.clone()),
            changes: Vec::new(),
            is_error: false,
        })
        .expect("serialize file change payload"),
        ItemKind::CommandExecution => serde_json::to_value(CommandExecutionPayload {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            command: command.to_string(),
            input: Some(input.clone()),
            source: devo_protocol::protocol::ExecCommandSource::Agent,
            command_actions,
            output: None,
            is_error: false,
        })
        .expect("serialize command execution payload"),
        ItemKind::Plan => serde_json::json!({
            "title": "Plan",
            "text": ""
        }),
        ItemKind::UserMessage
        | ItemKind::AgentMessage
        | ItemKind::Reasoning
        | ItemKind::ToolResult
        | ItemKind::McpToolCall
        | ItemKind::WebSearch
        | ItemKind::ImageView
        | ItemKind::ContextCompaction
        | ItemKind::ApprovalRequest
        | ItemKind::ApprovalDecision => unreachable!("tool start item kind must be tool-like"),
    };
    ToolStartItem { item_kind, payload }
}

pub(super) fn tool_start_item_from_input(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
) -> ToolStartItem {
    tool_start_item(
        tool_call_id,
        tool_name,
        command,
        input,
        display_kind,
        preparation_feedback,
        command_actions_from_tool_input(tool_name, command, input),
    )
}

pub(super) fn tool_start_item_from_result(
    tool_call_id: &str,
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    display_kind: ToolDisplayKind,
    preparation_feedback: ToolPreparationFeedback,
    summary: &str,
) -> ToolStartItem {
    tool_start_item(
        tool_call_id,
        tool_name,
        command,
        input,
        display_kind,
        preparation_feedback,
        command_actions_from_tool_result(tool_name, command, input, summary),
    )
}

pub(super) fn command_display_from_input(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "exec_command" => input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        "write_stdin" => {
            let process_id = input
                .get("process_id")
                .or_else(|| input.get("session_id"))
                .and_then(serde_json::Value::as_i64)
                .map(|id| id.to_string())
                .unwrap_or_else(|| "?".to_string());
            let chars = input
                .get("chars")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if chars.is_empty() {
                format!("poll process {process_id}")
            } else {
                format!("write_stdin process {process_id}")
            }
        }
        "read" => {
            let path = input
                .get("filePath")
                .or_else(|| input.get("path"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("read {path}")
        }
        "find" | "glob" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let command_name = if tool_name == "find" { "find" } else { "glob" };
            if path.is_empty() {
                format!("{command_name} {pattern}")
            } else {
                format!("{command_name} {pattern} in {path}")
            }
        }
        "grep" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if path.is_empty() {
                format!("grep {pattern}")
            } else {
                format!("grep {pattern} in {path}")
            }
        }
        "code_search" | "mcp__code_search__code_search" => code_search_display_from_input(input),
        _ => String::new(),
    }
}

pub(super) fn command_actions_from_tool_input(
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    crate::tool_actions::exploration_actions_from_tool_input(tool_name, command, input)
}

fn code_search_display_from_input(input: &serde_json::Value) -> String {
    match input
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("search")
    {
        "find_related" => {
            let path = input
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let line = input.get("line").and_then(serde_json::Value::as_u64);
            match (path.is_empty(), line) {
                (false, Some(line)) => format!("code_search related {path}:{line}"),
                (false, None) => format!("code_search related {path}"),
                (true, _) => "code_search related".to_string(),
            }
        }
        _ => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            match (query.is_empty(), path.is_empty()) {
                (false, false) => format!("code_search {query} in {path}"),
                (false, true) => format!("code_search {query}"),
                (true, false) => format!("code_search in {path}"),
                (true, true) => "code_search".to_string(),
            }
        }
    }
}

pub(super) fn command_actions_from_tool_result(
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
    summary: &str,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    let actions = command_actions_from_tool_input(tool_name, command, input);
    if !actions.is_empty() {
        return actions;
    }
    match tool_name {
        "read" => crate::tool_actions::read_action_from_tool_summary(summary)
            .into_iter()
            .collect(),
        _ => actions,
    }
}

pub(super) fn command_execution_item_id_for_progress(
    pending_tool_calls: &HashMap<String, PendingToolCall>,
    tool_use_id: &str,
) -> Option<ItemId> {
    pending_tool_calls
        .get(tool_use_id)
        .and_then(|pending| pending.item_id)
}

pub(super) fn user_shell_exec_input(command: &str, cwd: std::path::PathBuf) -> serde_json::Value {
    serde_json::json!({
        "cmd": command,
        "workdir": cwd,
        "login": true,
        "tty": true,
    })
}

pub(super) fn user_shell_command_payload(
    tool_call_id: &str,
    command: &str,
    input: serde_json::Value,
    command_actions: Vec<devo_protocol::parse_command::ParsedCommand>,
    output: Option<serde_json::Value>,
    is_error: bool,
) -> CommandExecutionPayload {
    CommandExecutionPayload {
        tool_call_id: tool_call_id.to_string(),
        tool_name: "exec_command".to_string(),
        command: command.to_string(),
        input: Some(input),
        source: devo_protocol::protocol::ExecCommandSource::UserShell,
        command_actions,
        output,
        is_error,
    }
}

const AGENT_COORDINATION_TOOL_NAMES: &[&str] = &[
    "spawn_agent",
    "send_message",
    "await_task",
    "list_tasks",
    "cancel_task",
    "wait_agent",
    "list_agents",
    "close_agent",
];

pub(super) fn without_agent_coordination_tools(
    registry: &devo_core::tools::ToolRegistry,
) -> devo_core::tools::ToolRegistry {
    let names = registry
        .tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .filter(|name| {
            !AGENT_COORDINATION_TOOL_NAMES
                .iter()
                .any(|hidden_name| *hidden_name == name)
        })
        .collect::<Vec<_>>();
    let names = names.iter().map(String::as_str).collect::<Vec<_>>();
    registry.restricted_to_specs(&names)
}

#[cfg(test)]
mod tests {
    use devo_core::{ToolCallItem, TurnItem};
    use devo_protocol::SessionHistoryMetadata;
    use pretty_assertions::assert_eq;

    use super::{command_actions_from_tool_input, command_display_from_input};
    use crate::projection::history_item_from_turn_item;

    #[test]
    fn live_and_replayed_exploration_actions_are_identical() {
        let cases = [
            (
                "read",
                serde_json::json!({
                    "filePath": "crates/server/src/projection.rs",
                    "offset": 20,
                    "limit": 10
                }),
            ),
            (
                "glob",
                serde_json::json!({
                    "pattern": "**/*.rs",
                    "path": "crates/server/src"
                }),
            ),
            (
                "grep",
                serde_json::json!({
                    "pattern": "SessionHistoryMetadata",
                    "path": "crates/server/src"
                }),
            ),
            (
                "code_search",
                serde_json::json!({
                    "operation": "search",
                    "query": "restored exploration metadata",
                    "path": "crates/server/src"
                }),
            ),
            (
                "code_search",
                serde_json::json!({
                    "operation": "find_related",
                    "file_path": "crates/server/src/projection.rs",
                    "line": 214
                }),
            ),
        ];

        for (tool_name, input) in cases {
            let projected = history_item_from_turn_item(&TurnItem::ToolCall(ToolCallItem {
                tool_call_id: "call-1".to_string(),
                tool_name: tool_name.to_string(),
                input: input.clone(),
            }))
            .expect("history item");
            let replay_actions = match projected.metadata.expect("explored metadata") {
                SessionHistoryMetadata::Explored { actions } => actions,
                other => panic!("unexpected metadata: {other:?}"),
            };

            assert_eq!(
                command_actions_from_tool_input(tool_name, &projected.title, &input),
                replay_actions,
                "live and replay actions differ for {tool_name}"
            );
        }
    }

    #[test]
    fn write_stdin_display_prefers_process_id_and_reads_legacy_session_id() {
        assert_eq!(
            command_display_from_input(
                "write_stdin",
                &serde_json::json!({
                    "process_id": 42,
                    "session_id": 99,
                    "chars": "hello"
                }),
            ),
            "write_stdin process 42"
        );
        assert_eq!(
            command_display_from_input(
                "write_stdin",
                &serde_json::json!({ "session_id": 99, "chars": "" }),
            ),
            "poll process 99"
        );
    }
}
