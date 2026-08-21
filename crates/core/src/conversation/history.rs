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

use devo_protocol::native::item::ItemEnvelope;
use devo_protocol::native::session::Session;
use devo_protocol::native::turn::Turn;

use super::legacy_projector::{LegacyProjectError, LegacyProjector};
use super::rollout_v2::{
    InternalRecordV2, ParsedRolloutLine, RolloutLineReadError, RolloutLineV2, parse_rollout_line,
};

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
        RolloutLineV2::Internal {
            entry:
                InternalRecordV2::SessionSettings {
                    field,
                    value,
                    epoch,
                    ..
                },
            ..
        } => {
            // Field-level settings lines (L2-DES-CONV-002 DD-4) fold into the
            // canonical session snapshot: the last line per field wins, and
            // the settings epoch raises the session version so canonical
            // readers observe the mutation.
            if let Some(session) = history.session.as_mut() {
                apply_settings_to_canonical_session(session, field, value);
                // `epoch + 1`: the SessionMeta-projected version starts at 1,
                // and the first settings write must already observe a bump.
                session.version = session.version.max(epoch + 1);
            }
        }
        RolloutLineV2::SessionTitleUpdated { title, .. } => {
            // Title changes are session metadata; fold them so canonical
            // readers (including the metadata-update response path) see the
            // latest title.
            if let Some(session) = history.session.as_mut() {
                session.title = Some(title);
            }
        }
        RolloutLineV2::SessionRollback {
            retained_turn_ids, ..
        } => {
            let retained: HashSet<&str> = retained_turn_ids.iter().map(|id| id.as_str()).collect();
            history
                .turns
                .retain(|turn| retained.contains(turn.id.as_str()));
            history
                .items
                .retain(|item| retained.contains(item.turn_id.as_str()));
        }
        // Other internal entries are not items; compaction snapshots shape
        // the prompt, not the displayed history; workspace lines are not part
        // of the conversational timeline.
        RolloutLineV2::Internal { .. }
        | RolloutLineV2::CompactionSnapshot { .. }
        | RolloutLineV2::WorkspaceCheckpoint { .. }
        | RolloutLineV2::WorkspaceChange { .. }
        | RolloutLineV2::WorkspaceRestoreStarted { .. }
        | RolloutLineV2::WorkspaceRestoreCompleted { .. } => {}
    }
}

/// Folds one field-level settings line into the canonical session snapshot.
/// Model-family fields live on `Session::model` / the session record rather
/// than `SessionSettings`; they are intentionally not folded here.
fn apply_settings_to_canonical_session(
    session: &mut devo_protocol::native::session::Session,
    field: crate::conversation::records::SessionSettingsField,
    value: serde_json::Value,
) {
    use crate::conversation::records::SessionSettingsField;
    match field {
        SessionSettingsField::PermissionPreset => {
            // The persisted value uses the legacy `PermissionPreset` wire
            // shape (kebab-case); map it onto the canonical profile enum.
            if let Ok(preset) = serde_json::from_value::<devo_protocol::PermissionPreset>(value) {
                session.settings.permission_profile = match preset {
                    devo_protocol::PermissionPreset::Default => {
                        devo_protocol::native::model::PermissionProfile::Default
                    }
                    devo_protocol::PermissionPreset::AutoReview => {
                        devo_protocol::native::model::PermissionProfile::AutoReview
                    }
                    devo_protocol::PermissionPreset::FullAccess => {
                        devo_protocol::native::model::PermissionProfile::FullAccess
                    }
                };
            }
        }
        SessionSettingsField::SandboxProfile => {
            if let Ok(name) = serde_json::from_value::<String>(value) {
                session.settings.sandbox_profile = Some(name);
            }
        }
        SessionSettingsField::ReasoningEffortSelection => {
            if let Ok(Some(raw)) = serde_json::from_value::<Option<String>>(value)
                && let Ok(effort) = raw.parse::<devo_protocol::ReasoningEffort>()
            {
                session.settings.reasoning_effort = Some(effort);
            }
        }
        SessionSettingsField::CollaborationMode => {
            if let Ok(raw) = serde_json::from_value::<String>(value) {
                session.settings.mode = Some(raw);
            }
        }
        SessionSettingsField::Model => {
            // Model lives on `Session::model`, not `SessionSettings`; fold the
            // slug so canonical readers observe the switch.
            if let Ok(Some(slug)) = serde_json::from_value::<Option<String>>(value) {
                session.model.model = slug;
            }
        }
        SessionSettingsField::ModelBindingId => {}
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
    use devo_protocol::native::item::ItemState;

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
            matches!(&history.items[0].item, devo_protocol::native::item::Item::AssistantMessage { text, .. } if text == "kept")
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

    /// Trace: L2-DES-CONV-002
    /// Verifies: field-level settings lines fold into the canonical session
    /// snapshot (last line per field wins) and bump its version (DD-4).
    #[test]
    fn session_settings_lines_fold_into_canonical_session() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let session_id = SessionId::new();
        let now = Utc.with_ymd_and_hms(2026, 8, 2, 12, 0, 0).unwrap();
        let record = crate::conversation::SessionRecord {
            id: session_id,
            rollout_path: dir.path().join("rollout.jsonl"),
            created_at: now,
            updated_at: now,
            last_activity_at: Some(now),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test".into(),
            model: Some("test-model".into()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            cwd: dir.path().to_path_buf(),
            additional_directories: Vec::new(),
            cli_version: "test".into(),
            title: None,
            title_state: crate::conversation::SessionTitleState::Unset,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id: None,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        };
        write_lines(
            &dir.path().join("rollout.jsonl"),
            &[
                RolloutLine::SessionMeta(Box::new(crate::conversation::SessionMetaLine {
                    timestamp: now,
                    session: record,
                })),
                RolloutLine::SessionSettings(crate::conversation::SessionSettingsLine {
                    timestamp: now,
                    session_id,
                    field: crate::conversation::SessionSettingsField::PermissionPreset,
                    value: serde_json::to_value(devo_protocol::PermissionPreset::FullAccess)
                        .expect("serialize preset"),
                    epoch: 0,
                }),
                RolloutLine::SessionSettings(crate::conversation::SessionSettingsLine {
                    timestamp: now,
                    session_id,
                    field: crate::conversation::SessionSettingsField::SandboxProfile,
                    value: serde_json::Value::String("strict".into()),
                    epoch: 0,
                }),
                RolloutLine::SessionSettings(crate::conversation::SessionSettingsLine {
                    timestamp: now,
                    session_id,
                    field: crate::conversation::SessionSettingsField::ReasoningEffortSelection,
                    value: serde_json::to_value(Some("high".to_string()))
                        .expect("serialize effort"),
                    epoch: 0,
                }),
            ],
        );

        let history = read_canonical_history(&dir.path().join("rollout.jsonl")).expect("read");
        let session = history.session.expect("session");
        assert_eq!(
            session.settings.permission_profile,
            devo_protocol::native::model::PermissionProfile::FullAccess
        );
        assert_eq!(session.settings.sandbox_profile.as_deref(), Some("strict"));
        assert_eq!(
            session.settings.reasoning_effort,
            Some(devo_protocol::ReasoningEffort::High)
        );
        // Three settings epochs (1, 2, 3) raise the SessionMeta version (1)
        // to 4.
        assert_eq!(session.version, 4);
    }
}
