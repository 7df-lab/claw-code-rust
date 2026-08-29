//! Startup reconciliation of the delivery log (08 §7).
//!
//! The rollout JSONL and SQLite cannot share a transaction, so the event log
//! uses outbox/reconciliation semantics: every persisted event row is derived
//! from a v2 rollout fact and idempotent by `(source_fact_id, event_kind,
//! stream_id)`. On startup this reconciler replays each rollout file from its
//! projection watermark and backfills any rows a crash prevented the append
//! path from writing. Crash windows covered:
//!
//! 1. crash before rollout fsync — no fact, no event (consistent);
//! 2. crash after fsync, before event insert — this reconciler backfills;
//! 3. crash after insert, before delivery — the row is replayed to clients
//!    from the log (P4 subscription replay).

use std::path::Path;

use anyhow::Context;
use anyhow::Result;

use devo_core::legacy_projector::LegacyProjector;
use devo_core::parse_rollout_line;
use devo_core::{ParsedRolloutLine, RolloutLineV2};

use crate::db::Database;
use crate::persistence::RolloutStore;

/// Aggregate outcome of one reconciliation pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    pub files_scanned: u64,
    /// Files skipped because a damaged line stopped projection; their
    /// watermarks are left untouched so the next startup retries.
    pub files_damaged: u64,
    pub rows_inserted: u64,
}

/// Replays every rollout file from its projection watermark and backfills
/// missing `event_log` rows. Idempotent: re-running inserts nothing.
pub(crate) fn reconcile_event_log(store: &RolloutStore, db: &Database) -> Result<ReconcileStats> {
    let mut stats = ReconcileStats::default();
    for path in store.rollout_paths()? {
        stats.files_scanned += 1;
        match reconcile_file(&path, db) {
            Ok(outcome) => {
                stats.rows_inserted += outcome.inserted;
                if outcome.damaged {
                    stats.files_damaged += 1;
                    tracing::warn!(
                        rollout = %path.display(),
                        reason = outcome.reason.as_deref().unwrap_or("unknown"),
                        "event_log reconciliation stopped at damaged line; watermark preserved"
                    );
                }
            }
            Err(error) => {
                stats.files_damaged += 1;
                tracing::warn!(
                    rollout = %path.display(),
                    %error,
                    "event_log reconciliation skipped damaged rollout"
                );
            }
        }
    }
    Ok(stats)
}

/// Outcome of reconciling one rollout file: rows actually inserted plus
/// whether a damaged line stopped the scan (watermark stays behind so the
/// next startup retries from there).
struct FileOutcome {
    inserted: u64,
    damaged: bool,
    reason: Option<String>,
}

fn reconcile_file(rollout_path: &Path, db: &Database) -> Result<FileOutcome> {
    let watermark = db.projection_watermark(rollout_path)?;
    let file = std::fs::File::open(rollout_path)
        .with_context(|| format!("open rollout file {}", rollout_path.display()))?;
    let reader = std::io::BufReader::new(file);
    // Legacy rows are projected forward so every log row derives from v2
    // facts only, exactly like the write path.
    let mut projector = LegacyProjector::new();
    let mut inserted = 0u64;
    // Rows are flushed per line and the watermark advances with them, so
    // progress up to a damaged line survives for the next startup.

    let mut lines = std::io::BufRead::lines(reader).enumerate().peekable();
    while let Some((line_index, line)) = lines.next() {
        let line_index = line_index as u64;
        let line = line.with_context(|| format!("read line from {}", rollout_path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if watermark.is_some_and(|watermark| line_index <= watermark) {
            continue;
        }
        let v2_lines: Vec<RolloutLineV2> = match parse_rollout_line(&line) {
            Ok(ParsedRolloutLine::Legacy(legacy)) => projector
                .project_line(&legacy)
                .with_context(|| format!("project legacy line in {}", rollout_path.display()))?,
            Ok(ParsedRolloutLine::V2(v2)) => vec![*v2],
            Err(devo_core::RolloutLineReadError::TruncatedTail) if lines.peek().is_none() => break,
            Err(error) => {
                return Ok(FileOutcome {
                    inserted,
                    damaged: true,
                    reason: Some(format!(
                        "rollout {} is damaged at line {}: {error}",
                        rollout_path.display(),
                        line_index + 1
                    )),
                });
            }
        };
        let mut rows = Vec::new();
        for (sub_index, v2_line) in v2_lines.iter().enumerate() {
            rows.extend(crate::persistence::event_log_rows_for_v2_line(
                rollout_path,
                line_index,
                sub_index as u64,
                v2_line,
            )?);
        }
        inserted += db.insert_event_log_rows(&rows)? as u64;
        db.set_projection_watermark(rollout_path, line_index)?;
    }

    Ok(FileOutcome {
        inserted,
        damaged: false,
        reason: None,
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use devo_core::{ItemId, SessionId, TextItem, TurnId, TurnItem, TurnStatus};
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;
    use crate::persistence::{RolloutStore, build_item_record, build_turn_record};

    /// Writes one session file (meta + turn + item = 3 physical rows) with a
    /// store that has NO event-log sink, simulating facts that were fsynced
    /// while the event projection never ran (crash window 2).
    fn write_session_file(data_root: &std::path::Path) -> (RolloutStore, std::path::PathBuf) {
        let store = RolloutStore::new(data_root.to_path_buf(), None);
        let record = store.create_session_record(
            SessionId::new(),
            Utc::now(),
            data_root.to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        store.append_session_meta(&record).expect("append meta");
        let turn_id = TurnId::new();
        let turn = build_turn_record(
            &crate::turn::TurnMetadata {
                turn_id,
                session_id: record.id,
                sequence: 1,
                status: TurnStatus::Completed,
                kind: devo_core::TurnKind::Regular,
                model: "test-model".into(),
                model_binding_id: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                request_model: "test-model".into(),
                request_thinking: None,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                usage: None,
                stop_reason: None,
                failure_reason: None,
            },
            None,
            None,
            None,
            None,
        );
        store.append_turn(&record, turn).expect("append turn");
        let item = build_item_record(
            record.id,
            turn_id,
            ItemId::new(),
            1,
            TurnItem::AgentMessage(TextItem { text: "hi".into() }),
            Some(TurnStatus::Running),
            None,
            None,
        );
        store.append_item(&record, item).expect("append item");
        (store, record.rollout_path)
    }

    #[test]
    fn reconcile_backfills_rows_and_is_idempotent() {
        let dir = TempDir::new().expect("temp dir");
        let (store, _path) = write_session_file(dir.path());
        let db = Database::open(dir.path().join("devo.db")).expect("open db");

        let stats = reconcile_event_log(&store, &db).expect("first reconcile");
        assert_eq!(stats.files_scanned, 1);
        assert_eq!(stats.files_damaged, 0);
        assert_eq!(stats.rows_inserted, 4);
        assert_eq!(db.event_log_len().expect("count"), 4);

        // Re-running is a no-op (watermark + primary-key idempotency).
        let stats = reconcile_event_log(&store, &db).expect("second reconcile");
        assert_eq!(stats.rows_inserted, 0);
        assert_eq!(db.event_log_len().expect("count"), 4);
    }

    #[test]
    fn reconcile_respects_watermark() {
        let dir = TempDir::new().expect("temp dir");
        let (store, path) = write_session_file(dir.path());
        let db = Database::open(dir.path().join("devo.db")).expect("open db");

        // Crash after projecting only the first row (session meta at index 0).
        db.set_projection_watermark(&path, 0)
            .expect("set watermark");
        let stats = reconcile_event_log(&store, &db).expect("reconcile");
        // Lines 1 (turn) and 2 (item) backfill; session/created does not.
        assert_eq!(stats.rows_inserted, 2);
        assert_eq!(db.event_log_len().expect("count"), 2);
        assert_eq!(db.projection_watermark(&path).expect("watermark"), Some(2));
    }

    #[test]
    fn reconcile_skips_damaged_file_and_keeps_watermark() {
        let dir = TempDir::new().expect("temp dir");
        let (store, path) = write_session_file(dir.path());
        let db = Database::open(dir.path().join("devo.db")).expect("open db");
        // Corrupt the tail of the file beyond the intact facts.
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open");
            file.write_all(b"{not json\n").expect("write garbage");
        }
        let stats = reconcile_event_log(&store, &db).expect("reconcile tolerates");
        assert_eq!(stats.files_damaged, 1);
        // All intact rows before the damage were still backfilled.
        assert_eq!(stats.rows_inserted, 4);
        assert_eq!(db.projection_watermark(&path).expect("watermark"), Some(2));
    }

    #[test]
    fn stream_seq_is_monotonic_per_stream() {
        let dir = TempDir::new().expect("temp dir");
        let (store, _path) = write_session_file(dir.path());
        let db = Database::open(dir.path().join("devo.db")).expect("open db");
        reconcile_event_log(&store, &db).expect("reconcile");

        // The per-cwd sessions stream holds exactly one session/created row.
        let sessions_stream = devo_core::sessions_stream_id(&dir.path().to_string_lossy());
        let sessions_rows = db
            .event_log_rows(&sessions_stream, 0)
            .expect("sessions stream");
        assert_eq!(sessions_rows.len(), 1);
        assert_eq!(sessions_rows[0].seq, 1);
        assert_eq!(sessions_rows[0].event_kind, "session/created");
    }
}
