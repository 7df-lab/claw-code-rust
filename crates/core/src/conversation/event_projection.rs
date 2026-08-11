//! Derivation of persisted delivery-log events from v2 rollout facts.
//!
//! Truth source: `devo-api-design/08-events-subscription.md` §5/§7. The
//! rollout JSONL is the canonical recovery log; every persisted event in the
//! SQLite `event_log` is derived from a v2 line by this pure mapping, so
//! crash recovery only ever *re-derives* the same rows (idempotent by source
//! fact) — a crash may delay an event, never lose or duplicate it.
//!
//! Only v2 lines produce events: legacy lines are first projected forward by
//! `LegacyProjector` during hydration/reconciliation, so all log rows come
//! from v2 facts.

use sha2::Digest;
use sha2::Sha256;

use devo_protocol::canonical::event::ServerNotification;
use devo_protocol::canonical::ids::RestorePlanId;
use devo_protocol::canonical::ids::SessionId;
use devo_protocol::canonical::item::ItemState;
use devo_protocol::canonical::turn::TurnStatus;

use super::rollout_v2::RolloutLineV2;

/// One derived persisted event (pre-sequencing). `event_kind` is the
/// notification method string (`item/started`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedEvent {
    pub event_kind: &'static str,
    pub stream_id: String,
    pub notification: ServerNotification,
}

/// The schema version stamped into `EventMeta.schema_version` for events
/// derived by this module.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// Stable identity of a rollout fact: `<rollout_path>#<line_index>[.<sub>]`.
/// The line index counts physical JSONL rows; `sub_index` distinguishes
/// multiple v2 facts projected from one legacy row (packed item expansion).
/// The event log is idempotent by this key (paired with event kind and
/// stream), so re-deriving the same fact after a crash is always a no-op.
pub fn source_fact_id(rollout_path: &std::path::Path, line_index: u64, sub_index: u64) -> String {
    if sub_index == 0 {
        format!("{}#{line_index}", rollout_path.to_string_lossy())
    } else {
        format!(
            "{}#{line_index}.{sub_index}",
            rollout_path.to_string_lossy()
        )
    }
}

/// Stream id of a session stream (`session:<session-id>`).
pub fn session_stream_id(session_id: &SessionId) -> String {
    format!("session:{session_id}")
}

/// Stream id of the per-cwd session-list stream (`sessions:<cwd-hash>`). The
/// hash is the first 16 hex chars of SHA-256 over the normalized cwd string;
/// P4's subscription selector must use this exact function.
pub fn sessions_stream_id(cwd: &str) -> String {
    let digest = Sha256::digest(cwd.as_bytes());
    format!("sessions:{}", hex_prefix(&digest, 8))
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    bytes[..len]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_terminal_item_state(state: ItemState) -> bool {
    match state {
        ItemState::Completed | ItemState::Failed | ItemState::Interrupted | ItemState::Lost => true,
        ItemState::Running | ItemState::Waiting => false,
    }
}

/// Derives the persisted events carried by one v2 rollout line. Line kinds
/// without a faithful canonical notification are skipped (each with its
/// reason on the match arm); they remain recoverable from the rollout itself.
pub fn events_from_v2_line(line: &RolloutLineV2) -> Vec<DerivedEvent> {
    match line {
        RolloutLineV2::Item { item, .. } => {
            let stream_id = session_stream_id(&item.session_id);
            let envelope = Box::new(item.clone());
            let (event_kind, notification) = if is_terminal_item_state(item.state) {
                // All terminal states share `item/completed` (08 §3).
                (
                    "item/completed",
                    ServerNotification::ItemCompleted { item: envelope },
                )
            } else if item.revision > 1 {
                (
                    "item/updated",
                    ServerNotification::ItemUpdated { item: envelope },
                )
            } else {
                (
                    "item/started",
                    ServerNotification::ItemStarted { item: envelope },
                )
            };
            vec![DerivedEvent {
                event_kind,
                stream_id,
                notification,
            }]
        }
        RolloutLineV2::Turn { turn, .. } => {
            let stream_id = session_stream_id(&turn.session_id);
            let (event_kind, notification) = match turn.status {
                TurnStatus::InProgress => (
                    "turn/started",
                    ServerNotification::TurnStarted {
                        turn: Box::new(turn.clone()),
                    },
                ),
                TurnStatus::Completed | TurnStatus::Interrupted | TurnStatus::Failed => (
                    "turn/completed",
                    ServerNotification::TurnCompleted {
                        turn: Box::new(turn.clone()),
                    },
                ),
            };
            vec![DerivedEvent {
                event_kind,
                stream_id,
                notification,
            }]
        }
        RolloutLineV2::SessionMeta { session, .. } => {
            // One fact, two streams: the session stream and the per-cwd
            // session-list stream. The log PK includes stream_id, so both
            // rows are idempotent independently.
            let session = session.as_ref().clone();
            let mut events = Vec::with_capacity(2);
            for stream_id in [
                session_stream_id(&session.id),
                sessions_stream_id(&session.cwd.to_string_lossy()),
            ] {
                events.push(DerivedEvent {
                    event_kind: "session/created",
                    stream_id,
                    notification: ServerNotification::SessionCreated {
                        session: Box::new(session.clone()),
                    },
                });
            }
            events
        }
        RolloutLineV2::WorkspaceRestoreStarted { record, .. } => {
            vec![DerivedEvent {
                event_kind: "workspace/restoreStarted",
                stream_id: session_stream_id(&SessionId::from_string(
                    record.session_id.to_string(),
                )),
                notification: ServerNotification::WorkspaceRestoreStarted {
                    session_id: SessionId::from_string(record.session_id.to_string()),
                    restore_plan_id: RestorePlanId::from_string(record.restore_id.0.to_string()),
                },
            }]
        }
        RolloutLineV2::WorkspaceRestoreCompleted { record, .. } => {
            let succeeded = record.outcomes.iter().all(|outcome| {
                matches!(
                    outcome.status,
                    crate::durable_record::RestoreFileStatus::Restored
                        | crate::durable_record::RestoreFileStatus::Skipped
                )
            });
            vec![DerivedEvent {
                event_kind: "workspace/restoreCompleted",
                stream_id: session_stream_id(&SessionId::from_string(
                    record.session_id.to_string(),
                )),
                notification: ServerNotification::WorkspaceRestoreCompleted {
                    session_id: SessionId::from_string(record.session_id.to_string()),
                    restore_plan_id: RestorePlanId::from_string(record.restore_id.0.to_string()),
                    succeeded,
                    error: None,
                },
            }]
        }
        // No faithful canonical notification exists for these kinds; they are
        // replay concerns, not delivery-log events:
        // - SessionTitleUpdated: `session/metadataUpdated` needs a full
        //   Session snapshot, unavailable from the line alone; the live path
        //   emits it.
        // - CompactionSnapshot: the compaction item itself yields item events.
        // - SessionRollback: P4's rollback preview/commit flow emits live
        //   events; the marker is replay state.
        // - Internal / WorkspaceCheckpoint / WorkspaceChange: rollout-only.
        RolloutLineV2::SessionTitleUpdated { .. }
        | RolloutLineV2::CompactionSnapshot { .. }
        | RolloutLineV2::SessionRollback { .. }
        | RolloutLineV2::Internal { .. }
        | RolloutLineV2::WorkspaceCheckpoint { .. }
        | RolloutLineV2::WorkspaceChange { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use devo_protocol::canonical::ids::ItemId;
    use devo_protocol::canonical::ids::TurnId;
    use devo_protocol::canonical::item::Item;
    use devo_protocol::canonical::item::ItemEnvelope;
    use devo_protocol::canonical::item::ItemState;
    use devo_protocol::canonical::item::UserInput;
    use devo_protocol::canonical::item::UserMessageEntry;
    use pretty_assertions::assert_eq;

    use super::*;

    fn item_envelope(state: ItemState, revision: u32) -> ItemEnvelope {
        ItemEnvelope {
            id: ItemId::new(),
            session_id: SessionId::new(),
            turn_id: TurnId::new(),
            seq: 1,
            revision,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            state,
            item: Item::UserMessage {
                client_user_message_id: None,
                content: vec![UserInput::Text {
                    text: "hi".to_owned(),
                }],
                entry: UserMessageEntry::TurnStart,
            },
        }
    }

    #[test]
    fn item_line_maps_lifecycle_to_event_kinds() {
        for (state, revision, expected) in [
            (ItemState::Running, 1, "item/started"),
            (ItemState::Waiting, 1, "item/started"),
            (ItemState::Running, 2, "item/updated"),
            (ItemState::Completed, 1, "item/completed"),
            (ItemState::Completed, 3, "item/completed"),
            (ItemState::Interrupted, 1, "item/completed"),
            (ItemState::Lost, 2, "item/completed"),
        ] {
            let line = RolloutLineV2::Item {
                v: 2,
                timestamp: Utc::now(),
                item: item_envelope(state, revision),
            };
            let events = events_from_v2_line(&line);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_kind, expected, "{state:?} rev {revision}");
        }
    }

    #[test]
    fn sessions_stream_id_is_stable() {
        assert_eq!(
            sessions_stream_id("/Users/dev/project"),
            sessions_stream_id("/Users/dev/project")
        );
        assert_ne!(
            sessions_stream_id("/Users/dev/project"),
            sessions_stream_id("/Users/dev/other")
        );
        assert!(sessions_stream_id("/x").starts_with("sessions:"));
    }
}
