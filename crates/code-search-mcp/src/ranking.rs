//! Hybrid ranking and code-aware reranking.
//!
//! Retrieval starts with dense semantic candidates and sparse BM25 candidates,
//! fuses their ranks with reciprocal rank fusion, then applies Semble-style
//! code-specific adjustments. Symbol-like queries lean more on sparse matching
//! because exact identifiers matter; natural-language queries keep semantic and
//! sparse signals balanced. The final penalties bias results toward production
//! code while still allowing tests/docs/examples to appear when their relevance
//! is strong.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use crate::index::SearchIndex;
use crate::tokens::{is_symbol_query, query_terms};
use crate::types::{Chunk, SearchFilters, SearchResult};

const RRF_K: f32 = 60.0;
const CANDIDATE_MULTIPLIER: usize = 5;
const SYMBOL_DEFINITION_KEYWORDS: &[&str] = &[
    "fn", "struct", "enum", "trait", "impl", "mod", "const", "static", "class", "def", "function",
];

/// Runs hybrid ranking for a query and returns the final top_k chunks.
///
/// Both retrieval channels intentionally over-fetch so reranking can correct for
/// path relevance, symbol definitions, and repeated file hits before truncation.
pub fn rank_search(
    index: &SearchIndex,
    query: &str,
    query_embedding: &[f32],
    top_k: usize,
    filters: &SearchFilters,
) -> Vec<SearchResult> {
    let candidate_limit = top_k.saturating_mul(CANDIDATE_MULTIPLIER).max(top_k);
    let semantic = index.semantic_search(query_embedding, candidate_limit, filters);
    let sparse = index.sparse_search(query, candidate_limit, filters);
    let alpha = resolve_alpha(query);
    let mut scores =
        HashMap::<usize, f32>::with_capacity(semantic.len().saturating_add(sparse.len()));

    for (rank, (chunk_id, _)) in semantic.into_iter().enumerate() {
        *scores.entry(chunk_id).or_default() += alpha * reciprocal_rank(rank);
    }
    for (rank, (chunk_id, _)) in sparse.into_iter().enumerate() {
        *scores.entry(chunk_id).or_default() += (1.0 - alpha) * reciprocal_rank(rank);
    }

    rerank(index, query, scores, top_k)
}

/// Chooses the dense/sparse mixture for the query shape.
fn resolve_alpha(query: &str) -> f32 {
    if is_symbol_query(query) { 0.3 } else { 0.5 }
}

/// Standard RRF contribution for a zero-based rank.
fn reciprocal_rank(rank: usize) -> f32 {
    1.0 / (RRF_K + rank as f32 + 1.0)
}

/// Applies code-aware boosts and penalties after dense/sparse fusion.
///
/// This stage is multiplicative so BM25 and semantic order still dominate, but
/// obvious code-search signals can break close ties: symbol definitions, file
/// stem/path matches, and production-code preference.
fn rerank(
    index: &SearchIndex,
    query: &str,
    scores: HashMap<usize, f32>,
    top_k: usize,
) -> Vec<SearchResult> {
    if scores.is_empty() {
        return Vec::new();
    }

    let mut contexts = Vec::with_capacity(scores.len());
    for (chunk_id, base_score) in scores {
        if let Some(chunk) = index.chunk(chunk_id) {
            let path = normalized_path(&chunk.file_path);
            contexts.push((chunk_id, base_score, path, chunk));
        }
    }
    let mut file_counts = HashMap::<&str, usize>::with_capacity(contexts.len());
    for (_, _, path, _) in &contexts {
        *file_counts.entry(path.as_ref()).or_default() += 1;
    }
    let all_files_unique = file_counts.len() == contexts.len();

    let symbol_query = is_symbol_query(query);
    let symbol = query
        .trim()
        .trim_end_matches("()")
        .rsplit("::")
        .next()
        .unwrap_or(query);
    let terms = query_terms(query);

    let score_candidate =
        |base_score: f32, path: &str, chunk: &Chunk, file_count: Option<usize>| {
            let lowercase_path = lowercase_path(path);
            let mut score = base_score;
            if let Some(file_count) = file_count {
                score *= multi_chunk_file_boost(file_count);
            }
            if symbol_query && contains_symbol_definition(&chunk.content, symbol) {
                score *= 1.35;
            }
            score *= path_keyword_boost(lowercase_path.as_ref(), &terms);
            score *= path_penalty(lowercase_path.as_ref());
            score
        };

    let mut saturated = if all_files_unique {
        // Unique-file candidate sets are common after retrieval over many files.
        // Avoid allocating candidate/count buffers when saturation will not use
        // them.
        drop(file_counts);
        contexts
            .into_iter()
            .map(|(_, base_score, path, chunk)| {
                let score = score_candidate(base_score, path.as_ref(), chunk, None);
                (score, chunk)
            })
            .collect::<Vec<_>>()
    } else {
        // `file_counts` borrows the path strings stored in `contexts`; materialize
        // cheap counts before moving contexts into candidate tuples.
        let context_counts = contexts
            .iter()
            .map(|(_, _, path, _)| file_counts.get(path.as_ref()).copied().unwrap_or(1))
            .collect::<Vec<_>>();
        drop(file_counts);
        let mut candidates = contexts
            .into_iter()
            .zip(context_counts)
            .map(|((chunk_id, base_score, path, chunk), file_count)| {
                let score = score_candidate(base_score, path.as_ref(), chunk, Some(file_count));
                (chunk_id, score, path, chunk)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut seen_per_file = HashMap::<Cow<'_, str>, usize>::with_capacity(candidates.len());
        // Saturation prevents one file with many adjacent chunks from crowding out
        // other relevant files while preserving deterministic ordering within ties.
        candidates
            .into_iter()
            .map(|(_chunk_id, score, path, chunk)| {
                let seen = seen_per_file.entry(path).or_default();
                let saturated_score = score / (1.0 + 0.2 * *seen as f32);
                *seen += 1;
                (saturated_score, chunk)
            })
            .collect::<Vec<_>>()
    };

    let mut compare_results = |left: &(f32, &Chunk), right: &(f32, &Chunk)| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.cmp_location(right.1))
    };
    if saturated.len() > top_k {
        let (top_results, _, _) = saturated.select_nth_unstable_by(top_k, &mut compare_results);
        top_results.sort_by(&mut compare_results);
        saturated.truncate(top_k);
    } else {
        saturated.sort_by(compare_results);
    }
    saturated
        .into_iter()
        .map(|(score, chunk)| SearchResult {
            score,
            chunk: chunk.clone(),
        })
        .collect()
}

fn normalized_path(path: &Path) -> Cow<'_, str> {
    let path = path.to_string_lossy();
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        path
    }
}

fn lowercase_path(path: &str) -> Cow<'_, str> {
    if path
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
    {
        Cow::Borrowed(path)
    } else {
        Cow::Owned(path.to_lowercase())
    }
}

/// Gives a small lift when independent chunks from the same file agree.
fn multi_chunk_file_boost(count: usize) -> f32 {
    1.0 + count.saturating_sub(1).min(3) as f32 * 0.08
}

/// Detects simple language-agnostic definition patterns for symbol queries.
fn contains_symbol_definition(content: &str, symbol: &str) -> bool {
    if symbol.is_empty() {
        return false;
    }
    content.match_indices(symbol).any(|(symbol_start, _)| {
        symbol_start > 0
            && content[..symbol_start].ends_with(' ')
            && SYMBOL_DEFINITION_KEYWORDS
                .iter()
                .any(|keyword| content[..symbol_start - 1].ends_with(keyword))
    })
}

/// Rewards query terms that appear in the file stem or recent path context.
fn path_keyword_boost(lowercase_path: &str, terms: &[String]) -> f32 {
    if terms.is_empty() {
        return 1.0;
    }
    let mut boost: f32 = 1.0;
    for term in terms {
        if lowercase_path.contains(term) {
            boost += 0.05;
        }
    }
    boost.min(1.25)
}

/// Penalizes paths that are usually less central than production source.
///
/// The penalties are soft so a test/doc/compat chunk can still win when the
/// retrieval signals are much stronger than nearby production-code candidates.
fn path_penalty(path: &str) -> f32 {
    let mut penalty = 1.0;
    if path.starts_with("tests/")
        || path.contains("/tests/")
        || path.starts_with("test/")
        || path.contains("/test/")
        || path.contains("_test.")
        || path.starts_with("spec/")
        || path.contains("/spec/")
        || path.contains("_spec.")
    {
        penalty *= 0.82;
    }
    if path.starts_with("examples/")
        || path.contains("/examples/")
        || path.starts_with("docs/")
        || path.contains("/docs/")
    {
        penalty *= 0.88;
    }
    if path.starts_with("legacy/")
        || path.contains("/legacy/")
        || path.starts_with("compat/")
        || path.contains("/compat/")
    {
        penalty *= 0.78;
    }
    if path.ends_with("__init__.py")
        || path.ends_with("package-info.java")
        || path.ends_with(".d.ts")
    {
        penalty *= 0.75;
    }
    penalty
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::path::PathBuf;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use crate::cache::{CachedFileRecord, CachedIndex, CachedIndexPayloadV4, content_hash};
    use crate::files::FileManifestEntry;
    use crate::index::SearchIndex;
    use crate::matrix::EmbeddingMatrix;
    use crate::types::{Chunk, ContentFilter};

    use super::*;

    fn index_with_chunks(chunks: Vec<Chunk>) -> SearchIndex {
        let vectors: Vec<Vec<f32>> = chunks
            .iter()
            .map(|chunk| {
                if chunk.content.contains("parser") {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect();
        let embeddings = EmbeddingMatrix::from_vectors(vectors).expect("matrix");
        let records = chunks
            .into_iter()
            .enumerate()
            .map(|(idx, chunk)| {
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
            .collect();
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

    /// Trace: L2-DES-TOOL-001
    /// Verifies: hybrid ranking applies BM25 and semantic RRF to return relevant chunks.
    #[test]
    fn rank_search_returns_relevant_chunk() {
        let index = index_with_chunks(vec![
            Chunk {
                content: "fn parse_input() { parser(); }".to_string(),
                file_path: PathBuf::from("src/parser.rs"),
                start_line: 1,
                end_line: 1,
                language: "rust".to_string(),
            },
            Chunk {
                content: "fn render_output() {}".to_string(),
                file_path: PathBuf::from("src/render.rs"),
                start_line: 1,
                end_line: 1,
                language: "rust".to_string(),
            },
        ]);

        let results = rank_search(
            &index,
            "parse input",
            &[1.0, 0.0],
            1,
            &SearchFilters::empty(),
        );

        assert_eq!(results[0].chunk.file_path, PathBuf::from("src/parser.rs"));
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: path penalties prefer production code over test chunks when base relevance ties.
    #[test]
    fn rerank_penalizes_test_paths() {
        let index = index_with_chunks(vec![
            Chunk {
                content: "fn parse_input() { parser(); }".to_string(),
                file_path: PathBuf::from("tests/parser_test.rs"),
                start_line: 1,
                end_line: 1,
                language: "rust".to_string(),
            },
            Chunk {
                content: "fn parse_input() { parser(); }".to_string(),
                file_path: PathBuf::from("src/parser.rs"),
                start_line: 1,
                end_line: 1,
                language: "rust".to_string(),
            },
        ]);

        let results = rank_search(
            &index,
            "parse_input",
            &[1.0, 0.0],
            1,
            &SearchFilters::empty(),
        );

        assert_eq!(results[0].chunk.file_path, PathBuf::from("src/parser.rs"));
    }

    #[test]
    fn rerank_partial_top_k_matches_full_sorted_prefix() {
        let chunk_count = 80;
        let chunks = (0..chunk_count)
            .map(|idx| Chunk {
                content: format!("fn parse_input_{idx}() {{ parser(); }}"),
                file_path: PathBuf::from(format!("src/parser/module_{}.rs", idx % 17)),
                start_line: idx + 1,
                end_line: idx + 1,
                language: "rust".to_string(),
            })
            .collect();
        let index = index_with_chunks(chunks);
        let scores = (0..chunk_count)
            .map(|idx| (idx, 1.0f32 / ((idx % 13) + 1) as f32))
            .collect::<HashMap<_, _>>();

        let top_k = 10;
        let partial = rerank(&index, "parse_input", scores.clone(), top_k);
        let mut full = rerank(&index, "parse_input", scores, chunk_count);
        full.truncate(top_k);

        assert_eq!(partial, full);
    }

    #[test]
    fn rerank_unique_file_fast_path_matches_full_sorted_prefix() {
        let chunk_count = 80;
        let chunks = (0..chunk_count)
            .map(|idx| Chunk {
                content: format!("fn unique_symbol_{idx}() {{ parser(); }}"),
                file_path: PathBuf::from(format!("src/unique/module_{idx}.rs")),
                start_line: idx + 1,
                end_line: idx + 1,
                language: "rust".to_string(),
            })
            .collect();
        let index = index_with_chunks(chunks);
        let scores = (0..chunk_count)
            .map(|idx| (idx, 1.0f32 / ((idx % 11) + 1) as f32))
            .collect::<HashMap<_, _>>();

        let top_k = 10;
        let partial = rerank(&index, "unique_symbol", scores.clone(), top_k);
        let mut full = rerank(&index, "unique_symbol", scores, chunk_count);
        full.truncate(top_k);

        assert_eq!(partial, full);
    }

    #[test]
    fn path_keyword_boost_uses_lowercase_path_terms() {
        assert_eq!(
            path_keyword_boost("src/parse_input.rs", &["parse".to_string()]),
            1.05
        );
        assert_eq!(
            path_keyword_boost("src/other.rs", &["parse".to_string()]),
            1.0
        );
    }

    #[test]
    fn lowercase_path_borrows_ascii_lowercase_paths() {
        assert!(matches!(
            lowercase_path("src/parser/module.rs"),
            std::borrow::Cow::Borrowed(_)
        ));
        let uppercase = lowercase_path("SRC/Parser/Module.rs");
        assert!(matches!(uppercase, std::borrow::Cow::Owned(_)));
        assert_eq!(uppercase.as_ref(), "src/parser/module.rs");
    }

    #[test]
    fn normalized_path_preserves_forward_slash_paths() {
        assert_eq!(
            normalized_path(Path::new("src/parser/module.rs")),
            "src/parser/module.rs"
        );
    }

    #[test]
    fn normalized_path_converts_backslash_separators() {
        assert_eq!(
            normalized_path(Path::new("src\\parser\\module.rs")),
            "src/parser/module.rs"
        );
    }

    #[test]
    fn contains_symbol_definition_matches_keyword_prefix() {
        assert_eq!(
            vec![
                contains_symbol_definition("pub fn parse_input() {}", "parse_input"),
                contains_symbol_definition("let parse_input = value;", "parse_input"),
            ],
            vec![true, false]
        );
    }

    #[test]
    #[ignore]
    fn bench_rank_search_many_candidates() {
        let chunks = (0..512)
            .map(|idx| {
                let file_path = if idx % 7 == 0 {
                    PathBuf::from(format!("tests/parser_{idx}_test.rs"))
                } else if idx % 11 == 0 {
                    PathBuf::from(format!("docs/parser_{idx}.md"))
                } else {
                    PathBuf::from(format!("src/parser/module_{idx}.rs"))
                };
                Chunk {
                    content: format!(
                        "fn parse_input_{idx}() {{ parser(); let value = parse_token_{idx}; }}"
                    ),
                    file_path,
                    start_line: idx + 1,
                    end_line: idx + 1,
                    language: "rust".to_string(),
                }
            })
            .collect();
        let index = index_with_chunks(chunks);
        let iterations = 10_000;
        let expected_len = rank_search(
            &index,
            "parse_input",
            &[1.0, 0.0],
            20,
            &SearchFilters::empty(),
        )
        .len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(rank_search(
                black_box(&index),
                black_box("parse_input"),
                black_box(&[1.0, 0.0]),
                black_box(20),
                black_box(&SearchFilters::empty()),
            ))
            .len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "rank_search_many_candidates iterations={iterations} chunks=512 top_k=20 elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_rerank_tie_heavy_scores() {
        let chunk_count = 2_000;
        let chunks = (0..chunk_count)
            .map(|idx| Chunk {
                content: format!("fn generated_{idx}() {{ value(); }}"),
                file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                start_line: idx + 1,
                end_line: idx + 1,
                language: "rust".to_string(),
            })
            .collect();
        let index = index_with_chunks(chunks);
        let scores = (0..chunk_count)
            .map(|idx| (idx, 1.0f32))
            .collect::<HashMap<_, _>>();
        let iterations = 1_000;
        let top_k = 20;
        let expected_len = rerank(&index, "lookup", scores.clone(), top_k).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(rerank(
                black_box(&index),
                black_box("lookup"),
                black_box(scores.clone()),
                black_box(top_k),
            ))
            .len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "rerank_tie_heavy_scores iterations={iterations} chunks={chunk_count} top_k={top_k} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
