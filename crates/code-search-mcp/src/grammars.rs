//! Tree-sitter grammar registry for code-search chunking.
//!
//! File discovery emits stable language labels such as `rust`, `python`, or
//! `typescriptreact`. This module is the only place that maps those labels to
//! concrete tree-sitter grammars. Keeping the mapping isolated lets chunking stay
//! generic and makes unsupported languages fall back to line chunks without
//! spreading grammar-specific branches through the retrieval pipeline.

use tree_sitter::Language;

/// Returns the tree-sitter grammar for a discovered language label.
///
/// The labels must stay in sync with `files::language_for_extension`. `None`
/// means the language intentionally uses line-based chunking.
pub fn language_for_chunking(language: &str) -> Option<Language> {
    match language {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" | "javascriptreact" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "typescriptreact" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "kotlin" => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" => Some(tree_sitter_cpp::LANGUAGE.into()),
        "csharp" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "ruby" => Some(tree_sitter_ruby::LANGUAGE.into()),
        "php" => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        "swift" => Some(tree_sitter_swift::LANGUAGE.into()),
        "scala" => Some(tree_sitter_scala::LANGUAGE.into()),
        "shell" => Some(tree_sitter_bash::LANGUAGE.into()),
        "powershell" => Some(tree_sitter_powershell::LANGUAGE.into()),
        "lua" => Some(tree_sitter_lua::LANGUAGE.into()),
        "r" => Some(tree_sitter_r::LANGUAGE.into()),
        "sql" => Some(tree_sitter_sequel::LANGUAGE.into()),
        "toml" => Some(tree_sitter_toml_ng::LANGUAGE.into()),
        "yaml" => Some(tree_sitter_yaml::LANGUAGE.into()),
        "json" => Some(tree_sitter_json::LANGUAGE.into()),
        "config" => Some(tree_sitter_ini::LANGUAGE.into()),
        "xml" => Some(tree_sitter_xml::LANGUAGE_XML.into()),
        "dockerfile" => Some(tree_sitter_containerfile::LANGUAGE.into()),
        "markdown" | "rst" | "text" | "data" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    /// Trace: L2-DES-TOOL-001
    /// Verifies: grammar registry covers the AST-enabled code and config labels.
    #[test]
    fn registry_covers_ast_enabled_languages() {
        let ast_languages = [
            "rust",
            "python",
            "javascript",
            "javascriptreact",
            "typescript",
            "typescriptreact",
            "go",
            "java",
            "kotlin",
            "c",
            "cpp",
            "csharp",
            "ruby",
            "php",
            "swift",
            "scala",
            "shell",
            "powershell",
            "lua",
            "r",
            "sql",
            "toml",
            "yaml",
            "json",
            "config",
            "xml",
            "dockerfile",
        ];
        let coverage = ast_languages
            .iter()
            .map(|language| language_for_chunking(language).is_some())
            .collect::<Vec<_>>();

        assert_eq!(coverage, vec![true; ast_languages.len()]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: docs and data labels intentionally keep line-based chunking.
    #[test]
    fn registry_leaves_docs_and_data_line_based() {
        let fallback = ["markdown", "rst", "text", "data"]
            .iter()
            .map(|language| language_for_chunking(language).is_none())
            .collect::<Vec<_>>();

        assert_eq!(fallback, vec![true, true, true, true]);
    }
}
