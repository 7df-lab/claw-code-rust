use super::*;
use std::path::Path;

use crate::execution::RuntimeSession;
use crate::runtime::session_actor::SessionActorState;

impl ServerRuntime {
    /// Replays one rollout file and applies SQLite side effects for a single session.
    pub(crate) async fn hydrate_runtime_session(
        self: &Arc<Self>,
        session_id: SessionId,
        rollout_path: &Path,
    ) -> anyhow::Result<RuntimeSession> {
        let mut runtime_session = self
            .rollout_store
            .load_session_from_rollout(rollout_path, &self.deps)
            .await?;
        if runtime_session.summary.session_id != session_id {
            anyhow::bail!(
                "rollout session id mismatch: expected {session_id}, got {}",
                runtime_session.summary.session_id
            );
        }
        self.apply_persisted_session_side_effects(session_id, &mut runtime_session)
            .await?;
        Ok(runtime_session)
    }

    async fn apply_persisted_session_side_effects(
        self: &Arc<Self>,
        session_id: SessionId,
        runtime_session: &mut RuntimeSession,
    ) -> anyhow::Result<()> {
        if !runtime_session.summary.ephemeral
            && let Err(err) = self.deps.db.upsert_session(
                &runtime_session.summary,
                runtime_session
                    .record
                    .as_ref()
                    .map(|record| record.rollout_path.as_path()),
            )
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "failed to seed restored session metadata to database"
            );
        }

        match self.deps.db.get_stats(&session_id) {
            Ok(Some(stats)) => {
                runtime_session.summary.total_input_tokens = stats.total_input_tokens;
                runtime_session.summary.total_output_tokens = stats.total_output_tokens;
                runtime_session.summary.total_tokens = stats.total_tokens;
                runtime_session.summary.total_cache_creation_tokens =
                    stats.total_cache_creation_tokens;
                runtime_session.summary.total_cache_read_tokens = stats.total_cache_read_tokens;
                runtime_session.summary.prompt_token_estimate = stats.prompt_token_estimate;
                if let Ok(mut core) = runtime_session.core_session.try_lock() {
                    core.total_input_tokens = stats.total_input_tokens;
                    core.total_output_tokens = stats.total_output_tokens;
                    core.total_tokens = stats.total_tokens;
                    core.total_cache_creation_tokens = stats.total_cache_creation_tokens;
                    core.total_cache_read_tokens = stats.total_cache_read_tokens;
                    core.prompt_token_estimate = stats.prompt_token_estimate;
                    // Align auto-compact pressure with UI / occupancy tip. Never
                    // inflate `last_turn_tokens` with turn-cumulative DB values —
                    // `last_input_tokens` is latest-query input only.
                    if let Some(occupancy) = stats.last_context_occupancy.as_ref() {
                        core.last_turn_tokens = occupancy.total_tokens as usize;
                    }
                    if stats.last_input_tokens > 0 {
                        core.last_input_tokens = stats.last_input_tokens;
                    }
                }
                if let Some(occupancy) = stats.last_context_occupancy {
                    runtime_session.summary.last_query_total_tokens =
                        occupancy.total_tokens as usize;
                    runtime_session.summary.last_context_occupancy = Some(occupancy);
                }
                tracing::debug!(
                    session_id = %session_id,
                    "restored token stats from database"
                );
            }
            Ok(None) => {
                let stats = crate::db::SessionStats {
                    total_input_tokens: runtime_session.summary.total_input_tokens,
                    total_output_tokens: runtime_session.summary.total_output_tokens,
                    total_tokens: runtime_session.summary.total_tokens,
                    total_cache_creation_tokens: runtime_session
                        .summary
                        .total_cache_creation_tokens,
                    total_cache_read_tokens: runtime_session.summary.total_cache_read_tokens,
                    last_input_tokens: 0,
                    turn_count: 0,
                    prompt_token_estimate: runtime_session.summary.prompt_token_estimate,
                    last_context_occupancy: runtime_session.summary.last_context_occupancy.clone(),
                };
                if let Err(err) = self.deps.db.update_stats(&session_id, &stats) {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "failed to persist initial token stats to database"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to load token stats from database"
                );
            }
        }

        // Restore the turn queue non-destructively: SQLite stays the durable
        // mirror until items are consumed or removed, so a restart before the
        // queue drains does not lose pending input.
        match self
            .deps
            .db
            .list_pending(&session_id, crate::db::QueueType::Turn)
        {
            Ok(items) => {
                if !items.is_empty() {
                    let core_session = runtime_session.core_session.lock().await;
                    let mut queue = core_session
                        .pending_turn_queue
                        .lock()
                        .expect("pending turn queue mutex should not be poisoned");
                    queue.extend(items);
                    tracing::debug!(
                        session_id = %session_id,
                        pending_count = queue.len(),
                        "restored pending turn queue from database"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to load pending turn queue from database"
                );
            }
        }

        // Stale steer inputs are no longer discarded (01 §4.3): they
        // degrade into the session turn queue like any other queued input.
        match self
            .deps
            .db
            .drain_pending(&session_id, crate::db::QueueType::Steer)
        {
            Ok(items) => {
                if !items.is_empty() {
                    let core_session = runtime_session.core_session.lock().await;
                    let mut queue = core_session
                        .pending_turn_queue
                        .lock()
                        .expect("pending turn queue mutex should not be poisoned");
                    for item in &items {
                        queue.push_back(item.clone());
                        if let Err(error) =
                            self.deps
                                .db
                                .push_pending(&session_id, crate::db::QueueType::Turn, item)
                        {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "failed to restore steer input into the turn queue"
                            );
                        }
                    }
                    tracing::debug!(
                        session_id = %session_id,
                        restored_steer_count = items.len(),
                        "degraded stale steer inputs into the pending turn queue"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to restore stale steer inputs from database"
                );
            }
        }

        match self.goal_durable_store.replay_goal_store(session_id).await {
            Ok(Some(mut goal_store)) => {
                if let Some(goal) = goal_store.get()
                    && goal.status == crate::goal::GoalStatus::Active
                {
                    let previous_status = goal.status;
                    match goal_store.set_status(devo_protocol::ThreadGoalStatus::Paused) {
                        Ok(paused_goal) => {
                            if let Err(error) = self
                                .goal_durable_store
                                .append_status_changed(
                                    &paused_goal,
                                    previous_status,
                                    Some(
                                        "Goal paused because the session was restored without explicit resume."
                                            .to_string(),
                                    ),
                                )
                                .await
                            {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %error,
                                    "failed to persist restored goal pause record"
                                );
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %error,
                                "failed to pause restored active goal"
                            );
                        }
                    }
                }
                self.goal_stores.lock().await.insert(session_id, goal_store);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to replay durable goal records"
                );
            }
        }

        Ok(())
    }

    /// Loads durable sessions from rollout files and installs them into the runtime map.
    /// Used by integration tests and bulk-restore tooling; production startup indexes only.
    pub async fn load_persisted_sessions(self: &Arc<Self>) -> anyhow::Result<()> {
        let sessions = self.rollout_store.load_sessions(&self.deps).await?;
        tracing::info!(session_count = sessions.len(), "loaded persisted sessions");

        for (session_id, mut runtime_session) in sessions {
            self.apply_persisted_session_side_effects(session_id, &mut runtime_session)
                .await?;
            if runtime_session.summary.parent_session_id.is_none() {
                self.insert_root_session_actor(runtime_session)
                    .await
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
            } else {
                self.insert_session_actor(SessionActorState::from_runtime_session(runtime_session))
                    .await;
            }
        }
        Ok(())
    }

    /// Rebuilds the SQLite session index from rollout SessionMeta headers.
    pub fn refresh_session_index(&self) -> anyhow::Result<()> {
        self.rollout_store.index_rollout_metadata(&self.deps.db)
    }

    /// Backfills rollout metadata into SQLite only when legacy rows lack index fields.
    pub fn backfill_session_index_if_required(&self) -> anyhow::Result<bool> {
        if !self.deps.db.session_index_backfill_required()? {
            return Ok(false);
        }
        self.rollout_store.index_rollout_metadata(&self.deps.db)?;
        Ok(true)
    }

    /// Completes deferred (in-progress) items for all active turns and
    /// persists interrupted turn records. Called on graceful shutdown.
    pub async fn shutdown(self: &Arc<Self>) {
        self.command_exec_manager.terminate_all().await;
        let session_handles = self.list_session_handles().await;

        for session_handle in session_handles {
            let session_id = session_handle.id();

            self.run_session_hook(
                session_id,
                devo_core::HookEvent::SessionEnd,
                serde_json::Map::from_iter([("reason".to_string(), serde_json::json!("other"))]),
            )
            .await;

            let Some(snapshot) = session_handle.take_shutdown_deferred_snapshot().await else {
                continue;
            };
            let Some(turn_id) = snapshot.active_turn_id else {
                continue;
            };

            if let Some((item_id, item_seq, text)) = snapshot.deferred_assistant
                && !text.trim().is_empty()
            {
                self.complete_item(
                    session_id,
                    turn_id,
                    item_id,
                    item_seq,
                    ItemKind::AgentMessage,
                    TurnItem::AgentMessage(TextItem { text: text.clone() }),
                    serde_json::json!({ "title": "Assistant", "text": text }),
                )
                .await;
            }
            if let Some((item_id, item_seq, text)) = snapshot.deferred_reasoning {
                self.complete_item(
                    session_id,
                    turn_id,
                    item_id,
                    item_seq,
                    ItemKind::Reasoning,
                    TurnItem::Reasoning(TextItem { text: text.clone() }),
                    serde_json::json!({ "title": "Reasoning", "text": text }),
                )
                .await;
            }

            let Some(interrupted_turn) = session_handle.interrupt_active_turn().await.flatten()
            else {
                continue;
            };
            if interrupted_turn.turn_id != turn_id {
                continue;
            }

            if snapshot.record.is_some()
                && let Err(error) = self
                    .persist_turn_line_deduped(session_id, &interrupted_turn)
                    .await
            {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to persist interrupted turn on shutdown"
                );
            }

            tracing::info!(
                session_id = %session_id,
                turn_id = %interrupted_turn.turn_id,
                "completed deferred items and interrupted turn on shutdown"
            );
        }

        let session_ids: Vec<SessionId> = self
            .list_session_handles()
            .await
            .into_iter()
            .map(|handle| handle.id())
            .collect();
        for session_id in session_ids {
            if let Some(handle) = self.sessions.lock().await.get(&session_id).cloned() {
                handle.shutdown().await;
            }
            self.remove_session_actor(session_id).await;
        }
    }
}
