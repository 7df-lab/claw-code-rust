//! Native typed `item/started` and `item/completed` dispatch.

use devo_core::ItemId;
use devo_protocol::TypedItemEventPayload;
use tokio::sync::mpsc;

use crate::events::WorkerEvent;

use super::approval_items;
use super::compaction_items;
use super::native_items;
use super::plan_items;

/// Projects a native typed item lifecycle notification into worker events.
pub(crate) fn dispatch_typed_item_lifecycle(
    method: &str,
    payload: &TypedItemEventPayload,
    item_id: ItemId,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) {
    let item = &payload.item.item;
    let transcript_events = if method == "item/started" {
        native_items::started_events(item, item_id, payload.context.item_seq)
    } else {
        native_items::completed_events(item, item_id)
    };
    let had_transcript = !transcript_events.is_empty();
    for event in transcript_events {
        let _ = event_tx.send(WorkerEvent::Transcript(event));
    }
    if had_transcript {
        return;
    }

    if method == "item/started" {
        if plan_items::handle_started(item, item_id, event_tx) {
            return;
        }
        if compaction_items::handle_started(item, event_tx) {
            return;
        }
        let turn_id = payload
            .context
            .turn_id
            .or_else(|| devo_core::TurnId::try_from(payload.item.turn_id.as_str()).ok());
        let _ = approval_items::handle_started(item, payload.context.session_id, turn_id, event_tx);
        return;
    }

    if plan_items::handle_completed(item, item_id, event_tx) {
        return;
    }
    if compaction_items::handle_completed(item, event_tx) {
        return;
    }
    let _ = approval_items::handle_completed(item, event_tx);
}
