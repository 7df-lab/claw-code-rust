use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use tokio::sync::RwLock;
use tracing::warn;

use super::process::UnifiedExecProcess;
use super::{MAX_PROCESSES, WARNING_PROCESSES};

struct ProcessEntry {
    process: Arc<UnifiedExecProcess>,
    created_at: std::time::Instant,
}

pub struct ProcessStore {
    processes: RwLock<HashMap<i32, ProcessEntry>>,
    next_id: AtomicI32,
}

impl ProcessStore {
    pub fn new() -> Self {
        ProcessStore {
            processes: RwLock::new(HashMap::new()),
            next_id: AtomicI32::new(1000),
        }
    }

    pub async fn allocate(&self, process: Arc<UnifiedExecProcess>) -> i32 {
        let mut map = self.processes.write().await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);

        if map.len() >= MAX_PROCESSES {
            self.prune_locked(&mut map);
        }

        if map.len() >= MAX_PROCESSES {
            warn!("max unified exec processes ({MAX_PROCESSES}) reached, removing oldest");
            if let Some(oldest) = map.iter().min_by_key(|(_, e)| e.created_at) {
                let oldest_id = *oldest.0;
                if let Some(entry) = map.remove(&oldest_id) {
                    entry.process.terminate();
                }
            }
        }

        if map.len() >= WARNING_PROCESSES {
            warn!(
                "unified exec processes at {}/{} (warning threshold)",
                map.len(),
                MAX_PROCESSES
            );
        }

        map.insert(
            id,
            ProcessEntry {
                process,
                created_at: std::time::Instant::now(),
            },
        );
        id
    }

    pub async fn get(&self, id: i32) -> Option<Arc<UnifiedExecProcess>> {
        let map = self.processes.read().await;
        map.get(&id).map(|entry| Arc::clone(&entry.process))
    }

    pub async fn remove(&self, id: i32) {
        let mut map = self.processes.write().await;
        if let Some(entry) = map.remove(&id) {
            entry.process.terminate();
        }
    }

    pub async fn len(&self) -> usize {
        self.processes.read().await.len()
    }

    pub async fn prune_exited(&self) {
        let mut map = self.processes.write().await;
        self.prune_locked(&mut map);
    }

    fn prune_locked(&self, map: &mut HashMap<i32, ProcessEntry>) {
        let to_remove: Vec<i32> = map
            .iter()
            .filter(|(_, e)| !e.process.is_running())
            .map(|(id, _)| *id)
            .collect();
        for id in to_remove {
            if let Some(entry) = map.remove(&id) {
                entry.process.terminate();
            }
        }
    }
}

impl Default for ProcessStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unified_exec::process::UnifiedExecProcess;
    use std::path::Path;

    fn spawn_echo() -> UnifiedExecProcess {
        let (proc, _rx) = UnifiedExecProcess::spawn(
            1,
            if cfg!(windows) {
                "echo test"
            } else {
                "echo test"
            },
            Path::new("."),
            None,
            false,
        )
        .expect("spawn should succeed");
        proc
    }

    #[tokio::test]
    async fn store_allocate_and_get() {
        let store = ProcessStore::new();
        let proc = spawn_echo();
        let id = store.allocate(Arc::new(proc)).await;
        assert!(store.get(id).await.is_some());
        assert!(store.get(9999).await.is_none());
    }

    #[tokio::test]
    async fn store_remove_terminates() {
        let store = ProcessStore::new();
        let proc = spawn_echo();
        let id = store.allocate(Arc::new(proc)).await;
        store.remove(id).await;
        assert!(store.get(id).await.is_none());
    }

    #[tokio::test]
    async fn store_len() {
        let store = ProcessStore::new();
        assert_eq!(store.len().await, 0);

        let proc = spawn_echo();
        store.allocate(Arc::new(proc)).await;
        assert_eq!(store.len().await, 1);

        let proc = spawn_echo();
        store.allocate(Arc::new(proc)).await;
        assert_eq!(store.len().await, 2);
    }

    #[tokio::test]
    async fn store_default_is_empty() {
        let store = ProcessStore::default();
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn store_concurrent_allocate_and_get() {
        let store = Arc::new(ProcessStore::new());
        let mut handles = Vec::new();

        for _i in 0..10 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let proc = spawn_echo();
                let id = s.allocate(Arc::new(proc)).await;
                assert!(s.get(id).await.is_some());
                id
            }));
        }

        let ids: Vec<i32> = futures::future::join_all(handles)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(ids.len(), 10);
        // All IDs should be unique
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), 10);
        assert_eq!(store.len().await, 10);
    }

    #[tokio::test]
    async fn store_prune_exited_removes_nothing_for_running() {
        // This test is best-effort since processes might exit quickly
        let store = ProcessStore::new();
        let proc = spawn_echo();
        let _id = store.allocate(Arc::new(proc)).await;

        // Don't prune immediately — allow process to potentially still be running
        let count_before = store.len().await;
        store.prune_exited().await;
        let count_after = store.len().await;

        // Process may have exited, but at minimum len should not increase
        assert_eq!(count_before, count_after);
        assert!(count_before >= 1);
    }
}
