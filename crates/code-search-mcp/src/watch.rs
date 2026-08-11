//! Filesystem watcher state for warm index reuse.
//!
//! The watcher is intentionally coarse: any filesystem event under the indexed
//! root marks the warm index dirty. The service never tries to interpret events;
//! it simply falls back to manifest discovery when the flag is dirty, stale, or
//! unavailable. That makes watcher clean state a latency optimization, not a
//! correctness requirement.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use crate::types::CodeSearchError;

/// Tracks whether a watched index root has seen filesystem events since indexing.
///
/// Implementations use a single dirty flag: any event marks the associated index
/// stale, and the service clears the flag only by replacing the watcher after a
/// manifest refresh. If watcher setup is unavailable, callers must treat the
/// index as unable to skip manifest checks.
pub struct IndexWatcher {
    dirty: Arc<AtomicBool>,
    available: bool,
    _watcher: Option<RecommendedWatcher>,
}

impl IndexWatcher {
    /// Starts a recursive watcher or returns an unavailable watcher on failure.
    ///
    /// Watch setup failure is expected on some platforms or filesystems, so the
    /// caller should continue with manifest checks instead of treating it as a
    /// hard indexing failure.
    pub fn watch(root: &Path) -> Self {
        Self::try_watch(root).unwrap_or_else(|_| Self::unavailable())
    }

    /// Starts a recursive watcher and reports setup errors.
    ///
    /// The callback only flips an atomic flag. Refresh creates a new watcher
    /// rather than clearing the old flag, which avoids races where an event lands
    /// while a manifest refresh is in flight.
    pub fn try_watch(root: &Path) -> Result<Self, CodeSearchError> {
        let dirty = Arc::new(AtomicBool::new(false));
        let handler_dirty = Arc::clone(&dirty);
        let mut watcher = RecommendedWatcher::new(
            move |event: Result<notify::Event, notify::Error>| {
                if event.is_ok() {
                    handler_dirty.store(true, Ordering::SeqCst);
                }
            },
            Config::default(),
        )
        .map_err(|error| CodeSearchError::Index(error.to_string()))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| CodeSearchError::Index(error.to_string()))?;
        Ok(Self {
            dirty,
            available: true,
            _watcher: Some(watcher),
        })
    }

    /// Creates a watcher state that can never skip manifest checks.
    pub fn unavailable() -> Self {
        Self {
            dirty: Arc::new(AtomicBool::new(true)),
            available: false,
            _watcher: None,
        }
    }

    /// Returns true when the service may skip a manifest walk.
    ///
    /// The service also applies a safety interval; this method only captures the
    /// watcher-specific part of the decision.
    pub fn can_skip_manifest_check(&self) -> bool {
        self.available && !self.dirty.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub fn clean_for_test() -> Self {
        Self {
            dirty: Arc::new(AtomicBool::new(false)),
            available: true,
            _watcher: None,
        }
    }

    #[cfg(test)]
    pub fn dirty_for_test() -> Self {
        Self {
            dirty: Arc::new(AtomicBool::new(true)),
            available: true,
            _watcher: None,
        }
    }

    #[cfg(test)]
    pub fn mark_dirty_for_test(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: watcher state exposes clean, dirty, and unavailable manifest-skip decisions.
    #[test]
    fn watcher_skip_decision_requires_available_clean_state() {
        let clean = IndexWatcher::clean_for_test();
        let dirty = IndexWatcher::dirty_for_test();
        let unavailable = IndexWatcher::unavailable();

        let decisions = vec![
            clean.can_skip_manifest_check(),
            dirty.can_skip_manifest_check(),
            unavailable.can_skip_manifest_check(),
        ];

        assert_eq!(decisions, vec![true, false, false]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: the test dirty marker follows the same flag contract as filesystem events.
    #[test]
    fn manual_dirty_mark_prevents_manifest_skip() {
        let watcher = IndexWatcher::clean_for_test();

        watcher.mark_dirty_for_test();

        assert_eq!(watcher.can_skip_manifest_check(), false);
    }
}
