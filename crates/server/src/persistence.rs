use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Context;
use anyhow::Result;
use chrono::Datelike;
use chrono::SecondsFormat;
use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

use devo_core::CommandExecutionItem;
use devo_core::CompactionSnapshotLine;
use devo_core::ContentBlock;
use devo_core::ItemId;
use devo_core::ItemLine;
use devo_core::ItemRecord;
use devo_core::Message;
use devo_core::MessageEditRecordedLine;
use devo_core::MessageEditRecordedRecord;
use devo_core::ParsedRolloutLine;
use devo_core::Role;
use devo_core::RolloutLine;
use devo_core::RolloutLineReadError;
use devo_core::SessionContext;
use devo_core::SessionContextUpdatedLine;
use devo_core::SessionId;
use devo_core::SessionMetaLine;
use devo_core::SessionRecord;
use devo_core::SessionRollbackLine;
use devo_core::SessionSettingsField;
use devo_core::SessionSettingsLine;
use devo_core::SessionTitleFinalSource;
use devo_core::SessionTitleState;
use devo_core::SessionTitleUpdatedLine;
use devo_core::TextItem;
use devo_core::ToolCallItem;
use devo_core::ToolResultItem;
use devo_core::TurnId;
use devo_core::TurnItem;
use devo_core::TurnKind;
use devo_core::TurnLine;
use devo_core::TurnRecord;
use devo_core::TurnStatus;
use devo_core::TurnSupersededLine;
use devo_core::TurnSupersededRecord;
use devo_core::TurnWorkspaceChangeRecordedLine;
use devo_core::TurnWorkspaceChangeRecordedRecord;
use devo_core::TurnWorkspaceCheckpointRecordedLine;
use devo_core::TurnWorkspaceCheckpointRecordedRecord;
use devo_core::TurnWorkspaceRestoreCompletedLine;
use devo_core::TurnWorkspaceRestoreCompletedRecord;
use devo_core::TurnWorkspaceRestoreStartedLine;
use devo_core::TurnWorkspaceRestoreStartedRecord;
use devo_core::V2InverseProjector;
use devo_core::Worklog;
use devo_core::legacy_projector::LegacyProjector;
use devo_core::parse_rollout_line;
use devo_core::rollout_v2::RolloutLineV2;
use devo_core::{EVENT_SCHEMA_VERSION, events_from_v2_line, source_fact_id};
use devo_protocol::native::event::{EventEnvelope, EventMeta};
use devo_protocol::native::ids::EventId;

use crate::db::{Database, NewEventLogRow};
use crate::execution::PersistedTurnItem;
use crate::execution::RuntimeSession;
use crate::execution::ServerRuntimeDependencies;
use crate::projection::history_item_from_turn_item;
use crate::session::SessionMetadata;
use crate::session::SessionRuntimeStatus;
use crate::turn::TurnMetadata;

/// Owns canonical append-only rollout persistence rooted at the server data directory.
pub(crate) struct RolloutStore {
    /// Root data directory that contains the `sessions/` hierarchy.
    data_root: PathBuf,
    /// Per-file locks that serialise concurrent writes to the same rollout file,
    /// preventing interleaved JSON lines.
    file_locks: Arc<StdMutex<HashMap<PathBuf, Arc<StdMutex<()>>>>>,
    /// Per-file write-path state (v2 single-write, 05 §2.2). One instance per
    /// rollout path, hydrated from the on-disk history on first append so
    /// item seqs and approval folds never collide with it.
    write_states: Arc<StdMutex<HashMap<PathBuf, WritePathState>>>,
    /// Delivery-log sink (08 §5/§7): after each fsynced append, derived
    /// events are projected into the SQLite `event_log` (best effort; the
    /// startup reconciler backfills anything missed). `None` in tests that
    /// do not exercise the event log.
    event_log: Option<Arc<Database>>,
}

/// Per-file write-path state: the forward projector plus the index of the
/// next JSONL row to be written (used as the `source_fact_id` line index).
pub(crate) struct WritePathState {
    projector: LegacyProjector,
    next_line_index: u64,
}

impl std::fmt::Debug for RolloutStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RolloutStore")
            .field("data_root", &self.data_root)
            .finish()
    }
}

impl Clone for RolloutStore {
    fn clone(&self) -> Self {
        Self {
            data_root: self.data_root.clone(),
            file_locks: Arc::clone(&self.file_locks),
            write_states: Arc::clone(&self.write_states),
            event_log: self.event_log.as_ref().map(Arc::clone),
        }
    }
}

impl RolloutStore {
    /// Creates a rollout store rooted at the supplied server home directory.
    pub(crate) fn new(data_root: PathBuf, event_log: Option<Arc<Database>>) -> Self {
        Self {
            data_root,
            file_locks: Arc::new(StdMutex::new(HashMap::new())),
            write_states: Arc::new(StdMutex::new(HashMap::new())),
            event_log,
        }
    }

    /// Constructs a canonical durable session record for a newly created session.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_session_record(
        &self,
        id: SessionId,
        created_at: chrono::DateTime<Utc>,
        cwd: PathBuf,
        additional_directories: Vec<PathBuf>,
        title: Option<String>,
        model: Option<String>,
        model_binding_id: Option<String>,
        reasoning_effort_selection: Option<String>,
        model_provider: String,
        parent_session_id: Option<SessionId>,
    ) -> SessionRecord {
        let rollout_path = self.rollout_path(created_at, id);
        let title_state = title
            .as_ref()
            .map(|_| SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate))
            .unwrap_or(SessionTitleState::Unset);
        SessionRecord {
            id,
            rollout_path,
            created_at,
            updated_at: created_at,
            last_activity_at: Some(created_at),
            source: "cli".into(),
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider,
            model,
            model_binding_id,
            reasoning_effort_selection,
            cwd,
            additional_directories,
            cli_version: env!("CARGO_PKG_VERSION").into(),
            title,
            title_state,
            sandbox_policy: "workspace-write".into(),
            approval_mode: "on-request".into(),
            effective_context_window: None,
            tokens_used: 0,
            first_user_message: None,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            parent_session_id,
            session_context: None,
            latest_turn_context: None,
            collaboration_mode: None,
            permission_preset: None,
            schema_version: 2,
        }
    }

    /// Appends the mandatory session header line to a durable rollout file.
    pub(crate) fn append_session_meta(&self, record: &SessionRecord) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: Utc::now(),
                session: record.clone(),
            })),
        )
    }

    /// Appends one field-level session settings line without requiring the
    /// actor-owned session record (L2-DES-CONV-002 Phase 2 persist-first
    /// path: the handler must not wait on the actor mailbox to persist).
    pub(crate) fn append_session_settings_at(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        field: SessionSettingsField,
        value: serde_json::Value,
    ) -> Result<()> {
        self.append_line(
            rollout_path,
            &RolloutLine::SessionSettings(SessionSettingsLine {
                timestamp: Utc::now(),
                session_id,
                field,
                value,
                // Placeholder: the per-file projector assigns the
                // authoritative epoch at write time.
                epoch: 0,
            }),
        )
    }

    /// Appends one turn line to the durable rollout journal.
    pub(crate) fn append_turn(&self, record: &SessionRecord, turn: TurnRecord) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::Turn(Box::new(TurnLine {
                timestamp: Utc::now(),
                turn,
            })),
        )
    }

    /// Appends one item line to the durable rollout journal.
    pub(crate) fn append_item(&self, record: &SessionRecord, item: ItemRecord) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::Item(ItemLine {
                timestamp: Utc::now(),
                item,
            }),
        )
    }

    /// Appends one session-title update line to the durable rollout journal.
    pub(crate) fn append_title_update(
        &self,
        record: &SessionRecord,
        title: String,
        title_state: SessionTitleState,
        previous_title: Option<String>,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::SessionTitleUpdated(SessionTitleUpdatedLine {
                timestamp: Utc::now(),
                session_id: record.id,
                title,
                title_state,
                previous_title,
            }),
        )
    }

    /// Appends the locked session context once to the durable rollout journal.
    pub(crate) fn append_session_context_updated(
        &self,
        record: &SessionRecord,
        session_context: SessionContext,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::SessionContextUpdated(Box::new(SessionContextUpdatedLine {
                timestamp: Utc::now(),
                session_id: record.id,
                session_context,
                schema_version: 1,
            })),
        )
    }

    /// Appends one turn line, recording session context separately when needed.
    pub(crate) fn append_turn_deduped(
        &self,
        record: &SessionRecord,
        session_context_recorded: &mut bool,
        turn: TurnRecord,
        session_context: Option<SessionContext>,
    ) -> Result<()> {
        if let Some(session_context) = session_context
            && !*session_context_recorded
        {
            self.append_session_context_updated(record, session_context)?;
            *session_context_recorded = true;
        }
        self.append_turn(record, turn)
    }

    /// Appends one compaction snapshot line to the durable rollout journal.
    pub(crate) fn append_compaction_snapshot(
        &self,
        record: &SessionRecord,
        snapshot: CompactionSnapshotLine,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::CompactionSnapshot(Box::new(snapshot)),
        )
    }

    /// Appends one accepted message-edit record to the durable rollout journal.
    #[allow(dead_code)]
    pub(crate) fn append_message_edit_recorded(
        &self,
        record: &SessionRecord,
        edit: MessageEditRecordedRecord,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::MessageEditRecorded(Box::new(MessageEditRecordedLine {
                timestamp: Utc::now(),
                record: edit,
            })),
        )
    }

    /// Appends one turn-superseded marker to the durable rollout journal.
    #[allow(dead_code)]
    pub(crate) fn append_turn_superseded(
        &self,
        record: &SessionRecord,
        superseded: TurnSupersededRecord,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::TurnSuperseded(Box::new(TurnSupersededLine {
                timestamp: Utc::now(),
                record: superseded,
            })),
        )
    }

    /// Appends one workspace-restore-start record to the durable rollout journal.
    #[allow(dead_code)]
    pub(crate) fn append_workspace_restore_started(
        &self,
        record: &SessionRecord,
        restore: TurnWorkspaceRestoreStartedRecord,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::TurnWorkspaceRestoreStarted(Box::new(TurnWorkspaceRestoreStartedLine {
                timestamp: Utc::now(),
                record: restore,
            })),
        )
    }

    /// Appends one workspace-checkpoint record to the durable rollout journal.
    pub(crate) fn append_workspace_checkpoint_recorded(
        &self,
        record: &SessionRecord,
        checkpoint: TurnWorkspaceCheckpointRecordedRecord,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::TurnWorkspaceCheckpointRecorded(Box::new(
                TurnWorkspaceCheckpointRecordedLine {
                    timestamp: Utc::now(),
                    record: checkpoint,
                },
            )),
        )
    }

    /// Appends one workspace-change record to the durable rollout journal.
    pub(crate) fn append_workspace_change_recorded(
        &self,
        record: &SessionRecord,
        change: TurnWorkspaceChangeRecordedRecord,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::TurnWorkspaceChangeRecorded(Box::new(TurnWorkspaceChangeRecordedLine {
                timestamp: Utc::now(),
                record: change,
            })),
        )
    }

    /// Appends one workspace-restore-completed record to the durable rollout journal.
    #[allow(dead_code)]
    pub(crate) fn append_workspace_restore_completed(
        &self,
        record: &SessionRecord,
        restore: TurnWorkspaceRestoreCompletedRecord,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::TurnWorkspaceRestoreCompleted(Box::new(
                TurnWorkspaceRestoreCompletedLine {
                    timestamp: Utc::now(),
                    record: restore,
                },
            )),
        )
    }

    /// Appends one rollback marker to the durable rollout journal.
    pub(crate) fn append_session_rollback(
        &self,
        record: &SessionRecord,
        retained_turn_ids: Vec<TurnId>,
        retained_item_ids: Vec<ItemId>,
        latest_turn_id: Option<TurnId>,
    ) -> Result<()> {
        self.append_line(
            &record.rollout_path,
            &RolloutLine::SessionRollback(Box::new(SessionRollbackLine {
                timestamp: Utc::now(),
                session_id: record.id,
                retained_turn_ids,
                retained_item_ids,
                latest_turn_id,
                schema_version: 1,
            })),
        )
    }

    /// Loads every durable session that can be rebuilt from canonical rollout files.
    pub(crate) async fn load_sessions(
        &self,
        deps: &ServerRuntimeDependencies,
    ) -> Result<HashMap<SessionId, RuntimeSession>> {
        let mut sessions = HashMap::new();
        for rollout_path in self.rollout_paths()? {
            match self.load_session_from_rollout(&rollout_path, deps).await {
                Ok(recovered) => {
                    sessions.insert(recovered.summary.session_id, recovered);
                }
                Err(error) => {
                    tracing::warn!(
                        rollout_path = %rollout_path.display(),
                        error = %error,
                        "failed to replay rollout; skipping persisted session"
                    );
                }
            }
        }
        Ok(sessions)
    }

    /// Indexes rollout SessionMeta headers into SQLite without replaying turns.
    pub(crate) fn index_rollout_metadata(&self, db: &crate::db::Database) -> Result<()> {
        let mut canonical =
            HashMap::<SessionId, (chrono::DateTime<Utc>, PathBuf, SessionRecord)>::new();
        for rollout_path in self.rollout_paths()? {
            match read_rollout_index_fields(&rollout_path) {
                Ok((record, last_activity_at)) => match canonical.get(&record.id) {
                    Some((existing_activity, existing_path, _)) => {
                        if last_activity_at > *existing_activity {
                            tracing::warn!(
                                session_id = %record.id,
                                kept_rollout_path = %rollout_path.display(),
                                replaced_rollout_path = %existing_path.display(),
                                "duplicate rollout for session id; keeping newest last_activity_at"
                            );
                            canonical.insert(record.id, (last_activity_at, rollout_path, record));
                        } else {
                            tracing::warn!(
                                session_id = %record.id,
                                kept_rollout_path = %existing_path.display(),
                                ignored_rollout_path = %rollout_path.display(),
                                "duplicate rollout for session id; keeping newest last_activity_at"
                            );
                        }
                    }
                    None => {
                        canonical.insert(record.id, (last_activity_at, rollout_path, record));
                    }
                },
                Err(error) => {
                    tracing::warn!(
                        rollout_path = %rollout_path.display(),
                        error = %error,
                        "failed to index rollout metadata; skipping file"
                    );
                }
            }
        }

        for (session_id, (last_activity_at, rollout_path, record)) in canonical {
            let metadata = session_metadata_from_record(&record, last_activity_at);
            if let Err(error) =
                db.upsert_rollout_index_session(&metadata, Some(rollout_path.as_path()))
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to upsert indexed session metadata"
                );
            }
        }
        Ok(())
    }

    /// Finds a rollout file by session id suffix when the SQLite index is stale.
    pub(crate) fn find_rollout_by_session_id(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PathBuf>> {
        let suffix = format!("-{session_id}.jsonl");
        for rollout_path in self.rollout_paths()? {
            let Some(file_name) = rollout_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if file_name.ends_with(&suffix) {
                return Ok(Some(rollout_path));
            }
        }
        Ok(None)
    }

    /// Resolves a session cwd from the SQLite index, falling back to rollout SessionMeta.
    pub(crate) fn resolve_indexed_session_cwd(
        &self,
        db: &crate::db::Database,
        session_id: &SessionId,
    ) -> Result<Option<PathBuf>> {
        if let Some(index) = db.get_session_index(session_id)? {
            return Ok(Some(index.metadata.cwd));
        }
        if let Some(rollout_path) = self.find_rollout_by_session_id(session_id)? {
            let (record, _) = read_rollout_index_fields(&rollout_path)?;
            return Ok(Some(record.cwd));
        }
        Ok(None)
    }

    /// Deletes canonical rollout files for a session.
    pub(crate) fn delete_session_rollouts(&self, session_id: &SessionId) -> Result<bool> {
        let suffix = format!("-{session_id}.jsonl");
        let mut deleted = false;
        for rollout_path in self.rollout_paths()? {
            let Some(file_name) = rollout_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !file_name.ends_with(&suffix) {
                continue;
            }
            let file_lock = {
                let mut locks = self
                    .file_locks
                    .lock()
                    .expect("rollout file-locks table poisoned");
                locks
                    .entry(rollout_path.clone())
                    .or_insert_with(|| Arc::new(StdMutex::new(())))
                    .clone()
            };
            let _guard = file_lock.lock().expect("rollout per-file lock poisoned");
            match std::fs::remove_file(&rollout_path) {
                Ok(()) => deleted = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("delete rollout {}", rollout_path.display()));
                }
            }
        }
        Ok(deleted)
    }

    pub(crate) async fn load_session_from_rollout(
        &self,
        rollout_path: &Path,
        deps: &ServerRuntimeDependencies,
    ) -> Result<RuntimeSession> {
        let file = File::open(rollout_path)
            .with_context(|| format!("open rollout file {}", rollout_path.display()))?;
        let reader = BufReader::new(file);
        let mut replay = ReplayState::default();
        // Dual read (05 §2.2): legacy lines replay directly, v2 lines are
        // projected back into legacy lines by the per-load inverse.
        let inverse = V2InverseProjector::new();
        let mut lines = reader.lines().enumerate().peekable();

        while let Some((line_index, line)) = lines.next() {
            let line =
                line.with_context(|| format!("read line from {}", rollout_path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let parsed = match parse_rollout_line(&line) {
                Ok(parsed) => parsed,
                // A truncated final line is a crash tail: the write never
                // completed, nothing was acknowledged.
                Err(RolloutLineReadError::TruncatedTail) if lines.peek().is_none() => break,
                // Fail closed: a damaged or unsupported mid-file line means
                // the session's history is unreadable past this point; the
                // session refuses to resume rather than silently dropping
                // history and appending onto a fork.
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "rollout {} is damaged at line {}; refusing to resume",
                            rollout_path.display(),
                            line_index + 1
                        )
                    });
                }
            };
            match parsed {
                ParsedRolloutLine::Legacy(legacy) => replay.apply_line(*legacy)?,
                ParsedRolloutLine::V2(v2) => {
                    for legacy_line in inverse.project_line(&v2).with_context(|| {
                        format!("project v2 line from {}", rollout_path.display())
                    })? {
                        replay.apply_line(legacy_line)?;
                    }
                }
            }
        }

        let mut recovered = replay
            .into_runtime_session(deps)
            .await
            .with_context(|| format!("replay rollout {}", rollout_path.display()))?;
        // Inverse-projected (v2) session records carry an empty rollout_path
        // by design; the real location is always the file being read.
        if let Some(record) = recovered.record.as_mut()
            && record.rollout_path.as_os_str().is_empty()
        {
            record.rollout_path = rollout_path.to_path_buf();
        }
        Ok(recovered)
    }

    /// Reads durable workspace checkpoints without reconstructing runtime state.
    ///
    /// P4d rollback plans need the pre-turn ghost commit plus its untracked
    /// manifest. This follows the same dual-read and fail-closed rules as
    /// `load_session_from_rollout`.
    pub(crate) fn workspace_checkpoints(
        &self,
        record: &SessionRecord,
    ) -> Result<Vec<TurnWorkspaceCheckpointRecordedRecord>> {
        let file = File::open(&record.rollout_path)
            .with_context(|| format!("open rollout file {}", record.rollout_path.display()))?;
        let reader = BufReader::new(file);
        let inverse = V2InverseProjector::new();
        let mut checkpoints = Vec::new();
        let mut lines = reader.lines().enumerate().peekable();
        while let Some((line_index, line)) = lines.next() {
            let line =
                line.with_context(|| format!("read line from {}", record.rollout_path.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let parsed = match parse_rollout_line(&line) {
                Ok(parsed) => parsed,
                Err(RolloutLineReadError::TruncatedTail) if lines.peek().is_none() => break,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "rollout {} is damaged at line {}; refusing checkpoint read",
                            record.rollout_path.display(),
                            line_index + 1
                        )
                    });
                }
            };
            let legacy_lines = match parsed {
                ParsedRolloutLine::Legacy(legacy) => vec![*legacy],
                ParsedRolloutLine::V2(v2) => inverse.project_line(&v2).with_context(|| {
                    format!(
                        "project v2 checkpoint line from {}",
                        record.rollout_path.display()
                    )
                })?,
            };
            for legacy in legacy_lines {
                if let RolloutLine::TurnWorkspaceCheckpointRecorded(line) = legacy {
                    checkpoints.push(line.record);
                }
            }
        }
        Ok(checkpoints)
    }

    pub(crate) fn rollout_paths(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let root = self.data_root.join("sessions");
        if !root.exists() {
            return Ok(files);
        }
        collect_rollout_files(&root, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn rollout_path(&self, created_at: chrono::DateTime<Utc>, session_id: SessionId) -> PathBuf {
        let partition = self
            .data_root
            .join("sessions")
            .join(format!("{:04}", created_at.year()))
            .join(format!("{:02}", created_at.month()))
            .join(format!("{:02}", created_at.day()));
        let timestamp = created_at
            .to_rfc3339_opts(SecondsFormat::Secs, true)
            .replace(':', "-");
        partition.join(format!("rollout-{timestamp}-{session_id}.jsonl"))
    }

    /// Appends one legacy-shaped line to the rollout file. The write path is
    /// single-write v2 (05 §2.2): the line is projected through the per-file
    /// [`LegacyProjector`] and every resulting v2 line becomes its own JSONL
    /// row. Legacy callers (the typed wrappers above) are unchanged.
    pub(crate) fn append_canonical_item(
        &self,
        record: &SessionRecord,
        item: devo_protocol::native::item::ItemEnvelope,
    ) -> Result<()> {
        let line = RolloutLineV2::Item {
            v: 2,
            timestamp: Utc::now(),
            item,
        };
        self.append_v2_lines(&record.rollout_path, vec![line])
    }

    pub(crate) fn append_goal_state(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        goal: Option<serde_json::Value>,
    ) -> Result<()> {
        self.append_v2_lines(
            rollout_path,
            vec![RolloutLineV2::Internal {
                v: 2,
                timestamp: Utc::now(),
                session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(
                    session_id,
                )),
                turn_id: None,
                seq: 0,
                entry: devo_core::InternalRecordV2::GoalState {
                    schema_version: 1,
                    goal,
                },
            }],
        )
    }

    pub(crate) fn append_usage_record(
        &self,
        rollout_path: &Path,
        session_id: SessionId,
        record: devo_protocol::native::usage::UsageRecord,
    ) -> Result<()> {
        self.append_v2_lines(
            rollout_path,
            vec![RolloutLineV2::Internal {
                v: 2,
                timestamp: record.recorded_at,
                session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(Uuid::from(
                    session_id,
                )),
                turn_id: record.turn_id.clone(),
                seq: 0,
                entry: devo_core::InternalRecordV2::UsageRecord { record },
            }],
        )
    }

    fn append_line(&self, rollout_path: &Path, line: &RolloutLine) -> Result<()> {
        if let Some(parent) = rollout_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create rollout directory {}", parent.display()))?;
        }
        // Acquire a per-file lock so concurrent writes to the same rollout file
        // do not interleave their JSON payloads.
        let file_lock = {
            let mut locks = self
                .file_locks
                .lock()
                .expect("rollout file-locks table poisoned");
            locks
                .entry(rollout_path.to_path_buf())
                .or_insert_with(|| Arc::new(StdMutex::new(())))
                .clone()
        };
        let _guard = file_lock.lock().expect("rollout per-file lock poisoned");

        let mut write_states = self
            .write_states
            .lock()
            .expect("rollout write-state table poisoned");
        let state = match write_states.get_mut(rollout_path) {
            Some(state) => state,
            None => {
                let state = hydrate_write_state(rollout_path)?;
                write_states
                    .entry(rollout_path.to_path_buf())
                    .or_insert(state)
            }
        };
        let v2_lines = state
            .projector
            .project_line(line)
            .with_context(|| format!("project rollout line for {}", rollout_path.display()))?;
        self.write_v2_lines(rollout_path, state, &v2_lines)
    }

    fn append_v2_lines(&self, rollout_path: &Path, v2_lines: Vec<RolloutLineV2>) -> Result<()> {
        if let Some(parent) = rollout_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create rollout directory {}", parent.display()))?;
        }
        let file_lock = {
            let mut locks = self
                .file_locks
                .lock()
                .expect("rollout file-locks table poisoned");
            locks
                .entry(rollout_path.to_path_buf())
                .or_insert_with(|| Arc::new(StdMutex::new(())))
                .clone()
        };
        let _guard = file_lock.lock().expect("rollout per-file lock poisoned");
        let mut write_states = self
            .write_states
            .lock()
            .expect("rollout write-state table poisoned");
        let state = match write_states.get_mut(rollout_path) {
            Some(state) => state,
            None => {
                let state = hydrate_write_state(rollout_path)?;
                write_states
                    .entry(rollout_path.to_path_buf())
                    .or_insert(state)
            }
        };
        for line in &v2_lines {
            state.projector.observe_v2_line(line);
        }
        self.write_v2_lines(rollout_path, state, &v2_lines)
    }

    fn write_v2_lines(
        &self,
        rollout_path: &Path,
        state: &mut WritePathState,
        v2_lines: &[RolloutLineV2],
    ) -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(rollout_path)
            .with_context(|| format!("open rollout file {}", rollout_path.display()))?;
        let first_line_index = state.next_line_index;
        for v2_line in v2_lines {
            serde_json::to_writer(&mut file, v2_line)
                .with_context(|| format!("serialize rollout line {}", rollout_path.display()))?;
            file.write_all(b"\n")
                .with_context(|| format!("write rollout newline {}", rollout_path.display()))?;
        }
        file.flush()
            .with_context(|| format!("flush rollout file {}", rollout_path.display()))?;
        // The rollout is the event log: an acknowledged write must survive a
        // crash, so every append ends in fsync (file data only, not the
        // directory entry — matches the pre-v2 durability floor plus the
        // event-log requirement).
        file.sync_data()
            .with_context(|| format!("fsync rollout file {}", rollout_path.display()))?;
        state.next_line_index += v2_lines.len() as u64;

        // Outbox projection (08 §5/§7): derive delivery-log events from the
        // fsynced facts. Best effort — a failure here is backfilled by the
        // startup reconciler, so a crash may delay an event but never lose
        // or duplicate it.
        if let Some(db) = &self.event_log
            && let Err(error) =
                project_events_into_log(db, rollout_path, first_line_index, v2_lines)
        {
            tracing::warn!(
                rollout = %rollout_path.display(),
                %error,
                "failed to project events into event_log; reconciliation will backfill"
            );
        }
        Ok(())
    }
}

/// Derives delivery-log rows from freshly written v2 lines and inserts them
/// idempotently, then advances the projection watermark.
fn project_events_into_log(
    db: &Database,
    rollout_path: &Path,
    first_line_index: u64,
    v2_lines: &[RolloutLineV2],
) -> Result<()> {
    let mut rows = Vec::new();
    let mut last_line_index = first_line_index;
    for (offset, v2_line) in v2_lines.iter().enumerate() {
        let line_index = first_line_index + offset as u64;
        last_line_index = line_index;
        rows.extend(event_log_rows_for_v2_line(
            rollout_path,
            line_index,
            0,
            v2_line,
        )?);
    }
    db.insert_event_log_rows(&rows)?;
    if !v2_lines.is_empty() {
        db.set_projection_watermark(rollout_path, last_line_index)?;
    }
    Ok(())
}

/// Builds the delivery-log rows derived from one v2 rollout fact (also used
/// by the startup reconciler, which passes a nonzero `sub_index` for v2
/// lines expanded from a packed legacy row).
pub(crate) fn event_log_rows_for_v2_line(
    rollout_path: &Path,
    line_index: u64,
    sub_index: u64,
    v2_line: &RolloutLineV2,
) -> Result<Vec<NewEventLogRow>> {
    let timestamp = v2_line_timestamp(v2_line);
    let mut rows = Vec::new();
    for derived in events_from_v2_line(v2_line) {
        let envelope = EventEnvelope {
            meta: EventMeta {
                event_id: EventId::new(),
                stream_id: derived.stream_id.clone(),
                // Allocated by the event_log insert (per-stream monotonic);
                // replay hydrates meta.seq from the stored row.
                seq: None,
                emitted_at: timestamp,
                persisted: true,
                schema_version: EVENT_SCHEMA_VERSION,
                actor_client_id: None,
            },
            notification: derived.notification,
        };
        rows.push(NewEventLogRow {
            source_fact_id: source_fact_id(rollout_path, line_index, sub_index),
            event_kind: derived.event_kind.to_owned(),
            stream_id: derived.stream_id,
            event_id: envelope.meta.event_id.to_string(),
            payload: serde_json::to_string(&envelope).context("serialize event envelope")?,
            created_at: timestamp.to_rfc3339(),
        });
    }
    Ok(rows)
}

/// The wall-clock timestamp carried by any v2 line variant.
fn v2_line_timestamp(line: &RolloutLineV2) -> chrono::DateTime<Utc> {
    match line {
        RolloutLineV2::SessionMeta { timestamp, .. }
        | RolloutLineV2::Turn { timestamp, .. }
        | RolloutLineV2::Item { timestamp, .. }
        | RolloutLineV2::Internal { timestamp, .. }
        | RolloutLineV2::SessionTitleUpdated { timestamp, .. }
        | RolloutLineV2::CompactionSnapshot { timestamp, .. }
        | RolloutLineV2::SessionRollback { timestamp, .. }
        | RolloutLineV2::WorkspaceCheckpoint { timestamp, .. }
        | RolloutLineV2::WorkspaceChange { timestamp, .. }
        | RolloutLineV2::WorkspaceRestoreStarted { timestamp, .. }
        | RolloutLineV2::WorkspaceRestoreCompleted { timestamp, .. } => *timestamp,
    }
}

/// Builds the write-path projector for an existing rollout file by replaying
/// its current contents: legacy lines go through the forward projector (so
/// the seq counter and approval folds advance exactly as if the file had
/// been written through the v2 path), v2 lines re-sync that state via
/// [`LegacyProjector::observe_v2_line`]. Bounded per path: runs once, on the
/// first append, and the result is cached in the store.
///
/// Fails closed on any damaged or unsupported line: appending onto history
/// the projector could not fully read would fork the session's history.
/// Builds the write-path state for an existing rollout file by replaying its
/// current contents: legacy lines go through the forward projector (so the
/// seq counter and approval folds advance exactly as if the file had been
/// written through the v2 path), v2 lines re-sync that state via
/// [`LegacyProjector::observe_v2_line`]. Bounded per path: runs once, on the
/// first append, and the result is cached in the store. Also returns the next
/// JSONL row index, which becomes the `source_fact_id` line index of every
/// subsequent append.
///
/// Fails closed on any damaged or unsupported line: appending onto history
/// the projector could not fully read would fork the session's history.
fn hydrate_write_state(rollout_path: &Path) -> Result<WritePathState> {
    let mut projector = LegacyProjector::new();
    let mut next_line_index = 0u64;
    let file = match File::open(rollout_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WritePathState {
                projector,
                next_line_index,
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("open rollout file {}", rollout_path.display()));
        }
    };
    let reader = BufReader::new(file);
    let mut lines = reader.lines().enumerate().peekable();
    while let Some((line_index, line)) = lines.next() {
        let line = line.with_context(|| format!("read line from {}", rollout_path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_rollout_line(&line) {
            Ok(ParsedRolloutLine::Legacy(legacy)) => {
                projector.project_line(&legacy).with_context(|| {
                    format!("hydrate projector from {}", rollout_path.display())
                })?;
            }
            Ok(ParsedRolloutLine::V2(v2)) => projector.observe_v2_line(&v2),
            Err(RolloutLineReadError::TruncatedTail) if lines.peek().is_none() => break,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "rollout {} is damaged at line {}; refusing to append",
                        rollout_path.display(),
                        line_index + 1
                    )
                });
            }
        }
        // The line index counts physical JSONL rows regardless of format.
        next_line_index += 1;
    }
    Ok(WritePathState {
        projector,
        next_line_index,
    })
}

#[derive(Default)]
struct ReplayState {
    session: Option<SessionRecord>,
    latest_turn: Option<TurnRecord>,
    latest_turn_metadata: Option<TurnMetadata>,
    latest_query_usage: Option<devo_protocol::TurnUsage>,
    latest_context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
    turn_records_by_id: HashMap<TurnId, TurnRecord>,
    loaded_item_count: u64,
    next_item_seq: u64,
    turns_seen: u32,
    total_input_tokens: usize,
    total_output_tokens: usize,
    total_tokens: usize,
    total_cache_creation_tokens: usize,
    total_cache_read_tokens: usize,
    last_input_tokens: usize,
    last_turn_tokens: usize,
    session_context: Option<devo_core::SessionContext>,
    /// Session context loaded from a dedicated `SessionContextUpdated` rollout line.
    /// Preserved across rollback because that line is session-scoped, not turn-scoped.
    recorded_session_context: Option<devo_core::SessionContext>,
    latest_turn_context: Option<devo_core::TurnContext>,
    session_context_recorded: bool,
    turn_kinds_by_id: HashMap<TurnId, TurnKind>,
    messages: Vec<Message>,
    history_items: Vec<crate::SessionHistoryItem>,
    pending_items: Vec<ReplayHistoryItem>,
    latest_compaction_snapshot: Option<CompactionSnapshotLine>,
    turn_order: Vec<TurnId>,
    superseded_turn_ids: HashSet<TurnId>,
    summarized_turn_ids: HashSet<TurnId>,
    last_activity_at: Option<chrono::DateTime<Utc>>,
    /// Field-level session settings accumulated during replay (L2-DES-CONV-002
    /// Phase 1). The last line per field wins; a `PermissionPreset` line
    /// clears the explicit `SandboxProfile` override (the preset re-implies
    /// the sandbox), matching the approved patch-interaction rule.
    session_settings: HashMap<SessionSettingsField, serde_json::Value>,
}

/// Applies an optional-string settings field line onto a record field,
/// logging divergence from the whole-record `SessionMeta` value.
fn apply_optional_string_setting(
    target: &mut Option<String>,
    value: serde_json::Value,
    session_id: SessionId,
    field_name: &str,
) {
    let parsed = match serde_json::from_value::<Option<String>>(value) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(session_id = %session_id, field = field_name, %error, "ignoring damaged settings line");
            return;
        }
    };
    let Some(new_value) = parsed else {
        return;
    };
    if target.as_ref().is_some_and(|current| *current != new_value) {
        tracing::warn!(
            session_id = %session_id,
            field = field_name,
            "settings field line disagrees with SessionMeta value; field line wins"
        );
    }
    *target = Some(new_value);
}

/// Auto-compact / status pressure restored on resume.
///
/// Prefers post-compaction (or tip) occupancy so a prior large query total cannot
/// re-trigger compaction after the context was already reduced. Falls back to
/// latest-query display total, then the reconstituted prompt estimate.
fn resume_context_pressure_tokens(
    occupancy: Option<&devo_protocol::native::item::ContextOccupancy>,
    latest_query_usage: Option<&devo_protocol::TurnUsage>,
    prompt_token_estimate: usize,
) -> (usize, usize) {
    let last_turn_tokens = occupancy
        .map(|occupancy| occupancy.total_tokens as usize)
        .or_else(|| latest_query_usage.map(devo_protocol::TurnUsage::display_total_tokens))
        .unwrap_or(prompt_token_estimate);
    let last_input_tokens = latest_query_usage
        .map(|usage| usage.input_tokens as usize)
        .or_else(|| occupancy.map(|occupancy| occupancy.total_tokens as usize))
        .unwrap_or(prompt_token_estimate);
    (last_turn_tokens, last_input_tokens)
}

impl ReplayState {
    fn apply_line(&mut self, line: RolloutLine) -> Result<()> {
        match line {
            RolloutLine::SessionMeta(line) => {
                let mut session = line.session;
                if session.last_activity_at.is_none() {
                    session.last_activity_at = Some(session.created_at);
                }
                self.last_activity_at = session.last_activity_at;
                self.session = Some(session);
            }
            RolloutLine::Turn(line) => {
                // Insert turn summary for the previous turn before processing the new turn
                if let Some(prev_turn) = self.latest_turn.clone() {
                    self.enqueue_terminal_history_items(&prev_turn);
                }

                let turn = line.turn;
                if self.superseded_turn_ids.contains(&turn.id) {
                    return Ok(());
                }
                self.apply_activity_timestamp(
                    turn.session_id,
                    turn.completed_at.unwrap_or(turn.started_at),
                    "turn line",
                )?;
                if !self.turn_records_by_id.contains_key(&turn.id) {
                    self.turn_order.push(turn.id);
                }
                self.turns_seen = self.turns_seen.max(turn.sequence);
                if let Some(usage) = &turn.usage {
                    self.total_input_tokens += usage.input_tokens as usize;
                    self.total_output_tokens += usage.output_tokens as usize;
                    self.total_tokens += usage.display_total_tokens();
                    self.total_cache_creation_tokens +=
                        usage.cache_creation_input_tokens.unwrap_or(0) as usize;
                    self.total_cache_read_tokens +=
                        usage.cache_read_input_tokens.unwrap_or(0) as usize;
                }
                if let Some(usage) = &turn.latest_query_usage {
                    self.last_input_tokens = usage.input_tokens as usize;
                    self.last_turn_tokens = usage.display_total_tokens();
                    self.latest_query_usage = Some(usage.clone());
                } else if turn.usage.is_some() {
                    // Older rollout records only contain aggregate turn usage.
                    // Do not mistake it for the latest model query.
                    self.last_input_tokens = 0;
                    self.last_turn_tokens = 0;
                    self.latest_query_usage = None;
                }
                if let Some(occupancy) = turn.context_occupancy.clone() {
                    self.latest_context_occupancy = Some(occupancy);
                }
                self.latest_turn_metadata = Some(turn_metadata_from_record(&turn));
                self.turn_kinds_by_id.insert(turn.id, turn.kind.clone());
                if let Some(session_context) = turn.session_context.clone() {
                    self.session_context = Some(session_context);
                    self.session_context_recorded = true;
                }
                if let Some(turn_context) = turn.turn_context.clone() {
                    self.latest_turn_context = Some(turn_context);
                }
                self.turn_records_by_id.insert(turn.id, turn.clone());
                self.latest_turn = Some(turn);
            }
            RolloutLine::Item(line) => {
                if !self.superseded_turn_ids.contains(&line.item.turn_id) {
                    self.apply_activity_timestamp(
                        line.item.session_id,
                        line.item.timestamp,
                        "item line",
                    )?;
                    self.loaded_item_count += 1;
                    self.next_item_seq = self.next_item_seq.max(line.item.seq + 1);
                    self.collect_item_line(line.item);
                }
            }
            RolloutLine::SessionTitleUpdated(line) => {
                let session = self
                    .session
                    .as_mut()
                    .context("title update without session header")?;
                session.title = Some(line.title);
                session.title_state = line.title_state;
                session.updated_at = line.timestamp;
            }
            RolloutLine::SessionContextUpdated(line) => {
                let line = *line;
                if let Some(session) = self.session.as_mut() {
                    if session.id != line.session_id {
                        anyhow::bail!(
                            "session context update session id does not match session header"
                        );
                    }
                    session.updated_at = line.timestamp;
                }
                self.recorded_session_context = Some(line.session_context.clone());
                self.session_context = Some(line.session_context);
                self.session_context_recorded = true;
            }
            RolloutLine::CompactionSnapshot(line) => {
                if let Some(occupancy) = line.context_occupancy.clone() {
                    self.latest_context_occupancy = Some(occupancy);
                }
                self.latest_compaction_snapshot = Some(*line);
            }
            RolloutLine::MessageEditRecorded(line) => {
                self.apply_record_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "message edit line",
                )?;
                self.apply_activity_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "message edit line",
                )?;
            }
            RolloutLine::TurnSuperseded(line) => {
                self.apply_record_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "turn superseded line",
                )?;
                self.apply_activity_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "turn superseded line",
                )?;
                self.apply_turn_superseded(line.record);
            }
            RolloutLine::TurnWorkspaceCheckpointRecorded(line) => {
                self.apply_record_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace checkpoint line",
                )?;
                self.apply_activity_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace checkpoint line",
                )?;
            }
            RolloutLine::TurnWorkspaceChangeRecorded(line) => {
                self.apply_record_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace change line",
                )?;
                self.apply_activity_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace change line",
                )?;
            }
            RolloutLine::TurnWorkspaceRestoreStarted(line) => {
                self.apply_record_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace restore started line",
                )?;
                self.apply_activity_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace restore started line",
                )?;
            }
            RolloutLine::TurnWorkspaceRestoreCompleted(line) => {
                self.apply_record_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace restore completed line",
                )?;
                self.apply_activity_timestamp(
                    line.record.session_id,
                    line.timestamp,
                    "workspace restore completed line",
                )?;
            }
            RolloutLine::SessionRollback(line) => {
                self.apply_session_rollback(*line)?;
            }
            RolloutLine::SessionSettings(line) => {
                // Approved patch-interaction rule: a preset change re-implies
                // the sandbox, so it clears any explicit override seen so far.
                if line.field == SessionSettingsField::PermissionPreset {
                    self.session_settings
                        .remove(&SessionSettingsField::SandboxProfile);
                }
                self.session_settings.insert(line.field, line.value);
            }
        }
        Ok(())
    }

    /// Applies accumulated field-level settings onto the replayed session
    /// record, ahead of the derivations in `into_runtime_session`. Field lines
    /// win over the whole-record `SessionMeta` values; a disagreement between
    /// the two is logged because it indicates a missed dual-write.
    fn apply_session_settings(&mut self, record: &mut SessionRecord) {
        let fields = std::mem::take(&mut self.session_settings);
        for (field, value) in fields {
            match field {
                SessionSettingsField::PermissionPreset => {
                    match serde_json::from_value::<devo_protocol::PermissionPreset>(value) {
                        Ok(preset) => {
                            if record.permission_preset.is_some_and(|p| p != preset) {
                                tracing::warn!(
                                    session_id = %record.id,
                                    "settings field line disagrees with SessionMeta permission_preset; field line wins"
                                );
                            }
                            record.permission_preset = Some(preset);
                        }
                        Err(error) => {
                            tracing::warn!(session_id = %record.id, %error, "ignoring damaged permissionPreset settings line");
                        }
                    }
                }
                SessionSettingsField::Model => {
                    apply_optional_string_setting(&mut record.model, value, record.id, "model");
                }
                SessionSettingsField::ModelBindingId => {
                    apply_optional_string_setting(
                        &mut record.model_binding_id,
                        value,
                        record.id,
                        "model_binding_id",
                    );
                }
                SessionSettingsField::ReasoningEffortSelection => {
                    apply_optional_string_setting(
                        &mut record.reasoning_effort_selection,
                        value,
                        record.id,
                        "reasoning_effort_selection",
                    );
                }
                SessionSettingsField::CollaborationMode => {
                    match serde_json::from_value::<devo_protocol::CollaborationMode>(value) {
                        Ok(mode) => {
                            if record.collaboration_mode.is_some_and(|m| m != mode) {
                                tracing::warn!(
                                    session_id = %record.id,
                                    "settings field line disagrees with SessionMeta collaboration_mode; field line wins"
                                );
                            }
                            record.collaboration_mode = Some(mode);
                        }
                        Err(error) => {
                            tracing::warn!(session_id = %record.id, %error, "ignoring damaged collaborationMode settings line");
                        }
                    }
                }
                // Applied to `core_session.config` after the preset
                // derivation, not to the record (the record has no sandbox
                // profile name field).
                SessionSettingsField::SandboxProfile => {
                    self.session_settings.insert(field, value);
                }
            }
        }
    }

    /// Returns the explicit sandbox profile override accumulated from settings
    /// field lines, if any survived preset re-derivation.
    fn sandbox_profile_override(&self) -> Option<String> {
        self.session_settings
            .get(&SessionSettingsField::SandboxProfile)
            .and_then(
                |value| match serde_json::from_value::<String>(value.clone()) {
                    Ok(name) => Some(name),
                    Err(error) => {
                        tracing::warn!(%error, "ignoring damaged sandboxProfile settings line");
                        None
                    }
                },
            )
    }

    async fn into_runtime_session(
        mut self,
        deps: &ServerRuntimeDependencies,
    ) -> Result<RuntimeSession> {
        // Insert turn summary for the last turn before converting
        if let Some(last_turn) = self.latest_turn.clone() {
            self.enqueue_terminal_history_items(&last_turn);
        }

        let mut record = self
            .session
            .take()
            .context("missing SessionMetaLine in rollout")?;
        let last_activity_at = self
            .last_activity_at
            .or(record.last_activity_at)
            .unwrap_or(record.updated_at);
        record.last_activity_at = Some(last_activity_at);
        // Field-level settings lines win over the whole-record SessionMeta
        // values (L2-DES-CONV-002 Phase 1); apply before the derivations below.
        self.apply_session_settings(&mut record);
        let sandbox_profile_override = self.sandbox_profile_override();
        let runtime_context = deps.context_for_workspace(&record.cwd).await?;
        let mut core_session = runtime_context.new_session_state(
            record.id,
            record.cwd.clone(),
            record.additional_directories.clone(),
        );
        let mut ordered_items = self.pending_items;
        ordered_items.sort_by(|left, right| {
            left.seq
                .cmp(&right.seq)
                .then_with(|| left.timestamp.cmp(&right.timestamp))
                .then_with(|| left.record_timestamp.cmp(&right.record_timestamp))
                .then_with(|| left.line_timestamp.cmp(&right.line_timestamp))
                .then_with(|| left.bucket_priority.cmp(&right.bucket_priority))
                .then_with(|| left.intra_record_order.cmp(&right.intra_record_order))
        });

        let mut replayed_messages = self.messages;
        let mut replayed_history_items = self.history_items;
        let mut replayed_persisted_turn_items = Vec::with_capacity(ordered_items.len());
        let mut tool_names_by_id = HashMap::new();
        for pending_item in ordered_items {
            match pending_item.payload {
                ReplayHistoryItemPayload::TurnItem(turn_item) => {
                    apply_turn_item(
                        &mut replayed_messages,
                        &mut replayed_history_items,
                        &mut tool_names_by_id,
                        &pending_item.turn_kind,
                        turn_item.clone(),
                    );
                    replayed_persisted_turn_items.push(PersistedTurnItem {
                        turn_id: pending_item.turn_id,
                        turn_kind: pending_item.turn_kind,
                        item_id: pending_item.item_id,
                        turn_item,
                    });
                }
                ReplayHistoryItemPayload::HistoryOnly(history_item) => {
                    replayed_history_items.push(history_item);
                }
            }
        }

        core_session.messages = replayed_messages;
        core_session.prompt_messages =
            self.latest_compaction_snapshot
                .as_ref()
                .and_then(|snapshot| {
                    build_prompt_messages_from_snapshot(&replayed_persisted_turn_items, snapshot)
                });
        core_session.session_context = self
            .session_context
            .or_else(|| record.session_context.clone());
        core_session.latest_turn_context = self
            .latest_turn_context
            .or_else(|| record.latest_turn_context.clone());
        if let Some(latest_turn_context) = core_session.latest_turn_context.as_ref() {
            core_session.collaboration_mode = latest_turn_context.collaboration_mode;
        }
        if let Some(mode) = record.collaboration_mode {
            core_session.collaboration_mode = mode;
        }
        if let Some(preset) = record.permission_preset {
            let safety_preset = match preset {
                devo_protocol::PermissionPreset::Default => devo_safety::PermissionPreset::Default,
                devo_protocol::PermissionPreset::AutoReview => {
                    devo_safety::PermissionPreset::AutoReview
                }
                devo_protocol::PermissionPreset::FullAccess => {
                    devo_safety::PermissionPreset::FullAccess
                }
            };
            let profile = devo_safety::RuntimePermissionProfile::from_preset(
                safety_preset,
                record.cwd.clone(),
            )
            .with_additional_roots(record.additional_directories.clone());
            let sandbox = Some(profile.implied_sandbox_profile().to_string());
            core_session.config.permission_mode = profile.permission_mode();
            core_session.config.permission_profile = profile;
            core_session.config.sandbox_profile = sandbox;
        }
        // An explicit sandbox override from settings field lines wins over the
        // preset-implied sandbox (approved patch-interaction rule).
        if let Some(sandbox) = sandbox_profile_override {
            core_session.config.sandbox_profile = Some(sandbox);
        }
        core_session.turn_count = self.turns_seen as usize;
        core_session.total_input_tokens = self.total_input_tokens;
        core_session.total_output_tokens = self.total_output_tokens;
        core_session.total_tokens = self.total_tokens;
        core_session.total_cache_creation_tokens = self.total_cache_creation_tokens;
        core_session.total_cache_read_tokens = self.total_cache_read_tokens;
        let prompt_bytes = core_session
            .prompt_source_messages()
            .iter()
            .map(|message| serde_json::to_string(message).map_or(0, |json| json.len()))
            .sum::<usize>();
        core_session.prompt_token_estimate =
            devo_protocol::approx_tokens_from_byte_count(prompt_bytes)
                .try_into()
                .unwrap_or(usize::MAX);
        let (last_turn_tokens, last_input_tokens) = resume_context_pressure_tokens(
            self.latest_context_occupancy.as_ref(),
            self.latest_query_usage.as_ref(),
            core_session.prompt_token_estimate,
        );
        core_session.last_input_tokens = last_input_tokens;
        core_session.last_turn_tokens = last_turn_tokens;
        let pending_turn_queue = std::sync::Arc::clone(&core_session.pending_turn_queue);
        let steer_input_queue = std::sync::Arc::clone(&core_session.steer_input_queue);
        let summary_model_selection = self
            .latest_turn_metadata
            .as_ref()
            .and_then(|turn| turn.model_binding_id.clone())
            .or_else(|| {
                self.latest_turn_metadata
                    .as_ref()
                    .map(|turn| turn.model.clone())
            })
            .or_else(|| record.model_binding_id.clone())
            .or_else(|| record.model.clone())
            .unwrap_or_else(|| runtime_context.default_model.clone());
        let turn_config = runtime_context.resolve_turn_config(Some(&summary_model_selection), None);
        let concrete_selection = |selection: Option<&str>| {
            selection
                .map(str::trim)
                .filter(|selection| !selection.is_empty())
                .filter(|selection| !selection.eq_ignore_ascii_case("default"))
                .map(str::to_ascii_lowercase)
        };
        let explicit_reasoning_effort_selection = self
            .latest_turn_metadata
            .as_ref()
            .and_then(|turn| concrete_selection(turn.reasoning_effort_selection.as_deref()))
            .or_else(|| concrete_selection(record.reasoning_effort_selection.as_deref()));
        let context_reasoning_effort_selection = core_session
            .latest_turn_context
            .as_ref()
            .and_then(|context| context.reasoning_effort)
            .or_else(|| {
                core_session
                    .session_context
                    .as_ref()
                    .and_then(|context| context.reasoning_effort)
            })
            .map(|effort| effort.label().to_lowercase());
        let summary_reasoning_effort_selection =
            turn_config.model.normalize_reasoning_effort_selection(
                explicit_reasoning_effort_selection
                    .as_deref()
                    .or(context_reasoning_effort_selection.as_deref()),
            );
        let summary_reasoning_effort = turn_config
            .model
            .resolve_reasoning_effort_selection(summary_reasoning_effort_selection.as_deref())
            .effective_reasoning_effort;
        record.model = Some(turn_config.model.slug.clone());
        record.model_binding_id = turn_config.model_binding_id.clone();
        record.reasoning_effort_selection = summary_reasoning_effort_selection.clone();

        let global_compaction_limit = runtime_context
            .config_store
            .lock()
            .expect("app config store mutex should not be poisoned")
            .effective_config()
            .compaction_token_limit;
        let applied_compaction_limit = crate::runtime::context_occupancy::resolved_compaction_limit(
            global_compaction_limit,
            &turn_config.model,
        );
        // Apply before wrapping in Mutex so resume never needs to lock a
        // single-owner Arc that `from_runtime_session` later unwraps.
        // Prefer the global config preference; ignore legacy session overrides.
        crate::runtime::context_occupancy::apply_resolved_compaction_limit(
            &mut core_session.config,
            applied_compaction_limit as usize,
        );

        let summary = SessionMetadata {
            session_id: record.id,
            cwd: record.cwd.clone(),
            additional_directories: record.additional_directories.clone(),
            created_at: record.created_at,
            updated_at: record.updated_at,
            last_activity_at,
            title: record.title.clone(),
            title_state: record.title_state.clone(),
            parent_session_id: record.parent_session_id,
            agent_path: record.agent_path.clone(),
            agent_nickname: record.agent_nickname.clone(),
            agent_role: record.agent_role.clone(),
            ephemeral: false,
            model: Some(turn_config.model.slug),
            model_binding_id: turn_config.model_binding_id.clone(),
            reasoning_effort_selection: summary_reasoning_effort_selection,
            reasoning_effort: summary_reasoning_effort,
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
            total_tokens: self.total_tokens,
            total_cache_creation_tokens: self.total_cache_creation_tokens,
            total_cache_read_tokens: self.total_cache_read_tokens,
            prompt_token_estimate: core_session.prompt_token_estimate,
            last_query_usage: self.latest_query_usage.clone(),
            last_query_total_tokens: self
                .latest_context_occupancy
                .as_ref()
                .map(|occupancy| occupancy.total_tokens as usize)
                .or_else(|| {
                    self.latest_query_usage
                        .as_ref()
                        .map(devo_protocol::TurnUsage::display_total_tokens)
                })
                .unwrap_or(0),
            last_context_occupancy: self.latest_context_occupancy.clone(),
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: core_session.collaboration_mode,
            effective_context_window: Some(applied_compaction_limit),
            permission_preset: record.permission_preset,
        };

        let config = core_session.config.clone();
        Ok(RuntimeSession {
            runtime_context,
            record: Some(record),
            summary,
            config,
            core_session: std::sync::Arc::new(Mutex::new(core_session)),
            active_turn: None,
            latest_turn: self.latest_turn_metadata,
            loaded_item_count: self.loaded_item_count,
            history_items: replayed_history_items,
            persisted_turn_items: replayed_persisted_turn_items,
            latest_compaction_snapshot: self.latest_compaction_snapshot,
            turn_records_by_id: self.turn_records_by_id,
            pending_turn_queue,
            steer_input_queue,
            agent_tool_policy: Default::default(),
            max_turns: None,
            deferred_assistant: None,
            deferred_reasoning: None,
            next_item_seq: self.next_item_seq.max(1),
            first_user_input: None,
            tool_registry: None,
            file_read_ledger: std::sync::Arc::new(devo_core::tools::FileReadLedger::new()),
            session_approval_cache: crate::execution::ApprovalGrantCache::default(),
            turn_approval_cache: crate::execution::ApprovalGrantCache::default(),
            session_context_recorded: self.session_context_recorded,
        })
    }

    fn apply_record_timestamp(
        &mut self,
        session_id: SessionId,
        timestamp: chrono::DateTime<Utc>,
        line_kind: &str,
    ) -> Result<()> {
        if let Some(session) = self.session.as_mut() {
            if session.id != session_id {
                anyhow::bail!("{line_kind} session id does not match session header");
            }
            session.updated_at = timestamp;
        }
        Ok(())
    }

    fn apply_activity_timestamp(
        &mut self,
        session_id: SessionId,
        timestamp: chrono::DateTime<Utc>,
        line_kind: &str,
    ) -> Result<()> {
        if let Some(session) = self.session.as_mut() {
            if session.id != session_id {
                anyhow::bail!("{line_kind} session id does not match session header");
            }
            let last_activity_at = self
                .last_activity_at
                .map(|current| current.max(timestamp))
                .unwrap_or(timestamp);
            self.last_activity_at = Some(last_activity_at);
            session.last_activity_at = Some(last_activity_at);
        }
        Ok(())
    }

    fn apply_turn_superseded(&mut self, record: TurnSupersededRecord) {
        self.superseded_turn_ids.insert(record.superseded_turn_id);
        let removed_item_ids = self
            .pending_items
            .iter()
            .filter(|item| item.turn_id == record.superseded_turn_id)
            .map(|item| item.item_id)
            .collect::<HashSet<_>>();

        self.pending_items
            .retain(|item| item.turn_id != record.superseded_turn_id);
        self.turn_order
            .retain(|turn_id| *turn_id != record.superseded_turn_id);
        self.turn_records_by_id.remove(&record.superseded_turn_id);
        self.turn_kinds_by_id.remove(&record.superseded_turn_id);

        if self
            .latest_turn_metadata
            .as_ref()
            .is_some_and(|turn| turn.turn_id == record.superseded_turn_id)
        {
            self.latest_turn = self
                .turn_order
                .iter()
                .rev()
                .find_map(|turn_id| self.turn_records_by_id.get(turn_id).cloned());
            self.latest_turn_metadata = self.latest_turn.as_ref().map(turn_metadata_from_record);
        }

        if self
            .latest_compaction_snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                removed_item_ids.contains(&snapshot.summary_item_id)
                    || snapshot
                        .preserved_item_ids
                        .iter()
                        .any(|item_id| removed_item_ids.contains(item_id))
            })
        {
            self.latest_compaction_snapshot = None;
        }

        self.recompute_turn_aggregates();
    }

    fn apply_session_rollback(&mut self, line: SessionRollbackLine) -> Result<()> {
        if let Some(session) = self.session.as_mut() {
            if session.id != line.session_id {
                anyhow::bail!("rollback line session id does not match session header");
            }
            session.updated_at = line.timestamp;
        }
        self.apply_activity_timestamp(line.session_id, line.timestamp, "rollback line")?;

        let retained_turn_ids = line
            .retained_turn_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let retained_item_ids = line
            .retained_item_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();

        self.pending_items.retain(|item| {
            retained_turn_ids.contains(&item.turn_id)
                && (matches!(&item.payload, ReplayHistoryItemPayload::HistoryOnly(_))
                    || retained_item_ids.contains(&item.item_id))
        });
        self.turn_order
            .retain(|turn_id| retained_turn_ids.contains(turn_id));
        self.turn_records_by_id
            .retain(|turn_id, _| retained_turn_ids.contains(turn_id));
        self.turn_kinds_by_id
            .retain(|turn_id, _| retained_turn_ids.contains(turn_id));
        self.superseded_turn_ids
            .retain(|turn_id| retained_turn_ids.contains(turn_id));
        self.summarized_turn_ids
            .retain(|turn_id| retained_turn_ids.contains(turn_id));

        self.latest_turn = line
            .latest_turn_id
            .and_then(|turn_id| self.turn_records_by_id.get(&turn_id).cloned());
        self.latest_turn_metadata = self.latest_turn.as_ref().map(turn_metadata_from_record);
        // Prefer the dedicated SessionContextUpdated side-channel. Rollback prunes
        // turn records but must not drop locked session context that was recorded
        // once for the rollout file.
        self.session_context = self
            .recorded_session_context
            .clone()
            .or_else(|| {
                self.latest_turn
                    .as_ref()
                    .and_then(|turn| turn.session_context.clone())
            })
            .or_else(|| {
                self.session
                    .as_ref()
                    .and_then(|session| session.session_context.clone())
            });
        self.session_context_recorded = self.recorded_session_context.is_some()
            || self
                .latest_turn
                .as_ref()
                .is_some_and(|turn| turn.session_context.is_some())
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.session_context.is_some());
        self.latest_turn_context = None;
        self.loaded_item_count = u64::try_from(retained_item_ids.len()).unwrap_or(u64::MAX);
        self.next_item_seq = self
            .pending_items
            .iter()
            .map(|item| item.seq.saturating_add(1))
            .max()
            .unwrap_or(1);
        self.recompute_turn_aggregates();

        if self
            .latest_compaction_snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                !retained_item_ids.contains(&snapshot.summary_item_id)
                    || snapshot
                        .preserved_item_ids
                        .iter()
                        .any(|item_id| !retained_item_ids.contains(item_id))
            })
        {
            self.latest_compaction_snapshot = None;
        }
        Ok(())
    }

    fn recompute_turn_aggregates(&mut self) {
        self.turns_seen = 0;
        self.total_input_tokens = 0;
        self.total_output_tokens = 0;
        self.total_tokens = 0;
        self.total_cache_creation_tokens = 0;
        self.total_cache_read_tokens = 0;
        self.last_input_tokens = 0;
        self.last_turn_tokens = 0;
        self.latest_query_usage = None;
        self.latest_context_occupancy = None;

        for turn_id in &self.turn_order {
            let Some(turn) = self.turn_records_by_id.get(turn_id) else {
                continue;
            };
            self.turns_seen = self.turns_seen.max(turn.sequence);
            if let Some(usage) = &turn.usage {
                self.total_input_tokens += usage.input_tokens as usize;
                self.total_output_tokens += usage.output_tokens as usize;
                self.total_tokens += usage.display_total_tokens();
                self.total_cache_creation_tokens +=
                    usage.cache_creation_input_tokens.unwrap_or(0) as usize;
                self.total_cache_read_tokens += usage.cache_read_input_tokens.unwrap_or(0) as usize;
            }
            if let Some(usage) = &turn.latest_query_usage {
                self.last_input_tokens = usage.input_tokens as usize;
                self.last_turn_tokens = usage.display_total_tokens();
                self.latest_query_usage = Some(usage.clone());
            } else if turn.usage.is_some() {
                self.last_input_tokens = 0;
                self.last_turn_tokens = 0;
                self.latest_query_usage = None;
            }
            if let Some(occupancy) = turn.context_occupancy.clone() {
                self.latest_context_occupancy = Some(occupancy);
            }
        }
        if let Some(occupancy) = self
            .latest_compaction_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.context_occupancy.clone())
        {
            self.latest_context_occupancy = Some(occupancy);
        }
    }

    fn collect_item_line(&mut self, item: ItemRecord) {
        let item_id = item.id;
        let record_timestamp = item.timestamp;
        let line_timestamp = record_timestamp;
        let seq = item.seq;
        let turn_kind = self
            .turn_kinds_by_id
            .get(&item.turn_id)
            .cloned()
            .unwrap_or_default();
        let mut intra_record_order = 0usize;

        for turn_item in item.output_items {
            self.pending_items.push(ReplayHistoryItem {
                turn_id: item.turn_id,
                turn_kind: turn_kind.clone(),
                item_id,
                seq,
                timestamp: record_timestamp,
                record_timestamp,
                line_timestamp,
                bucket_priority: 0,
                intra_record_order,
                payload: ReplayHistoryItemPayload::TurnItem(turn_item),
            });
            intra_record_order += 1;
        }

        for turn_item in item.input_items {
            self.pending_items.push(ReplayHistoryItem {
                turn_id: item.turn_id,
                turn_kind: turn_kind.clone(),
                item_id,
                seq,
                timestamp: record_timestamp,
                record_timestamp,
                line_timestamp,
                bucket_priority: 1,
                intra_record_order,
                payload: ReplayHistoryItemPayload::TurnItem(turn_item),
            });
            intra_record_order += 1;
        }
    }

    fn enqueue_terminal_history_items(&mut self, turn: &TurnRecord) {
        let outcome = match turn.status {
            TurnStatus::Failed => "failed",
            TurnStatus::Interrupted => "interrupted",
            TurnStatus::Completed => "",
            TurnStatus::Pending | TurnStatus::Running | TurnStatus::WaitingApproval => return,
        };
        if self.superseded_turn_ids.contains(&turn.id) || !self.summarized_turn_ids.insert(turn.id)
        {
            return;
        }

        let seq = self
            .pending_items
            .iter()
            .filter(|item| item.turn_id == turn.id)
            .map(|item| item.seq)
            .max()
            .unwrap_or(0);
        let timestamp = turn.completed_at.unwrap_or(turn.started_at);
        let duration_secs = turn.completed_at.and_then(|completed| {
            let seconds = (completed - turn.started_at).num_seconds();
            (seconds > 0).then_some(seconds as u64)
        });
        let mut intra_record_order = 0;

        if matches!(turn.status, TurnStatus::Failed)
            && let Some(error) = &turn.error
        {
            self.pending_items.push(ReplayHistoryItem {
                turn_id: turn.id,
                turn_kind: turn.kind.clone(),
                item_id: ItemId::new(),
                seq,
                timestamp,
                record_timestamp: timestamp,
                line_timestamp: timestamp,
                bucket_priority: 2,
                intra_record_order,
                payload: ReplayHistoryItemPayload::HistoryOnly(crate::SessionHistoryItem::new(
                    None,
                    crate::SessionHistoryItemKind::Error,
                    error.code.clone(),
                    error.message.clone(),
                )),
            });
            intra_record_order += 1;
        }

        let collaboration_mode = turn
            .turn_context
            .as_ref()
            .map(|context| context.collaboration_mode)
            .unwrap_or_default();
        self.pending_items.push(ReplayHistoryItem {
            turn_id: turn.id,
            turn_kind: turn.kind.clone(),
            item_id: ItemId::new(),
            seq,
            timestamp,
            record_timestamp: timestamp,
            line_timestamp: timestamp,
            bucket_priority: 2,
            intra_record_order,
            payload: ReplayHistoryItemPayload::HistoryOnly(crate::SessionHistoryItem {
                tool_call_id: None,
                kind: crate::SessionHistoryItemKind::TurnSummary,
                title: turn.model.clone(),
                body: outcome.to_string(),
                tool_io: None,
                metadata: Some(crate::SessionHistoryMetadata::TurnSummary { collaboration_mode }),
                duration_ms: duration_secs,
            }),
        });
    }
}

#[derive(Debug, Clone)]
struct ReplayHistoryItem {
    turn_id: TurnId,
    turn_kind: TurnKind,
    item_id: ItemId,
    seq: u64,
    timestamp: chrono::DateTime<Utc>,
    record_timestamp: chrono::DateTime<Utc>,
    line_timestamp: chrono::DateTime<Utc>,
    bucket_priority: u8,
    intra_record_order: usize,
    payload: ReplayHistoryItemPayload,
}

#[derive(Debug, Clone)]
enum ReplayHistoryItemPayload {
    TurnItem(TurnItem),
    HistoryOnly(crate::SessionHistoryItem),
}

pub(crate) fn build_prompt_messages_from_snapshot(
    persisted_turn_items: &[PersistedTurnItem],
    snapshot: &CompactionSnapshotLine,
) -> Option<Vec<Message>> {
    let ordered_items = persisted_turn_items
        .iter()
        .filter(|item| prompt_visible_persisted_turn_item(item))
        .collect::<Vec<_>>();
    let summary_index = ordered_items
        .iter()
        .position(|item| item.item_id == snapshot.summary_item_id)?;

    let mut by_item_id: HashMap<ItemId, PersistedTurnItem> = ordered_items
        .iter()
        .cloned()
        .map(|item| (item.item_id, item.clone()))
        .collect();

    let mut rebuilt = Vec::new();
    if let Some(summary_item) = by_item_id.remove(&snapshot.summary_item_id) {
        rebuilt.push(summary_item);
    }

    for preserved_id in &snapshot.preserved_item_ids {
        if let Some(item) = by_item_id.remove(preserved_id) {
            rebuilt.push(item);
        }
    }

    rebuilt.extend(
        ordered_items
            .iter()
            .skip(summary_index + 1)
            .filter(|item| item.item_id != snapshot.summary_item_id)
            .filter(|item| !snapshot.preserved_item_ids.contains(&item.item_id))
            .map(|item| (*item).clone()),
    );

    let mut messages = Vec::new();
    let mut tool_names_by_id = HashMap::new();
    for item in rebuilt {
        apply_prompt_turn_item(&mut messages, &mut tool_names_by_id, item.turn_item.clone());
    }
    Some(messages)
}

pub(crate) fn prompt_visible_persisted_turn_item(item: &PersistedTurnItem) -> bool {
    prompt_visible_turn_item(&item.turn_item)
}

fn prompt_visible_turn_item(item: &TurnItem) -> bool {
    matches!(
        item,
        TurnItem::ContextCompaction(_)
            | TurnItem::UserMessage(_)
            | TurnItem::SteerInput(_)
            | TurnItem::AgentMessage(_)
            | TurnItem::Reasoning(_)
            | TurnItem::ToolCall(_)
            | TurnItem::ToolResult(_)
            | TurnItem::CommandExecution(_)
            | TurnItem::Plan(_)
            | TurnItem::WebSearch(_)
            | TurnItem::ImageGeneration(_)
            | TurnItem::HookPrompt(_)
    )
}

pub(crate) fn apply_turn_item(
    messages: &mut Vec<Message>,
    history_items: &mut Vec<crate::SessionHistoryItem>,
    tool_names_by_id: &mut HashMap<String, String>,
    _turn_kind: &TurnKind,
    item: TurnItem,
) {
    let item = match item {
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id,
            tool_name,
            input,
        }) => {
            tool_names_by_id.insert(tool_call_id.clone(), tool_name.clone());
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id,
                tool_name,
                input,
            })
        }
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id,
            tool_name,
            output,
            display_content,
            is_error,
        }) => TurnItem::ToolResult(ToolResultItem {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.or_else(|| tool_names_by_id.get(&tool_call_id).cloned()),
            output,
            display_content,
            is_error,
        }),
        TurnItem::CommandExecution(CommandExecutionItem {
            tool_call_id,
            tool_name,
            command,
            input,
            output,
            is_error,
        }) => {
            tool_names_by_id.insert(tool_call_id.clone(), tool_name.clone());
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id,
                tool_name,
                command,
                input,
                output,
                is_error,
            })
        }
        other => other,
    };

    if let Some(history_item) = history_item_from_turn_item(&item) {
        history_items.push(history_item);
    }

    if prompt_visible_turn_item(&item) {
        apply_prompt_turn_item(messages, tool_names_by_id, item);
    }
}

fn apply_prompt_turn_item(
    messages: &mut Vec<Message>,
    tool_names_by_id: &mut HashMap<String, String>,
    item: TurnItem,
) {
    let item = match item {
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id,
            tool_name,
            input,
        }) => {
            tool_names_by_id.insert(tool_call_id.clone(), tool_name.clone());
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id,
                tool_name,
                input,
            })
        }
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id,
            tool_name,
            output,
            display_content,
            is_error,
        }) => TurnItem::ToolResult(ToolResultItem {
            tool_call_id: tool_call_id.clone(),
            tool_name: tool_name.or_else(|| tool_names_by_id.get(&tool_call_id).cloned()),
            output,
            display_content,
            is_error,
        }),
        TurnItem::CommandExecution(CommandExecutionItem {
            tool_call_id,
            tool_name,
            command,
            input,
            output,
            is_error,
        }) => {
            tool_names_by_id.insert(tool_call_id.clone(), tool_name.clone());
            TurnItem::CommandExecution(CommandExecutionItem {
                tool_call_id,
                tool_name,
                command,
                input,
                output,
                is_error,
            })
        }
        other => other,
    };

    match item {
        TurnItem::UserMessage(TextItem { text }) | TurnItem::SteerInput(TextItem { text }) => {
            messages.push(Message::user(text));
        }
        TurnItem::AgentMessage(TextItem { text }) if text.trim().is_empty() => {}
        TurnItem::AgentMessage(TextItem { text })
        | TurnItem::Plan(TextItem { text })
        | TurnItem::WebSearch(TextItem { text })
        | TurnItem::ImageGeneration(TextItem { text })
        | TurnItem::ContextCompaction(TextItem { text })
        | TurnItem::HookPrompt(TextItem { text }) => {
            messages.push(Message::assistant_text(text));
        }
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id,
            tool_name,
            input,
        }) => match messages.last_mut() {
            Some(message) if message.role == Role::Assistant => {
                message.content.push(ContentBlock::ToolUse {
                    id: tool_call_id,
                    name: tool_name,
                    input,
                });
            }
            _ => {
                messages.push(Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: tool_call_id,
                        name: tool_name,
                        input,
                    }],
                });
            }
        },
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id,
            tool_name: _,
            output,
            display_content: _,
            is_error,
        }) => {
            let content = match output {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
            match messages.last_mut() {
                Some(message)
                    if message.role == Role::User
                        && message
                            .content
                            .iter()
                            .all(|block| matches!(block, ContentBlock::ToolResult { .. })) =>
                {
                    message.content.push(ContentBlock::ToolResult {
                        tool_use_id: tool_call_id,
                        content,
                        is_error,
                    });
                }
                _ => {
                    messages.push(Message {
                        role: Role::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: tool_call_id,
                            content,
                            is_error,
                        }],
                    });
                }
            }
        }
        TurnItem::CommandExecution(CommandExecutionItem {
            tool_call_id,
            tool_name,
            input,
            output,
            is_error,
            ..
        }) => {
            match messages.last_mut() {
                Some(message) if message.role == Role::Assistant => {
                    message.content.push(ContentBlock::ToolUse {
                        id: tool_call_id.clone(),
                        name: tool_name,
                        input,
                    });
                }
                _ => {
                    messages.push(Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::ToolUse {
                            id: tool_call_id.clone(),
                            name: tool_name,
                            input,
                        }],
                    });
                }
            }
            let content = match output {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: tool_call_id,
                    content,
                    is_error,
                }],
            });
        }
        TurnItem::Reasoning(TextItem { text }) => match messages.last_mut() {
            Some(message) if message.role == Role::Assistant => {
                message.content.push(ContentBlock::Reasoning { text });
            }
            _ => {
                messages.push(Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Reasoning { text }],
                });
            }
        },
        TurnItem::ToolProgress(_)
        | TurnItem::ApprovalRequest(_)
        | TurnItem::ApprovalDecision(_)
        | TurnItem::TurnSummary(_) => {}
    }
}

fn read_rollout_index_fields(path: &Path) -> Result<(SessionRecord, chrono::DateTime<Utc>)> {
    let file = File::open(path).with_context(|| format!("open rollout file {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut session: Option<SessionRecord> = None;
    let mut last_activity_at: Option<chrono::DateTime<Utc>> = None;
    // Dual read for the index path (05 §2.2/§2.3). The index is a rebuildable
    // cache, so — unlike resume — unreadable lines are skipped, not fatal.
    let inverse = V2InverseProjector::new();

    for line in reader.lines() {
        let line = line.with_context(|| format!("read line from {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        let legacy_lines: Vec<RolloutLine> = match parse_rollout_line(&line) {
            Ok(ParsedRolloutLine::Legacy(legacy)) => vec![*legacy],
            Ok(ParsedRolloutLine::V2(v2)) => match inverse.project_line(&v2) {
                Ok(lines) => lines,
                Err(_) => continue,
            },
            Err(_) => continue,
        };
        for parsed in legacy_lines {
            match parsed {
                RolloutLine::SessionMeta(meta_line) => {
                    let mut record = meta_line.session;
                    if record.last_activity_at.is_none() {
                        record.last_activity_at = Some(record.created_at);
                    }
                    last_activity_at = record.last_activity_at;
                    session = Some(record);
                }
                RolloutLine::SessionTitleUpdated(line) => {
                    if let Some(record) = session.as_mut() {
                        record.title = Some(line.title);
                        record.title_state = line.title_state;
                        record.updated_at = line.timestamp;
                        last_activity_at = Some(line.timestamp);
                    }
                }
                _ => {}
            }
        }
    }

    let session = session
        .with_context(|| format!("missing SessionMeta line in rollout {}", path.display()))?;
    let last_activity_at = last_activity_at
        .or(session.last_activity_at)
        .unwrap_or(session.created_at);
    Ok((session, last_activity_at))
}

pub(crate) fn session_metadata_from_record(
    record: &SessionRecord,
    last_activity_at: chrono::DateTime<Utc>,
) -> SessionMetadata {
    SessionMetadata {
        session_id: record.id,
        cwd: record.cwd.clone(),
        additional_directories: record.additional_directories.clone(),
        created_at: record.created_at,
        updated_at: record.updated_at,
        last_activity_at,
        title: record.title.clone(),
        title_state: record.title_state.clone(),
        parent_session_id: record.parent_session_id,
        agent_path: record.agent_path.clone(),
        agent_nickname: record.agent_nickname.clone(),
        agent_role: record.agent_role.clone(),
        ephemeral: false,
        model: record.model.clone(),
        model_binding_id: record.model_binding_id.clone(),
        reasoning_effort_selection: record.reasoning_effort_selection.clone(),
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        last_context_occupancy: None,
        status: SessionRuntimeStatus::Idle,
        collaboration_mode: record
            .collaboration_mode
            .or_else(|| {
                record
                    .latest_turn_context
                    .as_ref()
                    .map(|context| context.collaboration_mode)
            })
            .unwrap_or_default(),
        // Do not revive legacy session-record overrides. Applied window comes
        // from AppConfig when the session is hydrated into a RuntimeSession.
        effective_context_window: None,
        permission_preset: record.permission_preset,
    }
}

fn collect_rollout_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root).with_context(|| format!("read dir {}", root.display()))? {
        let entry = entry.with_context(|| format!("read entry in {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type for {}", path.display()))?;
        if file_type.is_dir() {
            collect_rollout_files(&path, files)?;
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Creates one canonical persisted turn record from the transport-facing runtime state.
pub(crate) fn build_turn_record(
    turn: &TurnMetadata,
    session_context: Option<devo_core::SessionContext>,
    turn_context: Option<devo_core::TurnContext>,
    latest_query_usage: Option<devo_core::TurnUsage>,
    context_occupancy: Option<devo_protocol::native::item::ContextOccupancy>,
) -> TurnRecord {
    TurnRecord {
        id: turn.turn_id,
        session_id: turn.session_id,
        sequence: turn.sequence,
        started_at: turn.started_at,
        completed_at: turn.completed_at,
        status: turn.status.clone(),
        kind: turn.kind.clone(),
        model: turn.model.clone(),
        model_binding_id: turn.model_binding_id.clone(),
        reasoning_effort_selection: turn.reasoning_effort_selection.clone(),
        request_model: turn.request_model.clone(),
        request_thinking: turn.request_thinking.clone(),
        input_token_estimate: None,
        usage: turn.usage.clone(),
        latest_query_usage,
        context_occupancy,
        stop_reason: turn.stop_reason.clone(),
        failure_reason: turn.failure_reason,
        error: None,
        session_context,
        turn_context,
        schema_version: 4,
    }
}

fn turn_metadata_from_record(turn: &TurnRecord) -> TurnMetadata {
    TurnMetadata {
        turn_id: turn.id,
        session_id: turn.session_id,
        sequence: turn.sequence,
        status: turn.status.clone(),
        kind: turn.kind.clone(),
        model: turn.model.clone(),
        model_binding_id: turn.model_binding_id.clone(),
        reasoning_effort_selection: turn.reasoning_effort_selection.clone(),
        reasoning_effort: turn
            .turn_context
            .as_ref()
            .and_then(|context| context.reasoning_effort)
            .or_else(|| {
                turn.session_context
                    .as_ref()
                    .and_then(|context| context.reasoning_effort)
            }),
        request_model: turn.request_model.clone(),
        request_thinking: turn.request_thinking.clone(),
        started_at: turn.started_at,
        completed_at: turn.completed_at,
        usage: turn.usage.clone(),
        stop_reason: turn.stop_reason.clone(),
        failure_reason: turn.failure_reason,
    }
}

/// Creates one canonical persisted item record from a normalized turn item payload.
pub(crate) fn build_item_record(
    session_id: SessionId,
    turn_id: TurnId,
    item_id: devo_core::ItemId,
    seq: u64,
    item: TurnItem,
    turn_status: Option<TurnStatus>,
    worklog: Option<Worklog>,
) -> ItemRecord {
    ItemRecord {
        id: item_id,
        session_id,
        turn_id,
        seq,
        timestamp: Utc::now(),
        attempt_placement: None,
        turn_status,
        sibling_turn_ids: Vec::new(),
        input_items: Vec::new(),
        output_items: vec![item],
        worklog,
        error: None,
        schema_version: 1,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use chrono::TimeZone;
    use chrono::Utc;
    use pretty_assertions::assert_eq;

    use super::ParsedRolloutLine;
    use super::ReplayHistoryItemPayload;
    use super::ReplayState;
    use super::build_prompt_messages_from_snapshot;
    use super::parse_rollout_line;
    use crate::execution::PersistedTurnItem;
    use crate::execution::ServerRuntimeDependencies;
    use crate::persistence::apply_turn_item;
    use devo_core::CompactionSnapshotLine;
    use devo_core::ContentPart;
    use devo_core::EditId;
    use devo_core::EditState;
    use devo_core::EnvironmentContext;
    use devo_core::ItemId;
    use devo_core::ItemLine;
    use devo_core::ItemRecord;
    use devo_core::LanguageContext;
    use devo_core::Message;
    use devo_core::MessageEditRecordedLine;
    use devo_core::MessageEditRecordedRecord;
    use devo_core::Model;
    use devo_core::Persona;
    use devo_core::RolloutLine;
    use devo_core::SessionContext;
    use devo_core::SessionId;
    use devo_core::SessionMetaLine;
    use devo_core::SessionRecord;
    use devo_core::SessionRollbackLine;
    use devo_core::SessionTitleState;
    use devo_core::TextItem;
    use devo_core::ToolCallItem;
    use devo_core::ToolResultItem;
    use devo_core::TurnContext;
    use devo_core::TurnId;
    use devo_core::TurnItem;
    use devo_core::TurnKind;
    use devo_core::TurnLine;
    use devo_core::TurnRecord;
    use devo_core::TurnStatus;
    use devo_core::TurnSupersededLine;
    use devo_core::TurnSupersededRecord;
    use devo_core::WorkspaceRestorePolicy;

    #[test]
    fn replay_orders_items_by_sequence_before_timestamp() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let earlier = Utc.with_ymd_and_hms(2026, 4, 6, 8, 0, 0).unwrap();
        let later = Utc.with_ymd_and_hms(2026, 4, 6, 8, 0, 1).unwrap();
        let mut replay = ReplayState::default();

        replay
            .apply_line(RolloutLine::Item(ItemLine {
                timestamp: earlier,
                item: ItemRecord {
                    id: ItemId::new(),
                    session_id,
                    turn_id,
                    seq: 2,
                    timestamp: earlier,
                    attempt_placement: None,
                    turn_status: None,
                    sibling_turn_ids: Vec::new(),
                    input_items: Vec::new(),
                    output_items: vec![TurnItem::ToolCall(ToolCallItem {
                        tool_call_id: "call-1".to_string(),
                        tool_name: "bash".to_string(),
                        input: serde_json::json!({"command":"date"}),
                    })],
                    worklog: None,
                    error: None,
                    schema_version: 1,
                },
            }))
            .expect("replay later-seq line");
        replay
            .apply_line(RolloutLine::Item(ItemLine {
                timestamp: later,
                item: ItemRecord {
                    id: ItemId::new(),
                    session_id,
                    turn_id,
                    seq: 1,
                    timestamp: later,
                    attempt_placement: None,
                    turn_status: None,
                    sibling_turn_ids: Vec::new(),
                    output_items: vec![TurnItem::AgentMessage(TextItem {
                        text: "assistant 1".to_string(),
                    })],
                    input_items: Vec::new(),
                    worklog: None,
                    error: None,
                    schema_version: 1,
                },
            }))
            .expect("replay earlier-seq line");

        let mut items = replay.pending_items;
        items.sort_by(|left, right| {
            left.seq
                .cmp(&right.seq)
                .then_with(|| left.timestamp.cmp(&right.timestamp))
                .then_with(|| left.intra_record_order.cmp(&right.intra_record_order))
        });

        let titles = items
            .into_iter()
            .map(|item| match item.payload {
                ReplayHistoryItemPayload::TurnItem(TurnItem::AgentMessage(TextItem { text })) => {
                    text
                }
                ReplayHistoryItemPayload::TurnItem(TurnItem::ToolCall(ToolCallItem {
                    input,
                    ..
                })) => input["command"].as_str().unwrap().to_string(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>();

        assert_eq!(titles, vec!["assistant 1", "date"]);
    }

    #[test]
    fn replay_prefers_compaction_occupancy_over_prior_turn() {
        use devo_protocol::TurnUsage;
        use pretty_assertions::assert_eq;

        let now = Utc.with_ymd_and_hms(2026, 7, 8, 10, 0, 0).unwrap();
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let mut replay = ReplayState::default();
        let turn_occupancy = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
        );
        let compact_occupancy = devo_protocol::native::item::ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 100_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 8_000,
        );

        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: turn_id,
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: TurnKind::Regular,
                    model: "test-model".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    request_model: "test-model".into(),
                    request_thinking: None,
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: Some(TurnUsage {
                        input_tokens: 50,
                        output_tokens: 5,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                        reasoning_output_tokens: None,
                        total_tokens: Some(55),
                    }),
                    context_occupancy: Some(turn_occupancy.clone()),
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: None,
                    schema_version: 4,
                },
            })))
            .expect("turn");
        assert_eq!(replay.latest_context_occupancy, Some(turn_occupancy));
        replay
            .apply_line(RolloutLine::CompactionSnapshot(Box::new(
                CompactionSnapshotLine {
                    timestamp: now,
                    session_id,
                    turn_id,
                    summary_item_id: ItemId::new(),
                    preserved_item_ids: Vec::new(),
                    context_occupancy: Some(compact_occupancy.clone()),
                },
            )))
            .expect("compaction");
        assert_eq!(replay.latest_context_occupancy, Some(compact_occupancy));
    }

    #[test]
    fn resume_context_pressure_prefers_compaction_occupancy_over_large_query() {
        use pretty_assertions::assert_eq;

        use devo_protocol::TurnUsage;
        use devo_protocol::native::item::ContextOccupancy;

        let occupancy = ContextOccupancy::from_category_tokens(
            /*context_window_tokens*/ 250_000, /*base*/ 10_000, /*skills*/ 0,
            /*tools_builtin*/ 0, /*tools_mcp*/ 0, /*conversation*/ 40_000,
        );
        let usage = TurnUsage {
            input_tokens: 300_000,
            output_tokens: 20_000,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: Some(320_000),
        };

        let (last_turn, last_input) = super::resume_context_pressure_tokens(
            Some(&occupancy),
            Some(&usage),
            /*prompt_token_estimate*/ 12_000,
        );
        assert_eq!(last_turn, 50_000);
        assert_eq!(last_input, 300_000);

        let (last_turn, last_input) = super::resume_context_pressure_tokens(
            /*occupancy*/ None,
            Some(&usage),
            /*prompt_token_estimate*/ 12_000,
        );
        assert_eq!(last_turn, 320_000);
        assert_eq!(last_input, 300_000);

        let (last_turn, last_input) = super::resume_context_pressure_tokens(
            /*occupancy*/ None, /*latest_query_usage*/ None,
            /*prompt_token_estimate*/ 12_000,
        );
        assert_eq!(last_turn, 12_000);
        assert_eq!(last_input, 12_000);
    }

    #[test]
    fn replay_preserves_latest_query_usage_when_latest_turn_has_no_usage() {
        use devo_protocol::TurnUsage;

        let now = Utc.with_ymd_and_hms(2026, 7, 8, 10, 0, 0).unwrap();
        let session_id = SessionId::new();
        let mut replay = ReplayState::default();
        let usage = TurnUsage {
            input_tokens: 30,
            output_tokens: 12,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: Some(42),
        };

        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: TurnId::new(),
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: TurnKind::Regular,
                    model: "model-a".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    request_model: "model-a".into(),
                    request_thinking: None,
                    input_token_estimate: None,
                    usage: Some(usage.clone()),
                    latest_query_usage: Some(usage.clone()),
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: None,
                    schema_version: 2,
                },
            })))
            .expect("apply usage turn");
        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: TurnId::new(),
                    session_id,
                    sequence: 2,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Failed,
                    kind: TurnKind::Regular,
                    model: "model-a".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    request_model: "model-a".into(),
                    request_thinking: None,
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: Some(devo_protocol::TurnFailureReason::MaxTurnRequests),
                    error: None,
                    session_context: None,
                    turn_context: None,
                    schema_version: 2,
                },
            })))
            .expect("apply terminal turn without usage");

        assert_eq!(replay.latest_query_usage, Some(usage));
        assert_eq!(replay.last_turn_tokens, 42);
        assert_eq!(replay.last_input_tokens, 30);
    }

    #[test]
    fn replay_does_not_promote_aggregate_turn_usage_to_latest_query_usage() {
        use devo_protocol::TurnUsage;

        let now = Utc.with_ymd_and_hms(2026, 7, 8, 10, 0, 0).unwrap();
        let session_id = SessionId::new();
        let aggregate_usage = TurnUsage {
            input_tokens: 10_000,
            output_tokens: 2_000,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            reasoning_output_tokens: None,
            total_tokens: Some(12_000),
        };
        let mut replay = ReplayState::default();

        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: TurnId::new(),
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: TurnKind::Regular,
                    model: "model-a".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    request_model: "model-a".into(),
                    request_thinking: None,
                    input_token_estimate: None,
                    usage: Some(aggregate_usage),
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: None,
                    schema_version: 2,
                },
            })))
            .expect("apply legacy aggregate-only turn");

        assert_eq!(replay.latest_query_usage, None);
        assert_eq!(replay.last_turn_tokens, 0);
        assert_eq!(replay.last_input_tokens, 0);
    }

    #[test]
    fn replay_prunes_superseded_turn_from_rollout_projection() {
        let now = Utc.with_ymd_and_hms(2026, 6, 18, 8, 0, 0).unwrap();
        let session_id = SessionId::new();
        let original_turn_id = TurnId::new();
        let replacement_turn_id = TurnId::new();
        let original_item_id = ItemId::new();
        let replacement_item_id = ItemId::new();
        let edit_id = EditId::new();
        let mut replay = ReplayState::default();

        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: original_turn_id,
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: TurnKind::Regular,
                    model: "model-a".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    request_model: "model-a".into(),
                    request_thinking: None,
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: None,
                    schema_version: 2,
                },
            })))
            .expect("apply original turn");
        replay
            .apply_line(RolloutLine::Item(ItemLine {
                timestamp: now,
                item: ItemRecord {
                    id: original_item_id,
                    session_id,
                    turn_id: original_turn_id,
                    seq: 1,
                    timestamp: now,
                    attempt_placement: None,
                    turn_status: Some(TurnStatus::Completed),
                    sibling_turn_ids: Vec::new(),
                    input_items: vec![TurnItem::UserMessage(TextItem {
                        text: "original".into(),
                    })],
                    output_items: Vec::new(),
                    worklog: None,
                    error: None,
                    schema_version: 1,
                },
            }))
            .expect("apply original item");
        replay
            .apply_line(RolloutLine::MessageEditRecorded(Box::new(
                MessageEditRecordedLine {
                    timestamp: now,
                    record: MessageEditRecordedRecord {
                        schema_version: 1,
                        session_id,
                        edit_id,
                        target_message_id: original_item_id,
                        replacement_message_id: replacement_item_id,
                        target_turn_id: Some(original_turn_id),
                        replacement_turn_id: Some(replacement_turn_id),
                        queue_item_id: None,
                        edited_content_parts: vec![ContentPart::Text("edited".into())],
                        edited_mentions: Vec::new(),
                        workspace_restore_policy: WorkspaceRestorePolicy::Skip,
                        edit_state: EditState::Accepted,
                        requested_by_client_id: None,
                        created_at: now,
                    },
                },
            )))
            .expect("apply message edit line");
        replay
            .apply_line(RolloutLine::TurnSuperseded(Box::new(TurnSupersededLine {
                timestamp: now,
                record: TurnSupersededRecord {
                    schema_version: 1,
                    session_id,
                    superseded_turn_id: original_turn_id,
                    replacement_turn_id,
                    edit_id,
                    restore_id: None,
                    reason: "message_edit_previous".into(),
                    created_at: now,
                },
            })))
            .expect("apply superseded line");
        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: replacement_turn_id,
                    session_id,
                    sequence: 2,
                    started_at: now,
                    completed_at: None,
                    status: TurnStatus::Running,
                    kind: TurnKind::Regular,
                    model: "model-a".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    request_model: "model-a".into(),
                    request_thinking: None,
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: None,
                    schema_version: 2,
                },
            })))
            .expect("apply replacement turn");
        replay
            .apply_line(RolloutLine::Item(ItemLine {
                timestamp: now,
                item: ItemRecord {
                    id: replacement_item_id,
                    session_id,
                    turn_id: replacement_turn_id,
                    seq: 2,
                    timestamp: now,
                    attempt_placement: None,
                    turn_status: Some(TurnStatus::Running),
                    sibling_turn_ids: Vec::new(),
                    input_items: vec![TurnItem::UserMessage(TextItem {
                        text: "edited".into(),
                    })],
                    output_items: Vec::new(),
                    worklog: None,
                    error: None,
                    schema_version: 1,
                },
            }))
            .expect("apply replacement item");

        let projected_items = replay
            .pending_items
            .iter()
            .filter_map(|item| match &item.payload {
                ReplayHistoryItemPayload::TurnItem(turn_item) => Some(turn_item.clone()),
                ReplayHistoryItemPayload::HistoryOnly(_) => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            projected_items,
            vec![TurnItem::UserMessage(TextItem {
                text: "edited".into(),
            })]
        );
        assert_eq!(replay.turn_order, vec![replacement_turn_id]);
        assert_eq!(
            replay
                .latest_turn_metadata
                .as_ref()
                .map(|turn| turn.turn_id),
            Some(replacement_turn_id)
        );
    }

    #[test]
    fn index_rollout_metadata_reads_session_meta_only() {
        use chrono::Utc;
        use pretty_assertions::assert_eq;
        use std::io::Write;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let data_root = dir.path().to_path_buf();
        let session_id = SessionId::new();
        let now = Utc::now();
        let rollout_store = super::RolloutStore::new(data_root.clone(), None);
        let record = rollout_store.create_session_record(
            session_id,
            now,
            data_root.clone(),
            Vec::new(),
            Some("Indexed session".into()),
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&record.rollout_path)
            .expect("open rollout")
            .write_all(b"{\"type\":\"turn\",\"payload\":{}}\n")
            .expect("append non-meta line");

        let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
        rollout_store
            .index_rollout_metadata(&db)
            .expect("index rollout metadata");

        let index = db
            .get_session_index(&session_id)
            .expect("get index")
            .expect("indexed session");
        assert_eq!(index.metadata.title.as_deref(), Some("Indexed session"));
        assert_eq!(index.rollout_path, Some(record.rollout_path));
        assert_eq!(index.metadata.parent_session_id, None);

        let roots = db.list_root_sessions().expect("list roots");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].session_id, session_id);
    }

    #[test]
    fn index_rollout_metadata_overwrites_stale_sqlite_title_when_rollout_has_title() {
        use chrono::Utc;
        use devo_protocol::SessionMetadata;
        use devo_protocol::SessionRuntimeStatus;
        use devo_protocol::SessionTitleState;
        use pretty_assertions::assert_eq;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let data_root = dir.path().to_path_buf();
        let session_id = SessionId::new();
        let now = Utc::now();
        let rollout_store = super::RolloutStore::new(data_root.clone(), None);
        let record = rollout_store.create_session_record(
            session_id,
            now,
            data_root.clone(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        rollout_store
            .append_title_update(
                &record,
                "Canonical rollout title".into(),
                SessionTitleState::Final(devo_core::SessionTitleFinalSource::ModelGenerated),
                None,
            )
            .expect("append title update");

        let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
        db.upsert_session(
            &SessionMetadata {
                session_id,
                cwd: data_root.clone(),
                additional_directories: Vec::new(),
                created_at: now,
                updated_at: now,
                last_activity_at: now,
                title: Some("Existing title".into()),
                title_state: SessionTitleState::Final(
                    devo_core::SessionTitleFinalSource::ModelGenerated,
                ),
                parent_session_id: None,
                agent_path: None,
                agent_nickname: None,
                agent_role: None,
                ephemeral: false,
                model: Some("test-model".into()),
                model_binding_id: None,
                reasoning_effort_selection: None,
                reasoning_effort: None,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_tokens: 0,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
                prompt_token_estimate: 0,
                last_query_usage: None,
                last_query_total_tokens: 0,
                last_context_occupancy: None,
                status: SessionRuntimeStatus::Idle,
                collaboration_mode: Default::default(),
                effective_context_window: None,
                permission_preset: None,
            },
            None,
        )
        .expect("seed sqlite title");

        rollout_store
            .index_rollout_metadata(&db)
            .expect("index rollout metadata");

        let index = db
            .get_session_index(&session_id)
            .expect("get index")
            .expect("indexed session");
        assert_eq!(
            index.metadata.title.as_deref(),
            Some("Canonical rollout title")
        );
    }

    #[test]
    fn index_rollout_metadata_reads_session_title_updates() {
        use chrono::Utc;
        use devo_core::SessionTitleFinalSource;
        use devo_core::SessionTitleState;
        use pretty_assertions::assert_eq;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let data_root = dir.path().to_path_buf();
        let session_id = SessionId::new();
        let now = Utc::now();
        let rollout_store = super::RolloutStore::new(data_root.clone(), None);
        let record = rollout_store.create_session_record(
            session_id,
            now,
            data_root.clone(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        rollout_store
            .append_title_update(
                &record,
                "Updated from rollout".into(),
                SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
                None,
            )
            .expect("append title update");

        let db = crate::db::Database::open(data_root.join("index.db")).expect("open db");
        rollout_store
            .index_rollout_metadata(&db)
            .expect("index rollout metadata");

        let index = db
            .get_session_index(&session_id)
            .expect("get index")
            .expect("indexed session");
        assert_eq!(
            index.metadata.title.as_deref(),
            Some("Updated from rollout")
        );
    }

    #[test]
    fn replay_omits_empty_agent_messages_from_history_and_prompt() {
        let mut messages = Vec::new();
        let mut history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();

        for item in [
            TurnItem::UserMessage(TextItem {
                text: "hello".to_string(),
            }),
            TurnItem::AgentMessage(TextItem {
                text: String::new(),
            }),
            TurnItem::AgentMessage(TextItem {
                text: "  \n\t".to_string(),
            }),
            TurnItem::AgentMessage(TextItem {
                text: "visible answer".to_string(),
            }),
        ] {
            apply_turn_item(
                &mut messages,
                &mut history_items,
                &mut tool_names_by_id,
                &TurnKind::Regular,
                item,
            );
        }

        assert_eq!(
            history_items
                .iter()
                .map(|item| (item.kind.clone(), item.body.clone()))
                .collect::<Vec<_>>(),
            vec![
                (crate::SessionHistoryItemKind::User, "hello".to_string()),
                (
                    crate::SessionHistoryItemKind::Assistant,
                    "visible answer".to_string(),
                ),
            ]
        );
        assert_eq!(
            messages,
            vec![
                Message::user("hello"),
                Message::assistant_text("visible answer"),
            ]
        );
    }

    #[test]
    fn replay_backfills_tool_result_name_from_prior_tool_call() {
        let mut messages = Vec::new();
        let mut history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();

        apply_turn_item(
            &mut messages,
            &mut history_items,
            &mut tool_names_by_id,
            &TurnKind::Regular,
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                input: serde_json::json!({"filePath":"/tmp/test.txt"}),
            }),
        );
        apply_turn_item(
            &mut messages,
            &mut history_items,
            &mut tool_names_by_id,
            &TurnKind::Regular,
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "call-1".to_string(),
                tool_name: None,
                output: serde_json::Value::String("hello".to_string()),
                display_content: None,
                is_error: false,
            }),
        );

        assert_eq!(history_items.len(), 2);
        assert_eq!(history_items[0].title, "read /tmp/test.txt");
        assert_eq!(history_items[1].title, "read output");
    }

    #[test]
    fn replay_nameless_edit_tool_result_still_emits_edited_metadata() {
        // edit is LiveOnly: start is not persisted, so resume only sees ToolResult
        // with tool_name lost by the v2 canonical schema. Structured output must
        // still produce Edited metadata for transcript restore.
        let mut messages = Vec::new();
        let mut history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();

        apply_turn_item(
            &mut messages,
            &mut history_items,
            &mut tool_names_by_id,
            &TurnKind::Regular,
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "call-edit".to_string(),
                tool_name: None,
                output: serde_json::json!({
                    "diff": "diff --git a/foo.txt b/foo.txt\n--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n",
                    "files": [{
                        "path": "foo.txt",
                        "kind": "update",
                        "diff": "--- a/foo.txt\n+++ b/foo.txt\n@@ -1 +1 @@\n-old\n+new\n",
                        "oldContent": "old\n",
                        "postContent": "new\n",
                        "additions": 1,
                        "deletions": 1
                    }],
                    "output": "edited foo.txt"
                }),
                display_content: Some("edited foo.txt".to_string()),
                is_error: false,
            }),
        );

        assert_eq!(history_items.len(), 1);
        assert_eq!(history_items[0].body, "edited foo.txt");
        let Some(devo_protocol::SessionHistoryMetadata::Edited { changes }) =
            &history_items[0].metadata
        else {
            panic!(
                "expected Edited metadata, got {:?}",
                history_items[0].metadata
            );
        };
        assert!(changes.contains_key(&PathBuf::from("foo.txt")));
    }

    #[test]
    fn replay_uses_display_content_for_history_but_canonical_output_for_prompt() {
        let mut messages = Vec::new();
        let mut history_items = Vec::new();
        let mut tool_names_by_id = HashMap::new();

        apply_turn_item(
            &mut messages,
            &mut history_items,
            &mut tool_names_by_id,
            &TurnKind::Regular,
            TurnItem::ToolCall(ToolCallItem {
                tool_call_id: "call-1".to_string(),
                tool_name: "read".to_string(),
                input: serde_json::json!({"filePath":"/tmp/test.txt"}),
            }),
        );
        apply_turn_item(
            &mut messages,
            &mut history_items,
            &mut tool_names_by_id,
            &TurnKind::Regular,
            TurnItem::ToolResult(ToolResultItem {
                tool_call_id: "call-1".to_string(),
                tool_name: Some("read".to_string()),
                output: serde_json::Value::String("<content>canonical</content>".to_string()),
                display_content: Some("canonical".to_string()),
                is_error: false,
            }),
        );

        assert_eq!(history_items[1].body, "canonical");
        assert_eq!(
            messages.last(),
            Some(&Message {
                role: devo_core::Role::User,
                content: vec![devo_core::ContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "<content>canonical</content>".to_string(),
                    is_error: false,
                }],
            })
        );
    }

    #[test]
    fn prompt_messages_rebuild_from_compaction_snapshot_without_trimming_transcript() {
        let summary_item_id = ItemId::new();
        let preserved_item_id = ItemId::new();
        let later_item_id = ItemId::new();

        let persisted_turn_items = vec![
            PersistedTurnItem {
                turn_id: TurnId::new(),
                turn_kind: TurnKind::Regular,
                item_id: ItemId::new(),
                turn_item: TurnItem::UserMessage(TextItem {
                    text: "older user".to_string(),
                }),
            },
            PersistedTurnItem {
                turn_id: TurnId::new(),
                turn_kind: TurnKind::Regular,
                item_id: summary_item_id,
                turn_item: TurnItem::ContextCompaction(TextItem {
                    text: "<compaction_summary>summary</compaction_summary>".to_string(),
                }),
            },
            PersistedTurnItem {
                turn_id: TurnId::new(),
                turn_kind: TurnKind::Regular,
                item_id: preserved_item_id,
                turn_item: TurnItem::UserMessage(TextItem {
                    text: "latest user".to_string(),
                }),
            },
            PersistedTurnItem {
                turn_id: TurnId::new(),
                turn_kind: TurnKind::Regular,
                item_id: later_item_id,
                turn_item: TurnItem::AgentMessage(TextItem {
                    text: "latest assistant".to_string(),
                }),
            },
        ];

        let prompt_messages = build_prompt_messages_from_snapshot(
            &persisted_turn_items,
            &CompactionSnapshotLine {
                timestamp: Utc::now(),
                session_id: SessionId::new(),
                turn_id: TurnId::new(),
                summary_item_id,
                preserved_item_ids: vec![preserved_item_id],
                context_occupancy: None,
            },
        )
        .expect("prompt messages");

        assert_eq!(
            prompt_messages,
            vec![
                Message::assistant_text("<compaction_summary>summary</compaction_summary>"),
                Message::user("latest user"),
                Message::assistant_text("latest assistant"),
            ]
        );
    }

    #[test]
    fn replay_restores_context_snapshots_from_turn_records() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 8, 0, 0).unwrap();
        let session_context = SessionContext {
            base_instructions: "base".into(),
            available_skills: None,
            workspace_instructions: Some("workspace".into()),
            locked_agents_snapshot: None,
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/root"),
                shell: "bash".into(),
                current_date: "2026-04-27".into(),
                timezone: "UTC".into(),
            },
            language: LanguageContext::default(),
            persona: Persona::Default,
            model: Model {
                slug: "model-a".into(),
                ..Model::default()
            },
            reasoning_effort_selection: None,
            reasoning_effort: None,
            system_prompt_mode: devo_core::SystemPromptMode::CodingAgent,
        };
        let turn_context = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/next"),
                shell: "bash".into(),
                current_date: "2026-04-28".into(),
                timezone: "UTC".into(),
            },
            persona: Persona::Default,
            model: Model {
                slug: "model-b".into(),
                ..Model::default()
            },
            reasoning_effort_selection: Some("enabled".into()),
            reasoning_effort: None,
            observed_agents_snapshot: None,
            collaboration_mode: devo_core::CollaborationMode::Build,
        };
        let mut replay = ReplayState::default();

        replay
            .apply_line(RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: now,
                session: SessionRecord {
                    id: session_id,
                    rollout_path: PathBuf::from("rollout.jsonl"),
                    created_at: now,
                    updated_at: now,
                    last_activity_at: Some(now),
                    source: "cli".into(),
                    agent_nickname: None,
                    agent_role: None,
                    agent_path: None,
                    model_provider: "test".into(),
                    model: Some("model-a".into()),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    cwd: PathBuf::from("/tmp/root"),
                    additional_directories: Vec::new(),
                    cli_version: "0.1.0".into(),
                    title: None,
                    title_state: SessionTitleState::Unset,
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
                },
            })))
            .expect("apply session meta");
        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: turn_id,
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: devo_core::TurnKind::Regular,
                    model: "model-b".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: Some("enabled".into()),
                    request_model: "model-b".into(),
                    request_thinking: Some("enabled".into()),
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: Some(session_context.clone()),
                    turn_context: Some(turn_context.clone()),
                    schema_version: 2,
                },
            })))
            .expect("apply turn line");

        assert_eq!(replay.session_context, Some(session_context));
        assert_eq!(replay.latest_turn_context, Some(turn_context));
        assert!(replay.session_context_recorded);
    }

    fn sample_session_context(base_instructions: &str) -> SessionContext {
        SessionContext {
            base_instructions: base_instructions.into(),
            available_skills: None,
            workspace_instructions: Some("workspace".into()),
            locked_agents_snapshot: None,
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/root"),
                shell: "bash".into(),
                current_date: "2026-04-27".into(),
                timezone: "UTC".into(),
            },
            language: LanguageContext::default(),
            persona: Persona::Default,
            model: Model {
                slug: "model-a".into(),
                ..Model::default()
            },
            reasoning_effort_selection: None,
            reasoning_effort: None,
            system_prompt_mode: devo_core::SystemPromptMode::CodingAgent,
        }
    }

    #[test]
    fn replay_restores_context_from_session_context_updated_line() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 8, 0, 0).unwrap();
        let session_context = sample_session_context("base");
        let turn_context = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/next"),
                shell: "bash".into(),
                current_date: "2026-04-28".into(),
                timezone: "UTC".into(),
            },
            persona: Persona::Default,
            model: Model {
                slug: "model-b".into(),
                ..Model::default()
            },
            reasoning_effort_selection: Some("enabled".into()),
            reasoning_effort: None,
            observed_agents_snapshot: None,
            collaboration_mode: devo_core::CollaborationMode::Build,
        };
        let mut replay = ReplayState::default();

        replay
            .apply_line(RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: now,
                session: SessionRecord {
                    id: session_id,
                    rollout_path: PathBuf::from("rollout.jsonl"),
                    created_at: now,
                    updated_at: now,
                    last_activity_at: Some(now),
                    source: "cli".into(),
                    agent_nickname: None,
                    agent_role: None,
                    agent_path: None,
                    model_provider: "test".into(),
                    model: Some("model-a".into()),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    cwd: PathBuf::from("/tmp/root"),
                    additional_directories: Vec::new(),
                    cli_version: "0.1.0".into(),
                    title: None,
                    title_state: SessionTitleState::Unset,
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
                },
            })))
            .expect("apply session meta");
        replay
            .apply_line(RolloutLine::SessionContextUpdated(Box::new(
                devo_core::SessionContextUpdatedLine {
                    timestamp: now,
                    session_id,
                    session_context: session_context.clone(),
                    schema_version: 1,
                },
            )))
            .expect("apply session context");
        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: turn_id,
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: devo_core::TurnKind::Regular,
                    model: "model-b".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: Some("enabled".into()),
                    request_model: "model-b".into(),
                    request_thinking: Some("enabled".into()),
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: Some(turn_context.clone()),
                    schema_version: 2,
                },
            })))
            .expect("apply turn line");

        assert_eq!(replay.session_context, Some(session_context));
        assert_eq!(replay.latest_turn_context, Some(turn_context));
        assert!(replay.session_context_recorded);
    }

    #[test]
    fn replay_preserves_session_context_updated_across_rollback() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let now = Utc.with_ymd_and_hms(2026, 4, 27, 8, 0, 0).unwrap();
        let session_context = sample_session_context("base");
        let turn_context = TurnContext {
            environment: EnvironmentContext {
                cwd: PathBuf::from("/tmp/next"),
                shell: "bash".into(),
                current_date: "2026-04-28".into(),
                timezone: "UTC".into(),
            },
            persona: Persona::Default,
            model: Model {
                slug: "model-b".into(),
                ..Model::default()
            },
            reasoning_effort_selection: Some("enabled".into()),
            reasoning_effort: None,
            observed_agents_snapshot: None,
            collaboration_mode: devo_core::CollaborationMode::Build,
        };
        let mut replay = ReplayState::default();

        replay
            .apply_line(RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: now,
                session: SessionRecord {
                    id: session_id,
                    rollout_path: PathBuf::from("rollout.jsonl"),
                    created_at: now,
                    updated_at: now,
                    last_activity_at: Some(now),
                    source: "cli".into(),
                    agent_nickname: None,
                    agent_role: None,
                    agent_path: None,
                    model_provider: "test".into(),
                    model: Some("model-a".into()),
                    model_binding_id: None,
                    reasoning_effort_selection: None,
                    cwd: PathBuf::from("/tmp/root"),
                    additional_directories: Vec::new(),
                    cli_version: "0.1.0".into(),
                    title: None,
                    title_state: SessionTitleState::Unset,
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
                },
            })))
            .expect("apply session meta");
        replay
            .apply_line(RolloutLine::SessionContextUpdated(Box::new(
                devo_core::SessionContextUpdatedLine {
                    timestamp: now,
                    session_id,
                    session_context: session_context.clone(),
                    schema_version: 1,
                },
            )))
            .expect("apply session context");
        replay
            .apply_line(RolloutLine::Turn(Box::new(TurnLine {
                timestamp: now,
                turn: TurnRecord {
                    id: turn_id,
                    session_id,
                    sequence: 1,
                    started_at: now,
                    completed_at: Some(now),
                    status: TurnStatus::Completed,
                    kind: TurnKind::Regular,
                    model: "model-b".into(),
                    model_binding_id: None,
                    reasoning_effort_selection: Some("enabled".into()),
                    request_model: "model-b".into(),
                    request_thinking: Some("enabled".into()),
                    input_token_estimate: None,
                    usage: None,
                    latest_query_usage: None,
                    context_occupancy: None,
                    stop_reason: None,
                    failure_reason: None,
                    error: None,
                    session_context: None,
                    turn_context: Some(turn_context),
                    schema_version: 2,
                },
            })))
            .expect("apply turn line");
        replay
            .apply_line(RolloutLine::SessionRollback(Box::new(
                SessionRollbackLine {
                    timestamp: now,
                    session_id,
                    retained_turn_ids: Vec::new(),
                    retained_item_ids: Vec::new(),
                    latest_turn_id: None,
                    schema_version: 1,
                },
            )))
            .expect("apply rollback");

        assert_eq!(replay.session_context, Some(session_context));
        assert!(replay.session_context_recorded);
        assert!(replay.latest_turn.is_none());
    }

    #[test]
    fn append_turn_deduped_writes_session_context_once() {
        use crate::turn::TurnMetadata;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let data_root = dir.path().to_path_buf();
        let session_id = SessionId::new();
        let now = Utc::now();
        let rollout_store = super::RolloutStore::new(data_root.clone(), None);
        let record = rollout_store.create_session_record(
            session_id,
            now,
            data_root.clone(),
            Vec::new(),
            Some("dedupe test".into()),
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");

        let session_context = sample_session_context("unique-base-instruction-marker");
        let mut session_context_recorded = false;
        let turn_metadata = |sequence: u32| TurnMetadata {
            turn_id: TurnId::new(),
            session_id,
            sequence,
            status: TurnStatus::Completed,
            kind: TurnKind::Regular,
            model: "test-model".into(),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            request_model: "test-model".into(),
            request_thinking: None,
            started_at: now,
            completed_at: Some(now),
            usage: None,
            stop_reason: None,
            failure_reason: None,
        };

        for sequence in 1..=2 {
            let metadata = turn_metadata(sequence);
            rollout_store
                .append_turn_deduped(
                    &record,
                    &mut session_context_recorded,
                    super::build_turn_record(&metadata, None, None, None, None),
                    Some(session_context.clone()),
                )
                .expect("append deduped turn");
        }

        assert!(session_context_recorded);
        let rollout = std::fs::read_to_string(&record.rollout_path).expect("read rollout");
        assert_eq!(rollout.matches("unique-base-instruction-marker").count(), 1);
        // v2 write path: the locked context travels as an internal line.
        assert!(rollout.contains("\"sessionContext\""));
    }

    // ── v2 write switch / dual read (P3b) ─────────────────────────────

    struct NoopProvider;

    #[async_trait::async_trait]
    impl devo_provider::ModelProviderSDK for NoopProvider {
        async fn completion(
            &self,
            _request: devo_protocol::ModelRequest,
        ) -> anyhow::Result<devo_protocol::ModelResponse> {
            anyhow::bail!("noop provider does not support completion")
        }

        async fn completion_stream(
            &self,
            _request: devo_protocol::ModelRequest,
        ) -> anyhow::Result<
            std::pin::Pin<
                Box<dyn futures::Stream<Item = anyhow::Result<devo_protocol::StreamEvent>> + Send>,
            >,
        > {
            anyhow::bail!("noop provider does not support streaming")
        }

        fn name(&self) -> &str {
            "noop-provider"
        }
    }

    fn test_deps(data_root: &std::path::Path) -> ServerRuntimeDependencies {
        let provider: Arc<dyn devo_provider::ModelProviderSDK> = Arc::new(NoopProvider);
        ServerRuntimeDependencies::new(
            Arc::clone(&provider),
            Arc::new(devo_provider::SingleProviderRouter::new(provider)),
            Arc::new(devo_core::tools::ToolRegistry::new()),
            crate::empty_mcp_manager(),
            "test-model".to_string(),
            Arc::new(devo_core::PresetModelCatalog::default()),
            Arc::new(devo_core::ProviderVendorCatalog::default()),
            Box::new(devo_core::FileSystemSkillCatalog::new(
                devo_core::SkillsConfig {
                    bundled: Some(devo_core::BundledSkillsConfig { enabled: false }),
                    ..devo_core::SkillsConfig::default()
                },
            )),
            devo_core::AgentsMdConfig::default(),
            Arc::new(
                crate::db::Database::open(data_root.join("test.db")).expect("open test database"),
            ),
            Arc::new(std::sync::Mutex::new(
                devo_core::AppConfigStore::load(data_root.to_path_buf(), None)
                    .expect("load app config store"),
            )),
        )
    }

    fn write_raw_lines(path: &std::path::Path, raw_lines: &[String]) {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create rollout directory");
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open rollout for raw append");
        for raw in raw_lines {
            file.write_all(raw.as_bytes()).expect("write raw line");
            file.write_all(b"\n").expect("write newline");
        }
    }

    fn raw_rollout_lines(path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("read rollout")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn test_turn_metadata(session_id: SessionId, turn_id: TurnId) -> crate::turn::TurnMetadata {
        crate::turn::TurnMetadata {
            turn_id,
            session_id,
            sequence: 1,
            status: TurnStatus::Completed,
            kind: TurnKind::Regular,
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
        }
    }

    #[test]
    fn write_path_appends_only_v2_lines() {
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        let metadata = test_turn_metadata(record.id, TurnId::new());
        let turn = super::build_turn_record(&metadata, None, None, None, None);
        rollout_store
            .append_turn(&record, turn)
            .expect("append turn");
        let item = super::build_item_record(
            record.id,
            metadata.turn_id,
            ItemId::new(),
            1,
            TurnItem::AgentMessage(TextItem { text: "hi".into() }),
            Some(TurnStatus::Running),
            None,
        );
        rollout_store
            .append_item(&record, item)
            .expect("append item");

        let raw_lines = raw_rollout_lines(&record.rollout_path);
        assert_eq!(raw_lines.len(), 3);
        for raw in &raw_lines {
            assert!(raw.contains("\"v\":2"), "line is v2: {raw}");
            match parse_rollout_line(raw).expect("line parses") {
                ParsedRolloutLine::V2(_) => {}
                ParsedRolloutLine::Legacy(_) => panic!("freshly written line parsed as legacy"),
            }
        }
    }

    #[test]
    fn hydration_folds_approval_decision_onto_request_across_restart() {
        use devo_core::ApprovalDecisionItem;
        use devo_core::ApprovalRequestItem;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        let metadata = test_turn_metadata(record.id, TurnId::new());
        let turn = super::build_turn_record(&metadata, None, None, None, None);
        rollout_store
            .append_turn(&record, turn)
            .expect("append turn");
        let request_record_id = ItemId::new();
        rollout_store
            .append_item(
                &record,
                super::build_item_record(
                    record.id,
                    metadata.turn_id,
                    request_record_id,
                    1,
                    TurnItem::ApprovalRequest(ApprovalRequestItem {
                        approval_id: "appr-1".into(),
                        action_summary: "Run ls".into(),
                        justification: "listing".into(),
                        resource: Some("ShellExec".into()),
                        available_scopes: vec!["once".into()],
                        command_pattern: None,
                        command_prefix: None,
                        path: None,
                        host: None,
                        target: Some("ls".into()),
                    }),
                    Some(TurnStatus::Running),
                    None,
                ),
            )
            .expect("append approval request");

        // "Restart": a brand-new store must hydrate its projector from the
        // on-disk v2 history before appending.
        let restarted_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        restarted_store
            .append_item(
                &record,
                super::build_item_record(
                    record.id,
                    metadata.turn_id,
                    ItemId::new(),
                    2,
                    TurnItem::ApprovalDecision(ApprovalDecisionItem {
                        approval_id: "appr-1".into(),
                        decision: "approve".into(),
                        scope: "once".into(),
                        decision_source: None,
                    }),
                    Some(TurnStatus::Running),
                    None,
                ),
            )
            .expect("append approval decision");

        let approvals: Vec<devo_core::RolloutLineV2> = raw_rollout_lines(&record.rollout_path)
            .iter()
            .map(|raw| match parse_rollout_line(raw).expect("line parses") {
                ParsedRolloutLine::V2(line) => *line,
                ParsedRolloutLine::Legacy(_) => panic!("line parsed as legacy"),
            })
            .filter(|line| {
                matches!(
                    line,
                    devo_core::RolloutLineV2::Item { item, .. }
                        if matches!(item.item, devo_protocol::native::item::Item::Approval { .. })
                )
            })
            .collect();
        assert_eq!(approvals.len(), 2);
        let devo_core::RolloutLineV2::Item { item: request, .. } = &approvals[0] else {
            panic!("request line");
        };
        let devo_core::RolloutLineV2::Item { item: decision, .. } = &approvals[1] else {
            panic!("decision line");
        };
        // The decision folded onto the request's item id and seq — not an
        // orphan Warning with a fresh id.
        assert_eq!(request.id.as_str(), request_record_id.to_string());
        assert_eq!(decision.id, request.id);
        assert_eq!(decision.seq, request.seq);
        assert_eq!((request.revision, decision.revision), (1, 2));
        assert_eq!(
            decision.state,
            devo_protocol::native::item::ItemState::Completed
        );
        assert!(
            matches!(&decision.item, devo_protocol::native::item::Item::Approval { decision: Some(d), .. }
                if d.decision == devo_protocol::native::item::ApprovalDecisionKind::Approved
                    && d.scope == devo_protocol::native::item::ApprovalScope::Once)
        );
    }

    #[test]
    fn canonical_only_interaction_and_file_change_items_round_trip() {
        use devo_protocol::native::ids::{
            ItemId as CanonicalItemId, SessionId as CanonicalSessionId, TurnId as CanonicalTurnId,
        };
        use devo_protocol::native::item::{
            FileChangeEntry, FileChangeKind, Item, ItemEnvelope, ItemState,
        };
        use tempfile::TempDir;
        use uuid::Uuid;

        let dir = TempDir::new().expect("temp dir");
        let store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        store
            .append_session_meta(&record)
            .expect("append session meta");
        let turn_id = TurnId::new();
        let metadata = test_turn_metadata(record.id, turn_id);
        store
            .append_turn(
                &record,
                super::build_turn_record(&metadata, None, None, None, None),
            )
            .expect("append turn");
        let now = Utc::now();
        let session_id = CanonicalSessionId::from_legacy_uuid(Uuid::from(record.id));
        let turn_id = CanonicalTurnId::from_legacy_uuid(Uuid::from(turn_id));
        let question_item_id = CanonicalItemId::from_legacy_uuid(Uuid::now_v7());
        let waiting = ItemEnvelope {
            id: question_item_id.clone(),
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            seq: 1,
            revision: 1,
            created_at: now,
            updated_at: now,
            state: ItemState::Waiting,
            item: Item::UserInputRequest {
                request_id: "question-1".into(),
                target_item_id: None,
                questions: Vec::new(),
                answers: None,
            },
        };
        let file_change = ItemEnvelope {
            id: CanonicalItemId::from_legacy_uuid(Uuid::now_v7()),
            session_id,
            turn_id,
            seq: 2,
            revision: 1,
            created_at: now,
            updated_at: now,
            state: ItemState::Completed,
            item: Item::FileChange {
                call_id: "edit-1".into(),
                changes: vec![FileChangeEntry {
                    path: PathBuf::from("src/lib.rs"),
                    change: FileChangeKind::Update {
                        unified_diff: "@@ -1 +1 @@".into(),
                        move_path: None,
                    },
                }],
                sandbox: None,
            },
        };
        store
            .append_canonical_item(&record, waiting.clone())
            .expect("append waiting item");
        store
            .append_canonical_item(&record, file_change.clone())
            .expect("append file change");

        let history =
            devo_core::read_canonical_history(&record.rollout_path).expect("read history");
        assert_eq!(history.items, vec![waiting, file_change]);
    }

    #[test]
    fn hydration_fails_closed_on_damaged_history() {
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        write_raw_lines(
            &record.rollout_path,
            &[r#"{"v":2,"kind":"nope"}"#.to_string()],
        );

        let restarted_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let metadata = test_turn_metadata(record.id, TurnId::new());
        let turn = super::build_turn_record(&metadata, None, None, None, None);
        let error = restarted_store
            .append_turn(&record, turn)
            .expect_err("append onto damaged history must fail");
        assert!(
            format!("{error:#}").contains("refusing to append"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn hydration_tolerates_truncated_final_line() {
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        write_raw_lines(
            &record.rollout_path,
            &[r#"{"v":2,"kind":"item","timestamp":"2026"#.to_string()],
        );

        let restarted_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let metadata = test_turn_metadata(record.id, TurnId::new());
        let turn = super::build_turn_record(&metadata, None, None, None, None);
        restarted_store
            .append_turn(&record, turn)
            .expect("crash tail is tolerated");
    }

    #[tokio::test]
    async fn dual_read_fails_closed_on_mid_file_damage_but_tolerates_crash_tail() {
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let deps = test_deps(dir.path());
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        // Damaged middle line, then a valid line after it.
        write_raw_lines(
            &record.rollout_path,
            &[r#"{"v":2,"kind":"nope"}"#.to_string()],
        );
        let metadata = test_turn_metadata(record.id, TurnId::new());
        let turn = super::build_turn_record(&metadata, None, None, None, None);
        rollout_store
            .append_turn(&record, turn)
            .expect("append turn");

        let error = rollout_store
            .load_session_from_rollout(&record.rollout_path, &deps)
            .await
            .err()
            .expect("damaged mid-file line must fail the load");
        assert!(
            format!("{error:#}").contains("refusing to resume"),
            "unexpected error: {error:#}"
        );

        // A truncated final line (crash tail) is tolerated instead.
        let tail_record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        rollout_store
            .append_session_meta(&tail_record)
            .expect("append session meta");
        write_raw_lines(
            &tail_record.rollout_path,
            &[r#"{"v":2,"kind":"item","timestamp":"2026"#.to_string()],
        );
        let recovered = rollout_store
            .load_session_from_rollout(&tail_record.rollout_path, &deps)
            .await
            .expect("crash tail is tolerated");
        assert_eq!(recovered.summary.session_id, tail_record.id);
    }

    #[tokio::test]
    async fn mixed_v1_v2_file_resumes_with_reconciled_next_seq() {
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let deps = test_deps(dir.path());
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            Some("Mixed session".into()),
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        let metadata = test_turn_metadata(record.id, TurnId::new());

        // v1 portion: hand-written legacy lines, as produced before the v2
        // write switch (never rewritten afterwards).
        let legacy_lines = [
            RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: Utc::now(),
                session: record.clone(),
            })),
            RolloutLine::Turn(Box::new(TurnLine {
                timestamp: Utc::now(),
                turn: super::build_turn_record(&metadata, None, None, None, None),
            })),
            RolloutLine::Item(ItemLine {
                timestamp: Utc::now(),
                item: super::build_item_record(
                    record.id,
                    metadata.turn_id,
                    ItemId::new(),
                    1,
                    TurnItem::UserMessage(TextItem {
                        text: "legacy hello".into(),
                    }),
                    Some(TurnStatus::Running),
                    None,
                ),
            }),
        ];
        write_raw_lines(
            &record.rollout_path,
            &legacy_lines
                .iter()
                .map(|line| serde_json::to_string(line).expect("serialize legacy line"))
                .collect::<Vec<_>>(),
        );

        // The v2 write path appends onto the legacy file (hydrating first).
        rollout_store
            .append_item(
                &record,
                super::build_item_record(
                    record.id,
                    metadata.turn_id,
                    ItemId::new(),
                    2,
                    TurnItem::AgentMessage(TextItem {
                        text: "v2 reply".into(),
                    }),
                    Some(TurnStatus::Running),
                    None,
                ),
            )
            .expect("append v2 item");

        // The file mixes both formats; every line dispatches cleanly.
        let raw_lines = raw_rollout_lines(&record.rollout_path);
        assert_eq!(raw_lines.len(), 4);
        for (index, raw) in raw_lines.iter().enumerate() {
            let parsed = parse_rollout_line(raw).expect("line dispatches");
            if index < 3 {
                assert!(
                    matches!(parsed, ParsedRolloutLine::Legacy(_)),
                    "line {index} must be legacy"
                );
            } else {
                assert!(
                    matches!(parsed, ParsedRolloutLine::V2(_)),
                    "line {index} must be v2"
                );
            }
        }

        // Dual read: the resumed session holds the union of both histories,
        // and the next runtime seq matches the write-path projector's (the
        // v1 item took seq 1, the v2 item seq 2 — no collision).
        let recovered = rollout_store
            .load_session_from_rollout(&record.rollout_path, &deps)
            .await
            .expect("mixed file resumes");
        assert_eq!(recovered.summary.session_id, record.id);
        assert_eq!(recovered.summary.title.as_deref(), Some("Mixed session"));
        assert_eq!(recovered.loaded_item_count, 2);
        assert_eq!(recovered.next_item_seq, 3);
        let texts: Vec<&str> = recovered
            .history_items
            .iter()
            .map(|item| item.body.as_str())
            .collect();
        assert!(texts.contains(&"legacy hello"), "history: {texts:?}");
        assert!(texts.contains(&"v2 reply"), "history: {texts:?}");
    }

    fn append_basic_session_lines(
        rollout_store: &super::RolloutStore,
        data_root: &std::path::Path,
    ) -> devo_core::SessionRecord {
        let record = rollout_store.create_session_record(
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
        rollout_store
            .append_session_meta(&record)
            .expect("append session meta");
        let metadata = test_turn_metadata(record.id, TurnId::new());
        let turn = super::build_turn_record(&metadata, None, None, None, None);
        rollout_store
            .append_turn(&record, turn)
            .expect("append turn");
        let item = super::build_item_record(
            record.id,
            metadata.turn_id,
            ItemId::new(),
            1,
            TurnItem::AgentMessage(TextItem { text: "hi".into() }),
            Some(TurnStatus::Running),
            None,
        );
        rollout_store
            .append_item(&record, item)
            .expect("append item");
        record
    }

    #[test]
    fn append_projects_events_into_event_log() {
        use pretty_assertions::assert_eq;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let db = std::sync::Arc::new(
            crate::db::Database::open(dir.path().join("devo.db")).expect("open db"),
        );
        let rollout_store =
            super::RolloutStore::new(dir.path().to_path_buf(), Some(std::sync::Arc::clone(&db)));
        let record = append_basic_session_lines(&rollout_store, dir.path());

        // session/created lands on both the session stream and the per-cwd
        // sessions stream; turn and item facts land on the session stream.
        assert_eq!(db.event_log_len().expect("count"), 4);
        let session_stream = devo_core::session_stream_id(
            &devo_protocol::native::ids::SessionId::from_string(record.id.to_string()),
        );
        let rows = db
            .event_log_rows(&session_stream, 0)
            .expect("session stream");
        let kinds: Vec<&str> = rows.iter().map(|row| row.event_kind.as_str()).collect();
        assert_eq!(
            kinds,
            vec!["session/created", "turn/completed", "item/completed"]
        );
        let seqs: Vec<u64> = rows.iter().map(|row| row.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        // Three physical rows written; watermark is the last line index.
        assert_eq!(
            db.projection_watermark(&record.rollout_path)
                .expect("watermark"),
            Some(2)
        );

        // The stored envelope payload parses as a typed EventEnvelope whose
        // meta.seq is hydrated from the row at replay time.
        let envelope: devo_protocol::native::event::EventEnvelope =
            serde_json::from_str(&rows[2].payload).expect("envelope payload parses");
        assert_eq!(envelope.meta.seq, None);
        assert!(envelope.meta.persisted);
    }

    #[test]
    fn event_log_insert_is_idempotent_by_source_fact() {
        use pretty_assertions::assert_eq;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        let db = std::sync::Arc::new(
            crate::db::Database::open(dir.path().join("devo.db")).expect("open db"),
        );
        let rollout_store =
            super::RolloutStore::new(dir.path().to_path_buf(), Some(std::sync::Arc::clone(&db)));
        let record = append_basic_session_lines(&rollout_store, dir.path());
        assert_eq!(db.event_log_len().expect("count"), 4);

        // Re-deriving the same facts (simulated crash recovery) inserts nothing.
        let raw_lines = raw_rollout_lines(&record.rollout_path);
        let mut rows = Vec::new();
        for (index, raw) in raw_lines.iter().enumerate() {
            let ParsedRolloutLine::V2(v2) = parse_rollout_line(raw).expect("parse") else {
                panic!("v2 line expected");
            };
            rows.extend(
                super::event_log_rows_for_v2_line(&record.rollout_path, index as u64, 0, &v2)
                    .expect("derive rows"),
            );
        }
        assert_eq!(rows.len(), 4);
        let inserted = db.insert_event_log_rows(&rows).expect("re-insert");
        assert_eq!(inserted, 0);
        assert_eq!(db.event_log_len().expect("count"), 4);
    }

    // ── Field-level session settings log (L2-DES-CONV-002 Phase 1) ──

    fn settings_test_record() -> (tempfile::TempDir, super::RolloutStore, SessionRecord) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let rollout_store = super::RolloutStore::new(dir.path().to_path_buf(), None);
        let record = rollout_store.create_session_record(
            SessionId::new(),
            Utc::now(),
            dir.path().to_path_buf(),
            Vec::new(),
            None,
            Some("test-model".into()),
            None,
            None,
            "test-provider".into(),
            None,
        );
        (dir, rollout_store, record)
    }

    fn settings_line(
        record: &SessionRecord,
        field: devo_core::SessionSettingsField,
        value: serde_json::Value,
    ) -> RolloutLine {
        RolloutLine::SessionSettings(devo_core::SessionSettingsLine {
            timestamp: Utc::now(),
            session_id: record.id,
            field,
            value,
            epoch: 0,
        })
    }

    /// Trace: L2-DES-CONV-002
    /// Verifies: a field-level settings line wins over the whole-record
    /// SessionMeta value during replay (DD-4).
    #[test]
    fn session_settings_field_line_wins_over_session_meta_preset() {
        let (_dir, _store, mut record) = settings_test_record();
        record.permission_preset = Some(devo_protocol::PermissionPreset::Default);
        let mut replay = ReplayState::default();
        replay
            .apply_line(RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: Utc::now(),
                session: record.clone(),
            })))
            .expect("apply session meta");
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::PermissionPreset,
                serde_json::to_value(devo_protocol::PermissionPreset::FullAccess)
                    .expect("serialize preset"),
            ))
            .expect("apply settings line");

        let mut replayed = replay.session.take().expect("session record");
        replay.apply_session_settings(&mut replayed);
        assert_eq!(
            replayed.permission_preset,
            Some(devo_protocol::PermissionPreset::FullAccess)
        );
    }

    /// Trace: L2-DES-CONV-002
    /// Verifies: a PermissionPreset line clears the explicit SandboxProfile
    /// override accumulated so far (approved patch-interaction rule).
    #[test]
    fn session_settings_preset_line_clears_explicit_sandbox_override() {
        let (_dir, _store, record) = settings_test_record();
        let mut replay = ReplayState::default();
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::SandboxProfile,
                serde_json::Value::String("strict".into()),
            ))
            .expect("apply sandbox line");
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::PermissionPreset,
                serde_json::to_value(devo_protocol::PermissionPreset::Default)
                    .expect("serialize preset"),
            ))
            .expect("apply preset line");

        assert_eq!(replay.sandbox_profile_override(), None);
    }

    /// Trace: L2-DES-CONV-002
    /// Verifies: an explicit SandboxProfile line written after the preset line
    /// survives replay as the effective override.
    #[test]
    fn session_settings_explicit_sandbox_survives_when_written_after_preset() {
        let (_dir, _store, record) = settings_test_record();
        let mut replay = ReplayState::default();
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::PermissionPreset,
                serde_json::to_value(devo_protocol::PermissionPreset::Default)
                    .expect("serialize preset"),
            ))
            .expect("apply preset line");
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::SandboxProfile,
                serde_json::Value::String("strict".into()),
            ))
            .expect("apply sandbox line");

        assert_eq!(
            replay.sandbox_profile_override(),
            Some("strict".to_string())
        );
    }

    /// Trace: L2-DES-CONV-002
    /// Verifies: model-family field lines override the corresponding record
    /// fields during replay.
    #[test]
    fn session_settings_model_fields_override_record() {
        let (_dir, _store, mut record) = settings_test_record();
        record.model = Some("old-model".into());
        let mut replay = ReplayState::default();
        replay
            .apply_line(RolloutLine::SessionMeta(Box::new(SessionMetaLine {
                timestamp: Utc::now(),
                session: record.clone(),
            })))
            .expect("apply session meta");
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::Model,
                serde_json::to_value(Some("new-model".to_string())).expect("serialize model"),
            ))
            .expect("apply model line");
        replay
            .apply_line(settings_line(
                &record,
                devo_core::SessionSettingsField::ReasoningEffortSelection,
                serde_json::to_value(Some("high".to_string())).expect("serialize effort"),
            ))
            .expect("apply effort line");

        let mut replayed = replay.session.take().expect("session record");
        replay.apply_session_settings(&mut replayed);
        assert_eq!(replayed.model, Some("new-model".to_string()));
        assert_eq!(
            replayed.reasoning_effort_selection,
            Some("high".to_string())
        );
    }

    /// Trace: L2-DES-CONV-002
    /// Verifies: a settings line written through RolloutStore lands on disk as
    /// a v2 Internal SessionSettings record and inverse-projects back to the
    /// same legacy line (write path + both projectors).
    #[test]
    fn session_settings_line_roundtrips_through_store_and_projectors() {
        let (_dir, store, record) = settings_test_record();
        store.append_session_meta(&record).expect("append meta");
        store
            .append_session_settings_at(
                &record.rollout_path,
                record.id,
                devo_core::SessionSettingsField::SandboxProfile,
                serde_json::Value::String("workspace".into()),
            )
            .expect("append settings line");

        let raw_lines = std::fs::read_to_string(&record.rollout_path)
            .expect("read rollout")
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(raw_lines.len(), 2);
        let ParsedRolloutLine::V2(v2) =
            parse_rollout_line(raw_lines.last().expect("settings raw line")).expect("parse")
        else {
            panic!("settings line must parse as v2");
        };
        let devo_core::rollout_v2::RolloutLineV2::Internal { entry, .. } = &*v2 else {
            panic!("settings line must be a v2 Internal record");
        };
        assert_eq!(
            entry,
            &devo_core::InternalRecordV2::SessionSettings {
                schema_version: 1,
                field: devo_core::SessionSettingsField::SandboxProfile,
                value: serde_json::Value::String("workspace".into()),
                epoch: 1,
            }
        );

        let inverse = devo_core::V2InverseProjector::new();
        let legacy_lines = inverse.project_line(&v2).expect("inverse project");
        let [RolloutLine::SessionSettings(line)] = legacy_lines.as_slice() else {
            panic!("inverse must yield exactly one SessionSettings legacy line");
        };
        assert_eq!(line.session_id, record.id);
        assert_eq!(line.field, devo_core::SessionSettingsField::SandboxProfile);
        assert_eq!(line.value, serde_json::Value::String("workspace".into()));
    }
}
