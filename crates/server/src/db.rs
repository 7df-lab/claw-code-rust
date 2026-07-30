use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use rusqlite::{Connection, params, types::Type};
use serde_json;

use devo_protocol::{
    PendingInputId, PendingInputItem, PendingInputKind, SessionId, SessionMetadata,
    SessionRuntimeStatus, SessionTitleState,
};

/// Queue type for pending messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueType {
    /// Pending turn inputs (from turn/start while a turn is active).
    Turn,
    /// Inputs injected into the active turn by `turn/steer`.
    Steer,
}

impl QueueType {
    fn as_str(&self) -> &'static str {
        match self {
            QueueType::Turn => "turn",
            QueueType::Steer => "steer",
        }
    }
}

/// SQLite index row used for lazy session list and resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIndexRecord {
    pub metadata: SessionMetadata,
    pub rollout_path: Option<PathBuf>,
}

/// Source of a session metadata upsert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionUpsertSource {
    /// Live runtime writes should preserve existing non-null fields when the update omits them.
    RuntimeLive,
    /// Rollout index writes rebuild SQLite from canonical rollout metadata.
    RolloutIndex,
}

/// Session-level token statistics.
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub total_tokens: usize,
    pub total_cache_creation_tokens: usize,
    pub total_cache_read_tokens: usize,
    pub last_input_tokens: usize,
    pub turn_count: usize,
    pub prompt_token_estimate: usize,
}

/// One derived event row before per-stream sequencing (08 §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEventLogRow {
    /// Stable identity of the rollout fact: `<rollout_path>#<line_index>`.
    pub source_fact_id: String,
    /// Notification method, e.g. `item/started`.
    pub event_kind: String,
    pub stream_id: String,
    pub event_id: String,
    /// `EventEnvelope` JSON (meta + notification).
    pub payload: String,
    pub created_at: String,
}

/// A stored event row including its per-stream sequence number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLogRow {
    pub source_fact_id: String,
    pub event_kind: String,
    pub stream_id: String,
    pub event_id: String,
    pub seq: u64,
    pub payload: String,
    pub created_at: String,
}

/// Current index schema version recorded in `schema_meta` (05 §2.3).
/// Bump when a migration changes the index layout; the rollout files are
/// the rebuildable source of truth on any mismatch.
/// v2: adds `event_log` + `projection_watermark` (08 §5/§7).
/// v3: renames the persisted active-turn steer queue from `btw` to `steer`.
const CURRENT_SCHEMA_VERSION: u32 = 3;

/// SQLite database for session metadata, token stats, and pending queues.
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Opens or creates the SQLite database at the given path.
    pub fn open(db_path: PathBuf) -> Result<Self> {
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open database at {}", db_path.display()))?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Runs schema migrations.
    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT,
                title_state TEXT NOT NULL DEFAULT 'unset',
                model TEXT,
                thinking TEXT,
                cwd TEXT NOT NULL,
                additional_directories TEXT NOT NULL DEFAULT '[]',
                ephemeral INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                last_activity_at INTEGER NOT NULL DEFAULT 0,
                schema_version INTEGER NOT NULL DEFAULT 2
            );

            CREATE TABLE IF NOT EXISTS session_stats (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                total_input_tokens INTEGER NOT NULL DEFAULT 0,
                total_output_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                last_input_tokens INTEGER NOT NULL DEFAULT 0,
                turn_count INTEGER NOT NULL DEFAULT 0,
                prompt_token_estimate INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS pending_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                queue_type TEXT NOT NULL CHECK(queue_type IN ('turn', 'steer')),
                kind TEXT NOT NULL,
                content TEXT NOT NULL,
                pending_input_id TEXT,
                metadata TEXT,
                created_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_pending_session
                ON pending_messages(session_id, queue_type);
            ",
        )
        .context("failed to run database migrations")?;
        let has_session_additional_directories = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(sessions)")
                .context("failed to inspect sessions schema")?;
            let columns = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .context("failed to read sessions schema")?;
            let mut found = false;
            for column in columns {
                if column? == "additional_directories" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_session_additional_directories {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN additional_directories TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .context("failed to add additional_directories column")?;
        }
        let has_session_last_activity_at = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(sessions)")
                .context("failed to inspect sessions schema")?;
            let columns = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .context("failed to read sessions schema")?;
            let mut found = false;
            for column in columns {
                if column? == "last_activity_at" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_session_last_activity_at {
            conn.execute(
                "ALTER TABLE sessions ADD COLUMN last_activity_at INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .context("failed to add last_activity_at column")?;
            conn.execute(
                "UPDATE sessions SET last_activity_at = updated_at WHERE last_activity_at = 0",
                [],
            )
            .context("failed to backfill last_activity_at column")?;
        }
        let has_pending_input_id = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(pending_messages)")
                .context("failed to inspect pending_messages schema")?;
            let columns = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .context("failed to read pending_messages schema")?;
            let mut found = false;
            for column in columns {
                if column? == "pending_input_id" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_pending_input_id {
            conn.execute(
                "ALTER TABLE pending_messages ADD COLUMN pending_input_id TEXT",
                [],
            )
            .context("failed to add pending_input_id column")?;
        }
        let has_session_stats_total_tokens = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(session_stats)")
                .context("failed to inspect session_stats schema")?;
            let columns = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .context("failed to read session_stats schema")?;
            let mut found = false;
            for column in columns {
                if column? == "total_tokens" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_session_stats_total_tokens {
            conn.execute(
                "ALTER TABLE session_stats ADD COLUMN total_tokens INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .context("failed to add total_tokens column")?;
            conn.execute(
                "UPDATE session_stats SET total_tokens = total_input_tokens + total_output_tokens",
                [],
            )
            .context("failed to backfill total_tokens column")?;
        }
        if !sessions_has_column(&conn, "rollout_path")? {
            conn.execute("ALTER TABLE sessions ADD COLUMN rollout_path TEXT", [])
                .context("failed to add rollout_path column")?;
        }
        if !sessions_has_column(&conn, "parent_session_id")? {
            conn.execute("ALTER TABLE sessions ADD COLUMN parent_session_id TEXT", [])
                .context("failed to add parent_session_id column")?;
        }
        if !sessions_has_column(&conn, "agent_path")? {
            conn.execute("ALTER TABLE sessions ADD COLUMN agent_path TEXT", [])
                .context("failed to add agent_path column")?;
        }
        // Queue entries have an explicit position so `session/queue/update`
        // can reorder without rewriting row ids (P4c); existing rows keep
        // their insertion order (position = id).
        if !pending_messages_has_column(&conn, "position")? {
            conn.execute(
                "ALTER TABLE pending_messages ADD COLUMN position INTEGER",
                [],
            )
            .context("failed to add pending_messages position column")?;
            conn.execute(
                "UPDATE pending_messages SET position = id WHERE position IS NULL",
                [],
            )
            .context("failed to backfill pending_messages position")?;
        }
        let pending_messages_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'pending_messages'",
                [],
                |row| row.get(0),
            )
            .ok();
        if pending_messages_sql
            .as_deref()
            .is_some_and(|sql| sql.contains("'btw'"))
        {
            // P4c originally used `btw` for the active-turn steer queue. The
            // product `/btw` feature is an unrelated ephemeral side question,
            // so migrate the storage value and CHECK constraint together.
            conn.execute_batch(
                "
                BEGIN;
                ALTER TABLE pending_messages RENAME TO pending_messages_legacy;
                CREATE TABLE pending_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    queue_type TEXT NOT NULL CHECK(queue_type IN ('turn', 'steer')),
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    pending_input_id TEXT,
                    metadata TEXT,
                    created_at INTEGER NOT NULL,
                    position INTEGER NOT NULL
                );
                INSERT INTO pending_messages
                    (id, session_id, queue_type, kind, content, pending_input_id, metadata, created_at, position)
                SELECT id, session_id,
                    CASE queue_type WHEN 'btw' THEN 'steer' ELSE queue_type END,
                    kind, content, pending_input_id, metadata, created_at, position
                FROM pending_messages_legacy;
                DROP TABLE pending_messages_legacy;
                CREATE INDEX idx_pending_session
                    ON pending_messages(session_id, queue_type);
                COMMIT;
                ",
            )
            .context("failed to migrate pending steer queue from btw to steer")?;
        }
        // Schema version table (05 §2.3): the new authority going forward.
        // The ad-hoc column probes above are the v0 baseline and keep working
        // for databases created before this table existed.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .context("failed to create schema_meta table")?;
        conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [CURRENT_SCHEMA_VERSION.to_string()],
        )
        .context("failed to record schema version")?;
        // Persisted event log (08 §5/§7): the rollout JSONL is the canonical
        // recovery log; this table is the delivery log used for cursor replay.
        // Rows are idempotent by (source_fact_id, event_kind, stream_id);
        // `seq` is strictly increasing per stream. A database rebuild expires
        // all cursors and forces re-snapshot — rows are never modified.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS event_log (
                source_fact_id TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                stream_id TEXT NOT NULL,
                event_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                payload TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (source_fact_id, event_kind, stream_id)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS event_log_stream_seq
                ON event_log(stream_id, seq);

            CREATE TABLE IF NOT EXISTS projection_watermark (
                rollout_path TEXT PRIMARY KEY,
                last_line_index INTEGER NOT NULL
            );",
        )
        .context("failed to create event_log tables")?;
        Ok(())
    }

    /// The recorded schema version, if any. `None` means the database
    /// predates the `schema_meta` table (implicitly version 0).
    pub fn schema_version(&self) -> Result<Option<u32>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .ok();
        value
            .map(|value| {
                value
                    .parse::<u32>()
                    .context("invalid schema_version in schema_meta")
            })
            .transpose()
    }

    // === Event log (08 §5/§7) ===

    /// Idempotently inserts derived event rows. `seq` is allocated per stream
    /// inside the same statement, and the `(source_fact_id, event_kind,
    /// stream_id)` primary key makes re-projection of the same rollout fact a
    /// no-op — reconciliation never duplicates, only backfills. Returns the
    /// number of rows actually inserted.
    pub fn insert_event_log_rows(&self, rows: &[NewEventLogRow]) -> Result<usize> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut inserted = 0usize;
        for row in rows {
            let changes = conn
                .execute(
                    "INSERT OR IGNORE INTO event_log
                        (source_fact_id, event_kind, stream_id, event_id, seq, payload, created_at)
                     SELECT ?1, ?2, ?3, ?4,
                        (SELECT COALESCE(MAX(seq), 0) + 1 FROM event_log WHERE stream_id = ?3),
                        ?5, ?6",
                    params![
                        row.source_fact_id,
                        row.event_kind,
                        row.stream_id,
                        row.event_id,
                        row.payload,
                        row.created_at,
                    ],
                )
                .context("failed to insert event_log row")?;
            inserted += changes;
        }
        Ok(inserted)
    }

    /// Reads stored events of one stream after `after_seq`, ordered by seq
    /// (cursor replay, 08 §4).
    pub fn event_log_rows(&self, stream_id: &str, after_seq: u64) -> Result<Vec<EventLogRow>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT source_fact_id, event_kind, stream_id, event_id, seq, payload, created_at
                 FROM event_log WHERE stream_id = ?1 AND seq > ?2 ORDER BY seq",
            )
            .context("failed to prepare event_log read")?;
        let rows = stmt
            .query_map(params![stream_id, after_seq as i64], |row| {
                Ok(EventLogRow {
                    source_fact_id: row.get(0)?,
                    event_kind: row.get(1)?,
                    stream_id: row.get(2)?,
                    event_id: row.get(3)?,
                    seq: row.get::<_, i64>(4)? as u64,
                    payload: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })
            .context("failed to read event_log rows")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to decode event_log rows")
    }

    /// Total number of stored event rows (reconciliation tests).
    pub fn event_log_len(&self) -> Result<u64> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))
            .context("failed to count event_log rows")?;
        Ok(count as u64)
    }

    /// The highest stored seq of one stream — the subscription barrier seq
    /// (08 §4). `None` when the stream has no rows yet.
    pub fn event_log_max_seq(&self, stream_id: &str) -> Result<Option<u64>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let value: Option<i64> = conn
            .query_row(
                "SELECT MAX(seq) FROM event_log WHERE stream_id = ?1",
                params![stream_id],
                |row| row.get(0),
            )
            .context("failed to read stream barrier seq")?;
        Ok(value.map(|value| value as u64))
    }

    /// The last rollout line index projected into `event_log` for a file.
    pub fn projection_watermark(&self, rollout_path: &Path) -> Result<Option<u64>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let value: Option<i64> = conn
            .query_row(
                "SELECT last_line_index FROM projection_watermark WHERE rollout_path = ?1",
                params![rollout_path.to_string_lossy().as_ref()],
                |row| row.get(0),
            )
            .ok();
        Ok(value.map(|value| value as u64))
    }

    /// Advances the projection watermark for a rollout file.
    pub fn set_projection_watermark(
        &self,
        rollout_path: &Path,
        last_line_index: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "INSERT INTO projection_watermark (rollout_path, last_line_index) VALUES (?1, ?2)
             ON CONFLICT(rollout_path) DO UPDATE SET last_line_index = excluded.last_line_index",
            params![
                rollout_path.to_string_lossy().as_ref(),
                last_line_index as i64
            ],
        )
        .context("failed to update projection watermark")?;
        Ok(())
    }

    // === Session CRUD ===

    /// Inserts or updates a session's metadata and optional rollout index fields.
    pub fn upsert_session(
        &self,
        meta: &SessionMetadata,
        rollout_path: Option<&std::path::Path>,
    ) -> Result<()> {
        self.upsert_session_with_source(meta, rollout_path, SessionUpsertSource::RuntimeLive)
    }

    /// Inserts or updates session metadata using rollout-index semantics.
    pub fn upsert_rollout_index_session(
        &self,
        meta: &SessionMetadata,
        rollout_path: Option<&std::path::Path>,
    ) -> Result<()> {
        self.upsert_session_with_source(meta, rollout_path, SessionUpsertSource::RolloutIndex)
    }

    fn upsert_session_with_source(
        &self,
        meta: &SessionMetadata,
        rollout_path: Option<&std::path::Path>,
        source: SessionUpsertSource,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let additional_directories = serde_json::to_string(&meta.additional_directories)
            .context("failed to serialize session additional directories")?;
        let title_state_str = match &meta.title_state {
            SessionTitleState::Unset => "unset",
            SessionTitleState::Provisional => "provisional",
            SessionTitleState::Final(_) => "final",
        };
        let rollout_path_str = rollout_path.map(|path| path.to_string_lossy().into_owned());
        let parent_session_id = meta.parent_session_id.map(|id| id.to_string());
        let agent_path = meta.agent_path.clone();
        let update_clause = match source {
            SessionUpsertSource::RuntimeLive => {
                "title = COALESCE(excluded.title, sessions.title),
                title_state = CASE
                    WHEN excluded.title IS NOT NULL THEN excluded.title_state
                    ELSE sessions.title_state
                END,
                model = COALESCE(excluded.model, sessions.model),
                thinking = COALESCE(excluded.thinking, sessions.thinking),
                cwd = excluded.cwd,
                additional_directories = excluded.additional_directories,
                updated_at = excluded.updated_at,
                last_activity_at = excluded.last_activity_at,
                parent_session_id = COALESCE(excluded.parent_session_id, sessions.parent_session_id),
                agent_path = COALESCE(excluded.agent_path, sessions.agent_path),
                rollout_path = COALESCE(excluded.rollout_path, sessions.rollout_path)"
            }
            SessionUpsertSource::RolloutIndex => {
                "title = COALESCE(excluded.title, sessions.title),
                title_state = CASE
                    WHEN excluded.title IS NOT NULL THEN excluded.title_state
                    ELSE sessions.title_state
                END,
                model = COALESCE(excluded.model, sessions.model),
                thinking = COALESCE(excluded.thinking, sessions.thinking),
                cwd = excluded.cwd,
                additional_directories = excluded.additional_directories,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                last_activity_at = excluded.last_activity_at,
                parent_session_id = COALESCE(excluded.parent_session_id, sessions.parent_session_id),
                agent_path = COALESCE(excluded.agent_path, sessions.agent_path),
                rollout_path = COALESCE(excluded.rollout_path, sessions.rollout_path)"
            }
        };
        let sql = format!(
            "INSERT INTO sessions (id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, rollout_path, parent_session_id, agent_path, schema_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 3)
             ON CONFLICT(id) DO UPDATE SET {update_clause}"
        );
        conn.execute(
            &sql,
            params![
                meta.session_id.to_string(),
                meta.title,
                title_state_str,
                meta.model,
                meta.reasoning_effort_selection,
                meta.cwd.to_string_lossy().to_string(),
                additional_directories,
                meta.ephemeral as i32,
                meta.created_at.timestamp(),
                meta.updated_at.timestamp(),
                meta.last_activity_at.timestamp(),
                rollout_path_str,
                parent_session_id,
                agent_path,
            ],
        )
        .context("failed to upsert session")?;
        Ok(())
    }

    /// Returns true when durable sessions need rollout metadata backfilled into SQLite.
    pub fn session_index_backfill_required(&self) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions
                 WHERE ephemeral = 0 AND (rollout_path IS NULL OR rollout_path = '')",
                [],
                |row| row.get(0),
            )
            .context("failed to check session index backfill requirement")?;
        Ok(count > 0)
    }

    /// Retrieves a session's metadata by ID.
    pub fn get_session(&self, id: &SessionId) -> Result<Option<SessionMetadata>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, agent_path
                 FROM sessions WHERE id = ?1",
            )
            .context("failed to prepare get_session statement")?;
        let result = stmt.query_row(params![id.to_string()], |row| {
            parse_session_metadata_row(row, false)
        });

        match result {
            Ok(meta) => Ok(Some(meta)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Returns resume/list index fields for a session id.
    pub fn get_session_index(&self, id: &SessionId) -> Result<Option<SessionIndexRecord>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, agent_path, rollout_path
                 FROM sessions WHERE id = ?1",
            )
            .context("failed to prepare get_session_index statement")?;
        let result = stmt.query_row(params![id.to_string()], |row| {
            let metadata = parse_session_metadata_row(row, true)?;
            let rollout_path = row
                .get::<_, Option<String>>(13)?
                .map(PathBuf::from)
                .filter(|path| !path.as_os_str().is_empty());
            Ok(SessionIndexRecord {
                metadata,
                rollout_path,
            })
        });

        match result {
            Ok(index) => Ok(Some(index)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Lists durable user-visible sessions (roots and forks, excluding subagents).
    pub fn list_root_sessions(&self) -> Result<Vec<SessionMetadata>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, agent_path
                 FROM sessions
                 WHERE ephemeral = 0 AND agent_path IS NULL
                 ORDER BY last_activity_at DESC, updated_at DESC",
            )
            .context("failed to prepare list_root_sessions statement")?;
        let rows = stmt
            .query_map([], |row| parse_session_metadata_row(row, false))
            .context("failed to query root sessions")?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    /// Lists all sessions ordered by most recently updated.
    pub fn list_sessions(&self) -> Result<Vec<SessionMetadata>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT id, title, title_state, model, thinking, cwd, additional_directories, ephemeral, created_at, updated_at, last_activity_at, parent_session_id, agent_path
                 FROM sessions ORDER BY last_activity_at DESC, updated_at DESC",
            )
            .context("failed to prepare list_sessions statement")?;
        let rows = stmt
            .query_map([], |row| parse_session_metadata_row(row, false))
            .context("failed to query sessions")?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    /// Deletes a session and its related data.
    pub fn delete_session(&self, id: &SessionId) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            params![id.to_string()],
        )
        .context("failed to delete session")?;
        Ok(())
    }

    // === Session Stats ===

    /// Inserts or updates session token statistics.
    pub fn update_stats(&self, id: &SessionId, stats: &SessionStats) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "INSERT INTO session_stats (session_id, total_input_tokens, total_output_tokens,
                total_tokens, total_cache_creation_tokens, total_cache_read_tokens, last_input_tokens,
                turn_count, prompt_token_estimate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(session_id) DO UPDATE SET
                total_input_tokens = excluded.total_input_tokens,
                total_output_tokens = excluded.total_output_tokens,
                total_tokens = excluded.total_tokens,
                total_cache_creation_tokens = excluded.total_cache_creation_tokens,
                total_cache_read_tokens = excluded.total_cache_read_tokens,
                last_input_tokens = excluded.last_input_tokens,
                turn_count = excluded.turn_count,
                prompt_token_estimate = excluded.prompt_token_estimate",
            params![
                id.to_string(),
                stats.total_input_tokens as i64,
                stats.total_output_tokens as i64,
                stats.total_tokens as i64,
                stats.total_cache_creation_tokens as i64,
                stats.total_cache_read_tokens as i64,
                stats.last_input_tokens as i64,
                stats.turn_count as i64,
                stats.prompt_token_estimate as i64,
            ],
        )
        .context("failed to update session stats")?;
        Ok(())
    }

    /// Retrieves session token statistics.
    pub fn get_stats(&self, id: &SessionId) -> Result<Option<SessionStats>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let result = conn.query_row(
            "SELECT total_input_tokens, total_output_tokens, total_tokens, total_cache_creation_tokens,
                    total_cache_read_tokens, last_input_tokens, turn_count, prompt_token_estimate
             FROM session_stats WHERE session_id = ?1",
            params![id.to_string()],
            |row| {
                Ok(SessionStats {
                    total_input_tokens: row.get::<_, i64>(0)? as usize,
                    total_output_tokens: row.get::<_, i64>(1)? as usize,
                    total_tokens: row.get::<_, i64>(2)? as usize,
                    total_cache_creation_tokens: row.get::<_, i64>(3)? as usize,
                    total_cache_read_tokens: row.get::<_, i64>(4)? as usize,
                    last_input_tokens: row.get::<_, i64>(5)? as usize,
                    turn_count: row.get::<_, i64>(6)? as usize,
                    prompt_token_estimate: row.get::<_, i64>(7)? as usize,
                })
            },
        );

        match result {
            Ok(stats) => Ok(Some(stats)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // === Pending Messages ===

    /// Pushes a pending message to the specified queue.
    pub fn push_pending(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        item: &PendingInputItem,
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let (kind_str, content) = pending_kind_parts(&item.kind);
        let metadata_str = item.metadata.as_ref().map(|v| v.to_string());
        conn.execute(
            "INSERT INTO pending_messages (session_id, queue_type, kind, content, pending_input_id, metadata, created_at, position)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                (SELECT COALESCE(MAX(position), 0) + 1 FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2)",
            params![
                session_id.to_string(),
                queue.as_str(),
                kind_str,
                content,
                item.id.to_string(),
                metadata_str,
                item.created_at.timestamp(),
            ],
        )
        .context("failed to push pending message")?;
        Ok(())
    }

    /// Replaces one pending message's content (kind/content/metadata), keyed
    /// by its stable `pending_input_id` (`session/queue/update`). Returns
    /// whether the entry still existed.
    pub fn update_pending_content(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        item: &PendingInputItem,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let (kind_str, content) = pending_kind_parts(&item.kind);
        let metadata_str = item.metadata.as_ref().map(|v| v.to_string());
        let changes = conn
            .execute(
                "UPDATE pending_messages SET kind = ?4, content = ?5, metadata = ?6
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.to_string(),
                    queue.as_str(),
                    item.id.to_string(),
                    kind_str,
                    content,
                    metadata_str,
                ],
            )
            .context("failed to update pending message")?;
        Ok(changes == 1)
    }

    /// Merges one key into a pending message's metadata JSON (used for the
    /// `clientUserMessageId` dedup key, 01 §4.3). Returns whether the entry
    /// existed.
    pub fn set_pending_metadata_field(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        pending_input_id: &PendingInputId,
        key: &str,
        value: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let changes = conn
            .execute(
                "UPDATE pending_messages
                 SET metadata = json_set(COALESCE(metadata, '{}'), '$.' || ?4, ?5)
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.to_string(),
                    queue.as_str(),
                    pending_input_id.to_string(),
                    key,
                    value,
                ],
            )
            .context("failed to update pending message metadata")?;
        Ok(changes == 1)
    }

    /// Rewrites queue positions to 1..=N following `ordered_ids`
    /// (`session/queue/update` reorder). Ids not listed keep their relative
    /// order at the end.
    pub fn set_pending_positions(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        ordered_ids: &[PendingInputId],
    ) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        for (index, id) in ordered_ids.iter().enumerate() {
            conn.execute(
                "UPDATE pending_messages SET position = ?4
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.to_string(),
                    queue.as_str(),
                    id.to_string(),
                    (index + 1) as i64,
                ],
            )
            .context("failed to reorder pending messages")?;
        }
        Ok(())
    }

    /// Lists pending messages of one queue without draining them
    /// (subscription snapshots, 08 §4).
    pub fn list_pending(
        &self,
        session_id: &SessionId,
        queue: QueueType,
    ) -> Result<Vec<PendingInputItem>> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let mut stmt = conn
            .prepare(
                "SELECT kind, content, pending_input_id, metadata, created_at
                 FROM pending_messages
                 WHERE session_id = ?1 AND queue_type = ?2
                 ORDER BY position ASC, id ASC",
            )
            .context("failed to prepare list_pending statement")?;
        let items = stmt
            .query_map(params![session_id.to_string(), queue.as_str()], |row| {
                let kind_str: String = row.get(0)?;
                let content: String = row.get(1)?;
                let pending_input_id: Option<String> = row.get(2)?;
                let metadata_str: Option<String> = row.get(3)?;
                let created_at: i64 = row.get(4)?;
                Ok(pending_input_from_row(
                    &kind_str,
                    &content,
                    pending_input_id,
                    metadata_str,
                    created_at,
                ))
            })
            .context("failed to query pending messages")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to decode pending messages")?;
        Ok(items)
    }

    /// Drains all pending messages from the specified queue, deleting them in the process.
    pub fn drain_pending(
        &self,
        session_id: &SessionId,
        queue: QueueType,
    ) -> Result<Vec<PendingInputItem>> {
        let mut conn = self.conn.lock().expect("database mutex poisoned");

        let tx = conn
            .transaction()
            .context("failed to begin drain transaction")?;

        let items = {
            let mut stmt = tx
                .prepare(
                    "SELECT kind, content, pending_input_id, metadata, created_at
                     FROM pending_messages
                     WHERE session_id = ?1 AND queue_type = ?2
                     ORDER BY position ASC, id ASC",
                )
                .context("failed to prepare drain_pending statement")?;
            let rows = stmt
                .query_map(params![session_id.to_string(), queue.as_str()], |row| {
                    let kind_str: String = row.get(0)?;
                    let content: String = row.get(1)?;
                    let pending_input_id: Option<String> = row.get(2)?;
                    let metadata_str: Option<String> = row.get(3)?;
                    let created_at: i64 = row.get(4)?;

                    Ok(pending_input_from_row(
                        &kind_str,
                        &content,
                        pending_input_id,
                        metadata_str,
                        created_at,
                    ))
                })
                .context("failed to query pending messages")?;

            let mut items = Vec::new();
            for row in rows {
                items.push(row?);
            }
            items
        };

        tx.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2",
            params![session_id.to_string(), queue.as_str()],
        )
        .context("failed to delete drained messages")?;

        tx.commit().context("failed to commit drain transaction")?;

        Ok(items)
    }

    /// Removes one pending message from the specified queue by its stable pending input id.
    pub fn remove_pending_by_id(
        &self,
        session_id: &SessionId,
        queue: QueueType,
        pending_input_id: &PendingInputId,
    ) -> Result<bool> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let affected = conn
            .execute(
                "DELETE FROM pending_messages
                 WHERE session_id = ?1 AND queue_type = ?2 AND pending_input_id = ?3",
                params![
                    session_id.to_string(),
                    queue.as_str(),
                    pending_input_id.to_string(),
                ],
            )
            .context("failed to remove pending message by id")?;
        Ok(affected > 0)
    }

    /// Clears all pending messages from the specified queue.
    pub fn clear_pending(&self, session_id: &SessionId, queue: QueueType) -> Result<()> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "DELETE FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2",
            params![session_id.to_string(), queue.as_str()],
        )
        .context("failed to clear pending messages")?;
        Ok(())
    }

    /// Counts pending messages in the specified queue.
    #[allow(dead_code)]
    pub fn count_pending(&self, session_id: &SessionId, queue: QueueType) -> Result<usize> {
        let conn = self.conn.lock().expect("database mutex poisoned");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pending_messages WHERE session_id = ?1 AND queue_type = ?2",
            params![session_id.to_string(), queue.as_str()],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

fn sessions_has_column(conn: &Connection, column: &str) -> Result<bool> {
    table_has_column(conn, "sessions", column)
}

fn pending_messages_has_column(conn: &Connection, column: &str) -> Result<bool> {
    table_has_column(conn, "pending_messages", column)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("failed to inspect {table} schema"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to read sessions schema")?;
    for column_name in columns {
        if column_name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_session_metadata_row(
    row: &rusqlite::Row<'_>,
    _include_parent: bool,
) -> rusqlite::Result<SessionMetadata> {
    let id_str: String = row.get(0)?;
    let title: Option<String> = row.get(1)?;
    let title_state_str: String = row.get(2)?;
    let model: Option<String> = row.get(3)?;
    let thinking: Option<String> = row.get(4)?;
    let cwd_str: String = row.get(5)?;
    let additional_directories_str: String = row.get(6)?;
    let ephemeral: i32 = row.get(7)?;
    let created_at: i64 = row.get(8)?;
    let updated_at: i64 = row.get(9)?;
    let last_activity_at: i64 = row.get(10)?;
    let parent_session_id = row
        .get::<_, Option<String>>(11)?
        .map(|value| parse_session_id_column(value, 11))
        .transpose()?;
    let agent_path = row
        .get::<_, Option<String>>(12)?
        .filter(|path| !path.is_empty());

    let title_state = match title_state_str.as_str() {
        "provisional" => SessionTitleState::Provisional,
        "final" => SessionTitleState::Final(devo_protocol::SessionTitleFinalSource::ModelGenerated),
        _ => SessionTitleState::Unset,
    };

    Ok(SessionMetadata {
        session_id: parse_session_id_column(id_str, 0)?,
        cwd: PathBuf::from(&cwd_str),
        additional_directories: parse_additional_directories_column(additional_directories_str, 6)?,
        created_at: Utc
            .timestamp_opt(created_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
        updated_at: Utc
            .timestamp_opt(updated_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
        last_activity_at: Utc
            .timestamp_opt(last_activity_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
        title,
        title_state,
        parent_session_id,
        agent_path,
        agent_nickname: None,
        agent_role: None,
        ephemeral: ephemeral != 0,
        model,
        model_binding_id: None,
        reasoning_effort_selection: thinking,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        status: SessionRuntimeStatus::Idle,
        collaboration_mode: Default::default(),
    })
}

fn parse_session_id_column(id: String, column: usize) -> rusqlite::Result<SessionId> {
    SessionId::from_str(&id).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        )
    })
}

fn parse_additional_directories_column(
    value: String,
    column: usize,
) -> rusqlite::Result<Vec<PathBuf>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(error))
    })
}

/// Maps a `PendingInputKind` to its `(kind, content)` storage pair (shared
/// by `push_pending` and `update_pending_content`).
fn pending_kind_parts(kind: &PendingInputKind) -> (&'static str, String) {
    match kind {
        PendingInputKind::UserText { text } => ("user_text", text.clone()),
        PendingInputKind::UserInput {
            input,
            display_text,
            prompt_text,
            prompt_messages,
        } => {
            let content = serde_json::json!({
                "input": input,
                "display_text": display_text,
                "prompt_text": prompt_text,
                "prompt_messages": prompt_messages,
            });
            ("user_input", content.to_string())
        }
        PendingInputKind::ToolCallBlockedByHook {
            tool_use_id,
            reason,
        } => {
            let content = serde_json::json!({
                "tool_use_id": tool_use_id,
                "reason": reason,
            });
            ("tool_call_blocked", content.to_string())
        }
        PendingInputKind::BudgetLimitSteering => ("budget_limit", String::new()),
    }
}

/// Maps one `pending_messages` row to its `PendingInputItem` (shared by
/// `drain_pending` and `list_pending`).
fn pending_input_from_row(
    kind_str: &str,
    content: &str,
    pending_input_id: Option<String>,
    metadata_str: Option<String>,
    created_at: i64,
) -> PendingInputItem {
    let kind = match kind_str {
        "user_text" => PendingInputKind::UserText {
            text: content.to_string(),
        },
        "user_input" => serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .and_then(|value| {
                Some(PendingInputKind::UserInput {
                    input: serde_json::from_value(value.get("input")?.clone()).ok()?,
                    display_text: value
                        .get("display_text")?
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    prompt_text: value
                        .get("prompt_text")?
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    prompt_messages: value
                        .get("prompt_messages")
                        .and_then(|messages| serde_json::from_value(messages.clone()).ok())
                        .unwrap_or_default(),
                })
            })
            .unwrap_or(PendingInputKind::UserText {
                text: content.to_string(),
            }),
        "tool_call_blocked" => {
            let parsed: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
            PendingInputKind::ToolCallBlockedByHook {
                tool_use_id: parsed["tool_use_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                reason: parsed["reason"].as_str().unwrap_or_default().to_string(),
            }
        }
        "budget_limit" => PendingInputKind::BudgetLimitSteering,
        _ => PendingInputKind::UserText {
            text: content.to_string(),
        },
    };
    PendingInputItem {
        id: pending_input_id
            .and_then(|id| PendingInputId::try_from(id).ok())
            .unwrap_or_default(),
        kind,
        metadata: metadata_str.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: Utc
            .timestamp_opt(created_at, 0)
            .single()
            .unwrap_or_else(Utc::now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().expect("create temp dir");
        let db_path = dir.path().join("test.db");
        let db = Database::open(db_path).expect("open database");
        (db, dir)
    }

    #[test]
    fn schema_meta_records_current_schema_version() {
        let (db, _dir) = test_db();
        assert_eq!(
            db.schema_version().expect("read schema version"),
            Some(CURRENT_SCHEMA_VERSION)
        );
        // Re-opening an existing database keeps the recorded version.
        let (db, dir) = test_db();
        drop(db);
        let db = Database::open(dir.path().join("test.db")).expect("reopen database");
        assert_eq!(
            db.schema_version().expect("read schema version"),
            Some(CURRENT_SCHEMA_VERSION)
        );
    }

    #[test]
    fn migration_renames_legacy_btw_queue_rows_to_steer() {
        let dir = TempDir::new().expect("create temp dir");
        let db_path = dir.path().join("legacy.db");
        let session_id = SessionId::new();
        {
            let conn = Connection::open(&db_path).expect("open legacy database");
            conn.execute_batch(
                "
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    cwd TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE pending_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                    queue_type TEXT NOT NULL CHECK(queue_type IN ('turn', 'btw')),
                    kind TEXT NOT NULL,
                    content TEXT NOT NULL,
                    pending_input_id TEXT,
                    metadata TEXT,
                    created_at INTEGER NOT NULL,
                    position INTEGER
                );
                CREATE INDEX idx_pending_session
                    ON pending_messages(session_id, queue_type);
                ",
            )
            .expect("create legacy queue tables");
            conn.execute(
                "INSERT INTO sessions (id, cwd, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![session_id.to_string(), ".", 0, 0],
            )
            .expect("insert legacy session");
            conn.execute(
                "INSERT INTO pending_messages
                    (session_id, queue_type, kind, content, pending_input_id, created_at, position)
                 VALUES (?1, 'btw', 'user_text', 'keep steering', ?2, 0, 1)",
                params![session_id.to_string(), PendingInputId::new().to_string()],
            )
            .expect("insert legacy steer row");
        }

        let db = Database::open(db_path).expect("migrate legacy database");
        assert_eq!(
            db.count_pending(&session_id, QueueType::Steer)
                .expect("count migrated steer row"),
            1
        );
        let conn = db.conn.lock().expect("database mutex poisoned");
        let queue_type: String = conn
            .query_row("SELECT queue_type FROM pending_messages", [], |row| {
                row.get(0)
            })
            .expect("read migrated queue type");
        assert_eq!(queue_type, "steer");
        let old_value = conn.execute(
            "INSERT INTO pending_messages
                (session_id, queue_type, kind, content, created_at, position)
             VALUES (?1, 'btw', 'user_text', 'obsolete', 0, 2)",
            params![session_id.to_string()],
        );
        assert!(old_value.is_err(), "legacy queue type must be rejected");
    }

    #[test]
    fn migration_backfills_legacy_session_stats_total_tokens() {
        let dir = TempDir::new().expect("create temp dir");
        let db_path = dir.path().join("legacy.db");
        let session_id = SessionId::new();
        {
            let conn = Connection::open(&db_path).expect("open legacy database");
            conn.execute_batch(
                "
                CREATE TABLE session_stats (
                    session_id TEXT PRIMARY KEY,
                    total_input_tokens INTEGER NOT NULL DEFAULT 0,
                    total_output_tokens INTEGER NOT NULL DEFAULT 0,
                    total_cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                    total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    last_input_tokens INTEGER NOT NULL DEFAULT 0,
                    turn_count INTEGER NOT NULL DEFAULT 0,
                    prompt_token_estimate INTEGER NOT NULL DEFAULT 0
                );
                ",
            )
            .expect("create legacy session_stats");
            conn.execute(
                "INSERT INTO session_stats (session_id, total_input_tokens, total_output_tokens)
                 VALUES (?1, ?2, ?3)",
                params![session_id.to_string(), 40_i64, 2_i64],
            )
            .expect("insert legacy stats");
        }

        let db = Database::open(db_path).expect("migrate database");
        let stats = db
            .get_stats(&session_id)
            .expect("get migrated stats")
            .expect("stats row exists");

        assert_eq!(stats.total_tokens, 42);
    }

    fn sample_session(id: &str) -> SessionMetadata {
        SessionMetadata {
            session_id: SessionId::from_str(id).unwrap_or_default(),
            cwd: PathBuf::from("/tmp"),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Test Session".into()),
            title_state: SessionTitleState::Provisional,
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: false,
            model: Some("claude-sonnet-4-20250514".into()),
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
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: Default::default(),
        }
    }

    #[test]
    fn upsert_and_get_session() {
        let (db, _dir) = test_db();
        let mut meta = sample_session("session-1");
        meta.additional_directories = vec![PathBuf::from("/tmp/shared")];
        db.upsert_session(&meta, None).expect("upsert");

        let retrieved = db.get_session(&meta.session_id).expect("get");
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.session_id, meta.session_id);
        assert_eq!(retrieved.title, Some("Test Session".into()));
        assert_eq!(
            retrieved.additional_directories,
            meta.additional_directories
        );
    }

    #[test]
    fn list_sessions_ordered() {
        let (db, _dir) = test_db();
        let mut meta1 = sample_session("session-1");
        let mut meta2 = sample_session("session-2");
        let baseline = Utc::now();
        meta1.updated_at = baseline;
        meta1.last_activity_at = baseline;
        meta2.updated_at = baseline + chrono::Duration::seconds(10);
        meta2.last_activity_at = baseline - chrono::Duration::seconds(10);
        db.upsert_session(&meta1, None).expect("upsert");
        db.upsert_session(&meta2, None).expect("upsert");

        let sessions = db.list_sessions().expect("list");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, meta1.session_id);
    }

    #[test]
    fn list_sessions_rejects_invalid_persisted_session_id() {
        let (db, _dir) = test_db();
        let conn = db.conn.lock().expect("database mutex poisoned");
        conn.execute(
            "INSERT INTO sessions (id, title, title_state, cwd, ephemeral, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "",
                "Corrupt Session",
                "provisional",
                "/tmp",
                0_i32,
                1_i64,
                1_i64
            ],
        )
        .expect("insert corrupt session");
        drop(conn);

        let error = db
            .list_sessions()
            .expect_err("invalid persisted session id should fail closed");
        let message = error
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(message.contains("invalid length: found 0"), "{message}");
    }

    #[test]
    fn delete_session_cascades() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");

        db.delete_session(&meta.session_id).expect("delete");
        let retrieved = db.get_session(&meta.session_id).expect("get");
        assert!(retrieved.is_none());
    }

    #[test]
    fn update_and_get_stats() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");

        let stats = SessionStats {
            total_input_tokens: 1000,
            total_output_tokens: 500,
            total_tokens: 600,
            total_cache_creation_tokens: 100,
            total_cache_read_tokens: 50,
            last_input_tokens: 200,
            turn_count: 5,
            prompt_token_estimate: 800,
        };
        db.update_stats(&meta.session_id, &stats).expect("update");

        let retrieved = db.get_stats(&meta.session_id).expect("get");
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.total_input_tokens, 1000);
        assert_eq!(retrieved.total_output_tokens, 500);
        assert_eq!(retrieved.turn_count, 5);
    }

    #[test]
    fn push_and_drain_pending() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");

        let item1 = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "hello".into(),
            },
            None,
            Utc::now(),
        );
        let item2 = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "world".into(),
            },
            None,
            Utc::now(),
        );

        db.push_pending(&meta.session_id, QueueType::Turn, &item1)
            .expect("push");
        db.push_pending(&meta.session_id, QueueType::Turn, &item2)
            .expect("push");

        let count = db
            .count_pending(&meta.session_id, QueueType::Turn)
            .expect("count");
        assert_eq!(count, 2);

        let drained = db
            .drain_pending(&meta.session_id, QueueType::Turn)
            .expect("drain");
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].id, item1.id);
        assert_eq!(drained[1].id, item2.id);
        assert!(matches!(&drained[0].kind, PendingInputKind::UserText { text } if text == "hello"));
        assert!(matches!(&drained[1].kind, PendingInputKind::UserText { text } if text == "world"));

        let count = db
            .count_pending(&meta.session_id, QueueType::Turn)
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn queue_types_are_isolated() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");

        let turn_item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "turn msg".into(),
            },
            None,
            Utc::now(),
        );
        let steer_item = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "steer msg".into(),
            },
            None,
            Utc::now(),
        );

        db.push_pending(&meta.session_id, QueueType::Turn, &turn_item)
            .expect("push");
        db.push_pending(&meta.session_id, QueueType::Steer, &steer_item)
            .expect("push");

        let turn_count = db
            .count_pending(&meta.session_id, QueueType::Turn)
            .expect("count");
        let steer_count = db
            .count_pending(&meta.session_id, QueueType::Steer)
            .expect("count");
        assert_eq!(turn_count, 1);
        assert_eq!(steer_count, 1);

        db.clear_pending(&meta.session_id, QueueType::Steer)
            .expect("clear");
        let steer_count = db
            .count_pending(&meta.session_id, QueueType::Steer)
            .expect("count");
        assert_eq!(steer_count, 0);

        let turn_count = db
            .count_pending(&meta.session_id, QueueType::Turn)
            .expect("count");
        assert_eq!(turn_count, 1);
    }

    #[test]
    fn remove_pending_by_id_only_removes_matching_item() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");

        let first = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "first".into(),
            },
            None,
            Utc::now(),
        );
        let second = PendingInputItem::new(
            PendingInputKind::UserText {
                text: "second".into(),
            },
            None,
            Utc::now(),
        );

        db.push_pending(&meta.session_id, QueueType::Turn, &first)
            .expect("push first");
        db.push_pending(&meta.session_id, QueueType::Turn, &second)
            .expect("push second");

        let removed = db
            .remove_pending_by_id(&meta.session_id, QueueType::Turn, &first.id)
            .expect("remove first");
        assert!(removed);
        let removed_again = db
            .remove_pending_by_id(&meta.session_id, QueueType::Turn, &first.id)
            .expect("remove first again");
        assert!(!removed_again);

        let remaining = db
            .drain_pending(&meta.session_id, QueueType::Turn)
            .expect("drain");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, second.id);
    }

    #[test]
    fn drain_pending_empty_returns_empty() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");
        db.upsert_session(&meta, None).expect("upsert");

        let drained = db
            .drain_pending(&meta.session_id, QueueType::Turn)
            .expect("drain");
        assert!(drained.is_empty());
    }

    #[test]
    fn session_index_backfill_required_detects_missing_rollout_path() {
        let (db, _dir) = test_db();
        let meta = sample_session("session-1");

        assert!(
            !db.session_index_backfill_required()
                .expect("check empty db")
        );
        db.upsert_session(&meta, None)
            .expect("upsert without rollout path");

        assert!(
            db.session_index_backfill_required()
                .expect("check missing rollout path")
        );
        db.upsert_rollout_index_session(&meta, Some("/tmp/session.jsonl".as_ref()))
            .expect("upsert rollout path");
        assert!(
            !db.session_index_backfill_required()
                .expect("check populated rollout path")
        );
    }

    #[test]
    fn list_root_sessions_excludes_subagents_and_ephemeral() {
        let (db, _dir) = test_db();
        let root_id = SessionId::new();
        let subagent_id = SessionId::new();
        let ephemeral_id = SessionId::new();
        let rollout_path = PathBuf::from("/tmp/root.jsonl");

        let mut root = sample_session(&root_id.to_string());
        root.session_id = root_id;
        db.upsert_session(&root, Some(rollout_path.as_path()))
            .expect("upsert root");

        let mut subagent = sample_session(&subagent_id.to_string());
        subagent.session_id = subagent_id;
        subagent.parent_session_id = Some(root_id);
        subagent.agent_path = Some("root/review".into());
        db.upsert_session(&subagent, Some("/tmp/subagent.jsonl".as_ref()))
            .expect("upsert subagent");

        let mut ephemeral = sample_session(&ephemeral_id.to_string());
        ephemeral.session_id = ephemeral_id;
        ephemeral.ephemeral = true;
        db.upsert_session(&ephemeral, None)
            .expect("upsert ephemeral");

        let roots = db.list_root_sessions().expect("list root sessions");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].session_id, root_id);

        let index = db
            .get_session_index(&root_id)
            .expect("get index")
            .expect("root index");
        assert_eq!(index.rollout_path, Some(rollout_path));
        assert_eq!(index.metadata.parent_session_id, None);

        let subagent_index = db
            .get_session_index(&subagent_id)
            .expect("get subagent index")
            .expect("subagent index");
        assert_eq!(subagent_index.metadata.parent_session_id, Some(root_id));
    }
}
