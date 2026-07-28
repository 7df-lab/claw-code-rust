//! Stable, display-only summaries for tool calls.
//!
//! Tool specs and handlers live in different crates, so summaries are centralized
//! here until each tool can own its display policy without introducing cycles.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

fn make_relative<'a>(cwd: &Path, path: &'a str) -> Cow<'a, str> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Cow::Borrowed(path);
    }
    // Try Path::strip_prefix first (handles platform semantics correctly)
    if let Ok(rel) = p.strip_prefix(cwd) {
        if rel.as_os_str().is_empty() {
            Cow::Borrowed(".")
        } else if let Some(rel) = rel.to_str() {
            // Tool summaries are generated for every call; keep the common
            // UTF-8 path case borrowed and allocate only when normalizing.
            normalize_path_separators(Cow::Borrowed(rel))
        } else {
            Cow::Owned(normalize_path_separators(rel.to_string_lossy()).into_owned())
        }
    } else {
        // Fallback: string-level comparison with forward-slash normalization
        let cwd_str = normalize_path_separators(cwd.to_string_lossy());
        let path_str = normalize_path_separators(Cow::Borrowed(path));
        if path_str == cwd_str.as_ref() {
            Cow::Borrowed(".")
        } else if let Some(rel) = path_str
            .strip_prefix(cwd_str.as_ref())
            .and_then(|rest| rest.strip_prefix('/'))
        {
            Cow::Owned(rel.to_string())
        } else {
            Cow::Borrowed(path)
        }
    }
}

fn normalize_path_separators(text: Cow<'_, str>) -> Cow<'_, str> {
    if text.contains('\\') {
        Cow::Owned(text.replace('\\', "/"))
    } else {
        text
    }
}

fn string_arg<'a>(input: &'a Value, key: &str, default: &'a str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn string_arg_any<'a>(input: &'a Value, keys: &[&str], default: &'a str) -> &'a str {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .unwrap_or(default)
}

/// Compute a human-readable summary/title for a tool call, based on the tool
/// name and its input arguments. Paths are made relative to `cwd`.
pub fn tool_summary(name: &str, input: &serde_json::Value, cwd: &Path) -> String {
    match name {
        "bash" | "shell_command" => {
            let cmd = string_arg_any(input, &["command", "cmd"], "");
            format!("Shell: {cmd}")
        }
        "exec_command" => {
            let cmd = string_arg_any(input, &["cmd", "command"], "");
            format!("exec: {cmd}")
        }
        "read" => {
            let path = string_arg_any(input, &["filePath", "path"], "");
            let rel = make_relative(cwd, path);
            let mut s = String::with_capacity("read: ".len() + rel.len() + 32);
            s.push_str("read: ");
            s.push_str(&rel);
            let offset = input["offset"].as_u64();
            let limit = input["limit"].as_u64();
            match (offset, limit) {
                (Some(o), Some(l)) => {
                    write!(&mut s, " (offset:{o}, limit:{l})")
                        .expect("writing to a String cannot fail");
                }
                (Some(o), None) => {
                    write!(&mut s, " (offset:{o})").expect("writing to a String cannot fail");
                }
                (None, Some(l)) => {
                    write!(&mut s, " (limit:{l})").expect("writing to a String cannot fail");
                }
                (None, None) => {}
            }
            s
        }
        "write" => {
            let path = string_arg(input, "filePath", "");
            let rel = make_relative(cwd, path);
            format!("write: {rel}")
        }
        "edit" => {
            let path = string_arg(input, "filePath", "");
            let rel = make_relative(cwd, path);
            format!("edit: {rel}")
        }
        "grep" => {
            let pattern = string_arg(input, "pattern", "");
            let path = string_arg(input, "path", ".");
            let rel = make_relative(cwd, path);
            format!("grep: '{pattern}' in {rel}")
        }
        "code_search" => {
            let operation = string_arg(input, "operation", "search");
            match operation {
                "find_related" => {
                    let path = string_arg(input, "file_path", "");
                    let rel = make_relative(cwd, path);
                    let mut summary =
                        String::with_capacity("code_search related ".len() + rel.len() + 21);
                    summary.push_str("code_search related ");
                    summary.push_str(&rel);
                    summary.push(':');
                    if let Some(line) = input["line"].as_u64() {
                        write!(&mut summary, "{line}").expect("writing to a String cannot fail");
                    } else {
                        summary.push('?');
                    }
                    summary
                }
                _ => {
                    let query = string_arg(input, "query", "");
                    let path = string_arg(input, "path", ".");
                    let rel = make_relative(cwd, path);
                    format!("code_search: {query} in {rel}")
                }
            }
        }
        "find" | "glob" => {
            let pattern = string_arg(input, "pattern", "");
            let path = string_arg(input, "path", ".");
            let rel = make_relative(cwd, path);
            format!("{name}: {pattern} in {rel}")
        }
        "apply_patch" => "Patch".to_string(),
        "webfetch" | "web_fetch" | "web-fetch" | "fetch_url" | "fetch-url" => {
            let url = string_arg(input, "url", "");
            format!("web_fetch: {url}")
        }
        "web_search" | "websearch" | "web-search" => {
            let q = string_arg(input, "query", "");
            format!("web_search: {q}")
        }
        "skill" => {
            let name = string_arg(input, "name", "");
            format!("skill: {name}")
        }
        "spawn_agent" => {
            let message = string_arg(input, "message", "");
            if message.is_empty() {
                "spawn_agent".to_string()
            } else {
                format!("spawn_agent: {message}")
            }
        }
        "question" | "request_user_input" => "request_user_input".to_string(),
        "update_plan" => "update_plan".to_string(),
        "lsp" => {
            let path = string_arg(input, "filePath", "");
            let rel = make_relative(cwd, path);
            let mut summary = String::with_capacity("lsp: ".len() + rel.len() + 24);
            summary.push_str("lsp: ");
            summary.push_str(&rel);
            summary.push(':');
            if let Some(line) = input["line"].as_i64() {
                write!(&mut summary, "{line}:").expect("writing to a String cannot fail");
            } else {
                summary.push_str("?:");
            }
            if let Some(col) = input["character"].as_i64() {
                write!(&mut summary, "{col}").expect("writing to a String cannot fail");
            } else {
                summary.push('?');
            }
            summary
        }
        "invalid" => "invalid".to_string(),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::path::PathBuf;

    fn cwd() -> PathBuf {
        PathBuf::from("/project")
    }

    #[test]
    fn bash_summary() {
        let input = json!({"cmd": "echo hello"});
        let s = tool_summary("bash", &input, &cwd());
        assert_eq!(s, "Shell: echo hello");
    }

    #[test]
    fn shell_command_summary() {
        let input = json!({"command": "npm run build"});
        let s = tool_summary("shell_command", &input, &cwd());
        assert_eq!(s, "Shell: npm run build");
    }

    #[test]
    fn apply_patch_summary() {
        let input = json!({});
        let s = tool_summary("apply_patch", &input, &cwd());
        assert_eq!(s, "Patch");
    }

    #[test]
    fn exec_command_summary() {
        let input = json!({"cmd": "make test"});
        let s = tool_summary("exec_command", &input, &cwd());
        assert_eq!(s, "exec: make test");
    }

    #[test]
    fn read_summary_offset_limit() {
        let input = json!({"filePath": "src/main.rs", "offset": 10, "limit": 50});
        let s = tool_summary("read", &input, &cwd());
        assert_eq!(s, "read: src/main.rs (offset:10, limit:50)");
    }

    #[test]
    fn read_summary_offset_only() {
        let input = json!({"filePath": "src/main.rs", "offset": 100});
        let s = tool_summary("read", &input, &cwd());
        assert_eq!(s, "read: src/main.rs (offset:100)");
    }

    #[test]
    fn read_summary_limit_only() {
        let input = json!({"filePath": "src/main.rs", "limit": 25});
        let s = tool_summary("read", &input, &cwd());
        assert_eq!(s, "read: src/main.rs (limit:25)");
    }

    #[test]
    fn read_summary_no_offset_limit() {
        let input = json!({"filePath": "src/main.rs"});
        let s = tool_summary("read", &input, &cwd());
        assert_eq!(s, "read: src/main.rs");
    }

    #[test]
    fn read_summary_absolute_path_kept_when_outside_cwd() {
        let cwd = PathBuf::from("/project");
        let input = json!({"filePath": "/tmp/foo.txt"});
        let s = tool_summary("read", &input, &cwd);
        assert_eq!(s, "read: /tmp/foo.txt");
    }

    #[test]
    fn write_summary() {
        let input = json!({"filePath": "src/lib.rs"});
        let s = tool_summary("write", &input, &cwd());
        assert_eq!(s, "write: src/lib.rs");
    }

    #[test]
    fn grep_summary() {
        let input = json!({"pattern": "TODO", "path": "src/"});
        let s = tool_summary("grep", &input, &cwd());
        assert_eq!(s, "grep: 'TODO' in src/");
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: code_search summaries distinguish search and find-related operations.
    #[test]
    fn code_search_summary() {
        let input = json!({"operation": "search", "query": "parser error", "path": "src"});
        let s = tool_summary("code_search", &input, &cwd());
        assert_eq!(s, "code_search: parser error in src");

        let input = json!({
            "operation": "find_related",
            "file_path": "src/lib.rs",
            "line": 42
        });
        let s = tool_summary("code_search", &input, &cwd());
        assert_eq!(s, "code_search related src/lib.rs:42");
    }

    #[test]
    fn find_summary() {
        let input = json!({"pattern": "**/*.rs", "path": "src"});
        let s = tool_summary("find", &input, &cwd());
        assert_eq!(s, "find: **/*.rs in src");
    }

    #[test]
    fn glob_summary() {
        let input = json!({"pattern": "**/*.rs", "path": "src"});
        let s = tool_summary("glob", &input, &cwd());
        assert_eq!(s, "glob: **/*.rs in src");
    }

    #[test]
    fn lsp_summary() {
        let input = json!({"filePath": "src/lib.rs", "line": 10, "character": 5});
        let s = tool_summary("lsp", &input, &cwd());
        assert_eq!(s, "lsp: src/lib.rs:10:5");
    }

    #[test]
    fn make_relative_from_cwd() {
        let cwd = std::env::current_dir().unwrap_or_default();
        let sub = cwd.join("src").join("main.rs");
        let sub_str = sub.to_string_lossy().to_string();
        let rel = make_relative(&cwd, &sub_str);
        assert!(
            rel == "src/main.rs" || rel == "src\\main.rs",
            "make_relative('{sub_str}') = '{rel}', expected 'src/main.rs'"
        );
    }

    #[test]
    fn absolute_path_with_same_prefix_stays_absolute() {
        let cwd = std::env::current_dir().expect("current dir");
        let cwd_name = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .expect("current dir has utf-8 file name");
        let sibling = cwd
            .with_file_name(format!("{cwd_name}-backup"))
            .join("src")
            .join("main.rs");
        let sibling_text = sibling.to_string_lossy().to_string();
        let input = json!({"filePath": sibling_text});

        let summary = tool_summary("read", &input, &cwd);

        assert_eq!(summary, format!("read: {}", sibling.to_string_lossy()));
    }
}
