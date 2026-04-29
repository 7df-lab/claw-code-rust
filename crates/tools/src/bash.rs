use crate::shell_exec::{
    ShellExecRequest, default_max_output_tokens, default_timeout_ms, default_yield_time_ms,
    execute_shell_command, platform_shell_program,
};
use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

const DESCRIPTION: &str = include_str!("bash.txt");
const DESCRIPTION_MAX_BYTES_LABEL: &str = "64 KB";

/// Execute shell commands.
///
/// This is the most powerful built-in tool. It runs commands in a child
/// process and captures stdout/stderr.
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    /// TODO: the shell tool should be re implemented.
    fn description(&self) -> &str {
        let chaining = if cfg!(windows) {
            "If commands depend on each other and must run sequentially, use a single PowerShell command string. In Windows PowerShell 5.1, do not rely on Bash chaining semantics like `cmd1 && cmd2`; prefer `cmd1; if ($?) { cmd2 }` when the later command depends on earlier success."
        } else {
            "If commands depend on each other and must run sequentially, use a single shell command and chain with `&&` when later commands depend on earlier success."
        };
        Box::leak(
            DESCRIPTION
                .replace(
                    "${directory}",
                    &std::env::current_dir()
                        .map_or_else(|_| ".".to_string(), |path| path.display().to_string()),
                )
                .replace("${os}", std::env::consts::OS)
                .replace("${shell}", platform_shell_program(true))
                .replace("${chaining}", chaining)
                .replace("${maxBytes}", DESCRIPTION_MAX_BYTES_LABEL)
                .into_boxed_str(),
        )
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute in the selected platform shell"
                },
                "cmd": {
                    "type": "string",
                    "description": "Alias for command"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds"
                },
                "workdir": {
                    "type": "string",
                    "description": "The working directory to run the command in. Defaults to the current directory. Use this instead of 'cd' commands."
                },
                "description": {
                    "type": "string",
                    "description": "Clear, concise description of what this command does in 5-10 words."
                },
                "shell": {
                    "type": "string",
                    "description": "Optional shell binary to launch. Defaults to the user's default shell."
                },
                "tty": {
                    "type": "boolean",
                    "description": "Whether to allocate a TTY for the command. Defaults to false."
                },
                "login": {
                    "type": "boolean",
                    "description": "Whether to run the shell with login shell semantics. Defaults to true."
                },
                "yield_time_ms": {
                    "type": "integer",
                    "description": "How long to wait (in milliseconds) for output before yielding."
                },
                "max_output_tokens": {
                    "type": "integer",
                    "description": "Maximum number of tokens to return. Excess output will be truncated."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> anyhow::Result<ToolOutput> {
        let command = input
            .get("command")
            .or_else(|| input.get("cmd"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing 'command' field"))?;

        let timeout_ms = input["timeout"].as_u64().unwrap_or(default_timeout_ms());
        let workdir = input["workdir"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| ctx.cwd.clone());
        let description = input["description"]
            .as_str()
            .unwrap_or("shell command")
            .to_string();
        let shell_override = input["shell"].as_str().map(ToOwned::to_owned);
        let tty = input["tty"].as_bool().unwrap_or(false);
        let login = input["login"].as_bool().unwrap_or(true);
        let yield_time_ms = input["yield_time_ms"]
            .as_u64()
            .unwrap_or(default_yield_time_ms());
        let max_output_tokens = input["max_output_tokens"]
            .as_u64()
            .map(|value| value as usize)
            .unwrap_or(default_max_output_tokens());

        execute_shell_command(
            ShellExecRequest {
                command: command.to_string(),
                workdir,
                description,
                shell_override,
                tty,
                login,
                timeout_ms,
                yield_time_ms,
                max_output_tokens,
            },
            None,
        )
        .await
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use crate::shell_exec::{merge_streams, platform_shell_program, preview, truncate_output};

    #[test]
    fn resolve_shell_defaults_to_platform_shell_login() {
        assert_eq!(
            platform_shell_program(true),
            if cfg!(windows) { "powershell" } else { "bash" }
        );
    }

    #[test]
    fn preview_truncates_long_text() {
        let long = "a".repeat(30_001);
        let result = preview(&long);
        assert!(result.ends_with("\n\n..."));
    }

    #[test]
    fn truncate_output_handles_zero_tokens() {
        assert_eq!(truncate_output("text", 0), "");
    }

    #[test]
    fn truncate_output_limits_length() {
        let input = "a".repeat(200);
        let result = truncate_output(&input, 10);
        assert!(result.ends_with("\n\n... [truncated]"));
        assert!(result.len() < input.len());
    }

    #[test]
    fn merge_streams_combines_stdout_and_stderr() {
        let result = merge_streams("out", "err");
        assert!(result.contains("out"));
        assert!(result.contains("[stderr]"));
        assert!(result.contains("err"));
    }

    #[test]
    fn merge_streams_no_output() {
        assert_eq!(merge_streams("", ""), "(no output)");
    }

    #[test]
    fn truncate_output_keeps_short_text() {
        let input = "short";
        assert_eq!(truncate_output(input, 10), input);
    }
}
