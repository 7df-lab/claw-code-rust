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
        let Some(stream) = self.active_stream_state(session_id).await else {
            return false;
        };
        let record = {
            let stream = stream.lock().await;
            stream
                .turn_inline
                .as_ref()
                .and_then(|inline| inline.record.clone())
        };
        let Some(record) = record else {
            return false;
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
