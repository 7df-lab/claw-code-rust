//! In-memory searchable index.
//!
//! `SearchIndex` is the flattened runtime view produced from cache records: one
//! chunk id corresponds to one embedding matrix row and one BM25 document id.
//! Keeping those ids identical is the main invariant that lets ranking combine
//! sparse and dense results without translation tables. Large unfiltered semantic
//! searches may use HNSW for candidate generation, but exact cosine scores are
//! recomputed before ranking so the public behavior remains stable.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use bm25::{Document, SearchEngine, SearchEngineBuilder, Tokenizer};

use crate::cache::CachedIndex;
#[cfg(test)]
use crate::dense::EmbeddingProvider;
use crate::dense::cosine_similarity;
#[cfg(test)]
use crate::files::FileEntry;
use crate::files::FileManifestEntry;
use crate::matrix::EmbeddingMatrix;
#[cfg(test)]
use crate::refresh::IndexRefresh;
use crate::semantic::SemanticBackend;
use crate::tokens::{enrich_for_bm25, split_identifier_tokens};
use crate::types::{
    Chunk, CodeSearchError, ContentFilter, IndexStats, SearchFilters, SearchResult,
};

#[derive(Debug, Clone, Copy)]
pub struct CodeTokenizer;

impl Tokenizer for CodeTokenizer {
    /// Tokenizes code with identifier-aware splitting for BM25.
    ///
    /// The sparse index needs `parse_input`, `ParseInput`, and `parse input` to
    /// meet in the same token space, which is why this delegates to the Semble
    /// style identifier splitter instead of natural-language tokenization.
    fn tokenize(&self, input_text: &str) -> Vec<String> {
        split_identifier_tokens(input_text)
    }
}

/// Runtime index used by search and find-related operations.
///
/// The manifest is retained for warm-cache validation, while chunks, embeddings,
/// sparse BM25 state, and semantic candidate state are all aligned by chunk id.
pub struct SearchIndex {
    root: PathBuf,
    content: ContentFilter,
    manifest: Vec<FileManifestEntry>,
    chunks: Vec<Chunk>,
    embeddings: EmbeddingMatrix,
    semantic: SemanticBackend,
    bm25: SearchEngine<usize, u32, CodeTokenizer>,
    stats: IndexStats,
}

impl SearchIndex {
    #[cfg(test)]
    /// Builds an index directly from file entries for tests.
    pub fn build(
        root: PathBuf,
        content: ContentFilter,
        files: &[FileEntry],
        provider: &dyn EmbeddingProvider,
    ) -> Result<Self, CodeSearchError> {
        let outcome =
            IndexRefresh::refresh(root.as_path(), content, files.to_vec(), None, provider)?;
        Self::from_cached(CachedIndex {
            payload: outcome.payload,
            embeddings: outcome.embeddings,
        })
    }

    /// Builds the runtime index from a complete cached payload.
    ///
    /// Cache records can arrive grouped by file with row ranges into the cached
    /// matrix. This method flattens them into chunk order and copies the rows so
    /// runtime row ids are dense even after incremental refresh dropped files.
    pub fn from_cached(cached: CachedIndex) -> Result<Self, CodeSearchError> {
        let indexed_files = cached.payload.files.len();
        let mut manifest = Vec::new();
        let mut chunks = Vec::new();
        let mut embeddings = EmbeddingMatrix::empty();
        for record in cached.payload.files {
            if record.chunks.len() != record.embedding_count {
                return Err(CodeSearchError::Index(
                    "cached chunk and embedding counts do not match".to_string(),
                ));
            }
            manifest.push(record.manifest);
            chunks.extend(record.chunks);
            embeddings.extend_rows_from(
                &cached.embeddings,
                record.embedding_start,
                record.embedding_count,
            )?;
        }
        Self::from_parts(
            cached.payload.root,
            cached.payload.content,
            manifest,
            chunks,
            embeddings,
            indexed_files,
        )
    }

    /// Returns stable output statistics for the tool response.
    pub fn stats(&self) -> IndexStats {
        self.stats.clone()
    }

    /// Returns the canonical root used to build this index.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the content filter baked into this index.
    pub fn content(&self) -> ContentFilter {
        self.content
    }

    /// Checks whether an in-memory index still matches the current file set.
    ///
    /// The service only calls this after a manifest walk; a clean watcher can
    /// skip that walk for a short safety window.
    pub fn manifest_matches(&self, manifest: &[FileManifestEntry]) -> bool {
        self.manifest == manifest
    }

    /// Returns a chunk by the shared chunk/embedding/BM25 id.
    pub fn chunk(&self, chunk_id: usize) -> Option<&Chunk> {
        self.chunks.get(chunk_id)
    }

    #[cfg(test)]
    pub fn uses_hnsw_for_test(&self) -> bool {
        self.semantic.is_hnsw()
    }

    /// Runs dense semantic retrieval and returns exact cosine scores.
    ///
    /// HNSW is used only when there are no path or language filters. Filtered
    /// searches scan exactly so ANN recall cannot silently omit an allowed chunk
    /// before the filter is applied.
    pub fn semantic_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
        filters: &SearchFilters,
    ) -> Vec<(usize, f32)> {
        let candidate_ids = filters
            .is_empty()
            .then(|| {
                self.semantic
                    .candidate_ids(query_embedding, limit, self.embeddings.row_count())
            })
            .flatten();
        let mut scores = match candidate_ids {
            Some(ids) => ids
                .into_iter()
                .filter_map(|idx| {
                    // ANN returns ids only; exact cosine is recomputed here so
                    // ranking sees the same score scale as the full-scan path.
                    let embedding = self.embeddings.row(idx)?;
                    Some((idx, cosine_similarity(query_embedding, embedding)))
                })
                .filter(|(_, score)| *score > 0.0)
                .collect::<Vec<_>>(),
            None => (0..self.embeddings.row_count())
                .filter(|idx| {
                    self.chunks
                        .get(*idx)
                        .is_some_and(|chunk| filters.allows(chunk))
                })
                .filter_map(|idx| {
                    let embedding = self.embeddings.row(idx)?;
                    Some((idx, cosine_similarity(query_embedding, embedding)))
                })
                .filter(|(_, score)| *score > 0.0)
                .collect::<Vec<_>>(),
        };
        scores.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        scores.truncate(limit);
        scores
    }

    /// Runs BM25 sparse retrieval over enriched code chunks.
    pub fn sparse_search(
        &self,
        query: &str,
        limit: usize,
        filters: &SearchFilters,
    ) -> Vec<(usize, f32)> {
        self.bm25
            .search(query, limit)
            .into_iter()
            .filter(|result| result.score > 0.0)
            .filter(|result| {
                self.chunks
                    .get(result.document.id)
                    .is_some_and(|chunk| filters.allows(chunk))
            })
            .map(|result| (result.document.id, result.score))
            .collect()
    }

    /// Finds chunks semantically related to a source chunk.
    ///
    /// This path intentionally stays exact and same-language. `find_related` is
    /// usually scoped around a concrete code location, so predictable exclusion
    /// of the source chunk and language locality matter more than ANN latency.
    pub fn related_by_embedding(
        &self,
        source_idx: usize,
        limit: usize,
        filters: &SearchFilters,
    ) -> Vec<SearchResult> {
        let Some(source_chunk) = self.chunks.get(source_idx) else {
            return Vec::new();
        };
        let Some(source_embedding) = self.embeddings.row(source_idx) else {
            return Vec::new();
        };
        let mut candidates = (0..self.embeddings.row_count())
            .filter(|idx| *idx != source_idx)
            .filter_map(|idx| {
                let chunk = self.chunks.get(idx)?;
                if chunk.language != source_chunk.language || !filters.allows(chunk) {
                    return None;
                }
                let embedding = self.embeddings.row(idx)?;
                let score = cosine_similarity(source_embedding, embedding);
                (score > 0.0).then_some((idx, score))
            })
            .collect::<Vec<_>>();
        let mut compare_candidates = |left: &(usize, f32), right: &(usize, f32)| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| self.chunks[left.0].cmp_location(&self.chunks[right.0]))
        };
        let all_scores_equal = candidates.first().is_some_and(|(_, first_score)| {
            candidates
                .iter()
                .all(|(_, score)| score.total_cmp(first_score).is_eq())
        });
        if all_scores_equal && candidates.len() > limit {
            let (top_candidates, _, _) =
                candidates.select_nth_unstable_by(limit, &mut compare_candidates);
            top_candidates.sort_by(&mut compare_candidates);
        } else {
            candidates.sort_by(compare_candidates);
        }
        candidates.truncate(limit);
        candidates
            .into_iter()
            .filter_map(|(idx, score)| {
                self.chunks.get(idx).map(|chunk| SearchResult {
                    score,
                    chunk: chunk.clone(),
                })
            })
            .collect()
    }

    /// Locates the chunk that contains a 1-indexed source line.
    ///
    /// Paths are normalized with `/` separators so workspace-relative paths from
    /// tool input match cached paths on Windows and Unix.
    pub fn find_source_chunk(&self, file_path: &Path, line: usize) -> Option<usize> {
        let source_path = normalize_path_separators(file_path.to_string_lossy());
        self.chunks.iter().position(|chunk| {
            chunk.start_line <= line
                && line <= chunk.end_line
                && normalize_path_separators(chunk.file_path.to_string_lossy()).as_ref()
                    == source_path.as_ref()
        })
    }

    /// Constructs all runtime search structures from already-flattened pieces.
    fn from_parts(
        root: PathBuf,
        content: ContentFilter,
        manifest: Vec<FileManifestEntry>,
        chunks: Vec<Chunk>,
        embeddings: EmbeddingMatrix,
        indexed_files: usize,
    ) -> Result<Self, CodeSearchError> {
        if chunks.len() != embeddings.row_count() {
            return Err(CodeSearchError::Index(
                "cached chunk and embedding counts do not match".to_string(),
            ));
        }
        let semantic = SemanticBackend::build(&embeddings);
        let bm25 = build_bm25(&chunks);
        let stats = IndexStats {
            indexed_files,
            total_chunks: chunks.len(),
        };
        Ok(Self {
            root,
            content,
            manifest,
            chunks,
            embeddings,
            semantic,
            bm25,
            stats,
        })
    }
}

fn normalize_path_separators(path: Cow<'_, str>) -> Cow<'_, str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        path
    }
}

/// Builds the sparse search engine with file/path enrichment.
fn build_bm25(chunks: &[Chunk]) -> SearchEngine<usize, u32, CodeTokenizer> {
    let documents = chunks
        .iter()
        .enumerate()
        .map(|(idx, chunk)| Document::new(idx, enrich_for_bm25(chunk)))
        .collect::<Vec<_>>();
    SearchEngineBuilder::<usize, u32, CodeTokenizer>::with_tokenizer_and_documents(
        CodeTokenizer,
        documents,
    )
    .build()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::hint::black_box;
    use std::sync::Arc;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use crate::cache::{CachedFileRecord, CachedIndexPayloadV4, content_hash};
    use crate::dense::HashEmbeddingProvider;
    use crate::files::discover_files;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: index construction produces chunks, dense vectors, and searchable BM25 state.
    #[test]
    fn build_index_populates_sparse_and_dense_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("lib.rs"),
            "pub fn parse_input() {}\npub fn render_output() {}\n",
        )
        .expect("write");
        let files = discover_files(temp.path(), ContentFilter::Code).expect("files");
        let provider = Arc::new(HashEmbeddingProvider::new("test", 16));
        let index = SearchIndex::build(
            temp.path().to_path_buf(),
            ContentFilter::Code,
            &files,
            provider.as_ref(),
        )
        .expect("index");

        let sparse = index.sparse_search("parse input", 5, &SearchFilters::empty());

        assert_eq!(index.stats().indexed_files, 1);
        assert_eq!(index.stats().total_chunks, 1);
        assert_eq!(sparse.len(), 1);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: find-related source lookup uses relative paths and 1-indexed line spans.
    #[test]
    fn source_chunk_lookup_matches_line_range() {
        let chunk = Chunk {
            content: "fn parse() {}".to_string(),
            file_path: PathBuf::from("src/lib.rs"),
            start_line: 10,
            end_line: 12,
            language: "rust".to_string(),
        };
        let embeddings = EmbeddingMatrix::from_vectors(vec![vec![1.0]]).expect("matrix");
        let payload = CachedIndexPayloadV4::new(
            PathBuf::from("/repo"),
            ContentFilter::Code,
            "test".to_string(),
            &embeddings,
            vec![CachedFileRecord::new(
                FileManifestEntry {
                    path: PathBuf::from("src/lib.rs"),
                    size: 10,
                    modified_unix_nanos: 1,
                },
                crate::cache::content_hash("fn parse() {}"),
                vec![chunk],
                0,
                1,
            )],
        );
        let index = SearchIndex::from_cached(CachedIndex {
            payload,
            embeddings,
        })
        .expect("index");

        assert_eq!(
            index.find_source_chunk(Path::new("src/lib.rs"), 11),
            Some(0)
        );
        assert_eq!(index.find_source_chunk(Path::new("src/lib.rs"), 13), None);
    }

    #[test]
    fn source_chunk_lookup_normalizes_backslash_separators() {
        let index = index_with_chunks_and_embeddings(vec![(
            Chunk {
                content: "fn parse() {}".to_string(),
                file_path: PathBuf::from(r"src\lib.rs"),
                start_line: 10,
                end_line: 12,
                language: "rust".to_string(),
            },
            vec![1.0],
        )]);

        assert_eq!(
            index.find_source_chunk(Path::new("src/lib.rs"), 11),
            Some(0)
        );
        assert_eq!(
            index.find_source_chunk(Path::new(r"src\lib.rs"), 11),
            Some(0)
        );
    }

    #[test]
    #[ignore]
    fn bench_find_source_chunk_late_match() {
        let chunk_count = 10_000;
        let chunks = (0..chunk_count)
            .map(|idx| {
                (
                    Chunk {
                        content: format!("fn generated_{idx}() {{}}"),
                        file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                        start_line: idx * 3 + 1,
                        end_line: idx * 3 + 2,
                        language: "rust".to_string(),
                    },
                    vec![1.0, 0.0],
                )
            })
            .collect::<Vec<_>>();
        let index = index_with_chunks_and_embeddings(chunks);
        let target_path = Path::new("src/generated/file_9999.rs");
        let target_line = 29_999;
        let expected = index.find_source_chunk(target_path, target_line);
        let iterations = 5_000;
        let started = Instant::now();
        let mut found_sum = 0usize;

        for _ in 0..iterations {
            found_sum += black_box(&index)
                .find_source_chunk(black_box(target_path), black_box(target_line))
                .expect("source chunk");
        }

        let elapsed = started.elapsed();
        assert_eq!(expected, Some(chunk_count - 1));
        assert_eq!(found_sum, (chunk_count - 1) * iterations);
        println!(
            "find_source_chunk_late_match iterations={iterations} chunks={chunk_count} elapsed_ms={} per_lookup_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    fn related_by_embedding_returns_top_same_language_results() {
        let related_chunk = Chunk {
            content: "fn parse_related() {}".to_string(),
            file_path: PathBuf::from("src/related.rs"),
            start_line: 5,
            end_line: 5,
            language: "rust".to_string(),
        };
        let index = index_with_chunks_and_embeddings(vec![
            (
                Chunk {
                    content: "fn parse_input() {}".to_string(),
                    file_path: PathBuf::from("src/input.rs"),
                    start_line: 1,
                    end_line: 1,
                    language: "rust".to_string(),
                },
                vec![1.0, 0.0],
            ),
            (related_chunk.clone(), vec![1.0, 0.0]),
            (
                Chunk {
                    content: "def parse_related(): pass".to_string(),
                    file_path: PathBuf::from("src/related.py"),
                    start_line: 1,
                    end_line: 1,
                    language: "python".to_string(),
                },
                vec![1.0, 0.0],
            ),
            (
                Chunk {
                    content: "fn unrelated() {}".to_string(),
                    file_path: PathBuf::from("src/unrelated.rs"),
                    start_line: 10,
                    end_line: 10,
                    language: "rust".to_string(),
                },
                vec![0.0, 1.0],
            ),
        ]);

        let results = index.related_by_embedding(0, 5, &SearchFilters::empty());
        let expected = vec![SearchResult {
            score: 1.0,
            chunk: related_chunk,
        }];

        assert_eq!(results, expected);
    }

    #[test]
    fn related_by_embedding_partial_top_k_matches_full_sorted_prefix() {
        let chunk_count = 80;
        let chunks = (0..chunk_count)
            .map(|idx| {
                (
                    Chunk {
                        content: format!("fn generated_{idx}() {{ parse_input(); }}"),
                        file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                        start_line: idx + 1,
                        end_line: idx + 1,
                        language: "rust".to_string(),
                    },
                    vec![1.0, 0.0],
                )
            })
            .collect::<Vec<_>>();
        let index = index_with_chunks_and_embeddings(chunks);
        let top_k = 10;

        let partial = index.related_by_embedding(0, top_k, &SearchFilters::empty());
        let mut full = index.related_by_embedding(0, chunk_count, &SearchFilters::empty());
        full.truncate(top_k);

        assert_eq!(partial, full);
    }

    #[test]
    #[ignore]
    fn bench_related_by_embedding_many_candidates_small_top_k() {
        let chunk_count = 10_000;
        let content = "fn generated() { parse_input(); render_output(); }\n".repeat(20);
        let chunks = (0..chunk_count)
            .map(|idx| {
                (
                    Chunk {
                        content: format!("{content}// chunk {idx}"),
                        file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                        start_line: idx * 3 + 1,
                        end_line: idx * 3 + 2,
                        language: "rust".to_string(),
                    },
                    vec![1.0, idx as f32 / chunk_count as f32],
                )
            })
            .collect::<Vec<_>>();
        let index = index_with_chunks_and_embeddings(chunks);
        let iterations = 100;
        let top_k = 5;
        let expected = index.related_by_embedding(0, top_k, &SearchFilters::empty());
        let started = Instant::now();
        let mut total_results = 0usize;
        let mut total_content_bytes = 0usize;

        for _ in 0..iterations {
            let results = black_box(&index).related_by_embedding(
                black_box(0),
                black_box(top_k),
                black_box(&SearchFilters::empty()),
            );
            total_results += black_box(results.len());
            total_content_bytes += results
                .iter()
                .map(|result| black_box(result.chunk.content.len()))
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(expected.len(), top_k);
        assert_eq!(total_results, top_k * iterations);
        assert!(total_content_bytes > 0);
        println!(
            "related_by_embedding_many_candidates_small_top_k iterations={iterations} chunks={chunk_count} top_k={top_k} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_related_by_embedding_tie_heavy_candidates() {
        let chunk_count = 10_000;
        let content = "fn generated() { parse_input(); render_output(); }\n".repeat(10);
        let chunks = (0..chunk_count)
            .map(|idx| {
                (
                    Chunk {
                        content: format!("{content}// chunk {idx}"),
                        file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                        start_line: idx * 3 + 1,
                        end_line: idx * 3 + 2,
                        language: "rust".to_string(),
                    },
                    vec![1.0, 0.0],
                )
            })
            .collect::<Vec<_>>();
        let index = index_with_chunks_and_embeddings(chunks);
        let iterations = 100;
        let top_k = 5;
        let expected = index.related_by_embedding(0, top_k, &SearchFilters::empty());
        let started = Instant::now();
        let mut total_results = 0usize;
        let mut total_content_bytes = 0usize;

        for _ in 0..iterations {
            let results = black_box(&index).related_by_embedding(
                black_box(0),
                black_box(top_k),
                black_box(&SearchFilters::empty()),
            );
            total_results += black_box(results.len());
            total_content_bytes += results
                .iter()
                .map(|result| black_box(result.chunk.content.len()))
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(expected.len(), top_k);
        assert_eq!(total_results, top_k * iterations);
        assert!(total_content_bytes > 0);
        println!(
            "related_by_embedding_tie_heavy_candidates iterations={iterations} chunks={chunk_count} top_k={top_k} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: large-enough indexes use HNSW candidates and exact cosine reranking.
    #[test]
    fn semantic_search_uses_hnsw_candidates_with_exact_rerank() {
        let index = index_with_chunks_and_embeddings(vec![
            (
                Chunk {
                    content: "fn alpha() {}".to_string(),
                    file_path: PathBuf::from("src/a.rs"),
                    start_line: 1,
                    end_line: 1,
                    language: "rust".to_string(),
                },
                vec![1.0, 0.0],
            ),
            (
                Chunk {
                    content: "fn beta() {}".to_string(),
                    file_path: PathBuf::from("src/b.rs"),
                    start_line: 1,
                    end_line: 1,
                    language: "rust".to_string(),
                },
                vec![0.0, 1.0],
            ),
        ]);

        let results = index.semantic_search(&[1.0, 0.0], 1, &SearchFilters::empty());

        assert_eq!(index.uses_hnsw_for_test(), true);
        assert_eq!(results, vec![(0, 1.0)]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: path or language filtered semantic search remains exact instead of using ANN.
    #[test]
    fn filtered_semantic_search_uses_exact_path() {
        let index = index_with_chunks_and_embeddings(vec![
            (
                Chunk {
                    content: "fn alpha() {}".to_string(),
                    file_path: PathBuf::from("src/a.rs"),
                    start_line: 1,
                    end_line: 1,
                    language: "rust".to_string(),
                },
                vec![1.0, 0.0],
            ),
            (
                Chunk {
                    content: "fn beta() {}".to_string(),
                    file_path: PathBuf::from("src/b.rs"),
                    start_line: 1,
                    end_line: 1,
                    language: "rust".to_string(),
                },
                vec![0.0, 1.0],
            ),
        ]);
        let filters = SearchFilters::normalized(vec!["src/b.rs".to_string()], Vec::new());

        let results = index.semantic_search(&[1.0, 0.0], 5, &filters);

        assert_eq!(index.uses_hnsw_for_test(), true);
        assert_eq!(results, Vec::<(usize, f32)>::new());
    }

    #[test]
    fn filtered_semantic_search_top_k_matches_full_sorted_prefix() {
        let chunk_count = 80;
        let chunks = (0..chunk_count)
            .map(|idx| {
                (
                    Chunk {
                        content: format!("fn generated_{idx}() {{}}"),
                        file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                        start_line: idx + 1,
                        end_line: idx + 1,
                        language: "rust".to_string(),
                    },
                    vec![1.0, idx as f32 / chunk_count as f32],
                )
            })
            .collect();
        let index = index_with_chunks_and_embeddings(chunks);
        let filters = SearchFilters::normalized(Vec::new(), vec!["rust".to_string()]);
        let mut full = index.semantic_search(&[1.0, 0.0], chunk_count, &filters);
        full.truncate(10);

        assert_eq!(index.semantic_search(&[1.0, 0.0], 10, &filters), full);
    }

    #[test]
    #[ignore]
    fn bench_filtered_semantic_search_many_candidates_small_top_k() {
        let chunk_count = 10_000;
        let chunks = (0..chunk_count)
            .map(|idx| {
                (
                    Chunk {
                        content: format!("fn generated_{idx}() {{}}"),
                        file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                        start_line: idx + 1,
                        end_line: idx + 1,
                        language: "rust".to_string(),
                    },
                    vec![1.0, idx as f32 / chunk_count as f32],
                )
            })
            .collect();
        let index = index_with_chunks_and_embeddings(chunks);
        let filters = SearchFilters::normalized(Vec::new(), vec!["rust".to_string()]);
        let iterations = 100;
        let top_k = 5;
        let expected = index.semantic_search(&[1.0, 0.0], top_k, &filters);
        let started = Instant::now();
        let mut total_results = 0usize;

        for _ in 0..iterations {
            total_results += black_box(&index)
                .semantic_search(
                    black_box(&[1.0, 0.0]),
                    black_box(top_k),
                    black_box(&filters),
                )
                .len();
        }

        let elapsed = started.elapsed();
        assert_eq!(expected.len(), top_k);
        assert_eq!(total_results, top_k * iterations);
        println!(
            "filtered_semantic_search_many_candidates_small_top_k iterations={iterations} chunks={chunk_count} top_k={top_k} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    fn index_with_chunks_and_embeddings(chunks: Vec<(Chunk, Vec<f32>)>) -> SearchIndex {
        let vectors = chunks
            .iter()
            .map(|(_, embedding)| embedding.clone())
            .collect::<Vec<_>>();
        let embeddings = EmbeddingMatrix::from_vectors(vectors).expect("matrix");
        let records = chunks
            .into_iter()
            .enumerate()
            .map(|(idx, (chunk, _))| {
                CachedFileRecord::new(
                    FileManifestEntry {
                        path: chunk.file_path.clone(),
                        size: chunk.content.len() as u64,
                        modified_unix_nanos: 1,
                    },
                    content_hash(&chunk.content),
                    vec![chunk],
                    idx,
                    1,
                )
            })
            .collect::<Vec<_>>();
        let payload = CachedIndexPayloadV4::new(
            PathBuf::from("/repo"),
            ContentFilter::Code,
            "test".to_string(),
            &embeddings,
            records,
        );
        SearchIndex::from_cached(CachedIndex {
            payload,
            embeddings,
        })
        .expect("index")
    }
}
