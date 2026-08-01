//! Incremental index refresh.
//!
//! Refresh compares the current file manifest with a previous cache and only
//! re-reads, re-chunks, and re-embeds files whose path, size, or nanosecond mtime
//! changed. The output is still a complete cache payload: reused file records are
//! copied into a new flat embedding matrix so row ids stay dense and stable after
//! deletes, additions, and record reordering. That keeps the search index simple
//! and avoids carrying tombstones through ranking.

use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;

use crate::cache::{CachedFileRecord, CachedIndex, CachedIndexPayloadV4, content_hash};
use crate::chunking::chunk_file;
use crate::dense::EmbeddingProvider;
use crate::files::{FileEntry, FileManifestEntry, read_indexable_text};
use crate::matrix::EmbeddingMatrix;
use crate::types::{Chunk, CodeSearchError, ContentFilter};

const EMBEDDING_BATCH_SIZE: usize = 256;

/// Rebuilds a complete cache payload from a current file listing and optional
/// previous cache.
///
/// This type is stateless so the service can run refreshes inside blocking work
/// without hidden cross-query state. All reuse decisions are derived from the
/// supplied manifests and provider identity.
pub struct IndexRefresh;

impl IndexRefresh {
    /// Performs a manifest-aware refresh and returns a complete cache image.
    ///
    /// A previous cache is accepted only when root, content filter, cache
    /// version, and model id match. Changed files are read/chunked in parallel,
    /// their chunks are embedded in bounded batches, and unchanged rows are
    /// copied from the old matrix into the new matrix in final record order.
    pub fn refresh(
        root: &Path,
        content: ContentFilter,
        files: Vec<FileEntry>,
        previous_cache: Option<CachedIndex>,
        provider: &dyn EmbeddingProvider,
    ) -> Result<RefreshOutcome, CodeSearchError> {
        let previous_cache = previous_cache.filter(|cache| {
            cache
                .payload
                .is_valid_for(root, content, provider.model_id())
        });
        let (previous_embeddings, mut previous_records) = match previous_cache {
            Some(cache) => (
                cache.embeddings,
                cache
                    .payload
                    .files
                    .into_iter()
                    .map(|record| (record.manifest.path.clone(), record))
                    .collect::<HashMap<_, _>>(),
            ),
            None => (EmbeddingMatrix::empty(), HashMap::new()),
        };

        let mut slots = Vec::with_capacity(files.len());
        let mut pending_files = Vec::new();
        let mut reused_files = 0;

        for file in files {
            if let Some(record) = previous_records.remove(&file.manifest.path)
                && record.can_reuse_for(&file.manifest)
            {
                reused_files += 1;
                slots.push(RefreshSlot::Reused(record));
                continue;
            }
            let slot_idx = slots.len();
            pending_files.push((slot_idx, file));
            slots.push(RefreshSlot::Pending);
        }

        let deleted_files = previous_records.len();
        // Reading and chunking are CPU/file-system bound and independent per
        // file. Embedding stays batched after this stage so provider calls remain
        // predictable and do not explode into one request per changed chunk.
        let mut pending_records = pending_files
            .into_par_iter()
            .map(|(slot_idx, file)| PendingFileRecord::read(file).map(|record| (slot_idx, record)))
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        pending_records.sort_by_key(|(slot_idx, _)| *slot_idx);

        let changed_embeddings =
            EmbeddingMatrix::from_vectors(embed_pending_chunks(provider, &pending_records)?)?;
        let reembedded_files = pending_records.len();
        let mut pending_records = pending_records.into_iter();

        let mut embeddings = EmbeddingMatrix::empty();
        let mut changed_cursor = 0;
        let mut records = Vec::with_capacity(slots.len());

        for (slot_idx, slot) in slots.into_iter().enumerate() {
            match slot {
                RefreshSlot::Reused(record) => {
                    // Reused rows are copied into the new matrix instead of
                    // keeping old row ids. Deleted files then disappear
                    // completely, and chunk id == embedding row remains true.
                    let embedding_start = embeddings.row_count();
                    embeddings.extend_rows_from(
                        &previous_embeddings,
                        record.embedding_start,
                        record.embedding_count,
                    )?;
                    records.push(CachedFileRecord::new(
                        record.manifest,
                        record.content_hash,
                        record.chunks,
                        embedding_start,
                        record.embedding_count,
                    ));
                }
                RefreshSlot::Pending => {
                    // Zero-chunk pending records are valid: empty and unreadable
                    // files still get manifests so the refresh can skip them
                    // until the file metadata changes.
                    let (pending_slot_idx, pending) = pending_records.next().ok_or_else(|| {
                        CodeSearchError::Index("missing pending file record".to_string())
                    })?;
                    if pending_slot_idx != slot_idx {
                        return Err(CodeSearchError::Index(
                            "missing pending file record".to_string(),
                        ));
                    }
                    let embedding_count = pending.chunks.len();
                    let embedding_start = embeddings.row_count();
                    embeddings.extend_rows_from(
                        &changed_embeddings,
                        changed_cursor,
                        embedding_count,
                    )?;
                    changed_cursor += embedding_count;
                    records.push(CachedFileRecord::new(
                        pending.manifest,
                        pending.content_hash,
                        pending.chunks,
                        embedding_start,
                        embedding_count,
                    ));
                }
            }
        }

        records.sort_by(|left, right| left.manifest.path.cmp(&right.manifest.path));
        let payload = CachedIndexPayloadV4::new(
            root.to_path_buf(),
            content,
            provider.model_id().to_string(),
            &embeddings,
            records,
        );
        Ok(RefreshOutcome {
            payload,
            embeddings,
            reused_files,
            reembedded_files,
            deleted_files,
        })
    }
}

/// Result of an incremental refresh.
///
/// The counters are diagnostic only; callers should use `payload` and
/// `embeddings` as the canonical cache image.
#[derive(Debug, Clone, PartialEq)]
pub struct RefreshOutcome {
    pub payload: CachedIndexPayloadV4,
    pub embeddings: EmbeddingMatrix,
    pub reused_files: usize,
    pub reembedded_files: usize,
    pub deleted_files: usize,
}

enum RefreshSlot {
    Reused(CachedFileRecord),
    Pending,
}

/// File data that must be embedded before it can become a cache record.
struct PendingFileRecord {
    manifest: FileManifestEntry,
    content_hash: String,
    chunks: Vec<Chunk>,
}

impl PendingFileRecord {
    /// Reads a changed file and converts read failures into reusable zero-chunk
    /// records.
    ///
    /// Code search should not fail a whole repository because one candidate file
    /// disappears or becomes unreadable during indexing. A later manifest change
    /// will retry the file.
    fn read(file: FileEntry) -> Result<Self, CodeSearchError> {
        let FileEntry {
            absolute_path,
            language,
            manifest,
            ..
        } = file;
        let (content_hash_value, chunks) = match read_indexable_text(&absolute_path) {
            Ok(Some(text)) => (
                content_hash(&text),
                chunk_file(&manifest.path, &language, &text),
            ),
            Ok(None) => (content_hash(""), Vec::new()),
            Err(_) => (content_hash("unreadable"), Vec::new()),
        };
        Ok(Self {
            manifest,
            content_hash: content_hash_value,
            chunks,
        })
    }
}

/// Embeds changed chunk text in bounded provider calls.
///
/// Providers must return one vector per input chunk. The explicit count check
/// catches model/runtime bugs before the flattened matrix can drift out of sync
/// with chunk ids.
fn embed_pending_chunks(
    provider: &dyn EmbeddingProvider,
    pending_records: &[(usize, PendingFileRecord)],
) -> Result<Vec<Vec<f32>>, CodeSearchError> {
    let expected_chunks = pending_records
        .iter()
        .map(|(_, record)| record.chunks.len())
        .sum();
    let mut embeddings = Vec::with_capacity(expected_chunks);
    let mut batch = Vec::with_capacity(EMBEDDING_BATCH_SIZE.min(expected_chunks));

    for (_, record) in pending_records {
        for chunk in &record.chunks {
            batch.push(chunk.content.clone());
            if batch.len() == EMBEDDING_BATCH_SIZE {
                embeddings.extend(provider.embed(&batch)?);
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        embeddings.extend(provider.embed(&batch)?);
    }

    if embeddings.len() != expected_chunks {
        return Err(CodeSearchError::Index(format!(
            "embedding provider returned {} vectors for {} chunks",
            embeddings.len(),
            expected_chunks
        )));
    }
    Ok(embeddings)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use pretty_assertions::assert_eq;

    use crate::cache::CachedIndex;
    use crate::dense::{EmbeddingProvider, HashEmbeddingProvider};
    use crate::files::discover_files;
    use crate::index::SearchIndex;

    use super::*;

    #[derive(Debug)]
    struct CountingProvider {
        inner: HashEmbeddingProvider,
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl CountingProvider {
        fn new() -> Self {
            Self {
                inner: HashEmbeddingProvider::new("test", 16),
                batches: Mutex::new(Vec::new()),
            }
        }

        fn batches(&self) -> Vec<Vec<String>> {
            self.batches.lock().expect("batches").clone()
        }
    }

    impl EmbeddingProvider for CountingProvider {
        fn model_id(&self) -> &str {
            self.inner.model_id()
        }

        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, CodeSearchError> {
            self.batches.lock().expect("batches").push(texts.to_vec());
            self.inner.embed(texts)
        }
    }

    fn refresh(
        root: &Path,
        previous_cache: Option<CachedIndex>,
        provider: &CountingProvider,
    ) -> RefreshOutcome {
        let files = discover_files(root, ContentFilter::Code).expect("files");
        IndexRefresh::refresh(root, ContentFilter::Code, files, previous_cache, provider)
            .expect("refresh")
    }

    fn cached(outcome: &RefreshOutcome) -> CachedIndex {
        CachedIndex {
            payload: outcome.payload.clone(),
            embeddings: outcome.embeddings.clone(),
        }
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: unchanged files reuse cached chunks and embedding rows without another embedding call.
    #[test]
    fn unchanged_files_reuse_cached_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("lib.rs"), "pub fn parse_input() {}\n").expect("write");
        let provider = CountingProvider::new();

        let first = refresh(temp.path(), None, &provider);
        let second = refresh(temp.path(), Some(cached(&first)), &provider);

        assert_eq!(second.payload, first.payload);
        assert_eq!(second.embeddings, first.embeddings);
        assert_eq!(second.reused_files, 1);
        assert_eq!(second.reembedded_files, 0);
        assert_eq!(provider.batches().len(), 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: changing one file re-embeds only that file while reusing unchanged file records.
    #[test]
    fn changed_file_reembeds_only_that_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.rs"), "pub fn alpha() {}\n").expect("write a");
        fs::write(temp.path().join("b.rs"), "pub fn beta() {}\n").expect("write b");
        let provider = CountingProvider::new();
        let first = refresh(temp.path(), None, &provider);
        fs::write(
            temp.path().join("b.rs"),
            "pub fn beta_changed_with_longer_body() {}\n",
        )
        .expect("rewrite b");

        let second = refresh(temp.path(), Some(cached(&first)), &provider);

        assert_eq!(second.reused_files, 1);
        assert_eq!(second.reembedded_files, 1);
        assert_eq!(second.deleted_files, 0);
        assert_eq!(provider.batches().len(), 2);
        assert_eq!(provider.batches()[1].len(), 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: adding and deleting files updates the cache payload and flattened search index.
    #[test]
    fn added_and_deleted_files_update_flattened_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("a.rs"), "pub fn alpha() {}\n").expect("write a");
        fs::write(temp.path().join("b.rs"), "pub fn beta() {}\n").expect("write b");
        let provider = CountingProvider::new();
        let first = refresh(temp.path(), None, &provider);
        fs::remove_file(temp.path().join("a.rs")).expect("remove a");
        fs::write(temp.path().join("c.rs"), "pub fn gamma() {}\n").expect("write c");

        let second = refresh(temp.path(), Some(cached(&first)), &provider);
        let index = SearchIndex::from_cached(cached(&second)).expect("index");
        let payload_paths = second
            .payload
            .files
            .iter()
            .map(|record| record.manifest.path.clone())
            .collect::<Vec<_>>();
        let flattened_paths = (0..index.stats().total_chunks)
            .filter_map(|idx| index.chunk(idx).map(|chunk| chunk.file_path.clone()))
            .collect::<Vec<_>>();

        assert_eq!(
            payload_paths,
            vec![PathBuf::from("b.rs"), PathBuf::from("c.rs")]
        );
        assert_eq!(
            flattened_paths,
            vec![PathBuf::from("b.rs"), PathBuf::from("c.rs")]
        );
        assert_eq!(second.deleted_files, 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: empty files are cached as zero-chunk file records until their manifest changes.
    #[test]
    fn empty_files_create_reusable_zero_chunk_records() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("empty.rs"), "   \n").expect("write");
        let provider = CountingProvider::new();

        let first = refresh(temp.path(), None, &provider);
        let second = refresh(temp.path(), Some(cached(&first)), &provider);

        assert_eq!(first.payload.files[0].chunks, Vec::<Chunk>::new());
        assert_eq!(second.reused_files, 1);
        assert_eq!(provider.batches(), Vec::<Vec<String>>::new());
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: embedding provider calls are split into deterministic bounded batches.
    #[test]
    fn embedding_calls_are_batched() {
        let pending_records = vec![(
            0,
            PendingFileRecord {
                manifest: FileManifestEntry {
                    path: PathBuf::from("src/lib.rs"),
                    size: 0,
                    modified_unix_nanos: 1,
                },
                content_hash: content_hash("chunks"),
                chunks: (0..600)
                    .map(|idx| Chunk {
                        content: format!("chunk {idx}"),
                        file_path: PathBuf::from("src/lib.rs"),
                        start_line: idx + 1,
                        end_line: idx + 1,
                        language: "rust".to_string(),
                    })
                    .collect(),
            },
        )];
        let provider = CountingProvider::new();

        let embeddings = embed_pending_chunks(&provider, &pending_records).expect("embed");
        let batch_sizes = provider
            .batches()
            .into_iter()
            .map(|batch| batch.len())
            .collect::<Vec<_>>();

        assert_eq!(embeddings.len(), 600);
        assert_eq!(batch_sizes, vec![256, 256, 88]);
    }
}
