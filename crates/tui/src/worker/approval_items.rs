//! Native `Item::Approval` → approval request/decision worker events.

use devo_protocol::native::item::ApprovalDecisionKind;
use devo_protocol::native::item::ApprovalScope;
use devo_protocol::native::item::ApprovalTarget;
use devo_protocol::native::item::Item;
use tokio::sync::mpsc;

use crate::events::WorkerEvent;

pub(crate) fn handle_started(
    item: &Item,
    session_id: devo_core::SessionId,
    turn_id: Option<devo_core::TurnId>,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> bool {
    let Item::Approval {
        approval_id,
        action_summary,
        justification,
        resource,
        available_scopes,
        command_pattern,
        command_prefix,
        target,
        decision,
        ..
    } = item
    else {
        return false;
    };
    if decision.is_some() {
        return false;
    }
    let Some(turn_id) = turn_id else {
        return false;
    };
    let (path, host, target) = approval_target_parts(target.as_ref());
    let _ = event_tx.send(WorkerEvent::ApprovalRequest {
        session_id,
        turn_id,
        approval_id: approval_id.clone(),
        action_summary: action_summary.clone(),
        justification: justification.clone(),
        resource: resource.clone(),
        available_scopes: available_scopes.clone(),
        path,
        host,
        target,
        command_pattern: command_pattern.clone(),
        command_prefix: command_prefix.clone(),
    });
    true
}

pub(crate) fn handle_completed(item: &Item, event_tx: &mpsc::UnboundedSender<WorkerEvent>) -> bool {
    let Item::Approval {
        approval_id,
        decision: Some(decision),
        ..
    } = item
    else {
        return false;
    };
    let decision_label = match decision.decision {
        ApprovalDecisionKind::Approved => "approve",
        ApprovalDecisionKind::Denied => "deny",
        ApprovalDecisionKind::Cancelled => "cancel",
    };
    let scope = match decision.scope {
        ApprovalScope::Once => "once",
        ApprovalScope::Turn => "turn",
        ApprovalScope::Session => "session",
        ApprovalScope::PathPrefix => "path_prefix",
        ApprovalScope::Host => "host",
        ApprovalScope::Tool => "tool",
        ApprovalScope::CommandPrefix => "command_prefix",
        ApprovalScope::CommandPrefixPersist => "command_prefix_persist",
    };
    let _ = event_tx.send(WorkerEvent::ApprovalDecision {
        approval_id: approval_id.clone(),
        decision: decision_label.to_string(),
        scope: scope.to_string(),
        tool_name: None,
        rationale: None,
    });
    true
}

fn approval_target_parts(
    target: Option<&ApprovalTarget>,
) -> (Option<String>, Option<String>, Option<String>) {
    match target {
        Some(ApprovalTarget::Path { path }) => (Some(path.display().to_string()), None, None),
        Some(ApprovalTarget::Host { host }) => (None, Some(host.clone()), None),
        Some(ApprovalTarget::Command { command }) => (None, None, Some(command.clone())),
        None => (None, None, None),
    }
}
