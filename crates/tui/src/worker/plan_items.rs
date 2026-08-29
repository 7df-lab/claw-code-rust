//! Native `Item::Plan` → proposed-plan worker events.

use devo_core::ItemId;
use devo_protocol::native::item::Item;
use tokio::sync::mpsc;

use crate::events::WorkerEvent;

/// Whether this plan item should drive the proposed-plan streaming UI.
///
/// After wire projection, the legacy `"Proposed Plan"` title is lost; both
/// proposed-plan streams and `update_plan` tool Plan items become
/// `Item::Plan { entries }`. Emitting proposed-plan events for all Plan
/// lifecycle notifications matches the prior typed→legacy shim behavior.
fn is_proposed_plan_item(item: &Item) -> bool {
    matches!(item, Item::Plan { .. })
}

pub(crate) fn handle_started(
    item: &Item,
    item_id: ItemId,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> bool {
    if !is_proposed_plan_item(item) {
        return false;
    }
    let _ = event_tx.send(WorkerEvent::ProposedPlanStarted { item_id });
    true
}

pub(crate) fn handle_completed(
    item: &Item,
    item_id: ItemId,
    event_tx: &mpsc::UnboundedSender<WorkerEvent>,
) -> bool {
    let Item::Plan { entries } = item else {
        return false;
    };
    let final_text = entries
        .iter()
        .map(|entry| entry.step.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = event_tx.send(WorkerEvent::ProposedPlanCompleted {
        item_id,
        final_text,
    });
    true
}
