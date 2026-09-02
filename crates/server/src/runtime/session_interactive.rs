use std::collections::HashMap;

use devo_core::TurnId;
use devo_protocol::ApprovalDecisionValue;
use devo_protocol::SessionId;
use tokio::sync::Mutex;
use tokio::sync::oneshot;

use crate::execution::PendingApproval;
use crate::execution::PendingUserInput;

#[derive(Default)]
struct SessionInteractiveState {
    pending_approvals: HashMap<String, PendingApproval>,
    pending_approval_controllers: HashMap<String, PendingApprovalController>,
    pending_user_inputs: HashMap<String, PendingUserInput>,
}

struct PendingApprovalController {
    tx: tokio::sync::mpsc::UnboundedSender<(
        ApprovalDecisionValue,
        devo_protocol::ApprovalScopeValue,
    )>,
    available_scopes: Vec<String>,
}

/// Global interactive wait lanes keyed by session id.
///
/// Approval and user-input waits are routed here so a session actor blocked in
/// `query()` never has to process mailbox messages for client responses.
#[derive(Default)]
pub(crate) struct SessionInteractiveLanes {
    inner: Mutex<HashMap<SessionId, SessionInteractiveState>>,
}

impl SessionInteractiveLanes {
    pub(crate) async fn register_pending_approval(
        &self,
        host_session_id: SessionId,
        approval_id: String,
        pending: PendingApproval,
        controller_tx: tokio::sync::mpsc::UnboundedSender<(
            ApprovalDecisionValue,
            devo_protocol::ApprovalScopeValue,
        )>,
        available_scopes: Vec<String>,
    ) {
        let mut lanes = self.inner.lock().await;
        let state = lanes.entry(host_session_id).or_default();
        state.pending_approval_controllers.insert(
            approval_id.clone(),
            PendingApprovalController {
                tx: controller_tx,
                available_scopes,
            },
        );
        state.pending_approvals.insert(approval_id, pending);
    }

    pub(crate) async fn remove_pending_approval(
        &self,
        host_session_id: SessionId,
        approval_id: &str,
    ) -> Option<PendingApproval> {
        let mut lanes = self.inner.lock().await;
        let state = lanes.get_mut(&host_session_id)?;
        let removed = state.pending_approvals.remove(approval_id);
        state.pending_approval_controllers.remove(approval_id);
        if state.pending_approvals.is_empty() && state.pending_user_inputs.is_empty() {
            lanes.remove(&host_session_id);
        }
        removed
    }

    pub(crate) async fn take_pending_approval(
        &self,
        host_session_id: SessionId,
        approval_id: &str,
    ) -> Option<PendingApproval> {
        self.remove_pending_approval(host_session_id, approval_id)
            .await
    }

    pub(crate) async fn has_pending_approval(&self, approval_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .values()
            .any(|state| state.pending_approvals.contains_key(approval_id))
    }

    pub(crate) async fn register_pending_user_input(
        &self,
        session_id: SessionId,
        request_id: String,
        pending: PendingUserInput,
    ) {
        self.inner
            .lock()
            .await
            .entry(session_id)
            .or_default()
            .pending_user_inputs
            .insert(request_id, pending);
    }

    pub(crate) async fn has_pending_user_input_request(&self, request_id: &str) -> bool {
        self.inner
            .lock()
            .await
            .values()
            .any(|state| state.pending_user_inputs.contains_key(request_id))
    }

    pub(crate) async fn take_pending_user_input(
        &self,
        owner_session_id: SessionId,
        request_id: &str,
        expected_turn_id: TurnId,
    ) -> Result<PendingUserInput, UserInputTakeError> {
        let mut lanes = self.inner.lock().await;
        let Some(host_session_id) = lanes.iter().find_map(|(host_session_id, state)| {
            state
                .pending_user_inputs
                .get(request_id)
                .filter(|pending| pending.owner_session_id == owner_session_id)
                .map(|_| *host_session_id)
        }) else {
            return Err(UserInputTakeError::NotFound);
        };
        let state = lanes
            .get_mut(&host_session_id)
            .expect("pending user-input host lane exists");
        let Some(pending) = state.pending_user_inputs.remove(request_id) else {
            return Err(UserInputTakeError::NotFound);
        };
        if pending.turn_id != expected_turn_id {
            state
                .pending_user_inputs
                .insert(request_id.to_string(), pending);
            return Err(UserInputTakeError::WrongTurn);
        }
        if state.pending_approvals.is_empty() && state.pending_user_inputs.is_empty() {
            lanes.remove(&host_session_id);
        }
        Ok(pending)
    }

    pub(crate) async fn has_pending_interactive(&self, session_id: SessionId) -> bool {
        self.inner
            .lock()
            .await
            .iter()
            .any(|(host_session_id, state)| {
                (*host_session_id == session_id
                    && (!state.pending_approvals.is_empty()
                        || !state.pending_user_inputs.is_empty()))
                    || state
                        .pending_approvals
                        .values()
                        .any(|pending| pending.owner_session_id == session_id)
                    || state
                        .pending_user_inputs
                        .values()
                        .any(|pending| pending.owner_session_id == session_id)
            })
    }

    pub(crate) async fn has_pending_approval_for_session(
        &self,
        host_session_id: SessionId,
        owner_session_id: SessionId,
    ) -> bool {
        self.inner
            .lock()
            .await
            .get(&host_session_id)
            .is_some_and(|state| {
                state
                    .pending_approvals
                    .values()
                    .any(|pending| pending.owner_session_id == owner_session_id)
            })
    }

    pub(crate) async fn approval_controller(
        &self,
        approval_id: &str,
    ) -> Option<(
        SessionId,
        tokio::sync::mpsc::UnboundedSender<(
            ApprovalDecisionValue,
            devo_protocol::ApprovalScopeValue,
        )>,
    )> {
        self.inner
            .lock()
            .await
            .iter()
            .find_map(|(host_session_id, state)| {
                state
                    .pending_approval_controllers
                    .get(approval_id)
                    .map(|controller| (*host_session_id, controller.tx.clone()))
            })
    }

    pub(crate) async fn drain_pending_user_inputs_for_turn(
        &self,
        owner_session_id: SessionId,
        turn_id: TurnId,
    ) -> Vec<(String, PendingUserInput)> {
        let mut lanes = self.inner.lock().await;
        let mut removed = Vec::new();
        for state in lanes.values_mut() {
            let request_ids = state
                .pending_user_inputs
                .iter()
                .filter(|(_, pending)| {
                    pending.owner_session_id == owner_session_id && pending.turn_id == turn_id
                })
                .map(|(request_id, _)| request_id.clone())
                .collect::<Vec<_>>();
            for request_id in request_ids {
                if let Some(pending) = state.pending_user_inputs.remove(&request_id) {
                    removed.push((request_id, pending));
                }
            }
        }
        lanes.retain(|_, state| {
            !state.pending_approvals.is_empty() || !state.pending_user_inputs.is_empty()
        });
        removed
    }

    pub(crate) async fn clear_owner_session(&self, owner_session_id: SessionId) {
        let mut lanes = self.inner.lock().await;
        lanes.retain(|host_session_id, state| {
            if *host_session_id == owner_session_id {
                return false;
            }
            state
                .pending_approvals
                .retain(|_, pending| pending.owner_session_id != owner_session_id);
            state
                .pending_user_inputs
                .retain(|_, pending| pending.owner_session_id != owner_session_id);
            !state.pending_approvals.is_empty() || !state.pending_user_inputs.is_empty()
        });
    }

    pub(crate) async fn drain_pending_user_inputs_for_session(
        &self,
        session_id: SessionId,
    ) -> Vec<(String, PendingUserInput)> {
        let mut lanes = self.inner.lock().await;
        let mut removed = Vec::new();
        for (host_session_id, state) in lanes.iter_mut() {
            let request_ids = state
                .pending_user_inputs
                .iter()
                .filter(|(_, pending)| {
                    *host_session_id == session_id || pending.owner_session_id == session_id
                })
                .map(|(request_id, _)| request_id.clone())
                .collect::<Vec<_>>();
            for request_id in request_ids {
                if let Some(pending) = state.pending_user_inputs.remove(&request_id) {
                    removed.push((request_id, pending));
                }
            }
        }
        lanes.retain(|_, state| {
            !state.pending_approvals.is_empty() || !state.pending_user_inputs.is_empty()
        });
        removed
    }

    pub(crate) async fn clear_session(&self, session_id: SessionId) {
        self.clear_owner_session(session_id).await;
    }

    /// Clones actionable requests for a subscribed session. A controller of
    /// a host session sees its child requests, while a direct child
    /// subscription can recover the same request by owner id.
    pub(crate) async fn pending_snapshot(&self, session_id: SessionId) -> PendingSnapshot {
        let lanes = self.inner.lock().await;
        let mut snapshot = PendingSnapshot::default();
        for (host_session_id, state) in lanes.iter() {
            snapshot.approvals.extend(
                state
                    .pending_approvals
                    .iter()
                    .filter(|(_, pending)| {
                        *host_session_id == session_id || pending.owner_session_id == session_id
                    })
                    .map(|(approval_id, pending)| PendingApprovalSnapshot {
                        owner_session_id: pending.owner_session_id,
                        approval_id: approval_id.clone(),
                        turn_id: pending.turn_id,
                        tool_name: pending.tool_name.clone(),
                        resource: pending.resource.clone(),
                        path: pending.path.clone(),
                        host: pending.host.clone(),
                        command: pending.command.clone(),
                        command_pattern: pending.command_pattern.clone(),
                        command_prefix: pending.command_prefix.clone(),
                        available_scopes: state
                            .pending_approval_controllers
                            .get(approval_id)
                            .map(|controller| controller.available_scopes.clone())
                            .unwrap_or_default(),
                        persisted: pending.persisted.clone(),
                    }),
            );
            snapshot.user_inputs.extend(
                state
                    .pending_user_inputs
                    .iter()
                    .filter(|(_, pending)| {
                        *host_session_id == session_id || pending.owner_session_id == session_id
                    })
                    .map(|(request_id, pending)| PendingUserInputSnapshot {
                        owner_session_id: pending.owner_session_id,
                        request_id: request_id.clone(),
                        turn_id: pending.turn_id,
                        questions: pending.questions.clone(),
                        persisted: pending.persisted.clone(),
                    }),
            );
        }
        snapshot
    }
}

#[derive(Default)]
pub(crate) struct PendingSnapshot {
    pub(crate) approvals: Vec<PendingApprovalSnapshot>,
    pub(crate) user_inputs: Vec<PendingUserInputSnapshot>,
}

pub(crate) struct PendingApprovalSnapshot {
    pub(crate) owner_session_id: SessionId,
    pub(crate) approval_id: String,
    pub(crate) turn_id: devo_core::TurnId,
    pub(crate) tool_name: String,
    pub(crate) resource: Option<devo_safety::ResourceKind>,
    pub(crate) path: Option<std::path::PathBuf>,
    pub(crate) host: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) command_pattern: Option<Vec<String>>,
    pub(crate) command_prefix: Option<Vec<String>>,
    pub(crate) available_scopes: Vec<String>,
    pub(crate) persisted: Option<crate::execution::PersistedLivingItem>,
}

pub(crate) struct PendingUserInputSnapshot {
    pub(crate) owner_session_id: SessionId,
    pub(crate) request_id: String,
    pub(crate) turn_id: devo_core::TurnId,
    pub(crate) questions: Vec<devo_protocol::RequestUserInputQuestion>,
    pub(crate) persisted: Option<crate::execution::PersistedLivingItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UserInputTakeError {
    NotFound,
    WrongTurn,
}

pub(crate) async fn complete_approval_wait(
    rx: oneshot::Receiver<ApprovalDecisionValue>,
) -> Result<ApprovalDecisionValue, String> {
    match rx.await {
        Ok(ApprovalDecisionValue::Approve) => Ok(ApprovalDecisionValue::Approve),
        Ok(ApprovalDecisionValue::Deny) => Err("rejected by user".to_string()),
        Ok(ApprovalDecisionValue::Cancel) => Err("cancelled by user".to_string()),
        Err(_) => Err("approval channel closed".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn pending_approval_tracks_the_originating_child_session() {
        let lanes = SessionInteractiveLanes::default();
        let parent_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let (tx, _rx) = oneshot::channel();
        let (controller_tx, _controller_rx) = tokio::sync::mpsc::unbounded_channel();
        lanes
            .register_pending_approval(
                parent_session_id,
                "approval-1".to_string(),
                PendingApproval {
                    owner_session_id: child_session_id,
                    turn_id: TurnId::new(),
                    tool_name: "exec_command".to_string(),
                    resource: Some(devo_safety::ResourceKind::ShellExec),
                    path: None,
                    host: None,
                    command_prefix: None,
                    command_pattern: None,
                    requests_escalation: false,
                    command: None,
                    cwd: std::path::PathBuf::new(),
                    sandbox_permissions: String::new(),
                    persisted: None,
                    checkpoint: None,
                    tx,
                },
                controller_tx,
                vec!["once".to_string()],
            )
            .await;

        assert_eq!(
            lanes
                .has_pending_approval_for_session(parent_session_id, child_session_id)
                .await,
            true
        );
        assert_eq!(
            lanes
                .has_pending_approval_for_session(parent_session_id, parent_session_id)
                .await,
            false
        );
    }

    #[tokio::test]
    async fn parent_and_child_snapshots_recover_the_same_child_user_input() {
        let lanes = SessionInteractiveLanes::default();
        let parent_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let turn_id = TurnId::new();
        let (tx, _rx) = oneshot::channel();
        lanes
            .register_pending_user_input(
                parent_session_id,
                "question-1".to_string(),
                PendingUserInput {
                    owner_session_id: child_session_id,
                    turn_id,
                    questions: Vec::new(),
                    persisted: None,
                    tx,
                },
            )
            .await;

        let parent = lanes.pending_snapshot(parent_session_id).await;
        let child = lanes.pending_snapshot(child_session_id).await;
        assert_eq!(
            parent
                .user_inputs
                .iter()
                .map(|pending| {
                    (
                        pending.request_id.clone(),
                        pending.owner_session_id,
                        pending.turn_id,
                    )
                })
                .collect::<Vec<_>>(),
            vec![("question-1".to_string(), child_session_id, turn_id)]
        );
        assert_eq!(
            child
                .user_inputs
                .iter()
                .map(|pending| {
                    (
                        pending.request_id.clone(),
                        pending.owner_session_id,
                        pending.turn_id,
                    )
                })
                .collect::<Vec<_>>(),
            vec![("question-1".to_string(), child_session_id, turn_id)]
        );

        let pending = lanes
            .take_pending_user_input(child_session_id, "question-1", turn_id)
            .await
            .expect("child response resolves the host-lane request");
        assert_eq!(pending.owner_session_id, child_session_id);
        assert_eq!(
            lanes.has_pending_interactive(parent_session_id).await,
            false
        );
    }
}
