use std::collections::HashMap;

use devo_core::SessionId;
use devo_protocol::AcpContentBlock;
use devo_protocol::AcpEmbeddedResource;
use devo_protocol::AcpPlanEntryStatus;
use devo_protocol::AcpSessionNotification;
use devo_protocol::AcpSessionUpdate;
use devo_protocol::AcpToolCallContent;
use devo_protocol::AcpToolCallStatus;
use devo_protocol::AcpToolKind;
use devo_protocol::DEVO_SESSION_META;
use devo_protocol::DEVO_TURN_USAGE_META;
use devo_protocol::ItemId;
use devo_protocol::ServerEvent;
use devo_protocol::SessionMetadata;
use devo_protocol::SpawnAgentResult;
use devo_protocol::TurnMetadata;
use devo_protocol::TurnStatus;
use devo_protocol::TurnUsageUpdatedPayload;
use devo_protocol::original_event_from_acp_notification;

use crate::events::PlanStep;
use crate::events::PlanStepStatus;
use crate::events::SubagentMonitorEvent;
use crate::events::TextItemKind;
use crate::events::WorkerEvent;
use crate::shell_output::strip_shell_envelope;

struct AcpToolCallEventData {
    tool_call_id: String,
    title: Option<String>,
    status: Option<AcpToolCallStatus>,
    raw_input: Option<serde_json::Value>,
    raw_output: Option<serde_json::Value>,
    content: Vec<AcpToolCallContent>,
}

struct AcpSessionUpdateRender {
    session_id: SessionId,
    update: AcpSessionUpdate,
}

impl From<AcpSessionUpdateRender> for Vec<WorkerEvent> {
    fn from(render: AcpSessionUpdateRender) -> Self {
        let AcpSessionUpdateRender { session_id, update } = render;
        match update {
            AcpSessionUpdate::AgentMessageChunk {
                content,
                message_id,
                ..
            } => acp_content_display_text(&content)
                .into_iter()
                .map(|delta| {
                    if let Some(item_id) = message_item_id(message_id.as_deref()) {
                        WorkerEvent::TextItemDelta {
                            item_id,
                            kind: TextItemKind::Assistant,
                            delta,
                        }
                    } else {
                        WorkerEvent::TextDelta(delta)
                    }
                })
                .collect(),
            AcpSessionUpdate::AgentThoughtChunk {
                content,
                message_id,
                ..
            } => acp_content_display_text(&content)
                .into_iter()
                .map(|delta| {
                    if let Some(item_id) = message_item_id(message_id.as_deref()) {
                        WorkerEvent::TextItemDelta {
                            item_id,
                            kind: TextItemKind::Reasoning,
                            delta,
                        }
                    } else {
                        WorkerEvent::ReasoningDelta(delta)
                    }
                })
                .collect(),
            AcpSessionUpdate::Plan { entries, .. } => vec![WorkerEvent::PlanUpdated {
                explanation: None,
                steps: entries
                    .into_iter()
                    .map(|entry| PlanStep {
                        text: entry.content,
                        status: match entry.status {
                            AcpPlanEntryStatus::Pending => PlanStepStatus::Pending,
                            AcpPlanEntryStatus::InProgress => PlanStepStatus::InProgress,
                            AcpPlanEntryStatus::Completed => PlanStepStatus::Completed,
                        },
                    })
                    .collect(),
            }],
            AcpSessionUpdate::SessionInfoUpdate {
                title: Some(title), ..
            } => vec![WorkerEvent::SessionTitleUpdated {
                session_id: session_id.to_string(),
                title,
            }],
            AcpSessionUpdate::AvailableCommandsUpdate {
                available_commands, ..
            } => vec![WorkerEvent::AcpAvailableCommandsUpdated {
                commands: available_commands,
            }],
            AcpSessionUpdate::CurrentModeUpdate {
                current_mode_id, ..
            } => vec![WorkerEvent::AcpCurrentModeUpdated { current_mode_id }],
            AcpSessionUpdate::ConfigOptionUpdate { config_options, .. } => {
                vec![WorkerEvent::AcpConfigOptionsUpdated { config_options }]
            }
            AcpSessionUpdate::UsageUpdate {
                used,
                size,
                cost,
                meta,
            } => worker_events_from_acp_usage_update(used, size, cost, meta),
            AcpSessionUpdate::ToolCall {
                tool_call_id,
                title,
                kind,
                status,
                raw_input,
                raw_output,
                content,
                ..
            } => worker_events_from_acp_tool_call(
                AcpToolCallEventData {
                    tool_call_id,
                    title: Some(title),
                    status: Some(status),
                    raw_input,
                    raw_output,
                    content,
                },
                kind,
            ),
            AcpSessionUpdate::ToolCallUpdate {
                tool_call_id,
                title,
                kind,
                status,
                raw_input,
                raw_output,
                content,
                ..
            } => worker_events_from_acp_tool_call_update(
                AcpToolCallEventData {
                    tool_call_id,
                    title,
                    status,
                    raw_input,
                    raw_output,
                    content,
                },
                kind,
            ),
            AcpSessionUpdate::UserMessageChunk { .. }
            | AcpSessionUpdate::SessionInfoUpdate { title: None, .. } => Vec::new(),
        }
    }
}

#[cfg(test)]
pub(super) fn worker_events_from_acp_notification(
    params: &serde_json::Value,
    active_session_id: Option<SessionId>,
) -> Vec<WorkerEvent> {
    let Some(notification) = parse_acp_session_notification(params) else {
        return Vec::new();
    };
    if Some(notification.session_id) != active_session_id {
        return Vec::new();
    }
    worker_events_from_acp_session_notification(notification)
}

pub(super) fn parse_acp_session_notification(
    params: &serde_json::Value,
) -> Option<AcpSessionNotification> {
    serde_json::from_value::<AcpSessionNotification>(params.clone()).ok()
}

pub(super) fn worker_events_from_acp_session_notification(
    notification: AcpSessionNotification,
) -> Vec<WorkerEvent> {
    Vec::from(AcpSessionUpdateRender {
        session_id: notification.session_id,
        update: notification.update,
    })
}

pub(super) fn session_metadata_from_acp_update(
    update: &AcpSessionUpdate,
) -> Option<SessionMetadata> {
    let AcpSessionUpdate::SessionInfoUpdate {
        meta: Some(meta), ..
    } = update
    else {
        return None;
    };
    serde_json::from_value(meta.get(DEVO_SESSION_META)?.clone()).ok()
}

pub(super) fn spawn_task_message_from_acp_update(update: &AcpSessionUpdate) -> Option<String> {
    let raw_input = match update {
        AcpSessionUpdate::ToolCall { raw_input, .. } => raw_input.as_ref(),
        AcpSessionUpdate::ToolCallUpdate { raw_input, .. } => raw_input.as_ref(),
        _ => None,
    }?;
    raw_input
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub(super) fn spawn_agent_result_from_acp_update(
    update: &AcpSessionUpdate,
) -> Option<SpawnAgentResult> {
    match update {
        AcpSessionUpdate::ToolCall {
            status, raw_output, ..
        } if *status == AcpToolCallStatus::Completed => {
            spawn_agent_result_from_raw_output(raw_output.as_ref())
        }
        AcpSessionUpdate::ToolCallUpdate {
            status: Some(AcpToolCallStatus::Completed),
            raw_output,
            ..
        } => spawn_agent_result_from_raw_output(raw_output.as_ref()),
        AcpSessionUpdate::UserMessageChunk { .. }
        | AcpSessionUpdate::AgentMessageChunk { .. }
        | AcpSessionUpdate::AgentThoughtChunk { .. }
        | AcpSessionUpdate::ToolCall { .. }
        | AcpSessionUpdate::ToolCallUpdate { .. }
        | AcpSessionUpdate::Plan { .. }
        | AcpSessionUpdate::AvailableCommandsUpdate { .. }
        | AcpSessionUpdate::CurrentModeUpdate { .. }
        | AcpSessionUpdate::ConfigOptionUpdate { .. }
        | AcpSessionUpdate::SessionInfoUpdate { .. }
        | AcpSessionUpdate::UsageUpdate { .. } => None,
    }
}

pub(super) fn subagent_monitor_events_from_acp_session_notification(
    notification: AcpSessionNotification,
) -> Vec<WorkerEvent> {
    if let Some(events) = subagent_monitor_events_from_wrapped_server_event(&notification) {
        return events;
    }
    let session_id = notification.session_id;
    match notification.update {
        AcpSessionUpdate::AgentMessageChunk {
            content,
            message_id,
            ..
        } => acp_content_display_text(&content)
            .into_iter()
            .map(|delta| WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TextItemDelta {
                    session_id,
                    item_id: message_item_id(message_id.as_deref()),
                    kind: TextItemKind::Assistant,
                    delta,
                },
            })
            .collect(),
        AcpSessionUpdate::AgentThoughtChunk {
            content,
            message_id,
            ..
        } => acp_content_display_text(&content)
            .into_iter()
            .map(|delta| WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TextItemDelta {
                    session_id,
                    item_id: message_item_id(message_id.as_deref()),
                    kind: TextItemKind::Reasoning,
                    delta,
                },
            })
            .collect(),
        AcpSessionUpdate::Plan { entries, .. } => vec![WorkerEvent::SubagentMonitor {
            event: SubagentMonitorEvent::PlanUpdated {
                session_id,
                explanation: None,
                steps: entries
                    .into_iter()
                    .map(|entry| PlanStep {
                        text: entry.content,
                        status: plan_step_status_from_acp(entry.status),
                    })
                    .collect(),
            },
        }],
        AcpSessionUpdate::ToolCall {
            tool_call_id,
            title,
            kind: _,
            status,
            raw_input,
            raw_output,
            content,
            ..
        } => subagent_events_from_acp_tool_call(
            session_id,
            AcpToolCallEventData {
                tool_call_id,
                title: Some(title),
                status: Some(status),
                raw_input,
                raw_output,
                content,
            },
        ),
        AcpSessionUpdate::ToolCallUpdate {
            tool_call_id,
            title,
            kind: _,
            status,
            raw_input,
            raw_output,
            content,
            ..
        } => subagent_events_from_acp_tool_call_update(
            session_id,
            AcpToolCallEventData {
                tool_call_id,
                title,
                status,
                raw_input,
                raw_output,
                content,
            },
        ),
        AcpSessionUpdate::UserMessageChunk { content, .. } => acp_content_display_text(&content)
            .into_iter()
            .map(|message| WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TaskMessage {
                    session_id,
                    message,
                },
            })
            .collect(),
        AcpSessionUpdate::SessionInfoUpdate { .. }
        | AcpSessionUpdate::AvailableCommandsUpdate { .. }
        | AcpSessionUpdate::CurrentModeUpdate { .. }
        | AcpSessionUpdate::ConfigOptionUpdate { .. }
        | AcpSessionUpdate::UsageUpdate { .. } => Vec::new(),
    }
}

fn subagent_monitor_events_from_wrapped_server_event(
    notification: &AcpSessionNotification,
) -> Option<Vec<WorkerEvent>> {
    let (method, event) = original_event_from_acp_notification(notification)?;
    Some(subagent_monitor_events_from_server_event(
        notification.session_id,
        method.as_str(),
        event,
    ))
}

fn subagent_monitor_events_from_server_event(
    session_id: SessionId,
    method: &str,
    event: ServerEvent,
) -> Vec<WorkerEvent> {
    match (method, event) {
        ("turn/started", ServerEvent::TurnStarted(payload)) => {
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TurnStarted {
                    session_id,
                    turn_id: payload.turn.turn_id,
                },
            }]
        }
        ("turn/completed", ServerEvent::TurnCompleted(payload))
        | ("turn/interrupted", ServerEvent::TurnInterrupted(payload)) => {
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TurnFinished {
                    session_id,
                    status: subagent_turn_finished_status(payload.turn.status),
                },
            }]
        }
        ("turn/failed", ServerEvent::TurnFailed(payload)) => {
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TurnFailed {
                    session_id,
                    message: payload
                        .error
                        .map(|error| error.message)
                        .unwrap_or_else(|| subagent_turn_failure_message(&payload.turn)),
                },
            }]
        }
        ("session/status/changed", ServerEvent::SessionStatusChanged(payload)) => {
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::SessionStatusChanged {
                    session_id,
                    status: payload.status,
                },
            }]
        }
        _ => Vec::new(),
    }
}

fn subagent_turn_finished_status(status: TurnStatus) -> String {
    match status {
        TurnStatus::Completed => "done".to_string(),
        TurnStatus::Interrupted => "interrupted".to_string(),
        TurnStatus::Failed => "failed".to_string(),
        TurnStatus::Running | TurnStatus::Pending | TurnStatus::WaitingApproval => {
            "working".to_string()
        }
    }
}

fn subagent_turn_failure_message(turn: &TurnMetadata) -> String {
    turn.failure_reason
        .as_ref()
        .map(|reason| format!("{reason:?}"))
        .unwrap_or_else(|| "Turn failed".to_string())
}

/// Routes unwrapped server notifications (as produced by the stdio client) to
/// sub-agent monitor events for a known child session.
pub(super) fn subagent_monitor_events_from_unwrapped_server_notification(
    method: &str,
    event: ServerEvent,
) -> Vec<WorkerEvent> {
    let Some(session_id) = event.session_id() else {
        return Vec::new();
    };
    subagent_monitor_events_from_server_event(session_id, method, event)
}

fn message_item_id(message_id: Option<&str>) -> Option<ItemId> {
    message_id.and_then(|message_id| ItemId::try_from(message_id).ok())
}

fn plan_step_status_from_acp(status: AcpPlanEntryStatus) -> PlanStepStatus {
    match status {
        AcpPlanEntryStatus::Pending => PlanStepStatus::Pending,
        AcpPlanEntryStatus::InProgress => PlanStepStatus::InProgress,
        AcpPlanEntryStatus::Completed => PlanStepStatus::Completed,
    }
}

fn spawn_agent_result_from_raw_output(
    raw_output: Option<&serde_json::Value>,
) -> Option<SpawnAgentResult> {
    serde_json::from_value(raw_output?.clone()).ok()
}

fn worker_events_from_acp_usage_update(
    used: u64,
    size: u64,
    cost: Option<devo_protocol::AcpCost>,
    meta: Option<devo_protocol::AcpMeta>,
) -> Vec<WorkerEvent> {
    if let Some(payload) = turn_usage_payload_from_acp_meta(meta.as_ref()) {
        return vec![WorkerEvent::UsageUpdated {
            total_input_tokens: payload.total_input_tokens,
            total_output_tokens: payload.total_output_tokens,
            total_tokens: payload.total_tokens,
            total_cache_read_tokens: payload.total_cache_read_tokens,
            last_query_total_tokens: payload.usage.display_total_tokens(),
            last_query_input_tokens: payload.last_query_input_tokens,
        }];
    }

    vec![WorkerEvent::AcpUsageUpdated { used, size, cost }]
}

fn turn_usage_payload_from_acp_meta(
    meta: Option<&devo_protocol::AcpMeta>,
) -> Option<TurnUsageUpdatedPayload> {
    serde_json::from_value(meta?.get(DEVO_TURN_USAGE_META)?.clone()).ok()
}

fn worker_events_from_acp_tool_call(
    tool_call: AcpToolCallEventData,
    kind: AcpToolKind,
) -> Vec<WorkerEvent> {
    let title = tool_call
        .title
        .clone()
        .expect("ACP tool_call notifications always include a title");
    let status = tool_call
        .status
        .expect("ACP tool_call notifications always include a status");
    let tool_call_id = tool_call.tool_call_id.clone();
    let mut events = vec![WorkerEvent::ToolCall {
        tool_use_id: tool_call_id.clone(),
        summary: title.clone(),
        preparing: status == AcpToolCallStatus::Pending,
        parsed_commands: None,
    }];
    if let Some(input) = tool_call.raw_input.clone() {
        events.push(WorkerEvent::ToolCallDetails {
            tool_use_id: tool_call_id.clone(),
            tool_name: acp_tool_kind_label(kind).to_string(),
            input,
        });
    }
    events.extend(worker_events_from_acp_tool_content(AcpToolCallEventData {
        tool_call_id,
        title: Some(title),
        status: Some(status),
        raw_input: tool_call.raw_input,
        raw_output: tool_call.raw_output,
        content: tool_call.content,
    }));
    events
}

fn worker_events_from_acp_tool_call_update(
    tool_call: AcpToolCallEventData,
    kind: Option<AcpToolKind>,
) -> Vec<WorkerEvent> {
    let mut events = Vec::new();
    if let Some(input) = tool_call.raw_input.clone() {
        events.push(WorkerEvent::ToolCallDetails {
            tool_use_id: tool_call.tool_call_id.clone(),
            tool_name: kind.map(acp_tool_kind_label).unwrap_or("tool").to_string(),
            input,
        });
    }
    // Status-only updates (e.g. pending → in_progress) must not overwrite the
    // live title with a generic "Running"/"Pending" label. Only apply a title
    // when the ACP update actually carries one.
    if let Some(summary) = tool_call.title.clone() {
        events.push(WorkerEvent::ToolCallUpdated {
            tool_use_id: tool_call.tool_call_id.clone(),
            summary,
            parsed_commands: Vec::new(),
        });
    }
    events.extend(worker_events_from_acp_tool_content(tool_call));
    events
}

fn worker_events_from_acp_tool_content(tool_call: AcpToolCallEventData) -> Vec<WorkerEvent> {
    let mut events = Vec::new();
    let mut changes = HashMap::new();
    let mut text_parts = Vec::new();
    for item in tool_call.content {
        match item {
            AcpToolCallContent::Content { content } => {
                if let Some(text) = acp_content_display_text(&content) {
                    text_parts.push(text);
                }
            }
            AcpToolCallContent::Diff {
                path,
                old_text,
                new_text,
            } => {
                changes.insert(path, file_change_from_acp_diff(old_text, new_text));
            }
        }
    }
    if !changes.is_empty() {
        if let Some(input) = tool_call.raw_input.clone() {
            events.push(WorkerEvent::PatchAppliedIo {
                tool_use_id: tool_call.tool_call_id.clone(),
                tool_name: tool_call
                    .title
                    .clone()
                    .unwrap_or_else(|| "tool".to_string()),
                input,
                changes,
            });
        } else {
            events.push(WorkerEvent::PatchApplied {
                tool_use_id: tool_call.tool_call_id.clone(),
                changes,
            });
        }
    }
    // Shell tools append a JSON envelope ({command, exit, ...}) to the result
    // text; strip it so only the real command output is displayed.
    let text = strip_shell_envelope(&text_parts.join("\n"));
    if !text.is_empty() {
        if matches!(
            tool_call.status,
            Some(AcpToolCallStatus::Completed | AcpToolCallStatus::Failed)
        ) {
            events.push(acp_tool_result_event(
                tool_call.tool_call_id,
                tool_call.title,
                tool_call.raw_input,
                tool_call.raw_output,
                text,
                tool_call.status == Some(AcpToolCallStatus::Failed),
            ));
        } else {
            events.push(WorkerEvent::ToolOutputDelta {
                tool_use_id: tool_call.tool_call_id,
                delta: text,
            });
        }
    }
    events
}

fn subagent_events_from_acp_tool_call(
    session_id: SessionId,
    tool_call: AcpToolCallEventData,
) -> Vec<WorkerEvent> {
    let title = tool_call
        .title
        .clone()
        .expect("ACP tool_call notifications always include a title");
    let status = tool_call
        .status
        .expect("ACP tool_call notifications always include a status");
    let tool_call_id = tool_call.tool_call_id.clone();
    let mut events = vec![WorkerEvent::SubagentMonitor {
        event: SubagentMonitorEvent::ToolCall {
            session_id,
            tool_use_id: tool_call_id.clone(),
            summary: title.clone(),
        },
    }];
    events.extend(subagent_events_from_acp_tool_content(
        session_id,
        AcpToolCallEventData {
            tool_call_id,
            title: Some(title),
            status: Some(status),
            raw_input: tool_call.raw_input,
            raw_output: tool_call.raw_output,
            content: tool_call.content,
        },
    ));
    events
}

fn subagent_events_from_acp_tool_call_update(
    session_id: SessionId,
    tool_call: AcpToolCallEventData,
) -> Vec<WorkerEvent> {
    let mut events = Vec::new();
    // Status-only updates must not overwrite the live title with a generic
    // "Running"/"Pending" label.
    if let Some(summary) = tool_call.title.clone() {
        events.push(WorkerEvent::SubagentMonitor {
            event: SubagentMonitorEvent::ToolCallUpdated {
                session_id,
                tool_use_id: tool_call.tool_call_id.clone(),
                summary,
            },
        });
    }
    events.extend(subagent_events_from_acp_tool_content(session_id, tool_call));
    events
}

fn subagent_events_from_acp_tool_content(
    session_id: SessionId,
    tool_call: AcpToolCallEventData,
) -> Vec<WorkerEvent> {
    let mut events = Vec::new();
    let mut text_parts = Vec::new();
    let mut diff_count = 0usize;
    for item in tool_call.content {
        match item {
            AcpToolCallContent::Content { content } => {
                if let Some(text) = acp_content_display_text(&content) {
                    text_parts.push(text);
                }
            }
            AcpToolCallContent::Diff { .. } => {
                diff_count += 1;
            }
        }
    }

    // Shell tools append a JSON envelope ({command, exit, ...}) to the result
    // text; strip it so only the real command output is displayed.
    let text = strip_shell_envelope(&text_parts.join("\n"));
    if !text.is_empty() {
        if matches!(
            tool_call.status,
            Some(AcpToolCallStatus::Completed | AcpToolCallStatus::Failed)
        ) {
            events.push(WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::ToolResult {
                    session_id,
                    tool_use_id: tool_call.tool_call_id,
                    title: acp_tool_result_title(tool_call.title, tool_call.status),
                    preview: text,
                    is_error: tool_call.status == Some(AcpToolCallStatus::Failed),
                },
            });
        } else {
            events.push(WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::ToolOutputDelta {
                    session_id,
                    tool_use_id: tool_call.tool_call_id,
                    delta: text,
                },
            });
        }
    } else if diff_count > 0
        && matches!(
            tool_call.status,
            Some(AcpToolCallStatus::Completed | AcpToolCallStatus::Failed)
        )
    {
        let is_error = tool_call.status == Some(AcpToolCallStatus::Failed);
        let preview = match diff_count {
            1 => "1 file change".to_string(),
            count => format!("{count} file changes"),
        };
        events.push(WorkerEvent::SubagentMonitor {
            event: SubagentMonitorEvent::ToolResult {
                session_id,
                tool_use_id: tool_call.tool_call_id,
                title: acp_tool_result_title(tool_call.title, tool_call.status),
                preview,
                is_error,
            },
        });
    }
    events
}

fn acp_tool_result_title(title: Option<String>, status: Option<AcpToolCallStatus>) -> String {
    title.unwrap_or_else(|| {
        if status == Some(AcpToolCallStatus::Failed) {
            "Tool failed".to_string()
        } else {
            "Tool completed".to_string()
        }
    })
}

fn acp_tool_result_event(
    tool_call_id: String,
    title: Option<String>,
    raw_input: Option<serde_json::Value>,
    raw_output: Option<serde_json::Value>,
    preview: String,
    is_error: bool,
) -> WorkerEvent {
    let title = title.unwrap_or_else(|| {
        if is_error {
            "Tool failed".to_string()
        } else {
            "Tool completed".to_string()
        }
    });
    match (raw_input, raw_output) {
        (Some(input), Some(output)) => WorkerEvent::ToolResultIo {
            tool_use_id: tool_call_id,
            tool_name: title.clone(),
            title,
            input,
            output,
            display_content: Some(preview),
            is_error,
            truncated: false,
        },
        _ => WorkerEvent::ToolResult {
            tool_use_id: tool_call_id,
            title,
            preview,
            is_error,
            truncated: false,
        },
    }
}

fn file_change_from_acp_diff(
    old_text: Option<String>,
    new_text: String,
) -> devo_protocol::protocol::FileChange {
    match old_text {
        None => devo_protocol::protocol::FileChange::Add { content: new_text },
        Some(old_text) if new_text.is_empty() => {
            devo_protocol::protocol::FileChange::Delete { content: old_text }
        }
        Some(old_text) => devo_protocol::protocol::FileChange::Update {
            unified_diff: diffy::create_patch(&old_text, &new_text).to_string(),
            old_text: Some(old_text),
            new_text: Some(new_text),
            move_path: None,
        },
    }
}

fn acp_tool_kind_label(kind: AcpToolKind) -> &'static str {
    match kind {
        AcpToolKind::Read => "read",
        AcpToolKind::Edit => "edit",
        AcpToolKind::Delete => "delete",
        AcpToolKind::Move => "move",
        AcpToolKind::Search => "search",
        AcpToolKind::Execute => "execute",
        AcpToolKind::Think => "think",
        AcpToolKind::Fetch => "fetch",
        AcpToolKind::Other => "tool",
    }
}

fn acp_content_display_text(content: &AcpContentBlock) -> Option<String> {
    let text = match content {
        AcpContentBlock::Text { text, .. } => text.clone(),
        AcpContentBlock::Image { mime_type, uri, .. } => uri
            .as_ref()
            .map_or_else(|| format!("[image: {mime_type}]"), ToString::to_string),
        AcpContentBlock::Audio { mime_type, .. } => format!("[audio: {mime_type}]"),
        AcpContentBlock::ResourceLink {
            uri, title, name, ..
        } => {
            let label = title
                .as_deref()
                .filter(|title| !title.is_empty())
                .unwrap_or(name);
            format!("{label}: {uri}")
        }
        AcpContentBlock::Resource { resource, .. } => match resource {
            AcpEmbeddedResource::Text(resource) => resource.text.clone(),
            AcpEmbeddedResource::Blob(resource) => {
                let mime_type = resource.mime_type.as_deref().unwrap_or("unknown");
                format!("[resource: {} ({mime_type})]", resource.uri)
            }
        },
    };
    (!text.is_empty()).then_some(text)
}
