//! Test helpers that emit [`WorkerEvent::Transcript`] lifecycle events.
//!
//! Replaces removed tool-specific [`WorkerEvent`] variants in tests.

use std::collections::HashMap;
use std::path::PathBuf;

use devo_protocol::parse_command::ParsedCommand;
use devo_protocol::protocol::ExecCommandSource;
use devo_protocol::protocol::FileChange;

use devo_util_shell_command::parse_command::parse_command;

use devo_core::ItemId;

use crate::events::TextItemKind;
use crate::events::WorkerEvent;
use crate::transcript::lifecycle::ItemLifecycleEvent;
use crate::transcript::tool_state::command_source_from_tool_name;
use crate::transcript::tool_state::shell_command_from_input;

fn transcript(event: ItemLifecycleEvent) -> WorkerEvent {
    WorkerEvent::Transcript(event)
}

/// Shim for removed `WorkerEvent::TextItemStarted`.
pub(crate) fn text_item_started(item_id: ItemId, kind: TextItemKind) -> WorkerEvent {
    transcript(ItemLifecycleEvent::TextStarted {
        item_id,
        kind,
        item_seq: None,
    })
}

/// Shim for removed `WorkerEvent::TextItemDelta`.
pub(crate) fn text_item_delta(
    item_id: ItemId,
    kind: TextItemKind,
    delta: impl Into<String>,
) -> WorkerEvent {
    transcript(ItemLifecycleEvent::TextDelta {
        item_id,
        kind,
        delta: delta.into(),
    })
}

/// Shim for removed `WorkerEvent::TextItemCompleted`.
pub(crate) fn text_item_completed(
    item_id: ItemId,
    kind: TextItemKind,
    final_text: impl Into<String>,
) -> WorkerEvent {
    transcript(ItemLifecycleEvent::TextCompleted {
        item_id,
        kind,
        final_text: final_text.into(),
    })
}

fn infer_tool_from_summary(summary: &str, preparing: bool) -> (String, serde_json::Value) {
    if preparing {
        if summary.strip_prefix("write ").is_some() {
            return ("write".to_string(), serde_json::json!({}));
        }
        if summary == "Edit" {
            return ("edit".to_string(), serde_json::json!({}));
        }
        return ("apply_patch".to_string(), serde_json::json!({}));
    }
    if let Some(query) = summary
        .strip_prefix("Web Search(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return (
            "web_search".to_string(),
            serde_json::json!({ "query": query.trim_matches('"') }),
        );
    }
    if let Some(url) = summary
        .strip_prefix("Web Fetch(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return (
            "web_fetch".to_string(),
            serde_json::json!({ "url": url.trim_matches('"') }),
        );
    }
    if summary == "Edit" {
        return ("edit".to_string(), serde_json::json!({}));
    }
    if summary.starts_with("Code-Search") {
        return ("code_search".to_string(), serde_json::json!({}));
    }
    if let Some(cmd) = summary.strip_prefix("Shell ") {
        return ("bash".to_string(), serde_json::json!({ "command": cmd }));
    }
    if let Some(path) = summary.strip_prefix("Read ") {
        return (
            "read".to_string(),
            serde_json::json!({ "path": path.split_whitespace().next().unwrap_or(path) }),
        );
    }
    if let Some(path) = summary.strip_prefix("List ") {
        return ("glob".to_string(), serde_json::json!({ "path": path }));
    }
    if let Some(path) = summary.strip_prefix("Patch ") {
        return (
            "apply_patch".to_string(),
            serde_json::json!({ "path": path }),
        );
    }
    if summary == "read {}" || summary == "glob {}" || summary == "find {}" {
        let tool_name = summary.split_whitespace().next().unwrap_or("tool");
        return (tool_name.to_string(), serde_json::json!({}));
    }
    if summary.contains("powershell")
        || summary.starts_with("Get-")
        || summary.contains(" -NoProfile")
        || summary.contains(" -Command")
    {
        return (
            "bash".to_string(),
            serde_json::json!({ "command": summary }),
        );
    }
    if matches!(
        summary,
        "apply_patch" | "code_search" | "read" | "glob" | "grep" | "write" | "edit"
    ) {
        return (summary.to_string(), serde_json::json!({}));
    }
    (summary.to_string(), serde_json::json!({}))
}

fn parsed_commands_for_tool(
    summary: &str,
    input: &serde_json::Value,
    tool_name: &str,
    parsed_commands: Option<Vec<ParsedCommand>>,
) -> Vec<ParsedCommand> {
    if let Some(parsed_commands) = parsed_commands.filter(|parsed| !parsed.is_empty()) {
        return parsed_commands;
    }
    let explored = crate::worker::exploration_actions_from_tool_input(tool_name, summary, input);
    if !explored.is_empty() {
        return explored;
    }
    if matches!(
        tool_name,
        "web_search" | "web_fetch" | "websearch" | "web-search" | "webfetch"
    ) {
        return Vec::new();
    }
    if let Some(command) = shell_command_from_input(input) {
        return parse_command(&crate::exec_command::split_command_string(&command));
    }
    if summary.trim().is_empty() {
        return Vec::new();
    }
    parse_command(&crate::exec_command::split_command_string(summary))
}

fn tool_opened_event(
    tool_use_id: String,
    tool_name: String,
    input: serde_json::Value,
    parsed_commands: Option<Vec<ParsedCommand>>,
    summary: &str,
) -> WorkerEvent {
    let parsed_commands = parsed_commands_for_tool(summary, &input, &tool_name, parsed_commands);
    let command = shell_command_from_input(&input).or_else(|| {
        if matches!(
            tool_name.as_str(),
            "bash" | "shell_command" | "exec_command" | "shell" | "write_stdin"
        ) {
            Some(summary.to_string())
        } else {
            None
        }
    });
    transcript(ItemLifecycleEvent::ToolOpened {
        tool_use_id,
        tool_name: tool_name.clone(),
        command,
        command_source: command_source_from_tool_name(&tool_name),
        input,
        item_seq: None,
        parsed_commands,
    })
}

/// Shim for removed `WorkerEvent::ToolCall`.
pub(crate) fn tool_call(
    tool_use_id: String,
    summary: String,
    preparing: bool,
    parsed_commands: Option<Vec<ParsedCommand>>,
) -> WorkerEvent {
    let (tool_name, input) = infer_tool_from_summary(&summary, preparing);
    tool_opened_event(tool_use_id, tool_name, input, parsed_commands, &summary)
}

/// Shim for removed `WorkerEvent::ToolCallUpdated`.
pub(crate) fn tool_call_updated(
    tool_use_id: String,
    summary: String,
    parsed_commands: Vec<ParsedCommand>,
) -> WorkerEvent {
    let (tool_name, input) = infer_tool_from_summary(&summary, /*preparing*/ false);
    tool_opened_event(
        tool_use_id,
        tool_name,
        input,
        Some(parsed_commands),
        &summary,
    )
}

/// Shim for removed `WorkerEvent::ToolCallDetails`.
pub(crate) fn tool_call_details(
    tool_use_id: String,
    tool_name: String,
    input: serde_json::Value,
) -> WorkerEvent {
    tool_opened_event(tool_use_id, tool_name, input, None, "")
}

/// Shim for removed `WorkerEvent::ToolResult`.
pub(crate) fn tool_result(
    tool_use_id: String,
    title: String,
    preview: String,
    is_error: bool,
    truncated: bool,
) -> WorkerEvent {
    let (tool_name, input) = infer_tool_from_summary(&title, /*preparing*/ false);
    transcript(ItemLifecycleEvent::ToolClosed {
        tool_use_id,
        tool_name,
        input,
        output: Some(serde_json::Value::String(preview.clone())),
        display_content: Some(preview),
        file_changes: None,
        is_error,
        truncated,
    })
}

/// Shim for removed `WorkerEvent::ToolResultIo`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tool_result_io(
    tool_use_id: String,
    tool_name: String,
    title: String,
    input: serde_json::Value,
    output: serde_json::Value,
    display_content: Option<String>,
    is_error: bool,
    truncated: bool,
) -> WorkerEvent {
    let _ = title;
    let display = display_content.or_else(|| {
        output
            .as_str()
            .map(str::to_string)
            .or_else(|| Some(output.to_string()))
    });
    transcript(ItemLifecycleEvent::ToolClosed {
        tool_use_id,
        tool_name,
        input,
        output: Some(output),
        display_content: display,
        file_changes: None,
        is_error,
        truncated,
    })
}

/// Shim for removed `WorkerEvent::ToolOutputDelta`.
pub(crate) fn tool_output_delta(tool_use_id: String, delta: String) -> WorkerEvent {
    transcript(ItemLifecycleEvent::ToolOutputChunk {
        tool_use_id,
        chunk: delta,
    })
}

/// Shim for removed `WorkerEvent::ToolInputDelta`.
pub(crate) fn tool_input_delta(tool_use_id: String, delta: String) -> WorkerEvent {
    transcript(ItemLifecycleEvent::ToolInputChunk {
        tool_use_id,
        chunk: delta,
    })
}

/// Shim for removed `WorkerEvent::CommandExecutionStarted`.
pub(crate) fn command_execution_started(
    tool_use_id: String,
    command: String,
    input: Option<serde_json::Value>,
    source: ExecCommandSource,
    command_actions: Vec<ParsedCommand>,
) -> WorkerEvent {
    let input = input.unwrap_or_else(|| serde_json::json!({ "command": command.clone() }));
    transcript(ItemLifecycleEvent::ToolOpened {
        tool_use_id,
        tool_name: "exec_command".to_string(),
        input,
        item_seq: None,
        command: Some(command),
        command_source: Some(source),
        parsed_commands: command_actions,
    })
}

/// Shim for removed `WorkerEvent::PatchApplied`.
pub(crate) fn patch_applied(
    tool_use_id: String,
    changes: HashMap<PathBuf, FileChange>,
) -> WorkerEvent {
    transcript(ItemLifecycleEvent::ToolClosed {
        tool_use_id,
        tool_name: "apply_patch".to_string(),
        input: serde_json::Value::Null,
        output: None,
        display_content: None,
        file_changes: Some(changes),
        is_error: false,
        truncated: false,
    })
}

/// Shim for removed `WorkerEvent::PatchAppliedIo`.
pub(crate) fn patch_applied_io(
    tool_use_id: String,
    tool_name: String,
    input: serde_json::Value,
    changes: HashMap<PathBuf, FileChange>,
) -> WorkerEvent {
    transcript(ItemLifecycleEvent::ToolClosed {
        tool_use_id,
        tool_name,
        input,
        output: None,
        display_content: None,
        file_changes: Some(changes),
        is_error: false,
        truncated: false,
    })
}
