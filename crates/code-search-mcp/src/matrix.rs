//! Row-major embedding matrix.
//!
//! The rest of code-search treats chunk id, embedding row id, and BM25 document
//! id as the same number. This small type centralizes the row-major vector
//! invariant so cache loading, incremental refresh, exact scans, and HNSW
//! construction all reject dimension drift in the same way.

use crate::types::CodeSearchError;

/// Flat row-major f32 matrix used for dense embeddings.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingMatrix {
    dimensions: usize,
    rows: Vec<f32>,
}

impl EmbeddingMatrix {
    /// Creates an empty matrix with no known dimension.
    pub fn empty() -> Self {
        Self {
            dimensions: 0,
            rows: Vec::new(),
        }
    }

    /// Creates a matrix from already-flat row-major values.
    ///
    /// Empty matrices must have zero dimensions; non-empty matrices must have a
    /// value count divisible by the dimension so row slicing stays safe.
    pub fn new(dimensions: usize, rows: Vec<f32>) -> Result<Self, CodeSearchError> {
        if dimensions == 0 {
            if rows.is_empty() {
                return Ok(Self::empty());
            }
            return Err(CodeSearchError::Index(
                "embedding dimensions must be non-zero when rows are present".to_string(),
            ));
        }
        if !rows.len().is_multiple_of(dimensions) {
            return Err(CodeSearchError::Index(format!(
                "embedding matrix has {} values, not divisible by {dimensions} dimensions",
                rows.len()
            )));
        }
        Ok(Self { dimensions, rows })
    }

    /// Flattens provider vectors into the matrix representation.
    ///
    /// Providers are required to keep dimensions stable. This check catches
    /// mismatched rows before the cache can assign chunk ids to invalid vectors.
    pub fn from_vectors(vectors: Vec<Vec<f32>>) -> Result<Self, CodeSearchError> {
        let Some(first) = vectors.first() else {
            return Ok(Self::empty());
        };
        let dimensions = first.len();
        if dimensions == 0 {
            return Err(CodeSearchError::Index(
                "embedding provider returned zero-dimensional vectors".to_string(),
            ));
        }
        let mut rows = Vec::with_capacity(vectors.len() * dimensions);
        for vector in vectors {
            if vector.len() != dimensions {
                return Err(CodeSearchError::Index(
                    "embedding provider returned vectors with inconsistent dimensions".to_string(),
                ));
            }
            rows.extend(vector);
        }
        Self::new(dimensions, rows)
    }

    /// Returns the embedding dimension for every row.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Returns the number of complete rows.
    pub fn row_count(&self) -> usize {
        if self.dimensions == 0 {
            0
        } else {
            self.rows.len() / self.dimensions
        }
    }

    /// Returns a row slice without allocating.
    pub fn row(&self, idx: usize) -> Option<&[f32]> {
        if idx >= self.row_count() {
            return None;
        }
        let start = idx * self.dimensions;
        let end = start + self.dimensions;
        Some(&self.rows[start..end])
    }

    /// Returns all row-major values for binary cache serialization.
    pub fn rows(&self) -> &[f32] {
        &self.rows
    }

    /// Copies a range of rows from another matrix.
    ///
    /// Incremental refresh uses this for reused cache records so the new matrix
    /// can drop deleted files while keeping chunk id == row id in the runtime
    /// index.
    pub fn extend_rows_from(
        &mut self,
        other: &Self,
        start: usize,
        count: usize,
    ) -> Result<(), CodeSearchError> {
        if count == 0 {
            return Ok(());
        }
        if other.dimensions == 0 {
            return Err(CodeSearchError::Index(
                "cached embedding matrix has no rows".to_string(),
            ));
        }
        let end = start.checked_add(count).ok_or_else(|| {
            CodeSearchError::Index("cached embedding row range overflow".to_string())
        })?;
        if end > other.row_count() {
            return Err(CodeSearchError::Index(format!(
                "cached embedding row range {start}..{end} is outside the matrix"
            )));
        }
        if self.dimensions == 0 {
            self.dimensions = other.dimensions;
        } else if self.dimensions != other.dimensions {
            return Err(CodeSearchError::Index(format!(
                "embedding dimensions changed from {} to {}",
                self.dimensions, other.dimensions
            )));
        }
        let start_value = start * other.dimensions;
        let end_value = end * other.dimensions;
        self.rows
            .extend_from_slice(&other.rows[start_value..end_value]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: embedding matrices flatten vectors and expose stable row slices.
    #[test]
    fn matrix_flattens_vectors_to_row_major_storage() {
        let matrix =
            EmbeddingMatrix::from_vectors(vec![vec![1.0, 2.0], vec![3.0, 4.0]]).expect("matrix");

        assert_eq!(matrix.dimensions(), 2);
        assert_eq!(matrix.row_count(), 2);
        assert_eq!(matrix.row(1), Some([3.0, 4.0].as_slice()));
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: inconsistent embedding dimensions are rejected before indexing.
    #[test]
    fn matrix_rejects_inconsistent_dimensions() {
        let matrix = EmbeddingMatrix::from_vectors(vec![vec![1.0], vec![2.0, 3.0]]);

        assert_eq!(matrix.is_err(), true);
    }

    #[test]
    #[ignore]
    fn bench_extend_rows_from_many_rows() {
        let dimensions = 64;
        let row_count = 512;
        let source = EmbeddingMatrix::new(
            dimensions,
            (0..row_count * dimensions)
                .map(|value| value as f32 / dimensions as f32)
                .collect(),
        )
        .expect("source matrix");
        let iterations = 20_000;
        let started = Instant::now();
        let mut total_rows = 0usize;

        for _ in 0..iterations {
            let mut target = EmbeddingMatrix::empty();
            black_box(&mut target)
                .extend_rows_from(black_box(&source), black_box(0), black_box(row_count))
                .expect("extend rows");
            total_rows += black_box(target.row_count());
        }

        let elapsed = started.elapsed();
        assert_eq!(total_rows, row_count * iterations);
        println!(
            "extend_rows_from_many_rows iterations={iterations} rows={row_count} dimensions={dimensions} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_row_many_accesses() {
        let dimensions = 64;
        let row_count = 4_096;
        let matrix = EmbeddingMatrix::new(
            dimensions,
            (0..row_count)
                .flat_map(|row| (0..dimensions).map(move |column| (row + column) as f32))
                .collect(),
        )
        .expect("matrix");
        let iterations = 20_000;
        let expected_sum = (row_count * (row_count - 1) / 2) * iterations;
        let started = Instant::now();
        let mut total = 0usize;

        for _ in 0..iterations {
            for row in 0..row_count {
                total += black_box(matrix.row(black_box(row)))
                    .expect("row")
                    .first()
                    .copied()
                    .unwrap_or_default() as usize;
            }
        }

        let elapsed = started.elapsed();
        assert_eq!(total, expected_sum);
        println!(
            "embedding_matrix_row_many_accesses iterations={iterations} rows={row_count} dimensions={dimensions} elapsed_ms={} per_row_ns={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / (iterations * row_count) as f64
        );
    }
}
