//! Code-search service orchestration.
//!
//! The service owns model access, root validation, warm in-memory indexes, disk
//! cache lookup, incremental refresh, and final tool output construction. Its
//! refresh ladder is intentionally conservative: a clean watcher can skip the
//! manifest walk briefly, otherwise the service validates manifests before using
//! memory or disk cache. This keeps correctness ahead of latency while still
//! avoiding full repository work on repeated queries.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devo_network_proxy::NetworkProxyConfig;

use crate::cache::{cache_file_path, default_cache_dir, load_payload, save_payload};
use crate::dense::{EmbeddingProvider, Model2VecEmbeddingProvider};
use crate::files::{FileEntry, discover_files};
use crate::index::SearchIndex;
use crate::ranking::rank_search;
use crate::refresh::IndexRefresh;
use crate::types::{
    CodeSearchError, CodeSearchOperation, ContentFilter, IndexStats, RelatedRequest, SearchOutput,
    SearchRequest, trim_whitespace_in_place, validate_top_k,
};
use crate::watch::IndexWatcher;

const MANIFEST_SAFETY_INTERVAL: Duration = Duration::from_secs(30);
const MAX_WARM_INDEXES: usize = 8;
const WINDOWS_ERROR_INVALID_FUNCTION: i32 = 1;

/// Thread-safe entrypoint used by the Devo tool runtime.
///
/// `CodeSearchService` keeps a bounded in-memory index cache keyed by
/// root/content/model identity. The service is read-only with respect to the
/// workspace; its only writes are disposable cache files under the configured
/// cache directory.
pub struct CodeSearchService {
    provider: Arc<dyn EmbeddingProvider>,
    cache_dir: PathBuf,
    indexes: Mutex<HashMap<String, CachedIndexState>>,
    watcher_policy: WatcherPolicy,
}

impl CodeSearchService {
    /// Creates the production service with the default local model cache.
    pub fn production() -> Self {
        Self::production_with_network_proxy(NetworkProxyConfig::default())
    }

    /// Creates the production service with an explicit network proxy policy.
    pub fn production_with_network_proxy(network_proxy: NetworkProxyConfig) -> Self {
        Self::new(
            Arc::new(Model2VecEmbeddingProvider::default_cached_with_network_proxy(network_proxy)),
            default_cache_dir(),
        )
    }

    /// Creates a service with explicit provider and cache directory.
    ///
    /// Tests use this to substitute deterministic embeddings while the runtime
    /// uses the model2vec provider.
    pub fn new(provider: Arc<dyn EmbeddingProvider>, cache_dir: PathBuf) -> Self {
        Self {
            provider,
            cache_dir,
            indexes: Mutex::new(HashMap::new()),
            watcher_policy: WatcherPolicy::Enabled,
        }
    }

    #[cfg(test)]
    fn new_with_watcher_policy(
        provider: Arc<dyn EmbeddingProvider>,
        cache_dir: PathBuf,
        watcher_policy: WatcherPolicy,
    ) -> Self {
        Self {
            provider,
            cache_dir,
            indexes: Mutex::new(HashMap::new()),
            watcher_policy,
        }
    }

    /// Executes a hybrid code search and returns the stable tool response shape.
    ///
    /// Empty queries are handled before model/index work so validation callers can
    /// receive a cheap empty result instead of triggering model downloads.
    pub fn search(&self, request: SearchRequest) -> Result<SearchOutput, CodeSearchError> {
        let top_k = validate_top_k(request.top_k)?;
        let root = canonical_root(&request.root)?;
        let mut query = request.query;
        trim_whitespace_in_place(&mut query);
        if query.is_empty() {
            return Ok(SearchOutput {
                operation: CodeSearchOperation::Search,
                query: Some(query),
                root,
                content: request.content,
                results: Vec::new(),
                index_stats: IndexStats {
                    indexed_files: 0,
                    total_chunks: 0,
                },
            });
        }
        let index = self.index(root, request.content)?;
        let query_embedding = self.provider.embed(std::slice::from_ref(&query))?;
        let results = query_embedding
            .first()
            .map(|embedding| rank_search(&index, &query, embedding, top_k, &request.filters))
            .unwrap_or_default();
        Ok(SearchOutput {
            operation: CodeSearchOperation::Search,
            query: Some(query),
            root: index.root().to_path_buf(),
            content: index.content(),
            results,
            index_stats: index.stats(),
        })
    }

    /// Finds chunks related to a source file line.
    ///
    /// The source location is resolved against the same root and filters used by
    /// search. Missing source chunks are not errors because generated or ignored
    /// files may legitimately be outside the index.
    pub fn find_related(&self, request: RelatedRequest) -> Result<SearchOutput, CodeSearchError> {
        let top_k = validate_top_k(request.top_k)?;
        if request.line == 0 {
            return Err(CodeSearchError::InvalidInput(
                "`line` must be 1-indexed and greater than zero".to_string(),
            ));
        }
        let root = canonical_root(&request.root)?;
        let relative_path = normalize_source_path(&root, &request.file_path)?;
        let index = self.index(root, request.content)?;
        let source_idx = index.find_source_chunk(&relative_path, request.line);
        let results = source_idx
            .map(|idx| index.related_by_embedding(idx, top_k, &request.filters))
            .unwrap_or_default();
        Ok(SearchOutput {
            operation: CodeSearchOperation::FindRelated,
            query: None,
            root: index.root().to_path_buf(),
            content: index.content(),
            results,
            index_stats: index.stats(),
        })
    }

    /// Prepares the default retrieval index without running a query.
    pub fn prewarm(
        &self,
        root: &Path,
        content: ContentFilter,
    ) -> Result<IndexStats, CodeSearchError> {
        let root = canonical_root(root)?;
        self.index(root, content).map(|index| index.stats())
    }

    /// Returns a warm or refreshed index for a root/content pair.
    fn index(
        &self,
        root: PathBuf,
        content: ContentFilter,
    ) -> Result<Arc<SearchIndex>, CodeSearchError> {
        let key = memory_key(&root, content, self.provider.model_id());
        if let Some(index) = self.clean_warm_index(&key)? {
            return Ok(index);
        }

        let build_key = format!("{}|{key}", self.cache_dir.display());
        let index =
            crate::singleflight::run(build_key, || self.build_or_load_index(&key, &root, content))?;
        self.store_index(key, &root, Arc::clone(&index))?;
        Ok(index)
    }

    fn build_or_load_index(
        &self,
        key: &str,
        root: &Path,
        content: ContentFilter,
    ) -> Result<Arc<SearchIndex>, CodeSearchError> {
        let files = discover_files(root, content)?;
        if let Some(index) = self.matching_memory_index(key, root, &files)? {
            return Ok(index);
        }

        let cache_path = cache_file_path(&self.cache_dir, root, content, self.provider.model_id());
        let previous_payload = load_payload(&cache_path).filter(|cache| {
            cache
                .payload
                .is_valid_for(root, content, self.provider.model_id())
        });
        let outcome = IndexRefresh::refresh(
            root,
            content,
            files,
            previous_payload,
            self.provider.as_ref(),
        )?;
        save_payload(&cache_path, &outcome.payload, &outcome.embeddings)?;
        Ok(Arc::new(SearchIndex::from_cached(
            crate::cache::CachedIndex {
                payload: outcome.payload,
                embeddings: outcome.embeddings,
            },
        )?))
    }

    /// Returns `true` when the next `search` or `find_related` call for this
    /// root/content pair will trigger a full index build (no warm memory cache
    /// and no valid disk cache). Callers use this to surface a "building index"
    /// notice before the potentially slow first-run.
    pub fn needs_index_build(&self, root: &Path, content: ContentFilter) -> bool {
        let Ok(root) = canonical_root(root) else {
            return true;
        };
        let key = memory_key(&root, content, self.provider.model_id());
        if self.clean_warm_index(&key).ok().flatten().is_some() {
            return false;
        }
        let cache_path = cache_file_path(&self.cache_dir, &root, content, self.provider.model_id());
        let has_valid_cache = load_payload(&cache_path).is_some_and(|c| {
            c.payload
                .is_valid_for(&root, content, self.provider.model_id())
        });
        !has_valid_cache
    }

    /// Reuses an in-memory index without touching the filesystem when watcher
    /// state says it is both available and clean.
    fn clean_warm_index(&self, key: &str) -> Result<Option<Arc<SearchIndex>>, CodeSearchError> {
        let now = Instant::now();
        let mut indexes = self
            .indexes
            .lock()
            .map_err(|_| CodeSearchError::Index("index cache lock poisoned".to_string()))?;
        let Some(state) = indexes.get_mut(key) else {
            return Ok(None);
        };
        if !state.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL) {
            return Ok(None);
        }
        state.mark_used(now);
        Ok(Some(Arc::clone(&state.index)))
    }

    /// Reuses an in-memory index after a manifest walk proves it is still fresh.
    ///
    /// Storing it again replaces the watcher and updates the safety timestamp so
    /// future clean queries can take the no-walk path.
    fn matching_memory_index(
        &self,
        key: &str,
        root: &Path,
        files: &[FileEntry],
    ) -> Result<Option<Arc<SearchIndex>>, CodeSearchError> {
        let index = {
            let mut indexes = self
                .indexes
                .lock()
                .map_err(|_| CodeSearchError::Index("index cache lock poisoned".to_string()))?;
            let Some(state) = indexes.get_mut(key) else {
                return Ok(None);
            };
            // Only allocate the manifest snapshot when a warm candidate exists.
            // Cold and disk-cache paths can hand `files` directly to refresh.
            let manifest = files
                .iter()
                .map(|file| file.manifest.clone())
                .collect::<Vec<_>>();
            if !state.index.manifest_matches(&manifest) {
                return Ok(None);
            }
            state.mark_used(Instant::now());
            Arc::clone(&state.index)
        };
        self.store_index(key.to_string(), root, Arc::clone(&index))?;
        Ok(Some(index))
    }

    /// Installs an index in the warm cache with a watcher for its root.
    fn store_index(
        &self,
        key: String,
        root: &Path,
        index: Arc<SearchIndex>,
    ) -> Result<(), CodeSearchError> {
        let state = CachedIndexState::new(index, root, self.watcher_policy);
        let mut indexes = self
            .indexes
            .lock()
            .map_err(|_| CodeSearchError::Index("index cache lock poisoned".to_string()))?;
        indexes.insert(key, state);
        prune_warm_indexes(&mut indexes);
        Ok(())
    }
}

fn prune_warm_indexes(indexes: &mut HashMap<String, CachedIndexState>) {
    while indexes.len() > MAX_WARM_INDEXES {
        let Some(oldest_last_used) = indexes.values().map(|state| state.last_used).min() else {
            return;
        };
        let mut removed_oldest = false;
        // Remove via retain so eviction does not need to clone the selected key.
        indexes.retain(|_, state| {
            if !removed_oldest && state.last_used == oldest_last_used {
                removed_oldest = true;
                false
            } else {
                true
            }
        });
    }
}

impl Default for CodeSearchService {
    fn default() -> Self {
        Self::production()
    }
}

/// Canonicalizes and validates the requested search root.
fn canonical_root(root: &Path) -> Result<PathBuf, CodeSearchError> {
    let canonical = root.canonicalize()?;
    if !canonical.is_dir() {
        return Err(CodeSearchError::InvalidInput(format!(
            "search root is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

/// Converts an absolute source path into the workspace-relative path stored in
/// chunks and rejects absolute paths outside the root.
fn normalize_source_path(root: &Path, file_path: &Path) -> Result<PathBuf, CodeSearchError> {
    let relative_path = if file_path.is_absolute() {
        match file_path.canonicalize() {
            Ok(canonical) => strip_source_root(root, &canonical, file_path),
            Err(error) if is_missing_source_path_error(&error) => {
                normalize_missing_absolute_source_path(root, file_path)
            }
            Err(error) => Err(error.into()),
        }
    } else {
        Ok(normalize_relative_source_path(file_path))
    }?;
    if relative_path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(outside_root_error(file_path));
    }
    Ok(relative_path)
}

fn normalize_missing_absolute_source_path(
    root: &Path,
    file_path: &Path,
) -> Result<PathBuf, CodeSearchError> {
    let mut resolved = PathBuf::new();
    let mut components = file_path.components();
    while let Some(component) = components.next() {
        if component == Component::CurDir {
            continue;
        }
        resolved.push(component.as_os_str());
        match resolved.canonicalize() {
            Ok(canonical) => resolved = canonical,
            Err(error) if is_missing_source_path_error(&error) => {
                for remaining in components {
                    if remaining != Component::CurDir {
                        resolved.push(remaining.as_os_str());
                    }
                }
                let relative = strip_source_root(root, &resolved, file_path)?;
                return Ok(normalize_relative_source_path(&relative));
            }
            Err(error) => return Err(error.into()),
        }
    }
    strip_source_root(root, &resolved, file_path)
}

fn is_missing_source_path_error(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::NotFound
        || cfg!(windows) && error.raw_os_error() == Some(WINDOWS_ERROR_INVALID_FUNCTION)
}

fn strip_source_root(
    root: &Path,
    file_path: &Path,
    original_path: &Path,
) -> Result<PathBuf, CodeSearchError> {
    file_path
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| outside_root_error(original_path))
}

fn outside_root_error(file_path: &Path) -> CodeSearchError {
    CodeSearchError::InvalidInput(format!(
        "file path is outside the search root: {}",
        file_path.display()
    ))
}

fn normalize_relative_source_path(file_path: &Path) -> PathBuf {
    let separator_normalized = file_path.to_string_lossy().replace('\\', "/");
    let mut parts = Vec::<OsString>::new();
    for component in Path::new(&separator_normalized).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if parts
                    .last()
                    .is_some_and(|part| part.as_os_str() != OsStr::new(".."))
                {
                    parts.pop();
                } else {
                    parts.push(OsString::from(".."));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {
                parts.push(component.as_os_str().to_os_string());
            }
        }
    }
    parts.into_iter().collect()
}

/// Builds the in-memory index key from the dimensions that affect retrieval.
fn memory_key(root: &Path, content: ContentFilter, model_id: &str) -> String {
    format!("{}::{content:?}::{model_id}", root.display())
}

/// Warm index state plus the invalidation signal for its workspace root.
struct CachedIndexState {
    index: Arc<SearchIndex>,
    watcher: IndexWatcher,
    last_manifest_check: Instant,
    last_used: Instant,
}

impl CachedIndexState {
    /// Creates a warm state and starts a watcher when policy allows it.
    fn new(index: Arc<SearchIndex>, root: &Path, watcher_policy: WatcherPolicy) -> Self {
        let now = Instant::now();
        Self {
            index,
            watcher: watcher_policy.create_watcher(root),
            last_manifest_check: now,
            last_used: now,
        }
    }

    /// Returns true only when skipping discovery is still within the correctness
    /// budget: watcher available, no events seen, and safety interval not stale.
    fn can_reuse_without_manifest(&self, now: Instant, safety_interval: Duration) -> bool {
        self.watcher.can_skip_manifest_check()
            && now.duration_since(self.last_manifest_check) < safety_interval
    }

    fn mark_used(&mut self, now: Instant) {
        self.last_used = now;
    }
}

/// Test seam for watcher setup failure without mutating global environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatcherPolicy {
    Enabled,
    #[cfg(test)]
    Unavailable,
}

impl WatcherPolicy {
    /// Creates the watcher required by the selected policy.
    fn create_watcher(self, root: &Path) -> IndexWatcher {
        match self {
            Self::Enabled => IndexWatcher::watch(root),
            #[cfg(test)]
            Self::Unavailable => IndexWatcher::unavailable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;

    use crate::cache::{CachedIndex, CachedIndexPayloadV4, cache_file_path, load_payload};
    use crate::dense::HashEmbeddingProvider;
    use crate::matrix::EmbeddingMatrix;
    use crate::types::SearchFilters;

    use super::*;

    fn test_service(cache_dir: PathBuf) -> CodeSearchService {
        CodeSearchService::new_with_watcher_policy(
            Arc::new(HashEmbeddingProvider::new("test", 16)),
            cache_dir,
            WatcherPolicy::Unavailable,
        )
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: search returns the structured output shape with bounded result count.
    #[test]
    fn search_returns_structured_results() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            temp.path().join("parser.rs"),
            "pub fn parse_input() {}\npub fn render_output() {}\n",
        )
        .expect("write");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "parse input".to_string(),
                content: ContentFilter::Code,
                top_k: 1,
                filters: SearchFilters::empty(),
            })
            .expect("search");

        assert_eq!(output.operation, CodeSearchOperation::Search);
        assert_eq!(output.results.len(), 1);
        assert_eq!(output.index_stats.indexed_files, 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: find-related excludes the source chunk and prefers same-language chunks.
    #[test]
    fn find_related_excludes_source_chunk() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(
            temp.path().join("lib.rs"),
            "pub fn parse_input() {}\n\npub fn parse_related() {}\n",
        )
        .expect("write");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .find_related(RelatedRequest {
                root: temp.path().to_path_buf(),
                file_path: PathBuf::from("lib.rs"),
                line: 1,
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("related");

        assert_eq!(output.operation, CodeSearchOperation::FindRelated);
        assert!(
            output
                .results
                .iter()
                .all(|result| result.chunk.start_line != 1)
        );
    }

    #[test]
    fn find_related_missing_absolute_source_path_returns_empty_results() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(temp.path().join("lib.rs"), "pub fn parse_input() {}\n").expect("write");
        let root = temp.path().canonicalize().expect("canonical root");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .find_related(RelatedRequest {
                root: temp.path().to_path_buf(),
                file_path: root.join("generated").join("missing.rs"),
                line: 1,
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("missing source path");

        assert_eq!(output.operation, CodeSearchOperation::FindRelated);
        assert_eq!(output.root, root);
        assert!(output.results.is_empty());
    }

    #[test]
    fn source_path_normalization_collapses_relative_dot_components() {
        let root = Path::new("repo");
        let dotted_path = Path::new(".").join("src").join("..").join("lib.rs");
        let nested_dot_path = Path::new("src").join(".").join("lib.rs");

        let normalized_paths = vec![
            normalize_source_path(root, &dotted_path).expect("dotted path"),
            normalize_source_path(root, &nested_dot_path).expect("nested dot path"),
        ];

        assert_eq!(
            normalized_paths,
            vec![PathBuf::from("lib.rs"), Path::new("src").join("lib.rs")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn source_path_normalization_collapses_backslash_dot_components_on_unix() {
        let root = Path::new("repo");
        let dotted_path = PathBuf::from(r"src\..\lib.rs");
        let nested_dot_path = PathBuf::from(r"src\.\nested.rs");

        let normalized_paths = vec![
            normalize_source_path(root, &dotted_path).expect("dotted path"),
            normalize_source_path(root, &nested_dot_path).expect("nested dot path"),
        ];

        assert_eq!(
            normalized_paths,
            vec![PathBuf::from("lib.rs"), Path::new("src").join("nested.rs")]
        );
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: cache invalidates when the indexed file manifest changes.
    #[test]
    fn search_cache_invalidates_after_file_change() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        let file = temp.path().join("lib.rs");
        fs::write(&file, "pub fn alpha() {}\n").expect("write");
        let service = test_service(cache.path().to_path_buf());

        let first = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "alpha".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("first search");
        fs::write(&file, "pub fn beta() {}\n").expect("rewrite");
        let second = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "beta".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("second search");

        assert_eq!(first.index_stats.indexed_files, 1);
        assert_eq!(second.results[0].chunk.content, "pub fn beta() {}");
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: a dirty warm watcher state triggers manifest refresh before reuse.
    #[test]
    fn dirty_warm_state_refreshes_manifest() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        let file = temp.path().join("lib.rs");
        fs::write(&file, "pub fn alpha() {}\n").expect("write");
        let service = CodeSearchService::new(
            Arc::new(HashEmbeddingProvider::new("test", 16)),
            cache.path().to_path_buf(),
        );
        service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "alpha".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("first search");
        fs::write(&file, "pub fn beta_changed() {}\n").expect("rewrite");
        let root = temp.path().canonicalize().expect("canonical root");
        let key = memory_key(&root, ContentFilter::Code, "test");
        service
            .indexes
            .lock()
            .expect("indexes")
            .get(&key)
            .expect("state")
            .watcher
            .mark_dirty_for_test();

        let output = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "beta_changed".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("second search");

        assert_eq!(output.results[0].chunk.content, "pub fn beta_changed() {}");
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: clean watcher state can skip manifest refresh only inside the safety interval.
    #[test]
    fn cached_state_reuse_requires_clean_available_recent_watcher() {
        let now = Instant::now();
        let stale = now
            .checked_sub(MANIFEST_SAFETY_INTERVAL + Duration::from_secs(1))
            .expect("stale instant");
        let clean_recent = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::clean_for_test(),
            last_manifest_check: now,
            last_used: now,
        };
        let dirty_recent = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::dirty_for_test(),
            last_manifest_check: now,
            last_used: now,
        };
        let unavailable_recent = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::unavailable(),
            last_manifest_check: now,
            last_used: now,
        };
        let clean_stale = CachedIndexState {
            index: empty_index(),
            watcher: IndexWatcher::clean_for_test(),
            last_manifest_check: stale,
            last_used: now,
        };

        let reuse_decisions = vec![
            clean_recent.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
            dirty_recent.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
            unavailable_recent.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
            clean_stale.can_reuse_without_manifest(now, MANIFEST_SAFETY_INTERVAL),
        ];

        assert_eq!(reuse_decisions, vec![true, false, false, false]);
    }

    /// Verifies: warm indexes are bounded so long-running sessions cannot retain
    /// one full index for every root/content/model combination they have seen.
    #[test]
    fn warm_index_cache_prunes_oldest_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        let service = test_service(cache.path().to_path_buf());

        for index in 0..MAX_WARM_INDEXES {
            service
                .store_index(format!("key-{index}"), temp.path(), empty_index())
                .expect("store index");
        }
        service
            .indexes
            .lock()
            .expect("indexes")
            .get_mut("key-0")
            .expect("first state")
            .last_used = Instant::now()
            .checked_sub(Duration::from_secs(60))
            .expect("older instant");

        service
            .store_index("key-new".to_string(), temp.path(), empty_index())
            .expect("store new index");

        let indexes = service.indexes.lock().expect("indexes");
        assert_eq!(indexes.len(), MAX_WARM_INDEXES);
        assert!(!indexes.contains_key("key-0"));
        assert!(indexes.contains_key("key-new"));
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: stale v1 cache payloads are ignored and replaced by a current cache payload.
    #[test]
    fn stale_v1_cache_rebuilds_cleanly() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(temp.path().join("lib.rs"), "pub fn parse_input() {}\n").expect("write");
        let root = temp.path().canonicalize().expect("canonical root");
        let cache_path = cache_file_path(cache.path(), &root, ContentFilter::Code, "test");
        fs::create_dir_all(cache_path.parent().expect("cache parent")).expect("cache dir");
        fs::write(
            &cache_path,
            r#"{"cache_version":1,"root":"/repo","content":"code","model_id":"test","manifest":[],"chunks":[],"embeddings":[]}"#,
        )
        .expect("write v1");
        let service = test_service(cache.path().to_path_buf());

        let output = service
            .search(SearchRequest {
                root: temp.path().to_path_buf(),
                query: "parse input".to_string(),
                content: ContentFilter::Code,
                top_k: 5,
                filters: SearchFilters::empty(),
            })
            .expect("search");

        assert_eq!(output.index_stats.indexed_files, 1);
        assert_eq!(load_payload(&cache_path).is_some(), true);
    }

    #[test]
    fn needs_index_build_true_without_cache_false_after_build() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("cache");
        fs::write(temp.path().join("lib.rs"), "pub fn alpha() {}\n").expect("write");
        let service = test_service(cache.path().to_path_buf());

        assert!(
            service.needs_index_build(temp.path(), ContentFilter::Code),
            "should need build before first search"
        );

        let stats = service
            .prewarm(temp.path(), ContentFilter::Code)
            .expect("prewarm");
        assert_eq!(
            stats,
            IndexStats {
                indexed_files: 1,
                total_chunks: 1,
            }
        );

        assert!(
            !service.needs_index_build(temp.path(), ContentFilter::Code),
            "should not need build after first search"
        );
    }

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
}
