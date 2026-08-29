//! Structured transcript rendering for agent spawn, await, and list tools.

use devo_protocol::AwaitTaskResult;
use devo_protocol::ListTasksResult;
use devo_protocol::SpawnAgentResult;
use devo_protocol::TaskInfo;
use devo_protocol::TaskKind;
use devo_protocol::TaskState;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use serde_json::Value;

use crate::history_cell::AgentMessageCell;
use crate::history_cell::HistoryCell;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::transcript::model::ToolPhase;
use crate::transcript::presentation::tool_title_line;
use crate::transcript::presentation::tool_title_parts;
use crate::ui_consts::COMPLETED_COLOR;
use crate::ui_consts::REASONING_ACCENT_COLOR;

const MUTED_COLOR: Color = Color::Rgb(160, 163, 168);
const RUNNING_COLOR: Color = Color::Rgb(106, 200, 255);
const FAILED_COLOR: Color = Color::Rgb(255, 100, 100);

pub(crate) fn is_agent_task_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "spawn_agent"
            | "agent_spawn"
            | "await_task"
            | "wait_agent"
            | "agent_wait"
            | "list_tasks"
            | "list_agents"
            | "list_agent"
            | "agent_list"
            | "cancel_task"
            | "close_agent"
            | "agent_close"
    )
}

#[derive(Debug)]
pub(crate) struct AgentToolCell {
    tool_name: String,
    phase: ToolPhase,
    input: Option<Value>,
    output: Option<Value>,
    display_output: String,
    dot_prefix: Line<'static>,
}

impl AgentToolCell {
    pub(crate) fn new(
        tool_name: String,
        phase: ToolPhase,
        input: Option<Value>,
        output: Option<Value>,
        display_output: String,
        dot_prefix: Line<'static>,
    ) -> Self {
        Self {
            tool_name,
            phase,
            input,
            output,
            display_output,
            dot_prefix,
        }
    }

    fn title_line(&self) -> Line<'static> {
        let parts = tool_title_parts(
            self.phase,
            Some(self.tool_name.as_str()),
            self.input.as_ref(),
            &[],
            false,
            "",
        );
        tool_title_line(self.phase, &parts)
    }

    fn body_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = match self.tool_name.as_str() {
            "spawn_agent" | "agent_spawn" => self.spawn_body_lines(),
            "await_task" | "wait_agent" | "agent_wait" => self.await_body_lines(),
            "list_tasks" | "list_agents" | "list_agent" | "agent_list" => self.list_body_lines(),
            "cancel_task" | "close_agent" | "agent_close" => self.cancel_body_lines(),
            _ => self.fallback_body_lines(),
        };
        for line in &mut lines {
            *line = truncate_line_with_ellipsis_if_overflow(line.clone(), width as usize);
        }
        lines
    }

    fn spawn_body_lines(&self) -> Vec<Line<'static>> {
        let message = spawn_message_from_input(self.input.as_ref());
        if let Some(result) = self
            .output
            .as_ref()
            .and_then(|value| serde_json::from_value::<SpawnAgentResult>(value.clone()).ok())
        {
            let mut lines = vec![task_status_line(
                &result.status,
                &result.agent_nickname,
                Some(&result.agent_path),
            )];
            lines.push(meta_line("task", result.task_id.as_ref()));
            if let Some(message) = message.filter(|text| !text.is_empty()) {
                lines.push(quoted_preview_line(&message));
            }
            return lines;
        }
        message
            .filter(|text| !text.is_empty())
            .map(|text| vec![quoted_preview_line(&text)])
            .unwrap_or_default()
    }

    fn await_body_lines(&self) -> Vec<Line<'static>> {
        let target = await_target_from_input(self.input.as_ref());
        if let Some(result) = self
            .output
            .as_ref()
            .and_then(|value| serde_json::from_value::<AwaitTaskResult>(value.clone()).ok())
        {
            return match result {
                AwaitTaskResult::Terminal { task, output } => {
                    let mut lines = vec![task_info_line(&task)];
                    if let Some(output) = output.filter(|text| !text.trim().is_empty()) {
                        lines.push(quoted_preview_line(&output));
                    }
                    lines
                }
                AwaitTaskResult::TimedOut { task } => {
                    let timeout = await_timeout_label(self.input.as_ref());
                    vec![Line::from(vec![
                        Span::styled("● ", task_state_marker_style(task.state)),
                        Span::styled(task_display_label(&task), Style::default().bold()),
                        Span::styled(format!("  {timeout}"), Style::default().fg(MUTED_COLOR)),
                    ])]
                }
            };
        }
        target
            .map(|label| vec![meta_line("target", &label)])
            .unwrap_or_default()
    }

    fn list_body_lines(&self) -> Vec<Line<'static>> {
        if let Some(result) = self
            .output
            .as_ref()
            .and_then(|value| serde_json::from_value::<ListTasksResult>(value.clone()).ok())
        {
            if result.tasks.is_empty() {
                return vec![Line::from(Span::styled(
                    "  No background tasks",
                    Style::default().fg(MUTED_COLOR).italic(),
                ))];
            }
            let mut lines = vec![Line::from(Span::styled(
                format!(
                    "  {} task{}",
                    result.tasks.len(),
                    if result.tasks.len() == 1 { "" } else { "s" }
                ),
                Style::default().fg(MUTED_COLOR),
            ))];
            for task in &result.tasks {
                lines.push(task_row_line(task));
            }
            return lines;
        }
        self.fallback_body_lines()
    }

    fn cancel_body_lines(&self) -> Vec<Line<'static>> {
        if let Some(task) = self
            .output
            .as_ref()
            .and_then(|value| value.get("task"))
            .and_then(|value| serde_json::from_value::<TaskInfo>(value.clone()).ok())
        {
            return vec![task_info_line(&task)];
        }
        await_target_from_input(self.input.as_ref())
            .map(|label| vec![meta_line("target", &label)])
            .unwrap_or_default()
    }

    fn fallback_body_lines(&self) -> Vec<Line<'static>> {
        let text = self.display_output.trim();
        if text.is_empty() {
            return Vec::new();
        }
        vec![Line::from(Span::styled(
            format!("  {text}"),
            Style::default().fg(MUTED_COLOR),
        ))]
    }
}

impl HistoryCell for AgentToolCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![self.title_line()];
        lines.extend(self.body_lines(width));
        AgentMessageCell::new_with_prefix(lines, self.dot_prefix.clone(), "  ", false)
            .display_lines(width)
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.display_lines(width)
    }
}

fn spawn_message_from_input(input: Option<&Value>) -> Option<String> {
    input
        .and_then(|value| {
            value
                .get("message")
                .or_else(|| value.get("prompt"))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn await_target_from_input(input: Option<&Value>) -> Option<String> {
    input.and_then(|value| {
        value
            .get("task_id")
            .or_else(|| value.get("target"))
            .or_else(|| value.get("agent_nickname"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(ToString::to_string)
    })
}

fn await_timeout_label(input: Option<&Value>) -> String {
    input
        .and_then(|value| value.get("timeout_secs").and_then(Value::as_u64))
        .map(|secs| format!("timed out after {secs}s"))
        .or_else(|| {
            input
                .and_then(|value| value.get("timeout").and_then(Value::as_str))
                .map(ToString::to_string)
        })
        .unwrap_or_else(|| "timed out".to_string())
}

fn task_status_line(status: &str, nickname: &str, path: Option<&str>) -> Line<'static> {
    Line::from(vec![
        Span::styled("● ", status_marker_style(status)),
        Span::styled(nickname.to_string(), Style::default().bold()),
        Span::styled(
            path.map(|path| format!("  {path}")).unwrap_or_default(),
            Style::default().fg(MUTED_COLOR),
        ),
    ])
}

fn task_info_line(task: &TaskInfo) -> Line<'static> {
    let label = task_display_label(task);
    let detail = task_detail_suffix(task);
    Line::from(vec![
        Span::styled("● ", task_state_marker_style(task.state)),
        Span::styled(label, Style::default().bold()),
        Span::styled(
            detail.map(|text| format!("  {text}")).unwrap_or_default(),
            Style::default().fg(MUTED_COLOR),
        ),
    ])
}

fn task_row_line(task: &TaskInfo) -> Line<'static> {
    let label = task_display_label(task);
    let state = task_state_label(task.state);
    let detail = task_detail_suffix(task).unwrap_or_default();
    Line::from(vec![
        Span::raw("  "),
        Span::styled("● ", task_state_marker_style(task.state)),
        Span::styled(format!("{label:<16}"), Style::default().bold()),
        Span::styled(format!("{state:<11}"), task_state_text_style(task.state)),
        Span::styled(detail, Style::default().fg(MUTED_COLOR)),
    ])
}

fn task_display_label(task: &TaskInfo) -> String {
    match task.kind {
        TaskKind::Agent => task
            .agent
            .as_ref()
            .map(|agent| agent.agent_nickname.clone())
            .unwrap_or_else(|| task.task_id.as_ref().to_string()),
        TaskKind::Command => task
            .command
            .as_ref()
            .map(|command| compact_command_label(&command.command))
            .unwrap_or_else(|| task.task_id.as_ref().to_string()),
    }
}

fn task_detail_suffix(task: &TaskInfo) -> Option<String> {
    match task.kind {
        TaskKind::Agent => task.agent.as_ref().map(|agent| agent.agent_path.clone()),
        TaskKind::Command => task
            .command
            .as_ref()
            .and_then(|command| command.exit_code.map(|code| format!("exit {code}"))),
    }
}

fn compact_command_label(command: &str) -> String {
    const MAX_CHARS: usize = 24;
    let compact = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_CHARS {
        compact
    } else {
        format!(
            "{}…",
            compact
                .chars()
                .take(MAX_CHARS.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn meta_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label} "), Style::default().fg(MUTED_COLOR)),
        Span::styled(value.to_string(), Style::default().dim()),
    ])
}

fn quoted_preview_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  “{text}”"),
        Style::default().fg(MUTED_COLOR).italic(),
    ))
}

fn status_marker_style(status: &str) -> Style {
    match status.to_ascii_lowercase().as_str() {
        "completed" | "done" | "idle" => Style::default().fg(COMPLETED_COLOR).bold(),
        "running" | "working" | "active" | "spawning" => Style::default().fg(RUNNING_COLOR).bold(),
        "failed" => Style::default().fg(FAILED_COLOR).bold(),
        "interrupted" | "canceled" | "closed" => Style::default().fg(MUTED_COLOR).bold(),
        _ => Style::default().fg(REASONING_ACCENT_COLOR).bold(),
    }
}

fn task_state_marker_style(state: TaskState) -> Style {
    match state {
        TaskState::Completed => Style::default().fg(COMPLETED_COLOR).bold(),
        TaskState::Running => Style::default().fg(RUNNING_COLOR).bold(),
        TaskState::WaitingApproval => Style::default().fg(REASONING_ACCENT_COLOR).bold(),
        TaskState::Failed => Style::default().fg(FAILED_COLOR).bold(),
        TaskState::Canceled => Style::default().fg(MUTED_COLOR).bold(),
    }
}

fn task_state_text_style(state: TaskState) -> Style {
    Style::default().fg(match state {
        TaskState::Completed => COMPLETED_COLOR,
        TaskState::Running => RUNNING_COLOR,
        TaskState::WaitingApproval => REASONING_ACCENT_COLOR,
        TaskState::Failed => FAILED_COLOR,
        TaskState::Canceled => MUTED_COLOR,
    })
}

fn task_state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::WaitingApproval => "approval",
        TaskState::Running => "running",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Canceled => "canceled",
    }
}

#[cfg(test)]
mod tests {
    use devo_core::SessionId;
    use devo_protocol::TaskId;

    use super::*;
    use ratatui::style::Stylize;

    fn sample_spawn_output() -> Value {
        serde_json::to_value(SpawnAgentResult {
            task_id: TaskId("task-1".to_string()),
            child_session_id: SessionId::new(),
            agent_path: "root/reviewer".to_string(),
            agent_nickname: "reviewer".to_string(),
            status: "running".to_string(),
        })
        .expect("serialize spawn result")
    }

    #[test]
    fn spawn_agent_cell_renders_structured_body() {
        let cell = AgentToolCell::new(
            "spawn_agent".to_string(),
            ToolPhase::Completed,
            Some(serde_json::json!({
                "agent_nickname": "reviewer",
                "message": "check usage"
            })),
            Some(sample_spawn_output()),
            String::new(),
            Line::from(vec![Span::styled("▌", Style::default().dim()), " ".into()]),
        );
        let rendered = cell
            .display_lines(100)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Spawned agent reviewer"));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("root/reviewer"));
        assert!(rendered.contains("task task-1"));
        assert!(rendered.contains("check usage"));
    }

    #[test]
    fn list_tasks_cell_renders_task_rows() {
        let session_id = SessionId::new();
        let output = serde_json::to_value(ListTasksResult {
            tasks: vec![
                TaskInfo {
                    task_id: TaskId::from(session_id),
                    kind: TaskKind::Agent,
                    state: TaskState::Running,
                    agent: Some(devo_protocol::AgentTaskMetadata {
                        session_id,
                        parent_session_id: None,
                        agent_path: "root/reviewer".to_string(),
                        agent_nickname: "reviewer".to_string(),
                        agent_role: "default".to_string(),
                        last_task_message: None,
                    }),
                    command: None,
                },
                TaskInfo {
                    task_id: TaskId("cmd-1".to_string()),
                    kind: TaskKind::Command,
                    state: TaskState::Completed,
                    agent: None,
                    command: Some(devo_protocol::CommandTaskMetadata {
                        process_id: 42,
                        command: "cargo test".to_string(),
                        exit_code: Some(0),
                    }),
                },
            ],
        })
        .expect("serialize list result");
        let cell = AgentToolCell::new(
            "list_tasks".to_string(),
            ToolPhase::Completed,
            Some(serde_json::json!({})),
            Some(output),
            String::new(),
            Line::from(vec![Span::styled("▌", Style::default().dim()), " ".into()]),
        );
        let rendered = cell
            .display_lines(120)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Listed tasks"));
        assert!(rendered.contains("2 tasks"));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("running"));
        assert!(rendered.contains("cargo test"));
        assert!(rendered.contains("exit 0"));
    }
}
