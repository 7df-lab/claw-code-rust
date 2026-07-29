//! Canonical history reader: loads a session's effective history from its
//! rollout file in canonical form, regardless of the on-disk line format.
//!
//! Used by the paged history read API (`session/turns/list`,
//! `session/items/list`). The in-memory runtime model deliberately does not
//! retain turn records or item envelopes, so the rollout — dual-read and
//! forward-projected — is the only complete source. A read re-parses the
//! whole file; history reads are infrequent enough that this beats keeping
//! a second in-memory copy in sync (a cache can be added later behind the
//! same function).

use std::collections::HashSet;
use std::path::Path;

use devo_protocol::canonical::item::ItemEnvelope;
use devo_protocol::canonical::session::Session;
use devo_protocol::canonical::turn::Turn;

use super::legacy_projector::{LegacyProjectError, LegacyProjector};
use super::rollout_v2::{ParsedRolloutLine, RolloutLineReadError, RolloutLineV2, parse_rollout_line};

/// A session's effective canonical history, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CanonicalHistory {
    /// The session metadata line, when the file has one (files always do for
    /// durable sessions; `None` only for truncated reads).
    pub session: Option<Box<Session>>,
    /// Turn records in ascending `sequence` order.
    pub turns: Vec<Turn>,
    /// Item envelopes in ascending `seq` order, approval folds applied.
    pub items: Vec<ItemEnvelope>,
}

/// Errors from reading a rollout file as canonical history.
#[derive(Debug, thiserror::Error)]
pub enum HistoryReadError {
    /// The file could not be read.
    #[error("read rollout history: {0}")]
    Io(#[from] std::io::Error),
    /// A line failed the version dispatch. History reads are fail-closed,
    /// like resume: a damaged file errors rather than silently truncating
    /// the returned history.
    #[error("rollout history line {line_index} is unreadable: {error}")]
    DamagedLine {
        line_index: usize,
        error: RolloutLineReadError,
    },
    /// A legacy line failed to project forward.
    #[error("project legacy line: {0}")]
    Projection(#[from] LegacyProjectError),
}

/// Reads one rollout file into canonical history form. Legacy (v1) lines
/// are projected through a file-scoped [`LegacyProjector`] (so packed
/// records expand and approvals fold); v2 lines are used directly. A
/// truncated final line is tolerated as a crash tail, matching resume.
///
/// Rollback markers are honored at turn granularity: the last
/// `SessionRollback` line drops already-read turns (and their items) that
/// are not in its retained set. Item-level retention ids are not matched
/// because packed-record sibling ids cannot be recovered after projection;
/// rollback truncates at turn boundaries in practice, so turn granularity
/// is exact for the real use case.
pub fn read_canonical_history(path: &Path) -> Result<CanonicalHistory, HistoryReadError> {
    let text = std::fs::read_to_string(path)?;
    let mut projector = LegacyProjector::new();
    let mut history = CanonicalHistory::default();
    let lines: Vec<&str> = text.lines().collect();
    for (index, raw) in lines.iter().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let parsed = match parse_rollout_line(raw) {
            Ok(parsed) => parsed,
            // A truncated final line is a crash tail: the write never
            // completed, nothing was acknowledged.
            Err(RolloutLineReadError::TruncatedTail) if index + 1 == lines.len() => break,
            Err(error) => {
                return Err(HistoryReadError::DamagedLine {
                    line_index: index,
                    error,
                });
            }
        };
        let v2_lines = match parsed {
            ParsedRolloutLine::Legacy(line) => projector.project_line(&line)?,
            ParsedRolloutLine::V2(line) => vec![*line],
        };
        for line in v2_lines {
            apply_v2_line(&mut history, line);
        }
    }
    Ok(history)
}

fn apply_v2_line(history: &mut CanonicalHistory, line: RolloutLineV2) {
    match line {
        RolloutLineV2::SessionMeta { session, .. } => history.session = Some(session),
        RolloutLineV2::Turn { turn, .. } => history.turns.push(turn),
        RolloutLineV2::Item { item, .. } => history.items.push(item),
        RolloutLineV2::SessionRollback {
            retained_turn_ids, ..
        } => {
            let retained: HashSet<&str> = retained_turn_ids
                .iter()
                .map(|id| id.as_str())
                .collect();
            history.turns.retain(|turn| retained.contains(turn.id.as_str()));
            history
                .items
                .retain(|item| retained.contains(item.turn_id.as_str()));
        }
        // Internal entries are not items; title updates are folded into the
        // session snapshot by callers that need them; compaction snapshots
        // shape the prompt, not the displayed history; workspace lines are
        // not part of the conversational timeline.
        RolloutLineV2::Internal { .. }
        | RolloutLineV2::SessionTitleUpdated { .. }
        | RolloutLineV2::CompactionSnapshot { .. }
        | RolloutLineV2::WorkspaceCheckpoint { .. }
        | RolloutLineV2::WorkspaceChange { .. }
        | RolloutLineV2::WorkspaceRestoreStarted { .. }
        | RolloutLineV2::WorkspaceRestoreCompleted { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::conversation::records::{
        ItemLine, ItemRecord, RolloutLine, SessionRollbackLine, TextItem, TurnItem,
    };
    use crate::conversation::{ItemId, SessionId, TurnId, TurnStatus};
    use devo_protocol::canonical::item::ItemState;

    fn write_lines(path: &Path, lines: &[RolloutLine]) {
        let mut text = String::new();
        for line in lines {
            text.push_str(&serde_json::to_string(line).expect("serialize"));
            text.push('\n');
        }
        std::fs::write(path, text).expect("write fixture");
    }

    fn item_record(seq: u64, session_id: SessionId, turn_id: TurnId, text: &str) -> ItemRecord {
        ItemRecord {
            id: ItemId::new(),
            session_id,
            turn_id,
            seq,
            timestamp: Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap(),
            attempt_placement: None,
            turn_status: Some(TurnStatus::Running),
            sibling_turn_ids: Vec::new(),
            input_items: Vec::new(),
            output_items: vec![TurnItem::AgentMessage(TextItem { text: text.into() })],
            worklog: None,
            error: None,
            schema_version: 1,
        }
    }

    #[test]
    fn rollback_truncates_turns_and_their_items() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let kept_turn = TurnId::new();
        let dropped_turn = TurnId::new();
        let kept_item = item_record(1, session_id, kept_turn, "kept");
        let dropped_item = item_record(2, session_id, dropped_turn, "dropped");
        write_lines(
            &dir.path().join("rollout.jsonl"),
            &[
                RolloutLine::Item(ItemLine {
                    timestamp: kept_item.timestamp,
                    item: kept_item,
                }),
                RolloutLine::Item(ItemLine {
                    timestamp: dropped_item.timestamp,
                    item: dropped_item,
                }),
                RolloutLine::SessionRollback(Box::new(SessionRollbackLine {
                    timestamp: Utc.with_ymd_and_hms(2026, 7, 1, 12, 1, 0).unwrap(),
                    session_id,
                    retained_turn_ids: vec![kept_turn],
                    retained_item_ids: Vec::new(),
                    latest_turn_id: Some(kept_turn),
                    schema_version: 1,
                })),
            ],
        );

        let history = read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read");
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].state, ItemState::Completed);
        assert!(
            matches!(&history.items[0].item, devo_protocol::canonical::item::Item::AssistantMessage { text, .. } if text == "kept")
        );
    }

    #[test]
    fn truncated_final_line_is_tolerated() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item = item_record(1, session_id, turn_id, "ok");
        let mut text = String::new();
        text.push_str(
            &serde_json::to_string(&RolloutLine::Item(ItemLine {
                timestamp: item.timestamp,
                item,
            }))
            .expect("serialize"),
        );
        text.push('\n');
        text.push_str(r#"{"v":2,"kind":"item","timestamp":"2026"#);
        std::fs::write(dir.path().join("rollout.jsonl"), text).expect("write fixture");

        let history = read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read");
        assert_eq!(history.items.len(), 1);
    }

    #[test]
    fn damaged_middle_line_fails_closed() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item = item_record(1, session_id, turn_id, "ok");
        let mut text = String::new();
        text.push_str(
            &serde_json::to_string(&RolloutLine::Item(ItemLine {
                timestamp: item.timestamp,
                item,
            }))
            .expect("serialize"),
        );
        text.push('\n');
        text.push_str("{\"v\":2,\"kind\":\"nope\"}\n");
        std::fs::write(dir.path().join("rollout.jsonl"), text).expect("write fixture");

        let error = read_canonical_history(&dir.path().join("rollout.jsonl"))
            .expect_err("damaged line must fail");
        assert!(matches!(error, HistoryReadError::DamagedLine { .. }));
    }
}
