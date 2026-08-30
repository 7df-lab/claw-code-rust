//! Durable user-session fork: self-contained child rollout (Codex-aligned).
//!
//! User forks copy kept history into a new rollout and record lineage on
//! `fork_from_id` / `fork_at_turn_id`. Sub-agent parentage stays on
//! `parent_session_id`.

use std::collections::HashSet;

use chrono::Utc;
use tracing;

use super::super::*;
use crate::execution::PersistedTurnItem;
use crate::persistence::build_item_record;
use crate::runtime::handlers::session::RuntimeSessionTurnCutOptions;
use devo_core::SessionRecord;
use devo_core::TurnRecord;
use devo_protocol::native::rpc_session::RollbackMode;
use devo_protocol::native::rpc_session::SessionForkCut;

/// Options for creating a durable user fork.
pub(crate) struct DurableForkOptions {
    pub(crate) source_session_id: SessionId,
    pub(crate) fork_at_turn_id: Option<TurnId>,
    pub(crate) user_turn_index: Option<u32>,
    pub(crate) cut: SessionForkCut,
    pub(crate) title_override: Option<String>,
    pub(crate) cwd_override: Option<std::path::PathBuf>,
}

impl ServerRuntime {
    /// Builds a forked runtime session and persists a self-contained child
    /// rollout (session meta + kept turns/items + applicable compaction).
    pub(crate) async fn create_durable_user_fork(
        &self,
        source: &RuntimeSession,
        options: DurableForkOptions,
    ) -> Result<RuntimeSession, String> {
        let now = Utc::now();
        let forked_id = SessionId::new();
        let rollback_mode = match options.cut {
            SessionForkCut::Through => RollbackMode::ThroughUserTurn,
            SessionForkCut::Before => RollbackMode::BeforeUserTurn,
        };
        let mut forked_runtime = self
            .build_runtime_session_from_user_turn_cut(
                source,
                RuntimeSessionTurnCutOptions {
                    session_id: forked_id,
                    user_turn_index: options.user_turn_index,
                    rollback_mode,
                    cwd_override: options.cwd_override,
                    title_override: options.title_override,
                    created_at: now,
                },
            )
            .await?;

        // User forks are independent sessions — never reuse parent_session_id.
        forked_runtime.summary.parent_session_id = None;
        forked_runtime.summary.fork_from_id = Some(options.source_session_id);
        forked_runtime.summary.fork_at_turn_id = options.fork_at_turn_id;

        if forked_runtime.summary.ephemeral {
            return Ok(forked_runtime);
        }

        let mut record = self.rollout_store.create_session_record_with_fork(
            forked_id,
            now,
            forked_runtime.summary.cwd.clone(),
            forked_runtime.summary.additional_directories.clone(),
            forked_runtime.summary.title.clone(),
            forked_runtime.summary.model.clone(),
            forked_runtime.summary.model_binding_id.clone(),
            forked_runtime.summary.reasoning_effort_selection.clone(),
            forked_runtime.runtime_context.provider.name().to_string(),
            /*parent_session_id*/ None,
            Some(options.source_session_id),
            options.fork_at_turn_id,
        );
        if let Err(error) = self.rollout_store.append_session_meta(&record) {
            return Err(format!(
                "failed to persist forked session metadata: {error}"
            ));
        }

        if let Some(session_context) = {
            let core = forked_runtime.core_session.lock().await;
            core.session_context.clone()
        } {
            if let Err(error) = self
                .rollout_store
                .append_session_context_updated(&record, session_context)
            {
                tracing::warn!(
                    session_id = %forked_id,
                    error = %error,
                    "failed to persist forked session context"
                );
            } else {
                forked_runtime.session_context_recorded = true;
            }
        }

        write_kept_history_to_rollout(
            &self.rollout_store,
            &mut record,
            forked_id,
            &forked_runtime.persisted_turn_items,
            &forked_runtime.turn_records_by_id,
            forked_runtime.latest_compaction_snapshot.as_ref(),
        )?;

        forked_runtime.record = Some(record);
        Ok(forked_runtime)
    }
}

fn write_kept_history_to_rollout(
    rollout_store: &crate::persistence::RolloutStore,
    record: &mut SessionRecord,
    forked_id: SessionId,
    kept_items: &[PersistedTurnItem],
    turn_records_by_id: &std::collections::HashMap<TurnId, TurnRecord>,
    latest_compaction: Option<&devo_core::CompactionSnapshotLine>,
) -> Result<(), String> {
    let mut written_turns = HashSet::new();
    let mut item_seq = 1u64;
    for item in kept_items {
        if written_turns.insert(item.turn_id)
            && let Some(source_turn) = turn_records_by_id.get(&item.turn_id)
        {
            let mut turn = source_turn.clone();
            turn.session_id = forked_id;
            if let Err(error) = rollout_store.append_turn(record, turn) {
                return Err(format!("failed to persist forked turn: {error}"));
            }
        }
        let item_record = build_item_record(
            forked_id,
            item.turn_id,
            item.item_id,
            item_seq,
            item.turn_item.clone(),
            Some(devo_core::TurnStatus::Completed),
            None,
            None,
        );
        item_seq = item_seq.saturating_add(1);
        if let Err(error) = rollout_store.append_item(record, item_record) {
            return Err(format!("failed to persist forked item: {error}"));
        }
    }

    if let Some(snapshot) = latest_compaction
        && written_turns.contains(&snapshot.turn_id)
    {
        let mut snapshot = snapshot.clone();
        snapshot.session_id = forked_id;
        if let Err(error) = rollout_store.append_compaction_snapshot(record, snapshot) {
            tracing::warn!(
                session_id = %forked_id,
                error = %error,
                "failed to persist forked compaction snapshot"
            );
        }
    }
    Ok(())
}
