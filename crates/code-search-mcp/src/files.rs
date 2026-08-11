//! Workspace file discovery and text loading.
//!
//! The walker mirrors Semble's code-search assumptions: obey gitignore and
//! `.sembleignore`, skip symlinks and heavy generated directories, cap files at
//! one megabyte, and classify only languages/content types the retriever knows
//! how to index. Discovery produces a manifest that incremental refresh can use
//! as a cheap reuse key before any file content is read.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use crate::types::{CodeSearchError, ContentFilter, ContentKind};

pub const MAX_FILE_BYTES: u64 = 1_000_000;

/// Cheap file identity used for incremental cache reuse.
///
/// Path, size, and nanosecond mtime are intentionally the fast path. Content
/// hashes are recorded after reading, but avoiding reads for unchanged manifests
/// is what makes warm refreshes scale.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileManifestEntry {
    pub path: PathBuf,
    pub size: u64,
    pub modified_unix_nanos: u128,
}

/// Discovered file that is eligible for indexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub absolute_path: PathBuf,
    pub language: String,
    pub content_kind: ContentKind,
    pub manifest: FileManifestEntry,
}

/// Discovers indexable files under a root for a requested content filter.
///
/// Invalid directory entries, unreadable metadata, unknown languages, symlinks,
/// and oversized files are skipped rather than failing the whole search. Source
/// repositories often change while indexing, so the walker has to tolerate races.
pub fn discover_files(
    root: &Path,
    content_filter: ContentFilter,
) -> Result<Vec<FileEntry>, CodeSearchError> {
    let mut builder = WalkBuilder::new(root);
    let include_hidden = false;
    let follow_links = false;
    let require_git_context = true;
    builder
        .hidden(include_hidden)
        .follow_links(follow_links)
        .require_git(require_git_context)
        .add_custom_ignore_filename(".sembleignore")
        .filter_entry(|entry| !is_default_ignored(entry.path()));

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() || !file_type.is_file() {
            continue;
        }
        let absolute_path = entry.path().to_path_buf();
        let Some((language, content_kind)) = classify_path(&absolute_path) else {
            continue;
        };
        if !content_kind.is_selected_by(content_filter) {
            continue;
        }
        let Ok(metadata) = absolute_path.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let relative_path = absolute_path
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| absolute_path.clone());
        let modified_unix_nanos = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        files.push(FileEntry {
            absolute_path,
            language: language.to_string(),
            content_kind,
            manifest: FileManifestEntry {
                path: relative_path,
                size: metadata.len(),
                modified_unix_nanos,
            },
        });
    }
    files.sort_by(|left, right| left.manifest.path.cmp(&right.manifest.path));
    Ok(files)
}

/// Reads a file as lossy UTF-8 text for chunking.
///
/// Tiny whitespace-only files return `None` so refresh can create a reusable
/// zero-chunk record without embedding meaningless content.
pub fn read_indexable_text(path: &Path) -> Result<Option<String>, CodeSearchError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 128 && bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(error) => Ok(Some(String::from_utf8_lossy(error.as_bytes()).into_owned())),
    }
}

/// Classifies a path into retrieval language and content kind.
///
/// Unknown extensions are ignored to keep the index focused on text/code files
/// and to avoid pulling binary formats into lossy UTF-8 chunking.
pub fn classify_path(path: &Path) -> Option<(&'static str, ContentKind)> {
    let file_name = path.file_name()?.to_str()?;
    if file_name.eq_ignore_ascii_case("dockerfile") {
        return Some(("dockerfile", ContentKind::Config));
    }
    let extension = path.extension()?.to_str()?;
    let normalized_extension;
    let mut stack_extension = [0u8; 16];
    let extension = if extension
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
    {
        extension
    } else if extension.is_ascii() && extension.len() <= stack_extension.len() {
        for (target, byte) in stack_extension.iter_mut().zip(extension.bytes()) {
            *target = byte.to_ascii_lowercase();
        }
        std::str::from_utf8(&stack_extension[..extension.len()]).ok()?
    } else {
        normalized_extension = extension.to_lowercase();
        &normalized_extension
    };
    let language = language_for_extension(extension)?;
    let content_kind = content_kind_for_extension(extension);
    Some((language, content_kind))
}

fn language_for_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "rs" => Some("rust"),
        "py" | "pyw" => Some("python"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => Some("cpp"),
        "cs" => Some("csharp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "swift" => Some("swift"),
        "scala" => Some("scala"),
        "sh" | "bash" | "zsh" | "fish" => Some("shell"),
        "ps1" => Some("powershell"),
        "lua" => Some("lua"),
        "r" => Some("r"),
        "sql" => Some("sql"),
        "md" | "mdx" => Some("markdown"),
        "rst" => Some("rst"),
        "txt" => Some("text"),
        "toml" => Some("toml"),
        "yaml" | "yml" => Some("yaml"),
        "json" | "jsonc" | "json5" => Some("json"),
        "ini" | "cfg" | "conf" | "env" => Some("config"),
        "xml" => Some("xml"),
        "csv" | "tsv" | "psv" => Some("data"),
        _ => None,
    }
}

fn content_kind_for_extension(extension: &str) -> ContentKind {
    match extension {
        "md" | "mdx" | "rst" | "txt" => ContentKind::Docs,
        "toml" | "yaml" | "yml" | "json" | "jsonc" | "ini" | "cfg" | "conf" | "env" | "xml" => {
            ContentKind::Config
        }
        "csv" | "tsv" | "psv" | "json5" => ContentKind::Data,
        _ => ContentKind::Code,
    }
}

fn is_default_ignored(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name,
                ".git"
                    | ".hg"
                    | ".svn"
                    | ".cache"
                    | ".semble"
                    | ".venv"
                    | "node_modules"
                    | "target"
                    | "dist"
                    | "build"
                    | "out"
                    | "coverage"
            )
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::hint::black_box;
    use std::time::Instant;

    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: code search file discovery respects content filters and Semble ignore files.
    #[test]
    fn discover_files_respects_sembleignore_and_content_filter() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("main.rs"), "fn main() {}\n").expect("write");
        fs::write(temp.path().join("guide.md"), "# Guide\n").expect("write");
        fs::write(temp.path().join("skip.rs"), "fn skip() {}\n").expect("write");
        fs::write(temp.path().join(".sembleignore"), "skip.rs\n").expect("write");

        let files = discover_files(temp.path(), ContentFilter::Code).expect("discover files");
        let paths = files
            .into_iter()
            .map(|entry| entry.manifest.path)
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![PathBuf::from("main.rs")]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: code search skips files over the indexing size limit.
    #[test]
    fn discover_files_skips_large_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(
            temp.path().join("big.rs"),
            vec![b'a'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("write");

        let files = discover_files(temp.path(), ContentFilter::Code).expect("discover files");

        assert_eq!(files, Vec::new());
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: all-content mode excludes data files while retaining docs and config files.
    #[test]
    fn content_all_excludes_data_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::write(temp.path().join("data.csv"), "a,b\n").expect("write");
        fs::write(temp.path().join("config.toml"), "[x]\n").expect("write");
        fs::write(temp.path().join("readme.md"), "# Readme\n").expect("write");

        let paths = discover_files(temp.path(), ContentFilter::All)
            .expect("discover files")
            .into_iter()
            .map(|entry| entry.manifest.path)
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![PathBuf::from("config.toml"), PathBuf::from("readme.md")]
        );
    }

    #[test]
    fn classify_path_handles_case_insensitive_ascii_names() {
        let paths = [
            PathBuf::from("DOCKERFILE"),
            PathBuf::from("src/component.TSX"),
            PathBuf::from("config/settings.TOML"),
        ];
        let classified = paths
            .iter()
            .map(|path| classify_path(path))
            .collect::<Vec<_>>();

        assert_eq!(
            classified,
            vec![
                Some(("dockerfile", ContentKind::Config)),
                Some(("typescriptreact", ContentKind::Code)),
                Some(("toml", ContentKind::Config)),
            ]
        );
    }

    #[test]
    #[ignore]
    fn bench_classify_path_candidates() {
        let paths = (0..100_000)
            .map(|idx| match idx % 8 {
                0 => PathBuf::from(format!("src/module_{idx}.rs")),
                1 => PathBuf::from(format!("src/component_{idx}.TSX")),
                2 => PathBuf::from(format!("docs/guide_{idx}.MD")),
                3 => PathBuf::from(format!("config/settings_{idx}.TOML")),
                4 => PathBuf::from(format!("data/input_{idx}.csv")),
                5 => PathBuf::from(format!("scripts/run_{idx}.sh")),
                6 => PathBuf::from(format!("assets/image_{idx}.png")),
                _ => PathBuf::from("Dockerfile"),
            })
            .collect::<Vec<_>>();
        let expected_classified = paths.iter().filter_map(|path| classify_path(path)).count();
        let iterations = 100;
        let started = Instant::now();
        let mut classified = 0usize;

        for _ in 0..iterations {
            classified += paths
                .iter()
                .filter_map(|path| black_box(classify_path(black_box(path))))
                .count();
        }

        let elapsed = started.elapsed();
        assert_eq!(classified, expected_classified * iterations);
        println!(
            "classify_path_candidates iterations={iterations} paths={} elapsed_ms={} per_path_ns={:.2}",
            paths.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000_000.0 / (iterations * paths.len()) as f64
        );
    }
}
