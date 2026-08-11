//! Code chunking.
//!
//! Chunks are the unit that both BM25 and dense retrieval rank, so boundaries
//! need to be stable, source-locatable, and small enough for embeddings. The
//! chunker is designed to be language-extensible: supported programming and
//! config languages use tree-sitter AST boundaries first, while docs, data, and
//! parser failures fall back to line chunks with the same approximate size
//! target. This makes language coverage additive without changing the retrieval
//! contract that every chunk has source text and 1-indexed line bounds.

use std::ops::Range;
use std::path::Path;

use tree_sitter::Language;
use tree_sitter::Node;
use tree_sitter::Parser;

use crate::grammars::language_for_chunking;
use crate::types::Chunk;

const DESIRED_CHUNK_CHARS: usize = 1_500;
const RECURSION_DEPTH: usize = 500;

/// Splits a file into searchable chunks with 1-indexed line ranges.
///
/// Tree-sitter grammars provide semantic boundaries when a supported language
/// parses cleanly. Any missing grammar, parser setup error, syntax error, or
/// empty AST result returns to line chunking so incomplete code still remains
/// searchable.
pub fn chunk_file(relative_path: &Path, language: &str, content: &str) -> Vec<Chunk> {
    if let Some(parser_language) = language_for_chunking(language) {
        let ast_chunks = chunk_by_ast(relative_path, language, content, parser_language);
        if !ast_chunks.is_empty() {
            return ast_chunks;
        }
    }
    chunk_by_lines(relative_path, language, content)
}

/// Attempts AST chunking using a tree-sitter grammar selected by language label.
///
/// The returned chunks use the discovery language label rather than the grammar
/// crate name, preserving the public result language values.
fn chunk_by_ast(
    relative_path: &Path,
    language: &str,
    content: &str,
    parser_language: Language,
) -> Vec<Chunk> {
    let mut parser = Parser::new();
    if parser.set_language(&parser_language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let root = tree.root_node();
    if root.has_error() {
        return Vec::new();
    }

    let mut ranges = Vec::with_capacity(root.named_child_count());
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        collect_node_ranges(child, content, 0, &mut ranges);
    }
    merge_ranges(relative_path, language, content, ranges)
}

/// Recursively collects AST node byte ranges close to the target chunk size.
///
/// The recursion limit is a guard against pathological trees. Small named nodes
/// are still collected so compact definitions in languages such as config files
/// or shell scripts are not discarded before merge can add surrounding context.
fn collect_node_ranges(
    node: Node<'_>,
    content: &str,
    depth: usize,
    ranges: &mut Vec<Range<usize>>,
) {
    let range = node.byte_range();
    if range.end <= range.start {
        return;
    }
    let char_len = count_chars(&content[range.clone()]);
    if char_len <= DESIRED_CHUNK_CHARS || depth >= RECURSION_DEPTH || node.named_child_count() == 0
    {
        ranges.push(range);
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_node_ranges(child, content, depth + 1, ranges);
    }
}

/// Merges adjacent AST ranges without exceeding the target chunk size.
///
/// Tree-sitter often returns many small top-level nodes. Merging preserves source
/// order while giving the embedding model enough surrounding context.
fn merge_ranges(
    relative_path: &Path,
    language: &str,
    content: &str,
    mut ranges: Vec<Range<usize>>,
) -> Vec<Chunk> {
    if ranges.is_empty() {
        return Vec::new();
    }
    ranges.sort_by_key(|range| range.start);
    let line_starts = line_start_offsets(content);
    let mut merged = Vec::with_capacity(ranges.len());
    let mut current: Option<(Range<usize>, usize)> = None;

    for range in ranges {
        match current.take() {
            Some((active, active_chars)) => {
                let active_start = active.start;
                let active_end = active.end;
                let next_chars = if range.end >= active_end {
                    active_chars + count_chars(&content[active_end..range.end])
                } else {
                    count_chars(&content[active_start..range.end])
                };
                if next_chars <= DESIRED_CHUNK_CHARS {
                    current = Some((active_start..range.end, next_chars));
                } else {
                    push_byte_chunk(
                        &mut merged,
                        relative_path,
                        language,
                        content,
                        &line_starts,
                        active,
                    );
                    let range_chars = count_chars(&content[range.clone()]);
                    current = Some((range, range_chars));
                }
            }
            None => {
                let range_chars = count_chars(&content[range.clone()]);
                current = Some((range, range_chars));
            }
        }
    }
    if let Some((active, _)) = current {
        push_byte_chunk(
            &mut merged,
            relative_path,
            language,
            content,
            &line_starts,
            active,
        );
    }
    merged
}

/// Converts a byte range into a trimmed chunk while preserving original line
/// bounds.
fn push_byte_chunk(
    chunks: &mut Vec<Chunk>,
    relative_path: &Path,
    language: &str,
    content: &str,
    line_starts: &[usize],
    range: Range<usize>,
) {
    let text = content[range.clone()].trim().to_string();
    if text.is_empty() {
        return;
    }
    chunks.push(Chunk {
        content: text,
        file_path: relative_path.to_path_buf(),
        start_line: byte_to_line(line_starts, content.len(), range.start),
        end_line: byte_to_line(line_starts, content.len(), range.end),
        language: language.to_string(),
    });
}

fn line_start_offsets(content: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (idx, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn count_chars(text: &str) -> usize {
    if text.len() <= 256 && text.is_ascii() {
        text.len()
    } else {
        text.chars().count()
    }
}

/// Fallback chunker for unsupported languages and parser failures.
///
/// Line-based splitting never slices through UTF-8 bytes and keeps line spans
/// straightforward for `find_related`.
fn chunk_by_lines(relative_path: &Path, language: &str, content: &str) -> Vec<Chunk> {
    let mut chunks = Vec::with_capacity(content.len().div_ceil(DESIRED_CHUNK_CHARS));
    let mut current = String::with_capacity(content.len().min(DESIRED_CHUNK_CHARS));
    let mut current_chars = 0usize;
    let mut start_line: usize = 1;
    let mut current_line: usize = 1;

    for line in content.lines() {
        let line_chars = count_chars(line);
        let next_len = current_chars + line_chars + 1;
        if !current.is_empty() && next_len > DESIRED_CHUNK_CHARS {
            push_line_chunk(
                &mut chunks,
                relative_path,
                language,
                &mut current,
                start_line,
                current_line.saturating_sub(1),
            );
            current_chars = 0;
            start_line = current_line;
        }
        current.push_str(line);
        current.push('\n');
        current_chars += line_chars + 1;
        current_line += 1;
    }

    if !current.trim().is_empty() {
        push_line_chunk(
            &mut chunks,
            relative_path,
            language,
            &mut current,
            start_line,
            current_line.saturating_sub(1),
        );
    }
    chunks
}

fn push_line_chunk(
    chunks: &mut Vec<Chunk>,
    relative_path: &Path,
    language: &str,
    current: &mut String,
    start_line: usize,
    end_line: usize,
) {
    // The chunk must own its content, but copying would temporarily keep two
    // large buffers alive. Truncate the reusable line buffer and move it into
    // the chunk instead.
    let trimmed_len = current.trim_end().len();
    current.truncate(trimmed_len);
    chunks.push(Chunk {
        content: std::mem::take(current),
        file_path: relative_path.to_path_buf(),
        start_line,
        end_line,
        language: language.to_string(),
    });
}

/// Converts a byte offset to a 1-indexed line number.
fn byte_to_line(line_starts: &[usize], content_len: usize, byte_idx: usize) -> usize {
    let capped = byte_idx.min(content_len);
    line_starts.partition_point(|start| *start <= capped)
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::path::Path;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use crate::grammars::language_for_chunking;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: line fallback preserves 1-indexed source line boundaries.
    #[test]
    fn line_chunking_preserves_line_bounds() {
        let chunks = chunk_file(Path::new("README.md"), "markdown", "one\ntwo\nthree\n");
        let expected = vec![Chunk {
            content: "one\ntwo\nthree".to_string(),
            file_path: Path::new("README.md").to_path_buf(),
            start_line: 1,
            end_line: 3,
            language: "markdown".to_string(),
        }];
        assert_eq!(chunks, expected);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: supported programming and config language labels parse through AST chunking.
    #[test]
    fn ast_chunking_parses_supported_code_and_config_languages() {
        let cases = [
            (
                "rust",
                "src/lib.rs",
                "fn parse_input(value: &str) -> &str { value }\n",
                "parse_input",
            ),
            (
                "python",
                "parser.py",
                "def parse_input(value):\n    return value\n",
                "parse_input",
            ),
            (
                "javascript",
                "parser.js",
                "function parseInput(value) { return value; }\n",
                "parseInput",
            ),
            (
                "javascriptreact",
                "App.jsx",
                "const App = () => <div>parser</div>;\n",
                "parser",
            ),
            (
                "typescript",
                "parser.ts",
                "type Value = string;\nfunction parseInput(value: Value): Value { return value; }\n",
                "parseInput",
            ),
            (
                "typescriptreact",
                "App.tsx",
                "type Props = { value: string };\nexport function App(props: Props) { return <div>{props.value}</div>; }\n",
                "props",
            ),
            (
                "go",
                "parser.go",
                "package main\nfunc parseInput(value string) string { return value }\n",
                "parseInput",
            ),
            (
                "java",
                "Parser.java",
                "class Parser { String parseInput(String value) { return value; } }\n",
                "parseInput",
            ),
            (
                "kotlin",
                "Parser.kt",
                "fun parseInput(value: String): String { return value }\n",
                "parseInput",
            ),
            (
                "c",
                "parser.c",
                "int parse_input(int value) { return value; }\n",
                "parse_input",
            ),
            (
                "cpp",
                "parser.cpp",
                "int parse_input(int value) { return value; }\n",
                "parse_input",
            ),
            (
                "csharp",
                "Parser.cs",
                "class Parser { string ParseInput(string value) { return value; } }\n",
                "ParseInput",
            ),
            (
                "ruby",
                "parser.rb",
                "def parse_input(value)\n  value\nend\n",
                "parse_input",
            ),
            (
                "php",
                "parser.php",
                "<?php\nfunction parse_input($value) { return $value; }\n",
                "parse_input",
            ),
            (
                "swift",
                "Parser.swift",
                "func parseInput(_ value: String) -> String { return value }\n",
                "parseInput",
            ),
            (
                "scala",
                "Parser.scala",
                "object Parser { def parseInput(value: String): String = value }\n",
                "parseInput",
            ),
            (
                "shell",
                "parser.sh",
                "parse_input() {\n  echo \"$1\"\n}\n",
                "parse_input",
            ),
            (
                "powershell",
                "parser.ps1",
                "$Parser = \"parser\"\nWrite-Output $Parser\n",
                "Parser",
            ),
            (
                "lua",
                "parser.lua",
                "function parse_input(value)\n  return value\nend\n",
                "parse_input",
            ),
            (
                "r",
                "parser.r",
                "parse_input <- function(value) {\n  value\n}\n",
                "parse_input",
            ),
            (
                "sql",
                "schema.sql",
                "CREATE TABLE parser (id INTEGER);\nSELECT id FROM parser;\n",
                "parser",
            ),
            (
                "toml",
                "Cargo.toml",
                "[package]\nname = \"parser\"\n",
                "parser",
            ),
            (
                "yaml",
                "config.yaml",
                "package:\n  name: parser\n",
                "parser",
            ),
            (
                "json",
                "config.json",
                "{\"package\":{\"name\":\"parser\"}}\n",
                "parser",
            ),
            (
                "config",
                "settings.ini",
                "[package]\nname=parser\n",
                "parser",
            ),
            (
                "xml",
                "config.xml",
                "<package><name>parser</name></package>\n",
                "parser",
            ),
            (
                "dockerfile",
                "Dockerfile",
                "FROM alpine\nRUN echo parser\n",
                "parser",
            ),
        ];

        let parsed = cases
            .iter()
            .map(|(language, path, source, needle)| {
                let parser_language = language_for_chunking(language).expect("grammar");
                let chunks = chunk_by_ast(Path::new(path), language, source, parser_language);
                (
                    *language,
                    !chunks.is_empty(),
                    chunks.iter().all(|chunk| chunk.language == *language),
                    chunks.iter().any(|chunk| chunk.content.contains(needle)),
                )
            })
            .collect::<Vec<_>>();
        let expected = cases
            .iter()
            .map(|(language, _, _, _)| (*language, true, true, true))
            .collect::<Vec<_>>();

        assert_eq!(parsed, expected);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: small named AST nodes are retained instead of being dropped before merge.
    #[test]
    fn ast_chunking_keeps_small_named_nodes() {
        let chunks = chunk_by_ast(
            Path::new("src/lib.rs"),
            "rust",
            "fn a() {}\n",
            language_for_chunking("rust").expect("grammar"),
        );
        let expected = vec![Chunk {
            content: "fn a() {}".to_string(),
            file_path: Path::new("src/lib.rs").to_path_buf(),
            start_line: 1,
            end_line: 1,
            language: "rust".to_string(),
        }];

        assert_eq!(chunks, expected);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: docs and data labels keep line-based chunking.
    #[test]
    fn docs_and_data_use_line_chunking() {
        let outputs = ["markdown", "rst", "text", "data"]
            .iter()
            .map(|language| chunk_file(Path::new("note.txt"), language, "one\ntwo\nthree\n"))
            .collect::<Vec<_>>();
        let expected = ["markdown", "rst", "text", "data"]
            .into_iter()
            .map(|language| {
                vec![Chunk {
                    content: "one\ntwo\nthree".to_string(),
                    file_path: Path::new("note.txt").to_path_buf(),
                    start_line: 1,
                    end_line: 3,
                    language: language.to_string(),
                }]
            })
            .collect::<Vec<_>>();

        assert_eq!(outputs, expected);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: parser errors for supported languages fall back to line chunks.
    #[test]
    fn parser_errors_fall_back_to_line_chunking() {
        let chunks = chunk_file(Path::new("src/lib.rs"), "rust", "fn broken(\n");
        let expected = vec![Chunk {
            content: "fn broken(".to_string(),
            file_path: Path::new("src/lib.rs").to_path_buf(),
            start_line: 1,
            end_line: 1,
            language: "rust".to_string(),
        }];

        assert_eq!(chunks, expected);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: Rust code uses AST chunking when parser boundaries are available.
    #[test]
    fn rust_chunking_uses_ast_boundaries() {
        let source = r#"fn first() {
    println!("first");
}

fn second() {
    println!("second");
}
"#;
        let chunks = chunk_file(Path::new("src/lib.rs"), "rust", source);

        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content.contains("fn first"));
        assert!(chunks[0].content.contains("fn second"));
        assert_eq!(chunks[0].start_line, 1);
    }

    #[test]
    fn cached_byte_to_line_matches_original_offset_semantics() {
        let source = "a\nbc\n";
        let line_starts = line_start_offsets(source);
        let lines = [0, 1, 2, 3, 4, 5, 99]
            .into_iter()
            .map(|byte_idx| byte_to_line(&line_starts, source.len(), byte_idx))
            .collect::<Vec<_>>();

        assert_eq!(lines, vec![1, 1, 2, 2, 2, 3, 3]);
    }

    #[test]
    fn count_chars_preserves_unicode_semantics() {
        let counts = [
            count_chars("ascii text"),
            count_chars("解析 input"),
            count_chars(&"x".repeat(300)),
        ];

        assert_eq!(counts, [10, 8, 300]);
    }

    #[test]
    #[ignore]
    fn bench_line_chunking_many_lines() {
        let line_count = 10_000;
        let source = (0..line_count)
            .map(|line| {
                format!(
                    "line {line:05} contains enough words to resemble a documentation paragraph"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let expected_chunks = chunk_file(Path::new("README.md"), "markdown", &source);
        let expected_chunk_count = expected_chunks.len();
        let expected_content_bytes = expected_chunks
            .iter()
            .map(|chunk| chunk.content.len())
            .sum::<usize>();
        let iterations = 200;
        let started = Instant::now();
        let mut total_chunks = 0usize;
        let mut total_content_bytes = 0usize;

        for _ in 0..iterations {
            let chunks = chunk_file(
                black_box(Path::new("README.md")),
                "markdown",
                black_box(&source),
            );
            total_chunks += black_box(chunks.len());
            total_content_bytes += chunks
                .iter()
                .map(|chunk| black_box(chunk.content.len()))
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_chunks, expected_chunk_count * iterations);
        assert_eq!(total_content_bytes, expected_content_bytes * iterations);
        println!(
            "line_chunking_many_lines iterations={iterations} lines={line_count} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_merge_ranges_many_small_ranges() {
        let range_count = 600;
        let line = format!("{};", "x".repeat(1_398));
        let mut source = String::new();
        let mut ranges = Vec::new();
        for _ in 0..range_count {
            let start = source.len();
            source.push_str(&line);
            let end = source.len();
            source.push('\n');
            ranges.push(start..end);
        }
        let expected_chunks = merge_ranges(
            Path::new("src/generated.rs"),
            "rust",
            &source,
            ranges.clone(),
        );
        let expected_content_bytes = expected_chunks
            .iter()
            .map(|chunk| chunk.content.len())
            .sum::<usize>();
        let iterations = 50;
        let started = Instant::now();
        let mut total_chunks = 0usize;
        let mut total_content_bytes = 0usize;

        for _ in 0..iterations {
            let chunks = merge_ranges(
                black_box(Path::new("src/generated.rs")),
                "rust",
                black_box(&source),
                black_box(ranges.clone()),
            );
            total_chunks += black_box(chunks.len());
            total_content_bytes += chunks
                .iter()
                .map(|chunk| black_box(chunk.content.len()))
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_chunks, expected_chunks.len() * iterations);
        assert_eq!(total_content_bytes, expected_content_bytes * iterations);
        println!(
            "merge_ranges_many_small_ranges iterations={iterations} ranges={range_count} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_merge_ranges_many_tiny_ranges() {
        let range_count = 4_000;
        let line = "let value = value + 1;";
        let mut source = String::new();
        let mut ranges = Vec::new();
        for _ in 0..range_count {
            let start = source.len();
            source.push_str(line);
            let end = source.len();
            source.push('\n');
            ranges.push(start..end);
        }
        let expected_chunks = merge_ranges(
            Path::new("src/generated.rs"),
            "rust",
            &source,
            ranges.clone(),
        );
        let expected_content_bytes = expected_chunks
            .iter()
            .map(|chunk| chunk.content.len())
            .sum::<usize>();
        let iterations = 200;
        let started = Instant::now();
        let mut total_chunks = 0usize;
        let mut total_content_bytes = 0usize;

        for _ in 0..iterations {
            let chunks = merge_ranges(
                black_box(Path::new("src/generated.rs")),
                "rust",
                black_box(&source),
                black_box(ranges.clone()),
            );
            total_chunks += black_box(chunks.len());
            total_content_bytes += chunks
                .iter()
                .map(|chunk| black_box(chunk.content.len()))
                .sum::<usize>();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_chunks, expected_chunks.len() * iterations);
        assert_eq!(total_content_bytes, expected_content_bytes * iterations);
        println!(
            "merge_ranges_many_tiny_ranges iterations={iterations} ranges={range_count} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }
}
