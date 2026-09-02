use std::collections::HashMap;

use chrono::Utc;
use devo_protocol::native::ids::{
    ItemId as NativeItemId, SessionId as NativeSessionId, TurnId as NativeTurnId,
};
use devo_protocol::native::item::{
    ApprovalDecision, ApprovalDecisionKind, ApprovalDecisionSource, ApprovalScope, ApprovalTarget,
    FileChangeEntry, FileChangeKind, Item, ItemEnvelope, ItemState, UserQuestion,
    UserQuestionOption,
};
use uuid::Uuid;

use super::*;

impl ServerRuntime {
    pub(super) async fn persist_waiting_approval_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: devo_core::ItemId,
        seq: u64,
        request: &devo_core::tools::ToolPermissionRequest,
        available_scopes: &[String],
    ) -> Option<crate::execution::PersistedLivingItem> {
        let item_id = NativeItemId::from_legacy_uuid(Uuid::from(item_id));
        let now = Utc::now();
        let item = approval_envelope(
            item_id.clone(),
            session_id,
            turn_id,
            seq,
            1,
            now,
            now,
            ItemState::Waiting,
            &request.tool_call_id,
            request,
            available_scopes,
            None,
        );
        self.persist_native_active_turn_item(session_id, item)
            .await
            .then_some(crate::execution::PersistedLivingItem {
                item_id,
                seq,
                created_at: now,
            })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn persist_resolved_approval_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request: &devo_core::tools::ToolPermissionRequest,
        available_scopes: &[String],
        decision: ApprovalDecisionKind,
        scope: ApprovalScope,
        source: ApprovalDecisionSource,
        persisted: &crate::execution::PersistedLivingItem,
    ) {
        let now = Utc::now();
        let item = approval_envelope(
            persisted.item_id.clone(),
            session_id,
            turn_id,
            persisted.seq,
            2,
            persisted.created_at,
            now,
            ItemState::Completed,
            &request.tool_call_id,
            request,
            available_scopes,
            Some(ApprovalDecision {
                decision,
                scope,
                decision_source: source,
                decided_at: now,
            }),
        );
        self.persist_native_active_turn_item(session_id, item).await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn persist_completed_approval_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: devo_core::ItemId,
        seq: u64,
        approval_id: &str,
        request: &devo_core::tools::ToolPermissionRequest,
        decision: ApprovalDecisionKind,
        source: ApprovalDecisionSource,
    ) {
        let now = Utc::now();
        let item = approval_envelope(
            NativeItemId::from_legacy_uuid(Uuid::from(item_id)),
            session_id,
            turn_id,
            seq,
            1,
            now,
            now,
            ItemState::Completed,
            approval_id,
            request,
            &[],
            Some(ApprovalDecision {
                decision,
                scope: ApprovalScope::Once,
                decision_source: source,
                decided_at: now,
            }),
        );
        self.persist_native_active_turn_item(session_id, item).await;
    }

    pub(super) async fn persist_file_change_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        item_id: devo_core::ItemId,
        seq: u64,
        call_id: String,
        changes: &[(std::path::PathBuf, devo_protocol::FileChange)],
    ) {
        let now = Utc::now();
        let item = ItemEnvelope {
            id: NativeItemId::from_legacy_uuid(Uuid::from(item_id)),
            session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
            turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
            seq,
            revision: 2,
            created_at: now,
            updated_at: now,
            state: ItemState::Completed,
            item: Item::FileChange {
                call_id,
                changes: changes
                    .iter()
                    .map(|(path, change)| FileChangeEntry {
                        path: path.clone(),
                        change: match change {
                            devo_protocol::FileChange::Add { content } => FileChangeKind::Add {
                                content: content.clone(),
                            },
                            devo_protocol::FileChange::Delete { content } => {
                                FileChangeKind::Delete {
                                    content: content.clone(),
                                }
                            }
                            devo_protocol::FileChange::Update {
                                unified_diff,
                                move_path,
                                ..
                            } => FileChangeKind::Update {
                                unified_diff: unified_diff.clone(),
                                move_path: move_path.clone(),
                            },
                        },
                    })
                    .collect(),
                sandbox: None,
            },
        };
        self.persist_native_active_turn_item(session_id, item).await;
    }

    pub(super) async fn persist_waiting_user_input_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        questions: &[devo_protocol::RequestUserInputQuestion],
    ) -> Option<crate::execution::PersistedLivingItem> {
        let item_id = NativeItemId::from_legacy_uuid(Uuid::now_v7());
        let seq = self.allocate_item_sequence(session_id).await;
        let now = Utc::now();
        let item = ItemEnvelope {
            id: item_id.clone(),
            session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
            turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
            seq,
            revision: 1,
            created_at: now,
            updated_at: now,
            state: ItemState::Waiting,
            item: Item::UserInputRequest {
                request_id,
                target_item_id: None,
                questions: native_questions(questions),
                answers: None,
            },
        };
        self.persist_native_active_turn_item(session_id, item)
            .await
            .then_some(crate::execution::PersistedLivingItem {
                item_id,
                seq,
                created_at: now,
            })
    }

    pub(super) async fn persist_answered_user_input_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        questions: &[devo_protocol::RequestUserInputQuestion],
        response: &devo_protocol::RequestUserInputResponse,
        persisted: &crate::execution::PersistedLivingItem,
    ) {
        let now = Utc::now();
        let item = ItemEnvelope {
            id: persisted.item_id.clone(),
            session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
            turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
            seq: persisted.seq,
            revision: 2,
            created_at: persisted.created_at,
            updated_at: now,
            state: ItemState::Completed,
            item: Item::UserInputRequest {
                request_id,
                target_item_id: None,
                questions: native_questions(questions),
                answers: serde_json::to_value(response).ok(),
            },
        };
        self.persist_native_active_turn_item(session_id, item).await;
    }

    pub(super) async fn persist_terminal_user_input_item(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        request_id: String,
        questions: &[devo_protocol::RequestUserInputQuestion],
        state: ItemState,
        persisted: &crate::execution::PersistedLivingItem,
    ) {
        let now = Utc::now();
        let item = ItemEnvelope {
            id: persisted.item_id.clone(),
            session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
            turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
            seq: persisted.seq,
            revision: 2,
            created_at: persisted.created_at,
            updated_at: now,
            state,
            item: Item::UserInputRequest {
                request_id,
                target_item_id: None,
                questions: native_questions(questions),
                answers: None,
            },
        };
        self.persist_native_active_turn_item(session_id, item).await;
    }

    async fn persist_native_active_turn_item(
        &self,
        session_id: SessionId,
        item: ItemEnvelope,
    ) -> bool {
        let record = if let Some(stream) = self.active_stream_state(session_id).await {
            let stream = stream.lock().await;
            stream
                .turn_inline
                .as_ref()
                .and_then(|inline| inline.record.clone())
        } else {
            None
        };
        let record = match record {
            Some(record) => record,
            None => {
                let Some(handle) = self.session(session_id).await else {
                    return false;
                };
                let Some(record) = handle.record().await.flatten() else {
                    return false;
                };
                record
            }
        };
        match self.rollout_store.append_canonical_item(&record, item) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to persist canonical interaction item"
                );
                false
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn approval_envelope(
    item_id: NativeItemId,
    session_id: SessionId,
    turn_id: TurnId,
    seq: u64,
    revision: u32,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    state: ItemState,
    approval_id: &str,
    request: &devo_core::tools::ToolPermissionRequest,
    available_scopes: &[String],
    decision: Option<ApprovalDecision>,
) -> ItemEnvelope {
    ItemEnvelope {
        id: item_id,
        session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
        turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
        seq,
        revision,
        created_at,
        updated_at,
        state,
        item: Item::Approval {
            approval_id: approval_id.to_owned(),
            target_item_id: None,
            action_summary: request.action_summary.clone(),
            justification: request.justification.clone().unwrap_or_default(),
            resource: Some(format!("{:?}", request.resource)),
            available_scopes: available_scopes.to_vec(),
            command_pattern: request.command_pattern.clone(),
            command_prefix: request.command_prefix.clone(),
            target: request
                .path
                .as_ref()
                .map(|path| ApprovalTarget::Path { path: path.clone() })
                .or_else(|| {
                    request
                        .host
                        .clone()
                        .map(|host| ApprovalTarget::Host { host })
                })
                .or_else(|| {
                    request
                        .target
                        .clone()
                        .map(|command| ApprovalTarget::Command { command })
                }),
            decision,
        },
    }
}

pub(super) struct RecoveredWaitingApproval {
    pub approval_id: String,
    pub host_session_id: SessionId,
    pub owner_session_id: SessionId,
    pub turn_id: TurnId,
    pub available_scopes: Vec<String>,
    pub persisted: crate::execution::PersistedLivingItem,
    pub decision: Option<devo_protocol::native::item::ApprovalDecision>,
}

impl RecoveredWaitingApproval {
    pub(super) fn decided_approval(&self) -> Option<ApprovalDecisionValue> {
        self.decision.as_ref().map(|decision| {
            use devo_protocol::native::item::ApprovalDecisionKind;
            match decision.decision {
                ApprovalDecisionKind::Approved => ApprovalDecisionValue::Approve,
                ApprovalDecisionKind::Denied => ApprovalDecisionValue::Deny,
                ApprovalDecisionKind::Cancelled => ApprovalDecisionValue::Cancel,
            }
        })
    }

    pub(super) fn scope(&self) -> devo_protocol::ApprovalScopeValue {
        use devo_protocol::native::item::ApprovalScope;
        match self.decision.as_ref().map(|decision| decision.scope) {
            Some(ApprovalScope::Once) => devo_protocol::ApprovalScopeValue::Once,
            Some(ApprovalScope::Turn) => devo_protocol::ApprovalScopeValue::Turn,
            Some(ApprovalScope::Session) => devo_protocol::ApprovalScopeValue::Session,
            Some(ApprovalScope::PathPrefix) => devo_protocol::ApprovalScopeValue::PathPrefix,
            Some(ApprovalScope::Host) => devo_protocol::ApprovalScopeValue::Host,
            Some(ApprovalScope::Tool) => devo_protocol::ApprovalScopeValue::Tool,
            Some(ApprovalScope::CommandPrefix) => devo_protocol::ApprovalScopeValue::CommandPrefix,
            Some(ApprovalScope::CommandPrefixPersist) => {
                devo_protocol::ApprovalScopeValue::CommandPrefixPersist
            }
            None => devo_protocol::ApprovalScopeValue::Once,
        }
    }
}

/// Latest approval items per approval id. Later revisions win.
pub(super) fn core_item_id_from_native(
    native: &devo_protocol::native::ids::ItemId,
) -> Option<devo_core::ItemId> {
    let raw = native.as_str();
    if let Ok(uuid) = Uuid::parse_str(raw) {
        return Some(devo_core::ItemId::from(uuid));
    }
    let stripped = raw.strip_prefix("item_").unwrap_or(raw);
    Uuid::parse_str(stripped).ok().map(devo_core::ItemId::from)
}

pub(super) fn latest_waiting_approvals(
    items: &[ItemEnvelope],
    checkpoints: &std::collections::HashMap<
        String,
        devo_core::TurnApprovalCheckpointRecordedRecord,
    >,
) -> Vec<RecoveredWaitingApproval> {
    let mut latest: HashMap<String, &ItemEnvelope> = HashMap::new();
    for item in items {
        if let Item::Approval { approval_id, .. } = &item.item {
            latest.insert(approval_id.clone(), item);
        }
    }
    latest
        .into_values()
        .filter_map(|item| {
            let Item::Approval {
                approval_id,
                available_scopes,
                decision,
                ..
            } = &item.item
            else {
                return None;
            };
            if item.state != ItemState::Waiting || decision.is_some() {
                return None;
            }
            let owner_session_id = SessionId::try_from(item.session_id.as_str()).ok()?;
            let turn_id = TurnId::try_from(item.turn_id.as_str()).ok()?;
            let host_session_id = checkpoints
                .get(approval_id)
                .map(super::approval_checkpoint::host_session_id_from_checkpoint)
                .unwrap_or(owner_session_id);
            Some(RecoveredWaitingApproval {
                approval_id: approval_id.clone(),
                host_session_id,
                owner_session_id,
                turn_id,
                available_scopes: available_scopes.clone(),
                persisted: crate::execution::PersistedLivingItem {
                    item_id: item.id.clone(),
                    seq: item.seq,
                    created_at: item.created_at,
                },
                decision: None,
            })
        })
        .collect()
}

pub(super) fn latest_decided_approvals(
    items: &[ItemEnvelope],
    checkpoints: &std::collections::HashMap<
        String,
        devo_core::TurnApprovalCheckpointRecordedRecord,
    >,
) -> Vec<RecoveredWaitingApproval> {
    let mut latest: HashMap<String, &ItemEnvelope> = HashMap::new();
    for item in items {
        if let Item::Approval { approval_id, .. } = &item.item {
            latest.insert(approval_id.clone(), item);
        }
    }
    latest
        .into_values()
        .filter_map(|item| {
            let Item::Approval {
                approval_id,
                available_scopes,
                decision,
                ..
            } = &item.item
            else {
                return None;
            };
            let decision = decision.clone()?;
            let owner_session_id = SessionId::try_from(item.session_id.as_str()).ok()?;
            let turn_id = TurnId::try_from(item.turn_id.as_str()).ok()?;
            let host_session_id = checkpoints
                .get(approval_id)
                .map(super::approval_checkpoint::host_session_id_from_checkpoint)
                .unwrap_or(owner_session_id);
            Some(RecoveredWaitingApproval {
                approval_id: approval_id.clone(),
                host_session_id,
                owner_session_id,
                turn_id,
                available_scopes: available_scopes.clone(),
                persisted: crate::execution::PersistedLivingItem {
                    item_id: item.id.clone(),
                    seq: item.seq,
                    created_at: item.created_at,
                },
                decision: Some(decision),
            })
        })
        .collect()
}

pub(super) struct RecoveredWaitingUserInput {
    pub request_id: String,
    pub owner_session_id: SessionId,
    pub turn_id: TurnId,
    pub questions: Vec<devo_protocol::RequestUserInputQuestion>,
    pub persisted: crate::execution::PersistedLivingItem,
}

/// Latest unanswered `UserInputRequest` per request id. Later revisions win,
/// so a completed/interrupted answer is not resurrected after restart.
pub(super) fn latest_waiting_user_inputs(items: &[ItemEnvelope]) -> Vec<RecoveredWaitingUserInput> {
    let mut latest: HashMap<String, &ItemEnvelope> = HashMap::new();
    for item in items {
        if let Item::UserInputRequest { request_id, .. } = &item.item {
            latest.insert(request_id.clone(), item);
        }
    }
    latest
        .into_values()
        .filter_map(|item| {
            let Item::UserInputRequest {
                request_id,
                questions,
                answers,
                ..
            } = &item.item
            else {
                return None;
            };
            if item.state != ItemState::Waiting || answers.is_some() {
                return None;
            }
            let owner_session_id = SessionId::try_from(item.session_id.as_str()).ok()?;
            let turn_id = TurnId::try_from(item.turn_id.as_str()).ok()?;
            Some(RecoveredWaitingUserInput {
                request_id: request_id.clone(),
                owner_session_id,
                turn_id,
                questions: protocol_questions(questions),
                persisted: crate::execution::PersistedLivingItem {
                    item_id: item.id.clone(),
                    seq: item.seq,
                    created_at: item.created_at,
                },
            })
        })
        .collect()
}

fn protocol_questions(questions: &[UserQuestion]) -> Vec<devo_protocol::RequestUserInputQuestion> {
    questions
        .iter()
        .map(|question| devo_protocol::RequestUserInputQuestion {
            id: question.id.clone(),
            header: question.header.clone(),
            question: question.question.clone(),
            is_other: question.is_other,
            is_secret: question.is_secret,
            options: question.options.as_ref().map(|options| {
                options
                    .iter()
                    .map(|option| devo_protocol::RequestUserInputOption {
                        label: option.label.clone(),
                        description: option.description.clone(),
                    })
                    .collect()
            }),
        })
        .collect()
}

pub(super) fn native_questions(
    questions: &[devo_protocol::RequestUserInputQuestion],
) -> Vec<UserQuestion> {
    questions
        .iter()
        .map(|question| UserQuestion {
            id: question.id.clone(),
            header: question.header.clone(),
            question: question.question.clone(),
            is_other: question.is_other,
            is_secret: question.is_secret,
            options: question.options.as_ref().map(|options| {
                options
                    .iter()
                    .map(|option| UserQuestionOption {
                        label: option.label.clone(),
                        description: option.description.clone(),
                    })
                    .collect()
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use uuid::Uuid;

    fn waiting_envelope(
        request_id: &str,
        session_id: SessionId,
        turn_id: TurnId,
        revision: u32,
        state: ItemState,
        answers: Option<serde_json::Value>,
    ) -> ItemEnvelope {
        let now = Utc::now();
        ItemEnvelope {
            id: NativeItemId::from_legacy_uuid(Uuid::now_v7()),
            session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
            turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
            seq: u64::from(revision),
            revision,
            created_at: now,
            updated_at: now,
            state,
            item: Item::UserInputRequest {
                request_id: request_id.to_string(),
                target_item_id: None,
                questions: vec![UserQuestion {
                    id: "environment".into(),
                    header: "Environment".into(),
                    question: "Where should this run?".into(),
                    is_other: false,
                    is_secret: false,
                    options: None,
                }],
                answers,
            },
        }
    }

    #[test]
    fn latest_waiting_user_inputs_keeps_unanswered_waiting_items() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let waiting = waiting_envelope(
            "question-1",
            session_id,
            turn_id,
            1,
            ItemState::Waiting,
            None,
        );
        let recovered = latest_waiting_user_inputs(&[waiting]);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].request_id, "question-1");
        assert_eq!(recovered[0].owner_session_id, session_id);
        assert_eq!(recovered[0].turn_id, turn_id);
        assert_eq!(recovered[0].questions[0].id, "environment");
    }

    #[test]
    fn latest_waiting_user_inputs_drops_later_completed_revisions() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let waiting = waiting_envelope(
            "question-1",
            session_id,
            turn_id,
            1,
            ItemState::Waiting,
            None,
        );
        let completed = waiting_envelope(
            "question-1",
            session_id,
            turn_id,
            2,
            ItemState::Completed,
            Some(serde_json::json!({ "environment": { "answers": ["Local"] } })),
        );
        assert_eq!(
            latest_waiting_user_inputs(&[waiting, completed])
                .iter()
                .map(|item| item.request_id.as_str())
                .collect::<Vec<_>>(),
            Vec::<&str>::new()
        );
    }

    fn waiting_approval_envelope(
        approval_id: &str,
        session_id: SessionId,
        turn_id: TurnId,
        state: ItemState,
        decision: Option<ApprovalDecision>,
    ) -> ItemEnvelope {
        let now = Utc::now();
        ItemEnvelope {
            id: NativeItemId::from_legacy_uuid(Uuid::now_v7()),
            session_id: NativeSessionId::from_legacy_uuid(Uuid::from(session_id)),
            turn_id: NativeTurnId::from_legacy_uuid(Uuid::from(turn_id)),
            seq: 1,
            revision: 1,
            created_at: now,
            updated_at: now,
            state,
            item: Item::Approval {
                approval_id: approval_id.to_string(),
                target_item_id: None,
                action_summary: "Run command".to_string(),
                justification: String::new(),
                resource: Some("ShellExec".to_string()),
                available_scopes: vec!["once".to_string()],
                command_pattern: None,
                command_prefix: None,
                target: None,
                decision,
            },
        }
    }

    #[test]
    fn latest_waiting_approvals_keeps_unanswered_waiting_items() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let waiting =
            waiting_approval_envelope("approval-1", session_id, turn_id, ItemState::Waiting, None);
        assert_eq!(
            latest_waiting_approvals(&[waiting], &std::collections::HashMap::new())
                .iter()
                .map(|item| item.approval_id.as_str())
                .collect::<Vec<_>>(),
            vec!["approval-1"]
        );
    }

    #[test]
    fn core_item_id_from_native_accepts_legacy_and_prefixed_ids() {
        let legacy = Uuid::now_v7();
        let legacy_id = NativeItemId::from_legacy_uuid(legacy);
        let core_id = super::core_item_id_from_native(&legacy_id).expect("legacy uuid");
        assert_eq!(core_id, devo_core::ItemId::from(legacy));
        assert!(super::core_item_id_from_native(&NativeItemId::new()).is_some());
    }
}
