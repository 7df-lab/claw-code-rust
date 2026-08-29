//! Native `Item::ContextCompaction` → compaction worker events.

use devo_protocol::native::item::Item;
use tokio::sync::mpsc;

use crate::events::WorkerEvent;

pub(crate) fn handle_started(item: &Item, event_tx: &mpsc::UnboundedSender<WorkerEvent>) -> bool {
    if !matches!(item, Item::ContextCompaction { .. }) {
        return false;
    }
    let _ = event_tx.send(WorkerEvent::SessionCompactionStarted);
    true
}

pub(crate) fn handle_completed(item: &Item, event_tx: &mpsc::UnboundedSender<WorkerEvent>) -> bool {
    let Item::ContextCompaction { summary, .. } = item else {
        return false;
    };
    let summary = summary.as_deref().map(str::trim).unwrap_or("");
    let failed = summary.eq_ignore_ascii_case("Compaction failed")
        || summary.starts_with("Compaction failed")
        || summary
            .to_ascii_lowercase()
            .contains("\"status\":\"failed\"");
    if failed {
        let message = summary
            .strip_prefix("Compaction failed:")
            .or_else(|| summary.strip_prefix("Compaction failed"))
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .unwrap_or("Context compaction failed")
            .to_string();
        let _ = event_tx.send(WorkerEvent::SessionCompactionFailed { message });
        return true;
    }
    let title = if summary.is_empty() {
        "Context Compaction".to_string()
    } else {
        summary.to_string()
    };
    let _ = event_tx.send(WorkerEvent::ContextCompactionCompleted { title });
    true
}
