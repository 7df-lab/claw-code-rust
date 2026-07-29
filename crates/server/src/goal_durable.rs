use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::Utc;
use devo_core::DurableRecord;
use devo_core::GoalBudgetAccountedRecord;
use devo_core::GoalClearedRecord;
use devo_core::GoalContextSnapshotRecordedRecord;
use devo_core::GoalCreatedRecord;
use devo_core::GoalProgressRecordedRecord;
use devo_core::GoalProgressType;
use devo_core::GoalStatusChangedRecord;
use devo_core::JsonlSessionStore;
use devo_core::SessionStore;
use devo_core::StoreErrorCode;
use devo_protocol::SessionId;
use devo_protocol::TurnId;

use crate::db::Database;
use crate::goal::Goal;
use crate::goal::GoalBudget;
use crate::goal::GoalId;
use crate::goal::GoalStatus;
use crate::goal::GoalUsage;
use crate::goal::TurnRef;
use crate::persistence::RolloutStore;
use crate::runtime::GoalStore;

#[derive(Clone)]
pub(crate) struct GoalDurableStore {
    store: JsonlSessionStore,
    primary: Option<(RolloutStore, Arc<Database>)>,
}

impl GoalDurableStore {
    #[cfg(test)]
    pub(crate) fn new(server_home: PathBuf) -> Self {
        Self {
            store: JsonlSessionStore::new(server_home.join("goal-records")),
            primary: None,
        }
    }

    pub(crate) fn with_primary(
        server_home: PathBuf,
        rollout_store: RolloutStore,
        db: Arc<Database>,
    ) -> Self {
        Self {
            store: JsonlSessionStore::new(server_home.join("goal-records")),
            primary: Some((rollout_store, db)),
        }
    }

    pub(crate) async fn append_goal_created(&self, goal: &Goal) -> Result<()> {
        if self.append_primary_goal(goal.session_id, Some(goal))? {
            return Ok(());
        }
        let record = DurableRecord::GoalCreated(GoalCreatedRecord {
            schema_version: 1,
            goal_id: goal.durable_goal_id,
            session_id: goal.session_id,
            turn_id: goal
                .created_turn_id
                .map(|turn_ref| turn_ref.turn_id)
                .unwrap_or_default(),
            prompt: goal.prompt.clone(),
            description: goal.description.clone(),
            max_iterations: goal.budget.max_turns,
            budget: durable_budget_from_goal(&goal.budget),
            created_at: goal.created_at,
        });
        self.store.append(goal.session_id, record).await?;
        Ok(())
    }

    pub(crate) async fn append_status_changed(
        &self,
        goal: &Goal,
        previous_status: GoalStatus,
        reason: Option<String>,
    ) -> Result<()> {
        if self.append_primary_goal(goal.session_id, Some(goal))? {
            return Ok(());
        }
        let record = DurableRecord::GoalStatusChanged(GoalStatusChangedRecord {
            schema_version: 1,
            goal_id: goal.durable_goal_id,
            session_id: goal.session_id,
            previous_status: durable_status_from_goal(previous_status),
            new_status: durable_status_from_goal(goal.status),
            reason: reason.or_else(|| durable_status_reason(goal.status)),
            changed_at: goal.updated_at,
        });
        self.store.append(goal.session_id, record).await?;
        Ok(())
    }

    pub(crate) async fn append_budget_accounted(
        &self,
        goal: &Goal,
        turn_id: TurnId,
        token_delta: i64,
        turn_delta: u32,
        duration_delta_seconds: u64,
    ) -> Result<()> {
        if self.append_primary_goal(goal.session_id, Some(goal))? {
            return Ok(());
        }
        let record = DurableRecord::GoalBudgetAccounted(GoalBudgetAccountedRecord {
            schema_version: 1,
            goal_id: goal.durable_goal_id,
            session_id: goal.session_id,
            turn_id,
            budget_delta: devo_core::GoalBudget {
                max_turns: (turn_delta > 0).then_some(turn_delta),
                max_tokens: (token_delta > 0).then_some(token_delta),
                max_duration_seconds: (duration_delta_seconds > 0)
                    .then_some(duration_delta_seconds),
            },
            remaining_budget: remaining_budget(goal),
            recorded_at: Utc::now(),
        });
        self.store.append(goal.session_id, record).await?;
        Ok(())
    }

    pub(crate) async fn append_context_snapshot(
        &self,
        goal: &Goal,
        snapshot_id: String,
        summary: String,
    ) -> Result<()> {
        if self.append_primary_goal(goal.session_id, Some(goal))? {
            return Ok(());
        }
        let record =
            DurableRecord::GoalContextSnapshotRecorded(GoalContextSnapshotRecordedRecord {
                schema_version: 1,
                goal_id: goal.durable_goal_id,
                session_id: goal.session_id,
                snapshot_id,
                summary,
                recorded_at: Utc::now(),
            });
        self.store.append(goal.session_id, record).await?;
        Ok(())
    }

    pub(crate) async fn append_goal_cleared(
        &self,
        session_id: SessionId,
        goal_id: devo_core::GoalId,
        reason: Option<String>,
    ) -> Result<()> {
        if self.append_primary_goal(session_id, None)? {
            return Ok(());
        }
        let record = DurableRecord::GoalCleared(GoalClearedRecord {
            schema_version: 1,
            goal_id,
            session_id,
            reason,
            cleared_at: Utc::now(),
        });
        self.store.append(session_id, record).await?;
        Ok(())
    }

    pub(crate) async fn replay_goal_store(
        &self,
        session_id: SessionId,
    ) -> Result<Option<GoalStore>> {
        if let Some(goal) = self.replay_primary_goal(session_id)? {
            return Ok(goal.map(|goal| GoalStore {
                active_goal: Some(goal),
            }));
        }
        let mut replay = match self.store.replay(session_id, /*from_offset*/ 0).await {
            Ok(replay) => replay,
            Err(error) if error.code == StoreErrorCode::SessionNotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let records = replay.collect().await;
        let mut goal: Option<Goal> = None;

        for record in records {
            match record {
                DurableRecord::GoalCreated(record) => {
                    goal = Some(Goal {
                        goal_id: GoalId::from_durable(record.goal_id),
                        durable_goal_id: record.goal_id,
                        session_id: record.session_id,
                        prompt: record.prompt,
                        description: record.description,
                        status: GoalStatus::Active,
                        created_turn_id: Some(TurnRef {
                            turn_id: record.turn_id,
                            sequence: 0,
                        }),
                        created_at: record.created_at,
                        updated_at: record.created_at,
                        budget: goal_budget_from_durable(record.budget),
                        usage: GoalUsage::default(),
                        progress_summary: None,
                        blocker_summary: None,
                        verification_summary: None,
                    });
                }
                DurableRecord::GoalStatusChanged(record) => {
                    if let Some(goal) = goal
                        .as_mut()
                        .filter(|goal| goal.durable_goal_id == record.goal_id)
                    {
                        goal.status =
                            goal_status_from_durable(record.new_status, record.reason.as_deref());
                        match record.new_status {
                            devo_core::GoalStatus::Completed => {
                                goal.verification_summary = record.reason;
                            }
                            devo_core::GoalStatus::Paused
                            | devo_core::GoalStatus::Blocked
                            | devo_core::GoalStatus::Failed => {
                                goal.blocker_summary = record.reason;
                            }
                            devo_core::GoalStatus::Active
                            | devo_core::GoalStatus::Canceled
                            | devo_core::GoalStatus::Cleared => {}
                        }
                        goal.updated_at = record.changed_at;
                    }
                }
                DurableRecord::GoalBudgetAccounted(record) => {
                    if let Some(goal) = goal
                        .as_mut()
                        .filter(|goal| goal.durable_goal_id == record.goal_id)
                    {
                        goal.usage.turns_used += record.budget_delta.max_turns.unwrap_or_default();
                        goal.usage.tokens_used +=
                            record.budget_delta.max_tokens.unwrap_or_default();
                        goal.usage.duration_seconds +=
                            record.budget_delta.max_duration_seconds.unwrap_or_default();
                        goal.updated_at = record.recorded_at;
                    }
                }
                DurableRecord::GoalProgressRecorded(record) => {
                    if let Some(goal) = goal
                        .as_mut()
                        .filter(|goal| goal.durable_goal_id == record.goal_id)
                    {
                        apply_progress_record(goal, record);
                    }
                }
                DurableRecord::GoalCleared(record) => {
                    if goal
                        .as_ref()
                        .is_some_and(|goal| goal.durable_goal_id == record.goal_id)
                    {
                        goal = None;
                    }
                }
                DurableRecord::GoalReplaced(_)
                | DurableRecord::GoalContextSnapshotRecorded(_)
                | DurableRecord::SessionCreated(_)
                | DurableRecord::SessionForked(_)
                | DurableRecord::SessionMetadataUpdated(_)
                | DurableRecord::SessionDeleted(_)
                | DurableRecord::TurnStarted(_)
                | DurableRecord::TurnCompleted(_)
                | DurableRecord::TurnFailed(_)
                | DurableRecord::TurnInterrupted(_)
                | DurableRecord::ItemStarted(_)
                | DurableRecord::ItemContentAppended(_)
                | DurableRecord::ItemCompleted(_)
                | DurableRecord::ItemFailed(_)
                | DurableRecord::SteerRecorded(_)
                | DurableRecord::QueueItemRecorded(_)
                | DurableRecord::QueueItemResolved(_)
                | DurableRecord::TurnInterruptRequested(_)
                | DurableRecord::UsageRecorded(_)
                | DurableRecord::MessageEditRecorded(_)
                | DurableRecord::TurnSuperseded(_)
                | DurableRecord::TurnWorkspaceCheckpointRecorded(_)
                | DurableRecord::TurnWorkspaceChangeRecorded(_)
                | DurableRecord::TurnWorkspaceRestoreStarted(_)
                | DurableRecord::TurnWorkspaceRestoreCompleted(_)
                | DurableRecord::TurnResumeStarted(_)
                | DurableRecord::PlanCreated(_)
                | DurableRecord::PlanUpdated(_)
                | DurableRecord::SubagentSpawned(_)
                | DurableRecord::MemoryLinkRecorded(_)
                | DurableRecord::SubagentClosed(_)
                | DurableRecord::SubagentMailRecorded(_)
                | DurableRecord::SubagentStatusChanged(_)
                | DurableRecord::SubagentNotificationRecorded(_)
                | DurableRecord::BackgroundProcessUpdated(_)
                | DurableRecord::ContextSnapshotRecorded(_)
                | DurableRecord::ContextCompactionStarted(_)
                | DurableRecord::ContextCompactionCompleted(_) => {}
            }
        }

        Ok(goal.map(|goal| GoalStore {
            active_goal: Some(goal),
        }))
    }

    /// Returns `Some(snapshot)` when the main rollout contains any GoalState
    /// record. The nested option distinguishes an explicit clear from no
    /// primary record (which must fall back to legacy goal-records).
    fn replay_primary_goal(&self, session_id: SessionId) -> Result<Option<Option<Goal>>> {
        let Some((_, db)) = &self.primary else {
            return Ok(None);
        };
        let Some(index) = db.get_session_index(&session_id)? else {
            return Ok(None);
        };
        let Some(rollout_path) = index.rollout_path else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(&rollout_path)
            .with_context(|| format!("read goal state from {}", rollout_path.display()))?;
        let mut found = false;
        let mut current = None;
        let lines = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>();
        for (index, raw) in lines.iter().enumerate() {
            let parsed = match devo_core::parse_rollout_line(raw) {
                Ok(parsed) => parsed,
                Err(devo_core::RolloutLineReadError::TruncatedTail) if index + 1 == lines.len() => {
                    break;
                }
                Err(error) => return Err(anyhow::anyhow!("parse goal rollout: {error}")),
            };
            let devo_core::ParsedRolloutLine::V2(line) = parsed else {
                continue;
            };
            if let devo_core::RolloutLineV2::Internal {
                entry:
                    devo_core::InternalRecordV2::GoalState {
                        schema_version,
                        goal,
                    },
                ..
            } = *line
            {
                anyhow::ensure!(
                    schema_version == 1,
                    "unsupported Goal snapshot schema version {schema_version}"
                );
                found = true;
                current = goal
                    .map(serde_json::from_value)
                    .transpose()
                    .context("decode primary goal snapshot")?;
            }
        }
        Ok(found.then_some(current))
    }

    fn append_primary_goal(&self, session_id: SessionId, goal: Option<&Goal>) -> Result<bool> {
        let Some((rollout_store, db)) = &self.primary else {
            return Ok(false);
        };
        let index = db
            .get_session_index(&session_id)?
            .context("session index missing while persisting goal")?;
        let rollout_path = index
            .rollout_path
            .context("session rollout path missing while persisting goal")?;
        let goal = goal.map(serde_json::to_value).transpose()?;
        rollout_store.append_goal_state(&rollout_path, session_id, goal)?;
        Ok(true)
    }
}

fn durable_budget_from_goal(budget: &GoalBudget) -> Option<devo_core::GoalBudget> {
    (budget.max_turns.is_some()
        || budget.max_tokens.is_some()
        || budget.max_duration_seconds.is_some())
    .then_some(devo_core::GoalBudget {
        max_turns: budget.max_turns,
        max_tokens: budget.max_tokens,
        max_duration_seconds: budget.max_duration_seconds,
    })
}

fn goal_budget_from_durable(budget: Option<devo_core::GoalBudget>) -> GoalBudget {
    let Some(budget) = budget else {
        return GoalBudget::default();
    };
    GoalBudget {
        max_turns: budget.max_turns,
        max_tokens: budget.max_tokens,
        max_duration_seconds: budget.max_duration_seconds,
    }
}

fn remaining_budget(goal: &Goal) -> devo_core::GoalBudget {
    devo_core::GoalBudget {
        max_turns: goal
            .budget
            .max_turns
            .map(|budget| budget.saturating_sub(goal.usage.turns_used)),
        max_tokens: goal
            .budget
            .max_tokens
            .map(|budget| budget.saturating_sub(goal.usage.tokens_used)),
        max_duration_seconds: goal
            .budget
            .max_duration_seconds
            .map(|budget| budget.saturating_sub(goal.usage.duration_seconds)),
    }
}

fn durable_status_from_goal(status: GoalStatus) -> devo_core::GoalStatus {
    match status {
        GoalStatus::Active => devo_core::GoalStatus::Active,
        GoalStatus::Paused => devo_core::GoalStatus::Paused,
        GoalStatus::BudgetLimited => devo_core::GoalStatus::Blocked,
        GoalStatus::Completed => devo_core::GoalStatus::Completed,
        GoalStatus::Failed => devo_core::GoalStatus::Failed,
        GoalStatus::Blocked => devo_core::GoalStatus::Blocked,
        GoalStatus::Canceled => devo_core::GoalStatus::Canceled,
        GoalStatus::Cleared => devo_core::GoalStatus::Cleared,
    }
}

fn goal_status_from_durable(status: devo_core::GoalStatus, reason: Option<&str>) -> GoalStatus {
    match status {
        devo_core::GoalStatus::Active => GoalStatus::Active,
        devo_core::GoalStatus::Paused => GoalStatus::Paused,
        devo_core::GoalStatus::Completed => GoalStatus::Completed,
        devo_core::GoalStatus::Failed => GoalStatus::Failed,
        devo_core::GoalStatus::Blocked if reason == Some("budget_limited") => {
            GoalStatus::BudgetLimited
        }
        devo_core::GoalStatus::Blocked => GoalStatus::Blocked,
        devo_core::GoalStatus::Canceled => GoalStatus::Canceled,
        devo_core::GoalStatus::Cleared => GoalStatus::Cleared,
    }
}

fn durable_status_reason(status: GoalStatus) -> Option<String> {
    (status == GoalStatus::BudgetLimited).then(|| "budget_limited".to_string())
}

fn apply_progress_record(goal: &mut Goal, record: GoalProgressRecordedRecord) {
    match record.progress_type {
        GoalProgressType::Blocked => goal.blocker_summary = Some(record.summary),
        GoalProgressType::Milestone | GoalProgressType::PhaseComplete | GoalProgressType::Note => {
            goal.progress_summary = Some(record.summary);
        }
    }
    goal.updated_at = record.recorded_at;
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::io::Write;
    use tempfile::TempDir;

    fn active_goal(session_id: SessionId) -> Goal {
        Goal::from_create_params(devo_protocol::GoalCreateParams {
            session_id,
            objective: "finish durable goals".to_string(),
            token_budget: Some(100),
            replace_existing: false,
        })
        .expect("goal")
    }

    #[tokio::test]
    async fn primary_goal_snapshot_round_trips_without_goal_records_writer() {
        let temp = TempDir::new().expect("temp dir");
        let db = Arc::new(Database::open(temp.path().join("devo.db")).expect("open database"));
        let rollout_store = RolloutStore::new(temp.path().to_path_buf(), Some(Arc::clone(&db)));
        let session_id = SessionId::new();
        let record = rollout_store.create_session_record(
            session_id,
            Utc::now(),
            temp.path().to_path_buf(),
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
        let metadata = crate::persistence::session_metadata_from_record(&record, record.created_at);
        db.upsert_session(&metadata, Some(record.rollout_path.as_path()))
            .expect("index session");
        let store = GoalDurableStore::with_primary(
            temp.path().to_path_buf(),
            rollout_store.clone(),
            Arc::clone(&db),
        );
        let mut goal = active_goal(session_id);
        store
            .append_goal_created(&goal)
            .await
            .expect("append primary goal");
        goal.status = GoalStatus::Paused;
        goal.blocker_summary = Some("waiting".into());
        goal.updated_at = Utc::now();
        store
            .append_status_changed(&goal, GoalStatus::Active, Some("waiting".into()))
            .await
            .expect("append primary status");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&record.rollout_path)
            .expect("open rollout crash tail")
            .write_all(br#"{"v":2,"kind":"internal""#)
            .expect("append truncated crash tail");

        assert!(
            !temp.path().join("goal-records").exists(),
            "new Goal facts must not create the legacy store"
        );
        let restarted = GoalDurableStore::with_primary(
            temp.path().to_path_buf(),
            RolloutStore::new(temp.path().to_path_buf(), Some(Arc::clone(&db))),
            db,
        );
        let replayed = restarted
            .replay_goal_store(session_id)
            .await
            .expect("replay primary")
            .expect("goal");
        assert_eq!(replayed.get(), Some(&goal));
    }

    #[tokio::test]
    async fn goal_durable_store_replays_current_goal_projection() {
        // Trace: L2-DES-GOAL-001
        let temp = TempDir::new().expect("temp dir");
        let store = GoalDurableStore::new(temp.path().to_path_buf());
        let session_id = SessionId::new();
        let mut goal = active_goal(session_id);
        goal.created_turn_id = Some(TurnRef {
            turn_id: TurnId::new(),
            sequence: 0,
        });

        store
            .append_goal_created(&goal)
            .await
            .expect("append created");
        goal.usage.record_turn();
        goal.usage.record_tokens(17);
        store
            .append_budget_accounted(
                &goal,
                TurnId::new(),
                /*token_delta*/ 17,
                /*turn_delta*/ 1,
                /*duration_delta_seconds*/ 0,
            )
            .await
            .expect("append budget");
        let previous_status = goal.status;
        goal.status = GoalStatus::Completed;
        goal.verification_summary = Some("verified".to_string());
        goal.updated_at = Utc::now();
        store
            .append_status_changed(&goal, previous_status, Some("verified".to_string()))
            .await
            .expect("append status");

        let replayed = store
            .replay_goal_store(session_id)
            .await
            .expect("replay")
            .expect("store");

        assert_eq!(replayed.active_goal, Some(goal));
    }

    #[tokio::test]
    async fn goal_durable_store_replays_clear_as_no_goal() {
        // Trace: L2-DES-GOAL-001
        let temp = TempDir::new().expect("temp dir");
        let store = GoalDurableStore::new(temp.path().to_path_buf());
        let session_id = SessionId::new();
        let goal = active_goal(session_id);

        store
            .append_goal_created(&goal)
            .await
            .expect("append created");
        store
            .append_goal_cleared(
                session_id,
                goal.durable_goal_id,
                Some("user clear".to_string()),
            )
            .await
            .expect("append clear");

        let replayed = store.replay_goal_store(session_id).await.expect("replay");

        assert!(replayed.is_none());
    }

    #[tokio::test]
    async fn goal_durable_store_replays_paused_reason_as_blocker() {
        // Trace: L2-DES-GOAL-001
        let temp = TempDir::new().expect("temp dir");
        let store = GoalDurableStore::new(temp.path().to_path_buf());
        let session_id = SessionId::new();
        let mut goal = active_goal(session_id);
        goal.created_turn_id = Some(TurnRef {
            turn_id: TurnId::new(),
            sequence: 0,
        });

        store
            .append_goal_created(&goal)
            .await
            .expect("append created");
        let previous_status = goal.status;
        goal.status = GoalStatus::Paused;
        goal.blocker_summary = Some("provider 400 bad request".to_string());
        goal.updated_at = Utc::now();
        store
            .append_status_changed(&goal, previous_status, goal.blocker_summary.clone())
            .await
            .expect("append status");

        let replayed = store
            .replay_goal_store(session_id)
            .await
            .expect("replay")
            .expect("store");

        assert_eq!(replayed.active_goal, Some(goal));
    }
}
