//! Public request, response, and error types for the code-search crate.
//!
//! These types mirror the built-in tool schema: callers choose `search` or
//! `find_related`, a content filter, a bounded result count, and optional path or
//! language filters. The JSON-facing response shape is kept stable while the
//! internals can change cache layout, refresh strategy, or semantic backend.

use std::cmp::Ordering;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const DEFAULT_TOP_K: usize = 5;
pub const MAX_TOP_K: usize = 20;

/// High-level content partition selected by the tool input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentFilter {
    #[default]
    Code,
    Docs,
    Config,
    All,
}

/// File classification produced during discovery.
///
/// Data files are recognized separately so `all` can still avoid indexing CSV or
/// similar table data that is usually noisy for code retrieval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Code,
    Docs,
    Config,
    Data,
}

impl ContentKind {
    /// Returns whether this content kind belongs in a requested index.
    pub fn is_selected_by(self, filter: ContentFilter) -> bool {
        match filter {
            ContentFilter::Code => self == Self::Code,
            ContentFilter::Docs => self == Self::Docs,
            ContentFilter::Config => self == Self::Config,
            ContentFilter::All => self != Self::Data,
        }
    }
}

/// Supported code-search operation names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeSearchOperation {
    Search,
    FindRelated,
}

/// Searchable source fragment with stable workspace-relative location.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Chunk {
    pub content: String,
    pub file_path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub language: String,
}

impl Chunk {
    /// Formats the chunk location for deterministic tie-breaking and display.
    pub fn location(&self) -> String {
        let path = self.file_path.to_string_lossy();
        let mut location = String::with_capacity(path.len() + 42);
        if path.contains('\\') {
            for ch in path.chars() {
                location.push(if ch == '\\' { '/' } else { ch });
            }
        } else {
            location.push_str(&path);
        }
        let _ = write!(location, ":{}-{}", self.start_line, self.end_line);
        location
    }

    /// Compares two chunk locations with the same ordering as `location()`.
    pub fn cmp_location(&self, other: &Self) -> Ordering {
        let left_path = self.file_path.to_string_lossy();
        let right_path = other.file_path.to_string_lossy();
        cmp_normalized_path_with_suffix(&left_path, &right_path, ':')
            .then_with(|| cmp_decimal_text_with_suffix(self.start_line, other.start_line, b"-"))
            .then_with(|| cmp_decimal_text_with_suffix(self.end_line, other.end_line, b""))
    }
}

fn cmp_normalized_path_with_suffix(left: &str, right: &str, suffix: char) -> Ordering {
    let mut left_chars = left
        .chars()
        .map(|ch| if ch == '\\' { '/' } else { ch })
        .chain(std::iter::once(suffix));
    let mut right_chars = right
        .chars()
        .map(|ch| if ch == '\\' { '/' } else { ch })
        .chain(std::iter::once(suffix));
    loop {
        match (left_chars.next(), right_chars.next()) {
            (Some(left_ch), Some(right_ch)) => match left_ch.cmp(&right_ch) {
                Ordering::Equal => {}
                ordering => return ordering,
            },
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

fn cmp_decimal_text_with_suffix(left: usize, right: usize, suffix: &[u8]) -> Ordering {
    let write_decimal = |mut value: usize, buffer: &mut [u8; 32]| {
        if value == 0 {
            buffer[0] = b'0';
            return 1;
        }

        let mut len = 0;
        let mut remaining = value;
        while remaining > 0 {
            len += 1;
            remaining /= 10;
        }

        let mut index = len;
        while value > 0 {
            index -= 1;
            buffer[index] = b'0' + (value % 10) as u8;
            value /= 10;
        }
        len
    };

    let mut left_buffer = [0; 32];
    let mut right_buffer = [0; 32];
    let left_digits = write_decimal(left, &mut left_buffer);
    let right_digits = write_decimal(right, &mut right_buffer);
    let left_len = left_digits + suffix.len();
    let right_len = right_digits + suffix.len();
    left_buffer[left_digits..left_len].copy_from_slice(suffix);
    right_buffer[right_digits..right_len].copy_from_slice(suffix);
    left_buffer[..left_len].cmp(&right_buffer[..right_len])
}

/// Ranked result returned by both search operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub score: f32,
    pub chunk: Chunk,
}

/// Index size summary included in every response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexStats {
    pub indexed_files: usize,
    pub total_chunks: usize,
}

/// Stable JSON output shape for the built-in tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchOutput {
    pub operation: CodeSearchOperation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub root: PathBuf,
    pub content: ContentFilter,
    pub results: Vec<SearchResult>,
    pub index_stats: IndexStats,
}

/// Optional path and language filters applied before final ranking.
///
/// Paths are normalized to `/` separators and matched as exact files or directory
/// prefixes. Languages are lowercased classification names such as `rust`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilters {
    pub paths: Vec<String>,
    pub languages: Vec<String>,
}

impl SearchFilters {
    /// Creates an unfiltered search scope.
    pub fn empty() -> Self {
        Self {
            paths: Vec::new(),
            languages: Vec::new(),
        }
    }

    /// Returns true when no path or language restriction is active.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.languages.is_empty()
    }

    /// Normalizes tool input filters into the form used by search.
    pub fn normalized(paths: Vec<String>, languages: Vec<String>) -> Self {
        let mut normalized_paths = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(path) = normalize_filter_path(path) {
                normalized_paths.push(path);
            }
        }

        let mut normalized_languages = Vec::with_capacity(languages.len());
        for language in languages {
            if let Some(language) = normalize_filter_language(language) {
                normalized_languages.push(language);
            }
        }

        Self {
            paths: normalized_paths,
            languages: normalized_languages,
        }
    }

    /// Checks whether a chunk survives both path and language filters.
    pub fn allows(&self, chunk: &Chunk) -> bool {
        if !self.languages.is_empty()
            && !self
                .languages
                .iter()
                .any(|language| language.eq_ignore_ascii_case(&chunk.language))
        {
            return false;
        }
        if self.paths.is_empty() {
            return true;
        }

        let matches_filter = |path: &str| {
            self.paths.iter().any(|filter| {
                path == filter
                    || path
                        .strip_prefix(filter)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
        };
        let path = chunk.file_path.to_string_lossy();
        if path
            .bytes()
            .all(|byte| byte.is_ascii() && byte != b'\\' && !byte.is_ascii_uppercase())
        {
            matches_filter(&path)
        } else {
            let mut normalized = String::with_capacity(path.len());
            for ch in path.chars() {
                if ch == '\\' {
                    normalized.push('/');
                } else {
                    normalized.extend(ch.to_lowercase());
                }
            }
            matches_filter(&normalized)
        }
    }
}

fn normalize_filter_path(mut path: String) -> Option<String> {
    if path.contains('\\') {
        path = path.replace('\\', "/");
    }
    trim_slashes_in_place(&mut path);

    let normalized = {
        let mut parts = Vec::<&str>::new();
        for component in path.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    if parts
                        .last()
                        .is_some_and(|part| part != &".." && !part.ends_with(':'))
                    {
                        parts.pop();
                    } else {
                        parts.push(component);
                    }
                }
                _ => parts.push(component),
            }
        }
        parts.join("/")
    };
    path = lowercase_owned(normalized);
    (!path.is_empty()).then_some(path)
}

fn normalize_filter_language(mut language: String) -> Option<String> {
    trim_whitespace_in_place(&mut language);
    language = lowercase_owned(language);
    (!language.is_empty()).then_some(language)
}

fn trim_slashes_in_place(value: &mut String) {
    let end = value.trim_end_matches('/').len();
    value.truncate(end);
    let start = value.len() - value.trim_start_matches('/').len();
    if start > 0 {
        value.drain(..start);
    }
}

pub(crate) fn trim_whitespace_in_place(value: &mut String) {
    let end = value.trim_end().len();
    value.truncate(end);
    let start = value.len() - value.trim_start().len();
    if start > 0 {
        value.drain(..start);
    }
}

fn lowercase_owned(mut value: String) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
    {
        value
    } else if value.is_ascii() {
        // Filter strings are already owned. ASCII path/language filters can be
        // normalized in place; Unicode still needs `to_lowercase` because it can
        // expand into multiple codepoints.
        value.make_ascii_lowercase();
        value
    } else {
        value.to_lowercase()
    }
}

/// Internal request for the `search` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub root: PathBuf,
    pub query: String,
    pub content: ContentFilter,
    pub top_k: usize,
    pub filters: SearchFilters,
}

/// Internal request for the `find_related` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedRequest {
    pub root: PathBuf,
    pub file_path: PathBuf,
    pub line: usize,
    pub content: ContentFilter,
    pub top_k: usize,
    pub filters: SearchFilters,
}

/// Error categories surfaced by the code-search service.
///
/// Model failures are kept separate from index and input failures so the tool
/// runtime can report missing local model cache as a recoverable condition.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CodeSearchError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),
    #[error("index error: {0}")]
    Index(String),
    #[error("I/O error: {0}")]
    Io(String),
}

impl From<std::io::Error> for CodeSearchError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<anyhow::Error> for CodeSearchError {
    fn from(error: anyhow::Error) -> Self {
        Self::Index(error.to_string())
    }
}

/// Validates and returns the requested result limit.
///
/// The max bound protects retrieval from accidentally building huge candidate
/// sets through the public tool schema.
pub fn validate_top_k(top_k: usize) -> Result<usize, CodeSearchError> {
    if top_k == 0 {
        return Err(CodeSearchError::InvalidInput(
            "`top_k` must be greater than zero".to_string(),
        ));
    }
    if top_k > MAX_TOP_K {
        return Err(CodeSearchError::InvalidInput(format!(
            "`top_k` must be less than or equal to {MAX_TOP_K}"
        )));
    }
    Ok(top_k)
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn chunk_location_uses_slash_separators() {
        let chunk = Chunk {
            content: "content".to_string(),
            file_path: PathBuf::from(r"src\lib.rs"),
            start_line: 3,
            end_line: 7,
            language: "rust".to_string(),
        };

        assert_eq!(chunk.location(), "src/lib.rs:3-7");
    }

    #[test]
    fn chunk_location_comparator_matches_location_string_ordering() {
        let chunks = [
            Chunk {
                content: "a".to_string(),
                file_path: PathBuf::from("src"),
                start_line: 1,
                end_line: 2,
                language: "rust".to_string(),
            },
            Chunk {
                content: "b".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 10,
                end_line: 2,
                language: "rust".to_string(),
            },
            Chunk {
                content: "c".to_string(),
                file_path: PathBuf::from(r"src\lib.rs"),
                start_line: 2,
                end_line: 10,
                language: "rust".to_string(),
            },
            Chunk {
                content: "d".to_string(),
                file_path: PathBuf::from(r"src\lib.rs"),
                start_line: 2,
                end_line: 9,
                language: "rust".to_string(),
            },
            Chunk {
                content: "e".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 1,
                end_line: 10,
                language: "rust".to_string(),
            },
            Chunk {
                content: "f".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 1,
                end_line: 2,
                language: "rust".to_string(),
            },
            Chunk {
                content: "g".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 100,
                end_line: 1,
                language: "rust".to_string(),
            },
            Chunk {
                content: "h".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 11,
                end_line: 1,
                language: "rust".to_string(),
            },
        ];
        let orderings = chunks
            .iter()
            .flat_map(|left| {
                chunks.iter().map(move |right| {
                    (
                        left.cmp_location(right),
                        left.location().cmp(&right.location()),
                    )
                })
            })
            .collect::<Vec<_>>();
        let expected = orderings
            .iter()
            .map(|(_, string_ordering)| (*string_ordering, *string_ordering))
            .collect::<Vec<_>>();

        assert_eq!(orderings, expected);
    }

    #[test]
    fn search_filters_allow_exact_directory_and_language_matches() {
        let filters = SearchFilters::normalized(
            vec!["src/server".to_string(), "README.md".to_string()],
            vec!["Rust".to_string()],
        );
        let chunks = [
            Chunk {
                content: "server".to_string(),
                file_path: PathBuf::from("src/server/bootstrap.rs"),
                start_line: 1,
                end_line: 4,
                language: "rust".to_string(),
            },
            Chunk {
                content: "readme".to_string(),
                file_path: PathBuf::from("README.md"),
                start_line: 1,
                end_line: 1,
                language: "markdown".to_string(),
            },
            Chunk {
                content: "client".to_string(),
                file_path: PathBuf::from("src/client/bootstrap.rs"),
                start_line: 1,
                end_line: 4,
                language: "rust".to_string(),
            },
            Chunk {
                content: "server uppercase".to_string(),
                file_path: PathBuf::from(r"SRC\SERVER\BOOTSTRAP.RS"),
                start_line: 1,
                end_line: 4,
                language: "Rust".to_string(),
            },
        ];
        let allowed = chunks
            .iter()
            .map(|chunk| filters.allows(chunk))
            .collect::<Vec<_>>();

        assert_eq!(allowed, vec![true, false, false, true]);
    }

    #[test]
    fn search_filters_normalize_owned_inputs() {
        let filters = SearchFilters::normalized(
            vec![
                "/SRC\\Server/".to_string(),
                "///".to_string(),
                "readme.md".to_string(),
            ],
            vec![
                " Rust ".to_string(),
                " ".to_string(),
                "markdown".to_string(),
            ],
        );

        assert_eq!(
            filters,
            SearchFilters {
                paths: vec!["src/server".to_string(), "readme.md".to_string()],
                languages: vec!["rust".to_string(), "markdown".to_string()],
            }
        );
    }

    #[test]
    fn search_filters_normalize_relative_dot_components() {
        let filters = SearchFilters::normalized(
            vec![
                "./src/../src/server/./".to_string(),
                "crates\\core\\src\\..\\tests".to_string(),
            ],
            Vec::new(),
        );
        let chunks = [
            Chunk {
                content: "server".to_string(),
                file_path: PathBuf::from("src/server/bootstrap.rs"),
                start_line: 1,
                end_line: 4,
                language: "rust".to_string(),
            },
            Chunk {
                content: "tests".to_string(),
                file_path: PathBuf::from("crates/core/tests/search.rs"),
                start_line: 1,
                end_line: 4,
                language: "rust".to_string(),
            },
            Chunk {
                content: "client".to_string(),
                file_path: PathBuf::from("src/client/bootstrap.rs"),
                start_line: 1,
                end_line: 4,
                language: "rust".to_string(),
            },
        ];

        let allowed = chunks
            .iter()
            .map(|chunk| filters.allows(chunk))
            .collect::<Vec<_>>();

        assert_eq!(
            filters,
            SearchFilters {
                paths: vec!["src/server".to_string(), "crates/core/tests".to_string()],
                languages: Vec::new(),
            }
        );
        assert_eq!(allowed, vec![true, true, false]);
    }

    #[test]
    #[ignore]
    fn bench_search_filters_allows_filtered_candidates() {
        let filters = SearchFilters::normalized(
            vec![
                "src/server".to_string(),
                "crates/core/src/context".to_string(),
                "README.md".to_string(),
            ],
            vec!["rust".to_string(), "markdown".to_string()],
        );
        let chunks = (0..10_000)
            .map(|idx| {
                let file_path = match idx % 4 {
                    0 => PathBuf::from(format!("src/server/module_{idx}.rs")),
                    1 => PathBuf::from(format!("crates/core/src/context/file_{idx}.rs")),
                    2 => PathBuf::from(format!("README_{idx}.md")),
                    _ => PathBuf::from(format!("tests/generated_{idx}.rs")),
                };
                let language = if idx % 5 == 0 { "markdown" } else { "rust" };
                Chunk {
                    content: format!("chunk {idx}"),
                    file_path,
                    start_line: idx,
                    end_line: idx + 1,
                    language: language.to_string(),
                }
            })
            .collect::<Vec<_>>();
        let expected_allowed = chunks.iter().filter(|chunk| filters.allows(chunk)).count();
        let iterations = 1_000;
        let started = Instant::now();
        let mut total_allowed = 0usize;

        for _ in 0..iterations {
            total_allowed += chunks
                .iter()
                .filter(|chunk| black_box(&filters).allows(black_box(chunk)))
                .count();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_allowed, expected_allowed * iterations);
        println!(
            "search_filters_allows_filtered_candidates iterations={iterations} chunks={} elapsed_ms={} per_chunk_ns={:.2}",
            chunks.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / (iterations * chunks.len()) as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_search_filters_allows_unfiltered_candidates() {
        let filters = SearchFilters::empty();
        let chunks = (0..10_000)
            .map(|idx| Chunk {
                content: format!("chunk {idx}"),
                file_path: PathBuf::from(format!("src/generated/file_{idx}.rs")),
                start_line: idx,
                end_line: idx + 1,
                language: "rust".to_string(),
            })
            .collect::<Vec<_>>();
        let expected_allowed = chunks.iter().filter(|chunk| filters.allows(chunk)).count();
        let iterations = 10_000;
        let started = Instant::now();
        let mut total_allowed = 0usize;

        for _ in 0..iterations {
            total_allowed += chunks
                .iter()
                .filter(|chunk| black_box(&filters).allows(black_box(chunk)))
                .count();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_allowed, expected_allowed * iterations);
        println!(
            "search_filters_allows_unfiltered_candidates iterations={iterations} chunks={} elapsed_ms={} per_chunk_ns={:.2}",
            chunks.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / (iterations * chunks.len()) as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_chunk_location_unix_path() {
        let chunk = Chunk {
            content: "content".to_string(),
            file_path: Path::new("crates/code-search-mcp/src/index.rs").to_path_buf(),
            start_line: 123,
            end_line: 145,
            language: "rust".to_string(),
        };
        let iterations = 1_000_000;
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(chunk.location()).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(
            total_len,
            "crates/code-search-mcp/src/index.rs:123-145".len() * iterations
        );
        println!(
            "chunk_location_unix_path iterations={iterations} elapsed_ms={} per_call_ns={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_chunk_cmp_location_same_path_lines() {
        let left = Chunk {
            content: "left".to_string(),
            file_path: Path::new("crates/code-search-mcp/src/index.rs").to_path_buf(),
            start_line: 1_234_567,
            end_line: 1_234_589,
            language: "rust".to_string(),
        };
        let right = Chunk {
            content: "right".to_string(),
            file_path: Path::new("crates/code-search-mcp/src/index.rs").to_path_buf(),
            start_line: 1_234_568,
            end_line: 1_234_590,
            language: "rust".to_string(),
        };
        let iterations = 1_000_000;
        let started = Instant::now();
        let mut greater_count = 0usize;

        for _ in 0..iterations {
            if black_box(&left).cmp_location(black_box(&right)).is_gt() {
                greater_count += 1;
            }
        }

        let elapsed = started.elapsed();
        assert_eq!(
            left.cmp_location(&right),
            left.location().cmp(&right.location())
        );
        assert_eq!(greater_count, 0);
        println!(
            "chunk_cmp_location_same_path_lines iterations={iterations} elapsed_ms={} per_call_ns={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / iterations as f64
        );
    }
}
