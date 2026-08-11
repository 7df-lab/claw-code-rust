//! Semantic candidate backend.
//!
//! Small indexes use exact scans because the overhead of ANN construction is not
//! worthwhile. Larger indexes build an HNSW graph to produce candidate ids for
//! unfiltered semantic search, then `SearchIndex` recomputes exact cosine scores
//! before ranking. This separation keeps ANN as a latency optimization rather
//! than a source of truth for scores or filter behavior.

use hnsw_rs::prelude::{DistCosine, Hnsw};

use crate::matrix::EmbeddingMatrix;

const MAX_NB_CONNECTION: usize = 24;
const EF_CONSTRUCTION: usize = 200;
const MIN_EF_SEARCH: usize = 64;
const EF_SEARCH_MULTIPLIER: usize = 4;

#[cfg(not(test))]
const ANN_MIN_ROWS: usize = 1_024;
#[cfg(test)]
const ANN_MIN_ROWS: usize = 2;

pub enum SemanticBackend {
    Exact,
    Hnsw {
        index: Hnsw<'static, f32, DistCosine>,
        dimensions: usize,
    },
}

impl SemanticBackend {
    /// Builds the semantic candidate backend for a flat embedding matrix.
    ///
    /// The production threshold avoids spending memory and CPU on HNSW for small
    /// repositories where a linear scan is cheap and exact. Tests lower the
    /// threshold so ANN behavior stays covered.
    pub fn build(embeddings: &EmbeddingMatrix) -> Self {
        let row_count = embeddings.row_count();
        if row_count < ANN_MIN_ROWS || embeddings.dimensions() == 0 {
            return Self::Exact;
        }
        let max_layer = 16.min((row_count as f32).ln().floor().max(1.0) as usize);
        let mut hnsw = Hnsw::<f32, DistCosine>::new(
            MAX_NB_CONNECTION,
            row_count,
            max_layer,
            EF_CONSTRUCTION,
            DistCosine {},
        );
        let vectors = (0..row_count)
            .filter_map(|idx| embeddings.row(idx).map(<[f32]>::to_vec))
            .collect::<Vec<_>>();
        let refs = vectors
            .iter()
            .enumerate()
            .map(|(idx, vector)| (vector, idx))
            .collect::<Vec<_>>();
        hnsw.parallel_insert(&refs);
        hnsw.set_searching_mode(true);
        Self::Hnsw {
            index: hnsw,
            dimensions: embeddings.dimensions(),
        }
    }

    /// Returns ANN candidate ids, or `None` when callers should scan exactly.
    ///
    /// Dimension mismatches return an empty candidate set instead of panicking;
    /// that keeps model/cache inconsistencies recoverable as empty semantic hits
    /// while cache validation prevents normal mismatches.
    pub fn candidate_ids(
        &self,
        query_embedding: &[f32],
        limit: usize,
        total_rows: usize,
    ) -> Option<Vec<usize>> {
        match self {
            Self::Exact => None,
            Self::Hnsw { index, dimensions } => {
                if total_rows == 0 || query_embedding.len() != *dimensions {
                    return Some(Vec::new());
                }
                // Over-fetch so exact reranking still has room to correct ANN
                // ordering before the hybrid ranker truncates to top_k.
                let candidate_limit = total_rows.min(
                    limit
                        .saturating_mul(EF_SEARCH_MULTIPLIER)
                        .max(MIN_EF_SEARCH),
                );
                let ef_search = candidate_limit.max(MIN_EF_SEARCH).min(total_rows);
                Some(
                    index
                        .search(query_embedding, candidate_limit, ef_search)
                        .into_iter()
                        .map(|neighbor| neighbor.d_id)
                        .collect(),
                )
            }
        }
    }

    #[cfg(test)]
    pub fn is_hnsw(&self) -> bool {
        matches!(self, Self::Hnsw { .. })
    }
}
