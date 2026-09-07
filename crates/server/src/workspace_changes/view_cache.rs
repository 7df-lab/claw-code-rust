//! Short-lived in-process cache for git-backed workspace change views.
//!
//! Summary reads dominate the Changes open path (`name-status` / `numstat`).
//! Scope switches and refresh often hit the same tree; a fingerprint-keyed LRU
//! avoids re-listing. Full payloads are cached when present.
//!
//! Git child processes must use `stdin(Stdio::null())` — inheriting the server's
//! JSON-RPC stdin pipe deadlocks `workspace/changes/read` over stdio.
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use devo_protocol::{WorkspaceChangeScope, WorkspaceChangeView, WorkspaceDiffDetail};
use lru::LruCache;

use super::diff::apply_diff_detail;

const CACHE_CAPACITY: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ViewCacheKey {
    pub repo_root: PathBuf,
    pub scope: WorkspaceChangeScope,
    pub base_branch: Option<String>,
    pub ignore_whitespace: bool,
    pub fingerprint: String,
}

struct CachedEntry {
    view: WorkspaceChangeView,
}

static VIEW_CACHE: LazyLock<Mutex<LruCache<ViewCacheKey, CachedEntry>>> = LazyLock::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(CACHE_CAPACITY).expect("workspace view cache capacity"),
    ))
});

/// Cheap tree fingerprint that changes when index / worktree tracked content
/// changes. Untracked entries use `status` (directory collapsing) instead of
/// listing every file — large untracked trees (e.g. node_modules) must not
/// dominate Changes latency.
pub(super) async fn porcelain_fingerprint(repo_root: &Path) -> Option<String> {
    let head = git_rev_parse_head(repo_root).await.unwrap_or_default();
    let (index, worktree, untracked) = tokio::join!(
        git_stdout_trimmed(repo_root, &["diff-index", "--cached", "--raw", "HEAD"]),
        git_stdout_trimmed(repo_root, &["diff-files", "--raw"]),
        git_stdout_trimmed(
            repo_root,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        ),
    );
    Some(format!(
        "{head}\n{}\n{}\n{}",
        index?,
        worktree?,
        untracked.unwrap_or_default()
    ))
}

pub(super) fn branch_fingerprint(merge_base: &str, head: &str) -> String {
    format!("{merge_base}\n{head}")
}

pub(super) fn get(
    key: &ViewCacheKey,
    diff_detail: WorkspaceDiffDetail,
    max_diff_bytes: Option<u64>,
) -> Option<WorkspaceChangeView> {
    let mut cache = VIEW_CACHE.lock().ok()?;
    let entry = cache.get(key)?;
    // Summary-only entries cannot satisfy a Full request.
    if matches!(diff_detail, WorkspaceDiffDetail::Full) && entry.view.unified_diff.is_none() {
        return None;
    }
    let mut view = entry.view.clone();
    apply_diff_detail(&mut view, diff_detail, max_diff_bytes);
    Some(view)
}

pub(super) fn put(key: ViewCacheKey, view: WorkspaceChangeView) {
    if let Ok(mut cache) = VIEW_CACHE.lock() {
        // Prefer keeping a Full payload if we already have one for this key.
        if view.unified_diff.is_none()
            && cache
                .peek(&key)
                .is_some_and(|entry| entry.view.unified_diff.is_some())
        {
            return;
        }
        cache.put(key, CachedEntry { view });
    }
}

/// Cached merge-base lookups: `(repo, base_branch) -> (head, merge_base)`.
type MergeBaseCache = LruCache<(PathBuf, String), (String, String)>;
static MERGE_BASE_CACHE: LazyLock<Mutex<MergeBaseCache>> = LazyLock::new(|| {
    Mutex::new(LruCache::new(
        NonZeroUsize::new(32).expect("merge-base cache capacity"),
    ))
});

pub(super) fn cached_merge_base(
    repo_root: &Path,
    base_branch: &str,
    head: &str,
    compute: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let key = (repo_root.to_path_buf(), base_branch.to_string());
    if let Ok(mut cache) = MERGE_BASE_CACHE.lock()
        && let Some((cached_head, merge_base)) = cache.get(&key)
        && cached_head == head
    {
        return Some(merge_base.clone());
    }
    let merge_base = compute()?;
    if let Ok(mut cache) = MERGE_BASE_CACHE.lock() {
        cache.put(key, (head.to_string(), merge_base.clone()));
    }
    Some(merge_base)
}

#[cfg(test)]
pub(super) fn clear_for_tests() {
    if let Ok(mut cache) = VIEW_CACHE.lock() {
        cache.clear();
    }
    if let Ok(mut cache) = MERGE_BASE_CACHE.lock() {
        cache.clear();
    }
}

async fn git_rev_parse_head(repo_root: &Path) -> Option<String> {
    git_stdout_trimmed(repo_root, &["rev-parse", "HEAD"]).await
}

async fn git_stdout_trimmed(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(repo_root)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    // diff-index / diff-files exit 1 when differences exist.
    if !(output.status.success() || output.status.code() == Some(1)) {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}
