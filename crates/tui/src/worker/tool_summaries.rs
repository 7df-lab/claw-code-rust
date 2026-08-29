//! Tool call summary strings and lifecycle projection helpers.

use std::path::PathBuf;

use devo_protocol::ToolCallPayload;

use crate::events::PlanStepStatus;
use crate::events::WorkerEvent;
use crate::transcript::lifecycle::ItemLifecycleEvent;

pub(crate) fn summarize_tool_result_title(tool_name: Option<&str>, is_error: bool) -> String {
    match (tool_name, is_error) {
        (Some(tool_name), true) => format!("{tool_name} error"),
        (Some(tool_name), false) => format!("{tool_name} output"),
        (None, true) => "Tool error".to_string(),
        (None, false) => "Tool output".to_string(),
    }
}

pub(crate) fn tool_call_started_event(payload: ToolCallPayload) -> WorkerEvent {
    WorkerEvent::Transcript(super::tool_lifecycle::tool_opened_from_call(&payload))
}

pub(crate) fn summarize_tool_call(payload: &ToolCallPayload) -> String {
    if is_web_search_tool_name(&payload.tool_name)
        && let Some(query) = web_search_query(&payload.parameters)
    {
        return format!("Web Search({})", serde_json::Value::String(query));
    }
    if is_web_fetch_tool_name(&payload.tool_name)
        && let Some(url) = web_fetch_url(&payload.parameters)
    {
        return format!("Web Fetch({})", serde_json::Value::String(url));
    }

    match pretty_tool_call_summary(&payload.tool_name, &payload.parameters) {
        Some(summary) => summary,
        None => {
            let detail = summarize_tool_input(&payload.tool_name, &payload.parameters);
            if detail.is_empty() {
                payload.tool_name.clone()
            } else {
                format!("{} {detail}", payload.tool_name)
            }
        }
    }
}

fn pretty_tool_call_summary(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let quote = |text: &str| serde_json::Value::String(compact_tool_summary(text, 96)).to_string();
    let path_value = || {
        input
            .get("filePath")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("path").and_then(serde_json::Value::as_str))
            .map(make_path_relative)
    };
    match tool_name {
        "bash" | "shell_command" | "exec_command" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("cmd").and_then(serde_json::Value::as_str))
            .map(|command| format!("Shell {}", compact_tool_summary(command, 96))),
        "read" => path_value().map(|path| format!("Read {path}{}", fmt_line_range(input))),
        "write" => path_value().map(|path| format!("Write {path}")),
        "edit" => Some("Edit".to_string()),
        "apply_patch" => path_value().map(|path| format!("Patch {path}")),
        "find" | "glob" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(make_path_relative)
            .or_else(|| {
                input
                    .get("pattern")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
            .map(|path| format!("List {path}")),
        "grep" => {
            let pattern = input.get("pattern").and_then(serde_json::Value::as_str)?;
            let query = quote(pattern);
            match input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(make_path_relative)
            {
                Some(path) => Some(format!("Search {query} in {path}")),
                None => Some(format!("Search {query}")),
            }
        }
        "code_search" | "mcp__code_search__code_search" => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("pattern").and_then(serde_json::Value::as_str))
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("file_path").and_then(serde_json::Value::as_str))
                .map(make_path_relative);
            match (query.is_empty(), path) {
                (false, Some(path)) => Some(format!("Code-Search {} in {path}", quote(query))),
                (false, None) => Some(format!("Code-Search {}", quote(query))),
                (true, Some(path)) => Some(format!("Code-Search in {path}")),
                (true, None) => Some("Code-Search".to_string()),
            }
        }
        "spawn_agent" | "agent_spawn" => {
            let nickname = input
                .get("agent_nickname")
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("nickname").and_then(serde_json::Value::as_str))
                .or_else(|| input.get("agent_path").and_then(serde_json::Value::as_str))
                .unwrap_or("agent");
            let prompt = input
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("prompt").and_then(serde_json::Value::as_str))
                .unwrap_or_default();
            Some(format!("Spawn-Agent {} {}", quote(nickname), quote(prompt)))
        }
        "await_task" | "wait_agent" | "agent_wait" => {
            let target = input
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("target").and_then(serde_json::Value::as_str))
                .or_else(|| {
                    input
                        .get("agent_nickname")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("agent");
            let timeout = input
                .get("timeout_secs")
                .and_then(serde_json::Value::as_u64)
                .map(|secs| format!("{secs}s"))
                .or_else(|| {
                    input
                        .get("timeout")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                })
                .unwrap_or_else(|| "default".to_string());
            Some(format!("Await-Task {} {}", quote(target), quote(&timeout)))
        }
        "cancel_task" | "close_agent" | "agent_close" => {
            let target = input
                .get("task_id")
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("target").and_then(serde_json::Value::as_str))
                .or_else(|| {
                    input
                        .get("agent_nickname")
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("agent");
            Some(format!("Cancel-Task {}", quote(target)))
        }
        "list_tasks" | "list_agents" | "list_agent" | "agent_list" => {
            Some("List-Tasks".to_string())
        }
        _ => None,
    }
}

fn is_web_search_tool_name(tool_name: &str) -> bool {
    matches!(tool_name, "web_search" | "websearch" | "web-search")
}

fn is_web_fetch_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url"
    )
}

fn web_search_query(input: &serde_json::Value) -> Option<String> {
    input
        .get("query")
        .and_then(serde_json::Value::as_str)
        .filter(|query| !query.is_empty())
        .map(ToString::to_string)
}

fn web_fetch_url(input: &serde_json::Value) -> Option<String> {
    input
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn summarize_tool_call_update(payload: &ToolCallPayload) -> String {
    let summary = summarize_tool_call(payload);
    if payload.tool_name == "read"
        && summary == "read {}"
        && let Some(cmd) = payload
            .command_actions
            .iter()
            .find_map(|action| match action {
                devo_protocol::parse_command::ParsedCommand::Read { cmd, .. }
                    if !cmd.is_empty() =>
                {
                    Some(cmd.clone())
                }
                _ => None,
            })
    {
        return cmd;
    }
    if matches!(payload.tool_name.as_str(), "find" | "glob")
        && (summary == "find {}" || summary == "glob {}")
        && let Some(cmd) = payload
            .command_actions
            .iter()
            .find_map(|action| match action {
                devo_protocol::parse_command::ParsedCommand::ListFiles { cmd, .. }
                    if !cmd.is_empty() =>
                {
                    Some(cmd.clone())
                }
                _ => None,
            })
    {
        return cmd;
    }
    summary
}

fn read_command_action_from_parameters(
    command: &str,
    input: &serde_json::Value,
) -> Option<devo_protocol::parse_command::ParsedCommand> {
    let path = input
        .get("filePath")
        .or_else(|| input.get("path"))
        .and_then(serde_json::Value::as_str)?
        .trim();
    if path.is_empty() {
        return None;
    }
    let mut name = path.to_string();
    let offset = input.get("offset").and_then(serde_json::Value::as_u64);
    let limit = input.get("limit").and_then(serde_json::Value::as_u64);
    match (offset, limit) {
        (Some(offset), Some(limit)) => {
            let end = offset.saturating_add(limit.saturating_sub(1));
            name.push_str(&format!(" L:{offset}-{end}"));
        }
        (Some(offset), None) => name.push_str(&format!(" L:{offset}-")),
        (None, Some(limit)) => name.push_str(&format!(" L:1-{limit}")),
        (None, None) => {}
    }
    Some(devo_protocol::parse_command::ParsedCommand::Read {
        cmd: command.to_string(),
        name,
        path: PathBuf::from(path),
    })
}

fn find_command_action_from_parameters(
    command: &str,
    input: &serde_json::Value,
) -> Option<devo_protocol::parse_command::ParsedCommand> {
    let pattern = input
        .get("pattern")
        .and_then(serde_json::Value::as_str)
        .filter(|pattern| !pattern.is_empty())?;
    let path = input.get("path").and_then(serde_json::Value::as_str);
    let display = match path.filter(|path| !path.is_empty()) {
        Some(path) => format!("{pattern} in {path}"),
        None => pattern.to_string(),
    };
    Some(devo_protocol::parse_command::ParsedCommand::ListFiles {
        cmd: command.to_string(),
        path: Some(display),
    })
}

/// Mirrors `server/tool_actions.rs::exploration_actions_from_tool_input` for live-session parity with resume.
pub(crate) fn exploration_actions_from_tool_input(
    tool_name: &str,
    command: &str,
    input: &serde_json::Value,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    match tool_name {
        "read" => read_command_action_from_parameters(command, input)
            .into_iter()
            .collect(),
        "find" | "glob" => vec![devo_protocol::parse_command::ParsedCommand::ListFiles {
            cmd: command.to_string(),
            path: find_command_action_from_parameters(tool_name, input).and_then(|parsed| {
                match parsed {
                    devo_protocol::parse_command::ParsedCommand::ListFiles { path, .. } => path,
                    _ => None,
                }
            }),
        }],
        "grep" => vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: command.to_string(),
            query: input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            path: input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
        }],
        "code_search" | "mcp__code_search__code_search" => {
            code_search_command_action_from_parameters(command, input)
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

pub(crate) fn tool_call_started_actions(
    payload: &ToolCallPayload,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    if !payload.command_actions.is_empty() {
        return payload.command_actions.clone();
    }
    let command = summarize_tool_call(payload);
    let explored =
        exploration_actions_from_tool_input(&payload.tool_name, &command, &payload.parameters);
    if !explored.is_empty() {
        return explored;
    }
    if payload.tool_name == "read" {
        return vec![
            read_command_action_from_parameters("read", &payload.parameters).unwrap_or_else(|| {
                devo_protocol::parse_command::ParsedCommand::Read {
                    cmd: String::new(),
                    name: String::new(),
                    path: PathBuf::new(),
                }
            }),
        ];
    }
    Vec::new()
}

pub(crate) fn tool_call_updated_actions(
    payload: &ToolCallPayload,
    summary: &str,
) -> Vec<devo_protocol::parse_command::ParsedCommand> {
    if !payload.command_actions.is_empty() {
        return payload.command_actions.clone();
    }
    let explored =
        exploration_actions_from_tool_input(&payload.tool_name, summary, &payload.parameters);
    if !explored.is_empty() {
        return explored;
    }
    match payload.tool_name.as_str() {
        "read" => read_command_action_from_parameters(summary, &payload.parameters)
            .into_iter()
            .collect(),
        "find" | "glob" => find_command_action_from_parameters(summary, &payload.parameters)
            .into_iter()
            .collect(),
        "code_search" | "mcp__code_search__code_search" => {
            code_search_command_action_from_parameters(summary, &payload.parameters)
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

fn code_search_command_action_from_parameters(
    command: &str,
    input: &serde_json::Value,
) -> Option<devo_protocol::parse_command::ParsedCommand> {
    match input
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("search")
    {
        "find_related" => {
            let path = input
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.is_empty())?;
            let line = input
                .get("line")
                .and_then(serde_json::Value::as_u64)
                .map(|line| line.to_string())
                .unwrap_or_else(|| "?".to_string());
            Some(devo_protocol::parse_command::ParsedCommand::Search {
                cmd: command.to_string(),
                query: Some(format!("related {path}:{line}")),
                path: Some(path.to_string()),
            })
        }
        _ => {
            let query = input
                .get("query")
                .and_then(serde_json::Value::as_str)
                .filter(|query| !query.is_empty())?;
            Some(devo_protocol::parse_command::ParsedCommand::Search {
                cmd: command.to_string(),
                query: Some(query.to_string()),
                path: input
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
            })
        }
    }
}

fn make_path_relative(path: &str) -> String {
    let p = std::path::PathBuf::from(path);
    if p.is_absolute()
        && let Ok(cwd) = std::env::current_dir()
        && let Ok(rel) = p.strip_prefix(&cwd)
    {
        return rel.to_string_lossy().to_string();
    }
    path.to_string()
}

fn code_search_summary_from_input(input: &serde_json::Value) -> String {
    match input
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("search")
    {
        "find_related" => {
            let path = input
                .get("file_path")
                .and_then(serde_json::Value::as_str)
                .map(make_path_relative);
            let line = input.get("line").and_then(serde_json::Value::as_u64);
            match (path, line) {
                (Some(path), Some(line)) => format!("related {path}:{line}"),
                (Some(path), None) => format!("related {path}"),
                (None, _) => "related".to_string(),
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
                .map(make_path_relative);
            match (query.is_empty(), path) {
                (false, Some(path)) => format!("{query} in {path}"),
                (false, None) => query.to_string(),
                (true, Some(path)) => format!("in {path}"),
                (true, None) => String::new(),
            }
        }
    }
}

fn fmt_offset_limit(input: &serde_json::Value) -> String {
    let offset = input.get("offset").and_then(|v| v.as_u64());
    let limit = input.get("limit").and_then(|v| v.as_u64());
    match (offset, limit) {
        (Some(o), Some(l)) => format!(" (offset:{o}, limit:{l})"),
        (Some(o), None) => format!(" (offset:{o})"),
        (None, Some(l)) => format!(" (limit:{l})"),
        (None, None) => String::new(),
    }
}

fn fmt_line_range(input: &serde_json::Value) -> String {
    let offset = input.get("offset").and_then(serde_json::Value::as_u64);
    let limit = input.get("limit").and_then(serde_json::Value::as_u64);
    match (offset, limit) {
        (Some(start), Some(limit)) => {
            let end = start.saturating_add(limit.saturating_sub(1));
            format!(" L:{start}-{end}")
        }
        (Some(start), None) => format!(" L:{start}"),
        (None, Some(limit)) => format!(" L:0-{limit}"),
        (None, None) => String::new(),
    }
}

fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    let candidate = match tool_name {
        "bash" | "shell_command" | "exec_command" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("cmd").and_then(serde_json::Value::as_str))
            .map(|s| s.to_string()),
        "read" => input
            .get("filePath")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("path").and_then(serde_json::Value::as_str))
            .map(|path| {
                let rel = make_path_relative(path);
                let ext = fmt_offset_limit(input);
                format!("{rel}{ext}")
            }),
        "write" | "edit" | "apply_patch" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| input.get("filePath").and_then(serde_json::Value::as_str))
            .map(make_path_relative),
        "grep" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(make_path_relative);
            match path {
                Some(p) => Some(format!("'{pattern}' in {p}")),
                None => Some(format!("'{pattern}'")),
            }
        }
        "find" | "glob" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(make_path_relative);
            match path {
                Some(p) => Some(format!("{pattern} in {p}")),
                None => Some(pattern.to_string()),
            }
        }
        "code_search" | "mcp__code_search__code_search" => {
            Some(code_search_summary_from_input(input))
        }
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url" => web_fetch_url(input),
        "web_search" | "websearch" | "web-search" => web_search_query(input),
        "lsp" => {
            let path = input
                .get("filePath")
                .and_then(serde_json::Value::as_str)
                .map(make_path_relative);
            let line = input.get("line").and_then(|v| v.as_i64());
            let col = input.get("character").and_then(|v| v.as_i64());
            match (path, line, col) {
                (Some(p), Some(l), Some(c)) => Some(format!("{p}:{l}:{c}")),
                (Some(p), Some(l), None) => Some(format!("{p}:{l}")),
                (Some(p), None, _) => Some(p),
                _ => None,
            }
        }
        "question" => None,
        "skill" => input
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(|s| s.to_string()),
        "spawn_agent" => input
            .get("message")
            .and_then(serde_json::Value::as_str)
            .filter(|message| !message.is_empty())
            .map(|message| message.to_string()),
        _ => None,
    };

    candidate
        .map(|text| compact_tool_summary(&text, 96))
        .unwrap_or_else(|| compact_tool_summary(&render_json_preview(input), 96))
}

fn compact_tool_summary(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = compact.chars().count() > max_chars;
    let mut out = compact.chars().take(max_chars).collect::<String>();
    if truncated {
        out.push('…');
    }
    out
}

fn render_json_preview(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => truncate_tool_output(text),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
            truncate_tool_output(&pretty)
        }
        _ => truncate_tool_output(&value.to_string()),
    }
}

pub(crate) fn render_json_value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

pub(crate) fn parse_plan_step_status(status: &str) -> Option<PlanStepStatus> {
    match status {
        "pending" => Some(PlanStepStatus::Pending),
        "in_progress" => Some(PlanStepStatus::InProgress),
        "completed" => Some(PlanStepStatus::Completed),
        "cancelled" => Some(PlanStepStatus::Cancelled),
        _ => None,
    }
}

pub(crate) fn truncate_tool_output(content: &str) -> String {
    const MAX_LINES: usize = 8;
    const MAX_CHARS: usize = 1200;
    let content = normalize_display_output(content);
    let content = content.as_str();

    let mut lines = Vec::new();
    let mut chars = 0usize;
    for line in content.lines() {
        if lines.len() >= MAX_LINES || chars >= MAX_CHARS {
            break;
        }
        let remaining = MAX_CHARS.saturating_sub(chars);
        if line.chars().count() > remaining {
            let preview = line.chars().take(remaining).collect::<String>();
            lines.push(preview);
            break;
        }
        chars += line.chars().count();
        lines.push(line.to_string());
    }

    if lines.is_empty() && !content.is_empty() {
        let preview = content.chars().take(MAX_CHARS).collect::<String>();
        return if preview == content {
            preview
        } else {
            format!("{preview}\n… ")
        };
    }

    let preview = lines.join("\n");
    if preview == content {
        preview
    } else if preview.is_empty() {
        "… ".to_string()
    } else {
        format!("{preview}\n… ")
    }
}

pub(crate) fn normalize_display_output(content: &str) -> String {
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_matches('\n')
        .to_string()
}

/// Native-first lifecycle projection for tool call start.
pub(crate) fn lifecycle_from_tool_call_started(
    payload: &ToolCallPayload,
) -> Vec<ItemLifecycleEvent> {
    vec![super::tool_lifecycle::tool_opened_from_call(payload)]
}

/// Native-first lifecycle projection for finalized tool call metadata.
pub(crate) fn lifecycle_from_tool_call_completed(
    payload: &ToolCallPayload,
) -> Vec<ItemLifecycleEvent> {
    vec![super::tool_lifecycle::tool_opened_refresh_from_call(
        payload,
    )]
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn grep_exploration_actions_match_server_projection_shape() {
        let input = serde_json::json!({"pattern": "plan", "path": "crates"});
        assert_eq!(
            exploration_actions_from_tool_input("grep", "Search plan in crates", &input),
            vec![devo_protocol::parse_command::ParsedCommand::Search {
                cmd: "Search plan in crates".to_string(),
                query: Some("plan".to_string()),
                path: Some("crates".to_string()),
            }]
        );
    }
}
