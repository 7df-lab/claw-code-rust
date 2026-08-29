//! Restores durable history into the transcript projector.

use devo_protocol::SessionHistoryItem;

use crate::transcript::TranscriptProjector;

/// Rebuilds a projector from rich session history items (live + restore share this path).
pub(crate) fn restore_projector_from_history(items: &[SessionHistoryItem]) -> TranscriptProjector {
    let mut projector = TranscriptProjector::default();
    let committed = super::restore_session::committed_cells_from_history(items);
    projector.restore_committed(committed);
    projector
}
