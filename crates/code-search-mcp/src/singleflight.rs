//! Process-wide coordination for code-index builds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use crate::index::SearchIndex;
use crate::types::CodeSearchError;

type BuildResult = Result<Arc<SearchIndex>, CodeSearchError>;

struct BuildGate {
    result: Mutex<Option<BuildResult>>,
    completed: Condvar,
    waiters: AtomicUsize,
}

impl BuildGate {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            completed: Condvar::new(),
            waiters: AtomicUsize::new(0),
        }
    }

    fn wait(&self) -> BuildResult {
        self.waiters.fetch_add(1, Ordering::SeqCst);
        let mut result = self
            .result
            .lock()
            .map_err(|_| CodeSearchError::Index("index build gate lock poisoned".to_string()))?;
        while result.is_none() {
            result = self.completed.wait(result).map_err(|_| {
                CodeSearchError::Index("index build gate lock poisoned".to_string())
            })?;
        }
        let result = result
            .as_ref()
            .expect("completed build gate has a result")
            .clone();
        self.waiters.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn complete(&self, result: BuildResult) -> Result<(), CodeSearchError> {
        let mut slot = self
            .result
            .lock()
            .map_err(|_| CodeSearchError::Index("index build gate lock poisoned".to_string()))?;
        *slot = Some(result);
        self.completed.notify_all();
        Ok(())
    }
}

#[cfg(test)]
fn waiting_for(key: &str) -> usize {
    active_builds()
        .lock()
        .ok()
        .and_then(|builds| builds.get(key).and_then(Weak::upgrade))
        .map(|gate| gate.waiters.load(Ordering::SeqCst))
        .unwrap_or_default()
}

fn active_builds() -> &'static Mutex<HashMap<String, Weak<BuildGate>>> {
    static ACTIVE_BUILDS: OnceLock<Mutex<HashMap<String, Weak<BuildGate>>>> = OnceLock::new();
    ACTIVE_BUILDS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn run(key: String, build: impl FnOnce() -> BuildResult) -> BuildResult {
    let (gate, is_leader) = {
        let mut builds = active_builds().lock().map_err(|_| {
            CodeSearchError::Index("index build registry lock poisoned".to_string())
        })?;
        if let Some(gate) = builds.get(&key).and_then(Weak::upgrade) {
            (gate, false)
        } else {
            let gate = Arc::new(BuildGate::new());
            builds.insert(key.clone(), Arc::downgrade(&gate));
            (gate, true)
        }
    };

    if !is_leader {
        return gate.wait();
    }

    let result = build();
    gate.complete(result.clone())?;
    let mut builds = active_builds()
        .lock()
        .map_err(|_| CodeSearchError::Index("index build registry lock poisoned".to_string()))?;
    if builds
        .get(&key)
        .and_then(Weak::upgrade)
        .is_some_and(|current| Arc::ptr_eq(&current, &gate))
    {
        builds.remove(&key);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use pretty_assertions::assert_eq;

    use crate::cache::{CachedIndex, CachedIndexPayloadV4};
    use crate::matrix::EmbeddingMatrix;
    use crate::types::ContentFilter;

    use super::*;

    fn empty_index() -> Arc<SearchIndex> {
        let embeddings = EmbeddingMatrix::empty();
        let payload = CachedIndexPayloadV4::new(
            PathBuf::from("/repo"),
            ContentFilter::Code,
            "test".to_string(),
            &embeddings,
            Vec::new(),
        );
        Arc::new(
            SearchIndex::from_cached(CachedIndex {
                payload,
                embeddings,
            })
            .expect("index"),
        )
    }

    #[test]
    fn concurrent_callers_share_one_build_result() {
        let key = "singleflight-concurrent".to_string();
        let builds = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let leader_key = key.clone();
        let leader_builds = Arc::clone(&builds);
        let leader = std::thread::spawn(move || {
            run(leader_key, || {
                leader_builds.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).expect("signal build start");
                release_rx.recv().expect("release build");
                Ok(empty_index())
            })
            .expect("leader result")
        });
        started_rx.recv().expect("build started");

        let follower_key = key.clone();
        let follower_builds = Arc::clone(&builds);
        let follower = std::thread::spawn(move || {
            run(follower_key, || {
                follower_builds.fetch_add(1, Ordering::SeqCst);
                Ok(empty_index())
            })
            .expect("follower result")
        });
        for _ in 0..10_000 {
            if waiting_for(&key) == 1 {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(waiting_for(&key), 1);
        release_tx.send(()).expect("release leader");

        let leader_index = leader.join().expect("leader thread");
        let follower_index = follower.join().expect("follower thread");
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&leader_index, &follower_index));
    }

    #[test]
    fn failed_build_does_not_poison_later_retry() {
        let attempts = AtomicUsize::new(0);
        let key = "singleflight-retry".to_string();

        let first = run(key.clone(), || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err(CodeSearchError::Index("first failure".to_string()))
        });
        let second = run(key, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(empty_index())
        });

        let Err(first_error) = first else {
            panic!("first build should fail");
        };
        assert_eq!(first_error.to_string(), "index error: first failure");
        assert!(second.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
