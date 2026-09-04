use std::sync::Arc;

use devo_core::ResponseItem;
use devo_core::{ItemId, SessionId, TurnId};
use devo_protocol::native::item::{CompactionTrigger, ContextUsage, Item};
use devo_protocol::native::legacy_wire_from_native_item;

use super::super::ServerRuntime;
use crate::{
    EventContext, ItemEnvelope, ItemEventPayload, ServerEvent, SessionCompactionFailedPayload,
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
            .emit_native_item_started(
                session_id,
                turn_id,
                item_id,
                None,
                compaction_started_item(),
            )
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
            .emit_native_item_completed(
                session_id,
                turn_id,
                item_id,
                item_seq,
                compaction_completed_item(),
            )
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

fn compaction_usage() -> ContextUsage {
    ContextUsage {
        measured: false,
        ..ContextUsage::default()
    }
}

fn compaction_started_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::AutoThreshold,
        before: compaction_usage(),
        after: None,
        summary: Some("Compaction started".to_string()),
    }
}

fn compaction_completed_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::AutoThreshold,
        before: compaction_usage(),
        after: None,
        summary: Some("Context compacted".to_string()),
    }
}

fn compaction_failed_item(message: &str) -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::AutoThreshold,
        before: compaction_usage(),
        after: None,
        summary: Some(format!("Compaction failed: {message}")),
    }
}

fn manual_compaction_started_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::Manual,
        before: compaction_usage(),
        after: None,
        summary: Some("Compaction started".to_string()),
    }
}

fn manual_compaction_completed_item() -> Item {
    Item::ContextCompaction {
        trigger: CompactionTrigger::Manual,
        before: compaction_usage(),
        after: None,
        summary: Some("Context compacted".to_string()),
    }
}

#[cfg(test)]
pub(super) fn started_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
) -> ServerEvent {
    item_event_from_native(
        session_id,
        turn_id,
        item_id,
        None,
        ServerEvent::ItemStarted,
        compaction_started_item(),
    )
}

#[cfg(test)]
pub(super) fn completed_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: Option<u64>,
) -> ServerEvent {
    item_event_from_native(
        session_id,
        turn_id,
        item_id,
        item_seq,
        ServerEvent::ItemCompleted,
        compaction_completed_item(),
    )
}

pub(super) fn failed_events(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    message: String,
) -> [ServerEvent; 2] {
    [
        item_event_from_native(
            session_id,
            turn_id,
            item_id,
            None,
            ServerEvent::ItemCompleted,
            compaction_failed_item(&message),
        ),
        ServerEvent::SessionCompactionFailed(SessionCompactionFailedPayload {
            session_id,
            message,
        }),
    ]
}

pub(crate) fn manual_compaction_started_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: Option<u64>,
) -> ServerEvent {
    item_event_from_native(
        session_id,
        turn_id,
        item_id,
        item_seq,
        ServerEvent::ItemStarted,
        manual_compaction_started_item(),
    )
}

pub(crate) fn manual_compaction_completed_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: u64,
) -> ServerEvent {
    item_event_from_native(
        session_id,
        turn_id,
        item_id,
        Some(item_seq),
        ServerEvent::ItemCompleted,
        manual_compaction_completed_item(),
    )
}

pub(crate) fn manual_compaction_item_failed_event(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    message: String,
) -> ServerEvent {
    item_event_from_native(
        session_id,
        turn_id,
        item_id,
        None,
        ServerEvent::ItemCompleted,
        Item::ContextCompaction {
            trigger: CompactionTrigger::Manual,
            before: compaction_usage(),
            after: None,
            summary: Some(format!("Compaction failed: {message}")),
        },
    )
}

fn item_event_from_native(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: ItemId,
    item_seq: Option<u64>,
    wrap: impl FnOnce(ItemEventPayload) -> ServerEvent,
    native_item: Item,
) -> ServerEvent {
    let (item_kind, payload) =
        legacy_wire_from_native_item(&native_item).expect("compaction item must reverse-project");
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
            item_kind,
            payload,
        },
    })
}
