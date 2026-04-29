/// Compute a human-readable summary/title for a tool call, based on the tool
/// name and its input arguments. Used for client-side rendering.
pub fn tool_summary(name: &str, input: &serde_json::Value) -> String {
    match name {
        "bash" | "shell_command" => {
            let cmd = input
                .get("command")
                .or_else(|| input.get("cmd"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{name}: {cmd}")
        }
        "exec_command" => {
            let cmd = input
                .get("cmd")
                .or_else(|| input.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("exec: {cmd}")
        }
        "read" => {
            let path = input["filePath"].as_str().unwrap_or("");
            format!("read: {path}")
        }
        "write" => {
            let path = input["filePath"].as_str().unwrap_or("");
            format!("write: {path}")
        }
        "grep" => {
            let pattern = input["pattern"].as_str().unwrap_or("");
            let path = input["path"].as_str().unwrap_or(".");
            format!("grep: '{pattern}' in {path}")
        }
        "glob" => {
            let pattern = input["pattern"].as_str().unwrap_or("");
            let path = input["path"].as_str().unwrap_or(".");
            format!("glob: {pattern} in {path}")
        }
        "apply_patch" => "apply_patch".to_string(),
        "webfetch" => {
            let url = input["url"].as_str().unwrap_or("");
            format!("webfetch: {url}")
        }
        "websearch" => {
            let q = input["query"].as_str().unwrap_or("");
            format!("websearch: {q}")
        }
        "skill" => {
            let name = input["name"].as_str().unwrap_or("");
            format!("skill: {name}")
        }
        "question" => "question".to_string(),
        "update_plan" => "update_plan".to_string(),
        "task" => {
            let desc = input["description"].as_str().unwrap_or("");
            format!("task: {desc}")
        }
        "todowrite" => "todowrite".to_string(),
        "lsp" => {
            let path = input["filePath"].as_str().unwrap_or("");
            let line = input["line"]
                .as_i64()
                .map(|l| l.to_string())
                .unwrap_or_else(|| "?".into());
            let col = input["character"]
                .as_i64()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into());
            format!("lsp: {path}:{line}:{col}")
        }
        "invalid" => "invalid".to_string(),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_summary() {
        let input = json!({"cmd": "echo hello"});
        let s = tool_summary("bash", &input);
        assert_eq!(s, "bash: echo hello");
    }

    #[test]
    fn shell_command_summary() {
        let input = json!({"command": "npm run build"});
        let s = tool_summary("shell_command", &input);
        assert_eq!(s, "shell_command: npm run build");
    }

    #[test]
    fn exec_command_summary() {
        let input = json!({"cmd": "make test"});
        let s = tool_summary("exec_command", &input);
        assert_eq!(s, "exec: make test");
    }

    #[test]
    fn read_summary() {
        let input = json!({"filePath": "src/main.rs"});
        let s = tool_summary("read", &input);
        assert_eq!(s, "read: src/main.rs");
    }

    #[test]
    fn grep_summary() {
        let input = json!({"pattern": "TODO", "path": "src/"});
        let s = tool_summary("grep", &input);
        assert_eq!(s, "grep: 'TODO' in src/");
    }

    #[test]
    fn webfetch_summary() {
        let input = json!({"url": "https://example.com"});
        let s = tool_summary("webfetch", &input);
        assert_eq!(s, "webfetch: https://example.com");
    }

    #[test]
    fn websearch_summary() {
        let input = json!({"query": "rust async"});
        let s = tool_summary("websearch", &input);
        assert_eq!(s, "websearch: rust async");
    }

    #[test]
    fn unknown_tool_uses_name() {
        let input = json!({});
        let s = tool_summary("some_unknown_tool", &input);
        assert_eq!(s, "some_unknown_tool");
    }

    #[test]
    fn missing_params_graceful() {
        let input = json!({});
        let s = tool_summary("bash", &input);
        assert_eq!(s, "bash: ");
    }

    #[test]
    fn apply_patch_uses_static() {
        let input = json!({"patchText": "..."});
        let s = tool_summary("apply_patch", &input);
        assert_eq!(s, "apply_patch");
    }

    #[test]
    fn lsp_summary() {
        let input = json!({"filePath": "src/lib.rs", "line": 10, "character": 5});
        let s = tool_summary("lsp", &input);
        assert_eq!(s, "lsp: src/lib.rs:10:5");
    }
}
