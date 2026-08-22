//! Native approval bridge for server-initiated native approval requests.
//!
//! The server emits the approval item and uses the native reverse request
//! only to correlate the eventual client response. ACP permission requests
//! are intentionally not handled here.

use std::collections::HashMap;
use std::sync::Arc;

use devo_protocol::ApprovalDecisionValue;
use devo_protocol::ApprovalResponseParams;
use devo_protocol::ApprovalScopeValue;
use devo_protocol::acp_success_response;
use tokio::sync::Mutex;

pub(crate) type PendingApprovals = Arc<Mutex<HashMap<String, serde_json::Value>>>;

/// Registers a Native `approval/*/request` reverse request.
pub(crate) async fn handle_approval_request(
    request_id: serde_json::Value,
    params: serde_json::Value,
    pending_approvals: PendingApprovals,
) -> Result<(), String> {
    let approval_id = params
        .get("approvalId")
        .and_then(serde_json::Value::as_str)
        .filter(|approval_id| !approval_id.is_empty())
        .ok_or_else(|| "Native approval request params.approvalId is required".to_string())?;
    pending_approvals
        .lock()
        .await
        .insert(native_pending_key(approval_id), request_id);
    Ok(())
}

/// Resolves a pending Native approval and builds the JSON-RPC response for
/// the original server request.
pub(crate) async fn resolve_approval_response(
    pending_approvals: &PendingApprovals,
    params: &ApprovalResponseParams,
) -> Option<serde_json::Value> {
    let request_id = pending_approvals
        .lock()
        .await
        .remove(&native_pending_key(params.approval_id.as_ref()))?;
    let answer = devo_protocol::native::methods::ApprovalRespondParams {
        request_id: params.approval_id.to_string(),
        decision: native_approval_decision(params),
    };
    Some(acp_success_response(
        request_id,
        serde_json::to_value(answer).expect("serialize native approval answer"),
    ))
}

/// Drops a Native reverse-request correlation after the server completes the
/// approval through cancellation or another controller.
pub(crate) async fn discard_approval_request(
    pending_approvals: &PendingApprovals,
    approval_id: &str,
) {
    pending_approvals
        .lock()
        .await
        .remove(&native_pending_key(approval_id));
}

fn native_pending_key(approval_id: &str) -> String {
    format!("native:{approval_id}")
}

fn native_approval_decision(
    params: &ApprovalResponseParams,
) -> devo_protocol::native::item::ApprovalDecision {
    use devo_protocol::native::item::{ApprovalDecisionKind, ApprovalScope};
    let decision = match params.decision {
        ApprovalDecisionValue::Approve => ApprovalDecisionKind::Approved,
        ApprovalDecisionValue::Deny => ApprovalDecisionKind::Denied,
        ApprovalDecisionValue::Cancel => ApprovalDecisionKind::Cancelled,
    };
    let scope = match params.scope {
        ApprovalScopeValue::Once => ApprovalScope::Once,
        ApprovalScopeValue::Turn => ApprovalScope::Turn,
        ApprovalScopeValue::Session => ApprovalScope::Session,
        ApprovalScopeValue::PathPrefix => ApprovalScope::PathPrefix,
        ApprovalScopeValue::Host => ApprovalScope::Host,
        ApprovalScopeValue::Tool => ApprovalScope::Tool,
        ApprovalScopeValue::CommandPrefix => ApprovalScope::CommandPrefix,
        ApprovalScopeValue::CommandPrefixPersist => ApprovalScope::CommandPrefixPersist,
    };
    devo_protocol::native::item::ApprovalDecision {
        decision,
        scope,
        decision_source: devo_protocol::native::item::ApprovalDecisionSource::User,
        decided_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use devo_protocol::ApprovalDecisionValue;
    use devo_protocol::ApprovalResponseParams;
    use devo_protocol::ApprovalScopeValue;
    use devo_protocol::TurnId;
    use pretty_assertions::assert_eq;
    use tokio::sync::Mutex;

    use super::{
        PendingApprovals, discard_approval_request, handle_approval_request,
        resolve_approval_response,
    };

    #[tokio::test]
    async fn native_approval_request_resolves_with_approval_respond_params() {
        let pending_approvals: PendingApprovals = Arc::new(Mutex::new(HashMap::new()));

        handle_approval_request(
            serde_json::json!(55),
            serde_json::json!({
                "type": "approval",
                "approvalId": "call-9",
                "actionSummary": "cargo test",
                "justification": "",
                "availableScopes": ["once", "session"],
            }),
            Arc::clone(&pending_approvals),
        )
        .await
        .expect("Native approval request is accepted");

        let response_params = ApprovalResponseParams {
            session_id: devo_protocol::SessionId::new(),
            turn_id: TurnId::new(),
            approval_id: "call-9".to_string().into(),
            decision: ApprovalDecisionValue::Approve,
            scope: ApprovalScopeValue::Session,
        };
        let response = resolve_approval_response(&pending_approvals, &response_params)
            .await
            .expect("Native pending approval resolves");
        assert_eq!(response["id"], serde_json::json!(55));
        let answer: devo_protocol::native::methods::ApprovalRespondParams =
            serde_json::from_value(response["result"].clone()).expect("decode Native answer");
        assert_eq!(answer.request_id, "call-9");
        assert_eq!(
            answer.decision.decision,
            devo_protocol::native::item::ApprovalDecisionKind::Approved
        );
        assert_eq!(
            answer.decision.scope,
            devo_protocol::native::item::ApprovalScope::Session
        );
    }

    #[tokio::test]
    async fn completed_approval_discards_reverse_request_correlation() {
        let pending_approvals: PendingApprovals = Arc::new(Mutex::new(HashMap::new()));
        handle_approval_request(
            serde_json::json!(56),
            serde_json::json!({ "approvalId": "call-cancelled" }),
            Arc::clone(&pending_approvals),
        )
        .await
        .expect("Native approval request is accepted");

        discard_approval_request(&pending_approvals, "call-cancelled").await;

        assert!(pending_approvals.lock().await.is_empty());
    }
}
