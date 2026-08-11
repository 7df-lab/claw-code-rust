//! Identifier-aware tokenization for sparse retrieval.
//!
//! Code search needs `parse_input`, `ParseInput`, and `parse input` to overlap.
//! These helpers split snake_case and camel/PascalCase identifiers, preserve the
//! raw lowercase identifier as a token, and enrich BM25 documents with file/path
//! terms so short symbol queries can still find the right module.

use crate::types::Chunk;

/// Builds the BM25 document text for a chunk.
///
/// The file stem is repeated to emulate Semble's path-aware sparse weighting:
/// chunk content remains dominant, but filename/module matches get enough signal
/// to help identifier-heavy queries.
pub fn enrich_for_bm25(chunk: &Chunk) -> String {
    let mut enriched = String::with_capacity(chunk.content.len() + 96);
    enriched.push_str(&chunk.content);
    if let Some(stem) = chunk.file_path.file_stem().and_then(|stem| stem.to_str()) {
        enriched.push(' ');
        enriched.push_str(stem);
        enriched.push(' ');
        enriched.push_str(stem);
    }
    let mut tail_parts = [None; 4];
    let mut part_count = 0usize;
    for part in chunk
        .file_path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
    {
        tail_parts[part_count % tail_parts.len()] = Some(part);
        part_count += 1;
    }
    let tail_start = part_count.saturating_sub(4);
    let tail_end = part_count.saturating_sub(1);
    for index in tail_start..tail_end {
        if let Some(part) = tail_parts[index % tail_parts.len()] {
            enriched.push(' ');
            enriched.push_str(part);
        }
    }
    enriched
}

/// Returns normalized query terms used by path reranking.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut terms = split_identifier_tokens(query);
    terms.retain(|token| token.len() > 2);
    terms
}

/// Heuristically detects exact-symbol style queries.
///
/// Symbol queries use a lower semantic alpha because matching names and paths is
/// often more important than natural-language meaning for these inputs.
pub fn is_symbol_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().count() != 1 {
        return false;
    }
    trimmed.contains("::")
        || trimmed.contains('_')
        || trimmed.contains('.')
        || trimmed.chars().any(char::is_uppercase)
        || trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ':' | '.'))
}

/// Splits arbitrary code/query text into stable lowercase identifier tokens.
///
/// The returned set includes both the full lowercase identifier and its
/// camel/snake pieces. Sorting and deduplication keep BM25 input deterministic.
pub fn split_identifier_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::with_capacity(16);
    for raw in input
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|part| !part.is_empty())
    {
        push_lowercase_token(&mut tokens, raw);
        let skip_single_part_duplicate = !raw.contains('_');
        for part in raw.split('_').filter(|part| !part.is_empty()) {
            let mut chars = part.char_indices();
            let Some((_, mut previous)) = chars.next() else {
                continue;
            };
            let mut chars = chars.peekable();
            let mut start = 0;
            let mut saw_boundary = false;
            while let Some((byte_idx, current)) = chars.next() {
                let next_is_lower = chars
                    .peek()
                    .map(|(_, next)| next.is_ascii_lowercase())
                    .unwrap_or(false);
                let boundary = (current.is_ascii_uppercase() && previous.is_ascii_lowercase())
                    || (current.is_ascii_uppercase()
                        && previous.is_ascii_uppercase()
                        && next_is_lower);
                if boundary {
                    push_lowercase_token(&mut tokens, &part[start..byte_idx]);
                    start = byte_idx;
                    saw_boundary = true;
                }
                previous = current;
            }

            // The full raw identifier has already been inserted. For common one-word
            // identifiers, pushing the same part again only allocates work for sort/dedup.
            if saw_boundary || !skip_single_part_duplicate {
                push_lowercase_token(&mut tokens, &part[start..]);
            }
        }
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

fn push_lowercase_token(tokens: &mut Vec<String>, token: &str) {
    if token.bytes().any(|byte| byte.is_ascii_uppercase()) {
        tokens.push(token.to_ascii_lowercase());
    } else {
        tokens.push(token.to_string());
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::path::PathBuf;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: sparse code search tokenization splits identifiers into queryable terms.
    #[test]
    fn split_identifier_tokens_handles_camel_and_snake() {
        let tokens = split_identifier_tokens("CodeSearchHandler parse_JSONInput HTTPServer");
        let expected = vec![
            "code".to_string(),
            "codesearchhandler".to_string(),
            "handler".to_string(),
            "http".to_string(),
            "httpserver".to_string(),
            "input".to_string(),
            "json".to_string(),
            "parse".to_string(),
            "parse_jsoninput".to_string(),
            "search".to_string(),
            "server".to_string(),
        ];
        assert_eq!(tokens, expected);
    }

    #[test]
    fn split_identifier_tokens_preserves_single_part_output() {
        let tokens = split_identifier_tokens("parse ParseInput parse_input");

        assert_eq!(
            tokens,
            vec![
                "input".to_string(),
                "parse".to_string(),
                "parse_input".to_string(),
                "parseinput".to_string(),
            ]
        );
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: BM25 enrichment includes repeated file-stem and path context.
    #[test]
    fn enrich_for_bm25_adds_path_terms() {
        let chunk = Chunk {
            content: "fn parse_input() {}".to_string(),
            file_path: PathBuf::from("crates/core/src/parser.rs"),
            start_line: 1,
            end_line: 1,
            language: "rust".to_string(),
        };
        assert_eq!(
            enrich_for_bm25(&chunk),
            "fn parse_input() {} parser parser crates core src"
        );
    }

    #[test]
    #[ignore]
    fn bench_enrich_for_bm25_rust_chunks() {
        let chunks = (0..512)
            .map(|idx| Chunk {
                content: format!(
                    "pub fn parse_input_{idx}() {{ let rendered_output = parser.parse(value_{idx}); }}"
                ),
                file_path: PathBuf::from(format!(
                    "crates/core/src/parser/module_{idx}/parse_input_{idx}.rs"
                )),
                start_line: idx + 1,
                end_line: idx + 1,
                language: "rust".to_string(),
            })
            .collect::<Vec<_>>();
        let iterations = 2_000;
        let expected_len = chunks
            .iter()
            .map(enrich_for_bm25)
            .map(|text| text.len())
            .sum::<usize>();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(&chunks)
                .iter()
                .map(enrich_for_bm25)
                .map(|text| text.len())
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "enrich_for_bm25_rust_chunks iterations={iterations} chunks=512 elapsed_ms={} per_chunk_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / (iterations * chunks.len()) as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_split_identifier_tokens_many_symbols() {
        let identifiers = (0..512)
            .map(|idx| format!("CodeSearchHandler{idx} parse_JSONInput_{idx} HTTPServerConfig"))
            .collect::<Vec<_>>();
        let iterations = 5_000;
        let expected_len = identifiers
            .iter()
            .map(|identifier| split_identifier_tokens(identifier).len())
            .sum::<usize>();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(&identifiers)
                .iter()
                .map(|identifier| split_identifier_tokens(identifier).len())
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "split_identifier_tokens_many_symbols iterations={iterations} identifiers=512 elapsed_ms={} per_identifier_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / (iterations * identifiers.len()) as f64
        );
    }
}
