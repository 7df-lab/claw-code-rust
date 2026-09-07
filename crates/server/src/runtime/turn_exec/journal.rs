//! Persist-first tool facts written outside the session actor.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use devo_core::durable_execution::{
    ExecutionRecord, ExecutionReplay, ToolIntentJournal, read_execution_replay,
};
use devo_core::{InternalRecordV2, RolloutLineV2, SessionId, TurnId};
use tokio::sync::Mutex;

use super::super::ServerRuntime;

pub(crate) struct RolloutToolJournal {
    runtime: Arc<ServerRuntime>,
    path: PathBuf,
    session_id: SessionId,
    turn_id: TurnId,
    committed: Mutex<Option<ExecutionReplay>>,
}

impl RolloutToolJournal {
    pub(crate) fn new(
        runtime: Arc<ServerRuntime>,
        path: PathBuf,
        session_id: SessionId,
        turn_id: TurnId,
    ) -> Self {
        Self {
            runtime,
            path,
            session_id,
            turn_id,
            committed: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ToolIntentJournal for RolloutToolJournal {
    async fn replay(&self) -> anyhow::Result<ExecutionReplay> {
        let path = self.path.clone();
        let turn_id = self.turn_id;
        tokio::task::spawn_blocking(move || read_execution_replay(&path, turn_id)).await?
    }

    async fn commit(&self, record: ExecutionRecord) -> anyhow::Result<()> {
        let mut committed = self.committed.lock().await;
        if committed.is_none() {
            let path = self.path.clone();
            let turn_id = self.turn_id;
            *committed = Some(
                tokio::task::spawn_blocking(move || read_execution_replay(&path, turn_id))
                    .await??,
            );
        }
        let previous = committed.as_ref().expect("journal initialized");
        let mut updated = previous.clone();
        updated.apply(&record)?;
        if updated.artifacts == previous.artifacts
            && updated.completed == previous.completed
            && updated.stop_reason == previous.stop_reason
            && updated.items == previous.items
            && updated.recovery == previous.recovery
            && !matches!(record, ExecutionRecord::PromptCheckpoint { .. })
        {
            return Ok(());
        }
        let runtime = Arc::clone(&self.runtime);
        let path = self.path.clone();
        let line = RolloutLineV2::Internal {
            v: 2,
            timestamp: chrono::Utc::now(),
            session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(
                self.session_id.into(),
            ),
            turn_id: Some(devo_protocol::native::ids::TurnId::from_legacy_uuid(
                self.turn_id.into(),
            )),
            seq: 0,
            entry: InternalRecordV2::Execution { record },
        };
        tokio::task::spawn_blocking(move || {
            runtime.rollout_store.append_v2_lines(&path, vec![line])
        })
        .await??;
        *committed = Some(updated);
        Ok(())
    }
}
