//! Semantic tool verb pairs and title formatting for the transcript.
//!
//! Maps tool names + parameters + lifecycle phase to user-facing labels such as
//! `Reading foo.rs` / `Read foo.rs`, independent of legacy summary strings.

use std::path::Path;

use devo_protocol::parse_command::ParsedCommand;
use ratatui::prelude::*;
use ratatui::style::Style;
use ratatui::style::Stylize;

use crate::agent_tool_cell::is_agent_task_tool_name;
use crate::transcript::model::ToolPhase;
use crate::ui_consts::COMPLETED_COLOR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolVerbKind {
    Reasoning,
    Read,
    Write,
    Edit,
    Shell,
    Grep,
    Find,
    Skill,
    Generic,
}

impl ToolVerbKind {
    pub(crate) fn from_tool_name(tool_name: &str) -> Self {
        match tool_name {
            "read" => Self::Read,
            "write" => Self::Write,
            "edit" | "apply_patch" => Self::Edit,
            "bash" | "shell_command" | "exec_command" | "write_stdin" => Self::Shell,
            "grep" => Self::Grep,
            "find" | "glob" => Self::Find,
            "skill" => Self::Skill,
            _ => Self::Generic,
        }
    }

    fn running_verb(self) -> &'static str {
        match self {
            Self::Reasoning => "Thinking",
            Self::Read => "Reading",
            Self::Write => "Writing",
            Self::Edit => "Editing",
            Self::Shell => "Running",
            Self::Grep => "Grepping",
            Self::Find => "Finding",
            Self::Skill => "Loading",
            Self::Generic => "Running",
        }
    }

    fn completed_verb(self, tool_name: &str, change_is_add: bool) -> &'static str {
        match self {
            Self::Reasoning => "Thought",
            Self::Read => "Read",
            Self::Write if change_is_add => "Wrote",
            Self::Write => "Wrote",
            Self::Edit => "Edited",
            Self::Shell => "Ran",
            Self::Grep => "Grepped",
            Self::Find => "Found",
            Self::Skill => "Loaded",
            Self::Generic => {
                if tool_name == "web_search" || tool_name == "websearch" {
                    return "";
                }
                "Ran"
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ToolTitleParts {
    pub(crate) verb: String,
    pub(crate) detail: String,
}

pub(crate) fn tool_title_parts(
    phase: ToolPhase,
    tool_name: Option<&str>,
    input: Option<&serde_json::Value>,
    parsed_commands: &[ParsedCommand],
    completed_with_add: bool,
    summary_fallback: &str,
) -> ToolTitleParts {
    if phase == ToolPhase::Preparing {
        if summary_fallback == "apply_patch" || tool_name == Some("apply_patch") {
            return ToolTitleParts {
                verb: "Preparing apply_patch...".to_string(),
                detail: String::new(),
            };
        }
        if summary_fallback.starts_with("write ")
            || summary_fallback.starts_with("write:")
            || tool_name == Some("write")
        {
            let detail = path_from_input(input).unwrap_or_else(|| {
                summary_fallback
                    .strip_prefix("write ")
                    .or_else(|| summary_fallback.strip_prefix("write:"))
                    .unwrap_or(summary_fallback)
                    .to_string()
            });
            return ToolTitleParts {
                verb: "Preparing write...".to_string(),
                detail,
            };
        }
        if tool_name == Some("edit") {
            return ToolTitleParts {
                verb: "Preparing edit...".to_string(),
                detail: path_from_input(input).unwrap_or_default(),
            };
        }
        return ToolTitleParts {
            verb: "Preparing...".to_string(),
            detail: String::new(),
        };
    }

    if tool_name.is_some_and(is_agent_task_tool_name) {
        return agent_task_title_parts(tool_name.unwrap(), phase, input);
    }

    if tool_name.is_some_and(super::tool_state::is_shell_tool_name) {
        let completed = phase.is_terminal();
        let verb = if completed { "Ran" } else { "Running" };
        let detail = super::tool_state::shell_description_from_input(input).unwrap_or_else(|| {
            input
                .and_then(super::tool_state::shell_command_from_input)
                .map(|command| compact_shell_explanation(&command))
                .unwrap_or_else(|| normalize_summary_fallback(summary_fallback))
        });
        return ToolTitleParts {
            verb: verb.to_string(),
            detail,
        };
    }

    if let Some(parsed) = parsed_commands.first() {
        return title_from_parsed_command(parsed, phase);
    }

    if summary_fallback.starts_with("Web Search") || summary_fallback.starts_with("Web Fetch") {
        return ToolTitleParts {
            verb: String::new(),
            detail: summary_fallback.to_string(),
        };
    }

    if (tool_name == Some("web_search") || tool_name == Some("websearch"))
        && let Some(query) = input
            .and_then(|value| value.get("query"))
            .and_then(serde_json::Value::as_str)
    {
        return ToolTitleParts {
            verb: String::new(),
            detail: format!("Web Search(\"{query}\")"),
        };
    }
    if (tool_name == Some("web_fetch") || tool_name == Some("webfetch"))
        && let Some(url) = input
            .and_then(|value| value.get("url"))
            .and_then(serde_json::Value::as_str)
    {
        return ToolTitleParts {
            verb: String::new(),
            detail: format!("Web Fetch(\"{url}\")"),
        };
    }

    let tool_name = tool_name.unwrap_or("tool");
    let kind = ToolVerbKind::from_tool_name(tool_name);
    let completed = phase.is_terminal();
    let verb = if completed {
        kind.completed_verb(tool_name, completed_with_add)
            .to_string()
    } else {
        kind.running_verb().to_string()
    };
    let mut detail = detail_from_tool_input(tool_name, input);
    if detail.is_empty() && kind == ToolVerbKind::Generic {
        detail = normalize_summary_fallback(summary_fallback);
        if detail.is_empty() {
            detail = tool_name.to_string();
        }
    }
    ToolTitleParts { verb, detail }
}

fn agent_task_title_parts(
    tool_name: &str,
    phase: ToolPhase,
    input: Option<&serde_json::Value>,
) -> ToolTitleParts {
    let completed = phase.is_terminal();
    match tool_name {
        "spawn_agent" | "agent_spawn" => {
            let nickname = input
                .and_then(|value| {
                    value
                        .get("agent_nickname")
                        .or_else(|| value.get("nickname"))
                        .or_else(|| value.get("agent_path"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("agent");
            ToolTitleParts {
                verb: if completed {
                    "Spawned agent".to_string()
                } else {
                    "Spawning agent".to_string()
                },
                detail: nickname.to_string(),
            }
        }
        "await_task" | "wait_agent" | "agent_wait" => {
            let target = input
                .and_then(|value| {
                    value
                        .get("task_id")
                        .or_else(|| value.get("target"))
                        .or_else(|| value.get("agent_nickname"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("task");
            ToolTitleParts {
                verb: if completed {
                    "Awaited task".to_string()
                } else {
                    "Waiting for task".to_string()
                },
                detail: target.to_string(),
            }
        }
        "list_tasks" | "list_agents" | "list_agent" | "agent_list" => ToolTitleParts {
            verb: if completed {
                "Listed tasks".to_string()
            } else {
                "Listing tasks".to_string()
            },
            detail: String::new(),
        },
        "cancel_task" | "close_agent" | "agent_close" => {
            let target = input
                .and_then(|value| {
                    value
                        .get("task_id")
                        .or_else(|| value.get("target"))
                        .or_else(|| value.get("agent_nickname"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("task");
            ToolTitleParts {
                verb: if completed {
                    "Canceled task".to_string()
                } else {
                    "Canceling task".to_string()
                },
                detail: target.to_string(),
            }
        }
        _ => ToolTitleParts {
            verb: if completed { "Ran" } else { "Running" }.to_string(),
            detail: tool_name.to_string(),
        },
    }
}

fn normalize_summary_fallback(summary: &str) -> String {
    summary
        .strip_prefix("Running ")
        .or_else(|| summary.strip_prefix("Ran "))
        .unwrap_or(summary)
        .to_string()
}

fn compact_shell_explanation(command: &str) -> String {
    const MAX_CHARS: usize = 72;
    if command.chars().count() <= MAX_CHARS {
        return command.to_string();
    }
    format!(
        "{}…",
        command
            .chars()
            .take(MAX_CHARS.saturating_sub(1))
            .collect::<String>()
    )
}

pub(crate) fn tool_title_line(phase: ToolPhase, parts: &ToolTitleParts) -> Line<'static> {
    if phase == ToolPhase::Preparing {
        return Line::from(vec![
            Span::styled(parts.verb.clone(), tool_status_running_style()),
            if parts.detail.is_empty() {
                Span::raw("")
            } else {
                Span::styled(format!(" {}", parts.detail), tool_text_style())
            },
        ]);
    }

    let completed = phase.is_terminal();
    let verb_style = if completed {
        tool_status_done_style()
    } else {
        tool_status_running_style()
    };

    let degraded_suffix = if phase == ToolPhase::Degraded {
        " · result unavailable"
    } else {
        ""
    };

    if parts.verb.is_empty() {
        return Line::from(Span::styled(
            format!("{}{degraded_suffix}", parts.detail),
            tool_text_style(),
        ));
    }

    let detail = if parts.detail.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.detail)
    };

    Line::from(vec![
        Span::styled(parts.verb.clone(), verb_style),
        Span::styled(detail, tool_text_style()),
        Span::styled(degraded_suffix, tool_text_style()),
    ])
}

pub(crate) fn tool_status_running_style() -> Style {
    Style::default().fg(COMPLETED_COLOR).bold()
}

pub(crate) fn tool_status_done_style() -> Style {
    Style::default().fg(COMPLETED_COLOR).bold()
}

fn tool_text_style() -> Style {
    Style::default()
}

pub(crate) fn title_from_parsed_command(
    parsed: &ParsedCommand,
    phase: ToolPhase,
) -> ToolTitleParts {
    let completed = phase.is_terminal();
    match parsed {
        ParsedCommand::Read { name, path, cmd } => {
            let detail = read_display_name(name, path, cmd);
            let verb = if completed { "Read" } else { "Reading" };
            ToolTitleParts {
                verb: verb.to_string(),
                detail,
            }
        }
        ParsedCommand::Search { query, path, cmd } => {
            let detail = match (query, path) {
                (Some(q), Some(p)) => format!("'{q}' in {p}"),
                (Some(q), None) => format!("'{q}'"),
                _ => cmd.clone(),
            };
            let verb = if completed { "Grepped" } else { "Grepping" };
            ToolTitleParts {
                verb: verb.to_string(),
                detail,
            }
        }
        ParsedCommand::ListFiles { path, cmd } => {
            let detail = path.clone().unwrap_or_else(|| cmd.clone());
            let verb = if completed { "Found" } else { "Finding" };
            ToolTitleParts {
                verb: verb.to_string(),
                detail,
            }
        }
        ParsedCommand::Unknown { cmd } => {
            let verb = if completed { "Ran" } else { "Running" };
            ToolTitleParts {
                verb: verb.to_string(),
                detail: cmd.clone(),
            }
        }
    }
}

fn detail_from_tool_input(tool_name: &str, input: Option<&serde_json::Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    match ToolVerbKind::from_tool_name(tool_name) {
        ToolVerbKind::Read => path_from_input(Some(input))
            .map(|path| format!("{path}{}", line_range_suffix(input)))
            .unwrap_or_default(),
        ToolVerbKind::Write | ToolVerbKind::Edit => {
            path_from_input(Some(input)).unwrap_or_default()
        }
        ToolVerbKind::Shell => super::tool_state::shell_description_from_input(Some(input))
            .unwrap_or_else(|| {
                super::tool_state::shell_command_from_input(input)
                    .map(|command| compact_shell_explanation(&command))
                    .unwrap_or_default()
            }),
        ToolVerbKind::Grep => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            match input.get("path").and_then(serde_json::Value::as_str) {
                Some(path) => format!("'{pattern}' in {path}"),
                None => format!("'{pattern}'"),
            }
        }
        ToolVerbKind::Find => input
            .get("pattern")
            .or_else(|| input.get("path"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ToolVerbKind::Skill => {
            let name = input
                .get("name")
                .or_else(|| input.get("skill"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if path.is_empty() {
                name.to_string()
            } else if name.is_empty() {
                path.to_string()
            } else {
                format!("{name} ({path})")
            }
        }
        ToolVerbKind::Generic => input
            .get("query")
            .or_else(|| input.get("url"))
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_default(),
        ToolVerbKind::Reasoning => String::new(),
    }
}

fn path_from_input(input: Option<&serde_json::Value>) -> Option<String> {
    input.and_then(|input| {
        input
            .get("filePath")
            .or_else(|| input.get("path"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    })
}

fn line_range_suffix(input: &serde_json::Value) -> String {
    let offset = input.get("offset").and_then(serde_json::Value::as_u64);
    let limit = input.get("limit").and_then(serde_json::Value::as_u64);
    match (offset, limit) {
        (Some(start), Some(limit)) => format!(" L:{start}-{}", start.saturating_add(limit)),
        (Some(start), None) => format!(" L:{start}"),
        (None, Some(limit)) => format!(" L:0-{limit}"),
        (None, None) => String::new(),
    }
}

fn read_display_name(name: &str, path: &Path, cmd: &str) -> String {
    if !name.is_empty() {
        return name.to_string();
    }
    if let Some(file_name) = path.file_name() {
        return file_name.to_string_lossy().to_string();
    }
    let path = path.to_string_lossy();
    if !path.is_empty() {
        return path.to_string();
    }
    cmd.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn read_uses_reading_and_read_verbs() {
        let input = serde_json::json!({"filePath": "src/lib.rs", "offset": 10, "limit": 5});
        let running = tool_title_parts(
            ToolPhase::Running,
            Some("read"),
            Some(&input),
            &[],
            false,
            "",
        );
        assert_eq!(running.verb, "Reading");
        assert!(running.detail.contains("src/lib.rs"));

        let done = tool_title_parts(
            ToolPhase::Completed,
            Some("read"),
            Some(&input),
            &[],
            false,
            "",
        );
        assert_eq!(done.verb, "Read");
    }

    #[test]
    fn write_uses_writing_and_wrote_verbs() {
        let input = serde_json::json!({"filePath": "src/lib.rs"});
        let running = tool_title_parts(
            ToolPhase::Running,
            Some("write"),
            Some(&input),
            &[],
            false,
            "",
        );
        assert_eq!(running.verb, "Writing");
        assert_eq!(running.detail, "src/lib.rs");
    }

    #[test]
    fn grep_uses_grepping_and_grepped_verbs() {
        let parsed = vec![devo_protocol::parse_command::ParsedCommand::Search {
            cmd: "grep plan in crates".to_string(),
            query: Some("plan".to_string()),
            path: Some("crates".to_string()),
        }];
        let running = tool_title_parts(ToolPhase::Running, Some("grep"), None, &parsed, false, "");
        assert_eq!(running.verb, "Grepping");
        assert_eq!(running.detail, "'plan' in crates");
    }
}
