//! Sub-agent metadata helpers for the TUI worker.
//!
//! Live sub-agent monitor updates are routed from Native session, turn, and
//! item notifications. This module normalizes the protocol shapes used by the
//! monitor.

use devo_protocol::AgentInfo;
use devo_protocol::SessionMetadata;
use devo_protocol::native::session::SessionParent;

use crate::events::SubagentMonitorAgent;

/// Converts a canonical `SubAgent` item (from `agent/list`) into the
/// monitor shape. Facade approximations (L2-DES-APP-008 Phase B): the
/// canonical item has no agent_path/nickname, so the path falls back to the
/// session id and the nickname to the role; the status string maps back
/// from `SpawnedWorkState`.
pub(super) fn agent_from_native_subagent(
    item: &devo_protocol::native::item::ItemEnvelope,
) -> Option<SubagentMonitorAgent> {
    let devo_protocol::native::item::Item::SubAgent {
        agent_session_id,
        parent_session_id,
        role,
        task,
        state,
        ..
    } = &item.item
    else {
        return None;
    };
    let status = match state {
        devo_protocol::native::item::SpawnedWorkState::Running => "running",
        devo_protocol::native::item::SpawnedWorkState::Completed => "completed",
        devo_protocol::native::item::SpawnedWorkState::Failed => "failed",
        devo_protocol::native::item::SpawnedWorkState::Cancelled => "canceled",
        devo_protocol::native::item::SpawnedWorkState::Lost => "lost",
    };
    Some(SubagentMonitorAgent {
        session_id: devo_protocol::SessionId::try_from(agent_session_id.as_str()).ok()?,
        parent_session_id: devo_protocol::SessionId::try_from(parent_session_id.as_str()).ok()?,
        agent_path: agent_session_id.as_str().to_string(),
        nickname: role.clone().unwrap_or_else(|| "agent".to_string()),
        role: role.clone().unwrap_or_default(),
        status: status.to_string(),
        last_task_message: (!task.is_empty()).then(|| task.clone()),
    })
}

pub(super) fn agent_from_info(info: AgentInfo) -> Option<SubagentMonitorAgent> {
    Some(SubagentMonitorAgent {
        session_id: info.session_id,
        parent_session_id: info.parent_session_id?,
        agent_path: info.agent_path,
        nickname: info.agent_nickname,
        role: info.agent_role,
        status: info.status,
        last_task_message: info.last_task_message,
    })
}

pub(super) fn agent_from_session(session: &SessionMetadata) -> Option<SubagentMonitorAgent> {
    Some(SubagentMonitorAgent {
        session_id: session.session_id,
        parent_session_id: session.parent_session_id?,
        agent_path: session.agent_path.clone()?,
        nickname: session
            .agent_nickname
            .clone()
            .unwrap_or_else(|| session.session_id.to_string()),
        role: session
            .agent_role
            .clone()
            .unwrap_or_else(|| "default".to_string()),
        status: format!("{:?}", session.status).to_lowercase(),
        last_task_message: None,
    })
}

pub(super) fn agent_from_native_session(
    session: &devo_protocol::native::session::Session,
) -> Option<SubagentMonitorAgent> {
    let SessionParent::Agent {
        session_id: parent_session_id,
        role,
    } = session.parent.as_ref()?;
    let session_id = devo_protocol::SessionId::try_from(session.id.as_str()).ok()?;
    Some(SubagentMonitorAgent {
        session_id,
        parent_session_id: devo_protocol::SessionId::try_from(parent_session_id.as_str()).ok()?,
        agent_path: session.id.to_string(),
        nickname: session
            .title
            .clone()
            .unwrap_or_else(|| session.id.to_string()),
        role: role.clone().unwrap_or_else(|| "default".to_string()),
        status: match session.status {
            devo_protocol::native::session::SessionStatus::Idle => "idle",
            devo_protocol::native::session::SessionStatus::Active => "active",
        }
        .to_string(),
        last_task_message: None,
    })
}

// ── Live monitor event routing (moved from worker/acp_events.rs) ──
//
// Child-session events arrive on the devo envelope; these map the ones the
// sub-agent monitor renders.

/// Parses a `SpawnAgentResult` out of a spawn tool call's raw output.
pub(super) fn spawn_agent_result_from_raw_output(
    raw_output: Option<&serde_json::Value>,
) -> Option<devo_protocol::SpawnAgentResult> {
    serde_json::from_value(raw_output?.clone()).ok()
}

fn subagent_turn_finished_status(status: devo_protocol::TurnStatus) -> String {
    match status {
        devo_protocol::TurnStatus::Completed => "done".to_string(),
        devo_protocol::TurnStatus::Interrupted => "interrupted".to_string(),
        devo_protocol::TurnStatus::Failed => "failed".to_string(),
        devo_protocol::TurnStatus::Running
        | devo_protocol::TurnStatus::Pending
        | devo_protocol::TurnStatus::WaitingApproval => "working".to_string(),
    }
}

fn subagent_turn_failure_message(turn: &devo_protocol::TurnMetadata) -> String {
    turn.failure_reason
        .as_ref()
        .map(|reason| format!("{reason:?}"))
        .unwrap_or_else(|| "Turn failed".to_string())
}

fn subagent_monitor_events_from_server_event(
    session_id: devo_protocol::SessionId,
    method: &str,
    event: devo_protocol::ServerEvent,
) -> Vec<crate::events::WorkerEvent> {
    use crate::events::{SubagentMonitorEvent, WorkerEvent};
    match (method, event) {
        ("turn/started", devo_protocol::ServerEvent::TurnStarted(payload)) => {
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TurnStarted {
                    session_id,
                    turn_id: payload.turn.turn_id,
                },
            }]
        }
        ("turn/completed", devo_protocol::ServerEvent::TurnCompleted(payload))
        | ("turn/interrupted", devo_protocol::ServerEvent::TurnInterrupted(payload)) => {
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::TurnFinished {
                    session_id,
                    status: subagent_turn_finished_status(payload.turn.status),
                },
            }]
        }
        ("turn/failed", devo_protocol::ServerEvent::TurnFailed(payload)) => {
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
        ("session/status/changed", devo_protocol::ServerEvent::SessionStatusChanged(payload)) => {
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

/// Routes unwrapped server notifications (as produced by the stdio client) to
/// sub-agent monitor events for a known child session.
pub(super) fn subagent_monitor_events_from_unwrapped_server_notification(
    method: &str,
    event: devo_protocol::ServerEvent,
) -> Vec<crate::events::WorkerEvent> {
    let Some(session_id) = event.session_id() else {
        return Vec::new();
    };
    subagent_monitor_events_from_server_event(session_id, method, event)
}

/// Maps a typed (canonical) item event of a child session to sub-agent
/// monitor events (L2-DES-APP-009 cutover). Tool names/titles the ACP
/// envelope carried are approximated from the item: tool calls carry the
/// tool name; tool results have no title on the canonical item, so the
/// monitor title falls back to the tool name when embedded in the output.
pub(super) fn subagent_monitor_events_from_typed_item(
    session_id: devo_protocol::SessionId,
    item: &devo_protocol::native::item::ItemEnvelope,
) -> Vec<crate::events::WorkerEvent> {
    use crate::events::{SubagentMonitorEvent, WorkerEvent};
    match &item.item {
        devo_protocol::native::item::Item::ToolCall {
            call_id, tool_name, ..
        } => vec![WorkerEvent::SubagentMonitor {
            event: SubagentMonitorEvent::ToolCallUpdated {
                session_id,
                tool_use_id: call_id.clone(),
                summary: tool_name.clone(),
            },
        }],
        devo_protocol::native::item::Item::ToolResult {
            call_id,
            output,
            display_content,
            is_error,
            ..
        } => {
            let preview = display_content
                .clone()
                .unwrap_or_else(|| serde_json::to_string(output).unwrap_or_else(|_| String::new()));
            vec![WorkerEvent::SubagentMonitor {
                event: SubagentMonitorEvent::ToolResult {
                    session_id,
                    tool_use_id: call_id.clone(),
                    title: String::new(),
                    preview,
                    is_error: *is_error,
                },
            }]
        }
        _ => Vec::new(),
    }
}
