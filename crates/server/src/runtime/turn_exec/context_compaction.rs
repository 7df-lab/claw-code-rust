use std::sync::Arc;

use devo_core::ResponseItem;
use devo_core::{ItemId, SessionId, TurnId};

use super::super::ServerRuntime;
use crate::{
    EventContext, ItemEnvelope, ItemEventPayload, ItemKind, ServerEvent,
    SessionCompactionFailedPayload,
};

#[derive(Default)]
pub(super) struct ContextCompactionLifecycle {
    item_id: Option<ItemId>,
}

impl ContextCompactionLifecycle {
    pub(super) async fn start(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        if self.item_id.is_some() {
            self.fail(
                runtime,
                session_id,
                turn_id,
                "compaction restarted before the previous lifecycle completed".to_string(),
            )
            .await;
        }
        let item_id = ItemId::new();
        self.item_id = Some(item_id);
        runtime
            .broadcast_event(started_event(session_id, turn_id, item_id))
            .await;
    }

    pub(super) async fn complete(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
        compacted_items: Vec<ResponseItem>,
    ) {
        let Some(item_id) = self.item_id.take() else {
            return;
        };
        let item_seq = runtime
            .persist_in_turn_compaction(session_id, turn_id, item_id, &compacted_items)
            .await;
        runtime
            .broadcast_event(completed_event(session_id, turn_id, item_id, item_seq))
            .await;
    }

    pub(super) async fn fail(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
        message: String,
    ) {
        if let Some(item_id) = self.item_id.take() {
            for event in failed_events(session_id, turn_id, item_id, message) {
                runtime.broadcast_event(event).await;
            }
        } else {
            runtime
                .broadcast_event(ServerEvent::SessionCompactionFailed(
                    SessionCompactionFailedPayload {
                        session_id,
                        message,
                    },
                ))
                .await;
        }
    }

    pub(super) async fn close_if_open(
        &mut self,
        runtime: &Arc<ServerRuntime>,
        session_id: SessionId,
        turn_id: TurnId,
    ) {
        if self.item_id.is_some() {
            self.fail(
                runtime,
                session_id,
                turn_id,
                "compaction lifecycle ended before completion".to_string(),
            )
            .await;
        }
    }
}

pub(super) fn started_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
) -> ServerEvent {
    item_event(
        session_id,
        turn_id,
        item_id,
        None,
        ServerEvent::ItemStarted,
        serde_json::json!({ "title": "Compaction started" }),
    )
}

pub(super) fn completed_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: Option<u64>,
) -> ServerEvent {
    item_event(
        session_id,
        turn_id,
        item_id,
        item_seq,
        ServerEvent::ItemCompleted,
        serde_json::json!({ "title": "Context compacted" }),
    )
}

pub(super) fn failed_events(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    message: String,
) -> [ServerEvent; 2] {
    [
        item_event(
            session_id,
            turn_id,
            item_id,
            None,
            ServerEvent::ItemCompleted,
            serde_json::json!({
                "title": "Compaction failed",
                "status": "failed",
                "message": message.clone(),
            }),
        ),
        ServerEvent::SessionCompactionFailed(SessionCompactionFailedPayload {
            session_id,
            message,
        }),
    ]
}

fn item_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: Option<u64>,
    wrap: impl FnOnce(ItemEventPayload) -> ServerEvent,
    payload: serde_json::Value,
) -> ServerEvent {
    wrap(ItemEventPayload {
        context: EventContext {
            session_id,
            turn_id: Some(turn_id),
            item_id: Some(item_id),
            seq: item_seq.unwrap_or(0),
            item_seq,
        },
        item: ItemEnvelope {
            item_id,
            item_kind: ItemKind::ContextCompaction,
            payload,
        },
    })
}
