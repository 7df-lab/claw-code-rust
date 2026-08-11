use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use tokio::sync::Mutex;

use devo_protocol::SessionId;

use crate::execution::RuntimeSession;
use crate::runtime::ServerRuntime;
use crate::runtime::session_actor::SessionActorState;
use crate::runtime::session_actor::SessionHandle;

pub(crate) const PARENT_SESSION_LRU_CAPACITY: usize = 16;

#[derive(Debug, Default)]
pub(crate) struct SessionLoadGate {
    locks: Mutex<HashMap<SessionId, Arc<Mutex<()>>>>,
}

pub(crate) struct SessionLoadPermit {
    session_id: SessionId,
    lock: Arc<Mutex<()>>,
    gate: Arc<SessionLoadGate>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl SessionLoadGate {
    pub(crate) async fn acquire(self: &Arc<Self>, session_id: SessionId) -> SessionLoadPermit {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(session_id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let guard = lock.clone().lock_owned().await;
        SessionLoadPermit {
            session_id,
            lock,
            gate: Arc::clone(self),
            guard: Some(guard),
        }
    }
}

impl Drop for SessionLoadPermit {
    fn drop(&mut self) {
        drop(self.guard.take());
        if Arc::strong_count(&self.lock) != 2 {
            return;
        }
        let session_id = self.session_id;
        let lock = Arc::clone(&self.lock);
        let gate = Arc::clone(&self.gate);
        tokio::spawn(async move {
            let mut locks = gate.locks.lock().await;
            if locks.get(&session_id).is_some_and(|existing| {
                Arc::ptr_eq(existing, &lock) && Arc::strong_count(existing) == 2
            }) {
                locks.remove(&session_id);
            }
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LoadSessionError {
    #[error("session not found")]
    SessionNotFound,
    #[error(
        "subagent sessions cannot be resumed directly; resume the parent session {parent_session_id} instead"
    )]
    SubagentNotResumable { parent_session_id: SessionId },
    #[error("session metadata exists but rollout file is missing; session cannot be restored")]
    RolloutMissing,
    #[error("failed to restore session: {0}")]
    RestoreFailed(String),
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ParentSessionLru {
    order: VecDeque<SessionId>,
    capacity: usize,
}

impl ParentSessionLru {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            order: VecDeque::new(),
            capacity,
        }
    }

    pub(crate) fn touch(&mut self, session_id: SessionId) {
        self.order.retain(|id| *id != session_id);
        self.order.push_front(session_id);
    }

    pub(crate) fn remove(&mut self, session_id: SessionId) {
        self.order.retain(|id| *id != session_id);
    }

    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }
}

impl ServerRuntime {
    pub(crate) async fn get_or_load_parent_session(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> Result<SessionHandle, LoadSessionError> {
        if let Some(handle) = self.session(session_id).await {
            self.touch_parent_session_lru(session_id).await;
            return Ok(handle);
        }

        let _load_permit = self.parent_session_load_gate.acquire(session_id).await;
        if let Some(handle) = self.session(session_id).await {
            self.touch_parent_session_lru(session_id).await;
            return Ok(handle);
        }

        let index = self
            .deps
            .db
            .get_session_index(&session_id)
            .map_err(|error| LoadSessionError::RestoreFailed(error.to_string()))?
            .ok_or(LoadSessionError::SessionNotFound)?;

        if index.metadata.agent_path.is_some() {
            let parent_session_id = index
                .metadata
                .parent_session_id
                .ok_or(LoadSessionError::SessionNotFound)?;
            return Err(LoadSessionError::SubagentNotResumable { parent_session_id });
        }

        let stored_rollout_path = index.rollout_path.clone();
        let rollout_path = match stored_rollout_path {
            Some(ref path) if path.exists() => path.clone(),
            Some(_) | None => self
                .rollout_store
                .find_rollout_by_session_id(&session_id)
                .map_err(|error| LoadSessionError::RestoreFailed(error.to_string()))?
                .filter(|path| path.exists())
                .ok_or(LoadSessionError::RolloutMissing)?,
        };

        if stored_rollout_path.as_ref() != Some(&rollout_path)
            && let Err(error) = self
                .deps
                .db
                .upsert_rollout_index_session(&index.metadata, Some(rollout_path.as_path()))
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to backfill rollout_path after suffix lookup"
            );
        }

        let runtime_session = self
            .hydrate_runtime_session(session_id, &rollout_path)
            .await
            .map_err(|error| LoadSessionError::RestoreFailed(error.to_string()))?;
        self.insert_root_session_actor(runtime_session).await
    }

    pub(crate) async fn insert_root_session_actor(
        self: &Arc<Self>,
        runtime_session: RuntimeSession,
    ) -> Result<SessionHandle, LoadSessionError> {
        let session_id = runtime_session.summary.session_id;
        let handle = self
            .insert_session_actor(SessionActorState::from_runtime_session(runtime_session))
            .await;
        self.touch_parent_session_lru(session_id).await;
        self.evict_parent_sessions_if_needed(Some(session_id)).await;
        Ok(handle)
    }

    pub(crate) async fn after_root_session_insert(self: &Arc<Self>, session_id: SessionId) {
        self.touch_parent_session_lru(session_id).await;
        self.evict_parent_sessions_if_needed(Some(session_id)).await;
    }

    pub(crate) async fn touch_parent_session_lru(&self, session_id: SessionId) {
        let mut lru = self.session_lru.lock().await;
        lru.touch(session_id);
    }

    async fn evict_parent_sessions_if_needed(self: &Arc<Self>, exclude: Option<SessionId>) {
        let candidates: Vec<SessionId> = {
            let lru = self.session_lru.lock().await;
            if lru.len() <= lru.capacity {
                return;
            }
            lru.order.iter().rev().copied().collect()
        };
        for session_id in candidates {
            if exclude == Some(session_id) {
                continue;
            }
            if self.is_parent_session_pinned(session_id).await {
                continue;
            }
            self.evict_parent_session_cascade(session_id).await;
            return;
        }
    }

    pub(crate) async fn is_parent_session_pinned(&self, session_id: SessionId) -> bool {
        if self.active_turns.has_session(session_id).await {
            return true;
        }
        let connections = self.connections.lock().await;
        for connection in connections.values() {
            if connection
                .subscriptions
                .iter()
                .any(|subscription| subscription.session_id == Some(session_id))
            {
                return true;
            }
        }
        false
    }

    pub(crate) async fn evict_parent_session_cascade(&self, parent_session_id: SessionId) {
        let child_session_ids = {
            let registries = self.agent_registries.lock().await;
            registries
                .get(&parent_session_id)
                .map(|registry| registry.children_of(parent_session_id))
                .unwrap_or_default()
        };

        for child_session_id in child_session_ids {
            if let Some(handle) = self.remove_session_actor(child_session_id).await {
                handle.shutdown().await;
            }
            self.goal_stores.lock().await.remove(&child_session_id);
        }

        if let Some(handle) = self.remove_session_actor(parent_session_id).await {
            handle.shutdown().await;
        }
        self.goal_stores.lock().await.remove(&parent_session_id);
        self.session_lru.lock().await.remove(parent_session_id);
        self.agent_registries
            .lock()
            .await
            .remove(&parent_session_id);
        self.agent_mailboxes.lock().await.remove(&parent_session_id);
        self.agent_output_buffers
            .lock()
            .await
            .remove(&parent_session_id);
        self.agent_wait_cursors
            .lock()
            .await
            .remove(&parent_session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_session_lru_tracks_recency_for_eviction() {
        let mut lru = ParentSessionLru::new(2);
        let first = SessionId::new();
        let second = SessionId::new();
        let third = SessionId::new();
        lru.touch(first);
        lru.touch(second);
        lru.touch(third);
        assert_eq!(lru.len(), 3);
        lru.touch(second);
        assert_eq!(lru.len(), 3);
    }

    #[test]
    fn parent_session_lru_capacity_allows_exact_capacity() {
        let mut lru = ParentSessionLru::new(PARENT_SESSION_LRU_CAPACITY);
        for _ in 0..PARENT_SESSION_LRU_CAPACITY {
            lru.touch(SessionId::new());
        }
        assert_eq!(lru.len(), PARENT_SESSION_LRU_CAPACITY);
    }
}
