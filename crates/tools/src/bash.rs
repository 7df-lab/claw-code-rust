use crate::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Instant;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use tracing::info;

const DESCRIPTION: &str = include_str!("bash.txt");
const MAX_METADATA_LENGTH: usize = 30_000;
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_YIELD_TIME_MS: u64 = 1_000;
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 16_000;
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
                .replace("${shell}", platform_shell(true).program)
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

        let timeout_ms = input["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT_MS);
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
            .unwrap_or(DEFAULT_YIELD_TIME_MS);
        let max_output_tokens = input["max_output_tokens"]
            .as_u64()
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);

        if !workdir.exists() {
            return Ok(ToolOutput::error(format!(
                "working directory does not exist: {}",
                workdir.display()
            )));
        }

        let shell = resolve_shell(shell_override.as_deref(), login);
        let command_to_run = if cfg!(windows) && shell.program.eq_ignore_ascii_case("powershell") {
            format!(
                concat!(
                    "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                    "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                    "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                    "[System.Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ",
                    "{}"
                ),
                command
            )
        } else {
            command.to_string()
        };

        if tty {
            return run_with_pty(
                shell,
                command_to_run,
                workdir,
                description,
                timeout_ms,
                yield_time_ms,
                max_output_tokens,
            )
            .await;
        }

        info!(command, shell = shell.program, "executing shell command");
        let command_preview = preview(&command_to_run);
        let mut child = Command::new(shell.program);
        child
            .args(shell.args)
            .arg(&command_to_run)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&workdir);

        if cfg!(windows) {
            child.env("PYTHONUTF8", "1");
        }

        let result = timeout(Duration::from_millis(timeout_ms), child.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let result_text = merge_streams(&stdout, &stderr);
                let result_text = truncate_output(&result_text, max_output_tokens);
                if output.status.success() {
                    Ok(ToolOutput {
                        content: result_text.clone(),
                        is_error: false,
                        metadata: Some(json!({
                            "output": preview(&result_text),
                            "command": command_preview,
                            "exit": output.status.code(),
                            "description": description,
                            "cwd": workdir,
                            "yield_time_ms": yield_time_ms,
                        })),
                    })
                } else {
                    let code = output.status.code().unwrap_or(-1);
                    Ok(ToolOutput {
                        content: format!("exit code {}\n{}", code, result_text),
                        is_error: true,
                        metadata: Some(json!({
                            "output": preview(&result_text),
                            "command": command_preview,
                            "exit": code,
                            "description": description,
                            "cwd": workdir,
                            "yield_time_ms": yield_time_ms,
                        })),
                    })
                }
            }
            Ok(Err(e)) => Ok(ToolOutput::error(format!("failed to spawn process: {}", e))),
            Err(_) => Ok(ToolOutput::error(format!(
                "command timed out after {}ms",
                timeout_ms
            ))),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }
}

struct ShellSpec {
    program: &'static str,
    args: &'static [&'static str],
}

fn resolve_shell(shell: Option<&str>, login: bool) -> ShellSpec {
    let shell = shell.unwrap_or("");
    let normalized = shell.to_ascii_lowercase();

    if normalized.contains("powershell") || normalized == "pwsh" || normalized == "powershell" {
        return ShellSpec {
            program: "powershell",
            args: &["-NoLogo", "-NoProfile", "-Command"],
        };
    }

    if normalized.ends_with("cmd") || normalized.ends_with("cmd.exe") || normalized == "cmd" {
        return ShellSpec {
            program: "cmd",
            args: &["/C"],
        };
    }

    if normalized.contains("zsh") {
        return ShellSpec {
            program: "zsh",
            args: if login { &["-lc"] } else { &["-c"] },
        };
    }

    if normalized.contains("bash") {
        return ShellSpec {
            program: "bash",
            args: if login { &["-lc"] } else { &["-c"] },
        };
    }

    if login {
        platform_shell(true)
    } else {
        platform_shell(false)
    }
}

fn preview(text: &str) -> String {
    if text.len() <= MAX_METADATA_LENGTH {
        return text.to_string();
    }
    format!("{}\n\n...", &text[..MAX_METADATA_LENGTH])
}

fn truncate_output(text: &str, max_output_tokens: usize) -> String {
    if max_output_tokens == 0 {
        return String::new();
    }
    let max_chars = max_output_tokens.saturating_mul(4);
    let mut out: String = text.chars().take(max_chars).collect();
    if out.len() < text.len() {
        out.push_str("\n\n... [truncated]");
    }
    out
}

fn merge_streams(stdout: &str, stderr: &str) -> String {
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(stderr);
    }
    if result.is_empty() {
        "(no output)".to_string()
    } else {
        result
    }
}

fn platform_shell(login: bool) -> ShellSpec {
    if cfg!(windows) {
        ShellSpec {
            program: "powershell",
            args: &["-NoProfile", "-Command"],
        }
    } else {
        ShellSpec {
            program: "bash",
            args: if login { &["-lc"] } else { &["-c"] },
        }
    }
}

async fn run_with_pty(
    shell: ShellSpec,
    command_to_run: String,
    workdir: PathBuf,
    description: String,
    timeout_ms: u64,
    yield_time_ms: u64,
    max_output_tokens: usize,
) -> anyhow::Result<ToolOutput> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| anyhow::anyhow!("failed to open PTY: {error}"))?;

    let mut builder = CommandBuilder::new(shell.program);
    builder.args(shell.args);
    builder.arg(&command_to_run);
    builder.cwd(&workdir);
    if cfg!(windows) {
        builder.env("PYTHONUTF8", "1");
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
    }

    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| anyhow::anyhow!("failed to spawn PTY command: {error}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| anyhow::anyhow!("failed to clone PTY reader: {error}"))?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(0) => break,
                Ok(size) => {
                    if tx.send(buffer[..size].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let started = Instant::now();
    let sleep_ms = yield_time_ms.max(10);
    let timeout = Duration::from_millis(timeout_ms);
    let mut output = Vec::new();
    let mut exit_code = None;
    let mut timed_out = false;

    loop {
        while let Ok(chunk) = rx.try_recv() {
            output.extend_from_slice(&chunk);
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|error| anyhow::anyhow!("failed to poll PTY child: {error}"))?
        {
            exit_code = Some(status.exit_code() as i32);
            break;
        }

        if started.elapsed() >= timeout {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break;
        }

        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
    }

    while let Ok(chunk) = rx.try_recv() {
        output.extend_from_slice(&chunk);
    }

    let mut text = String::from_utf8_lossy(&output).into_owned();
    text = truncate_output(&text, max_output_tokens);

    if timed_out {
        return Ok(ToolOutput {
            content: format!("command timed out after {}ms\n{}", timeout_ms, text),
            is_error: true,
            metadata: Some(json!({
                "output": preview(&text),
                "command": command_to_run,
                "exit": exit_code,
                "description": description,
                "cwd": workdir,
                "yield_time_ms": yield_time_ms,
                "tty": true,
            })),
        });
    }

    let is_error = exit_code.unwrap_or(1) != 0;
    Ok(ToolOutput {
        content: if is_error {
            format!("exit code {}\n{}", exit_code.unwrap_or(-1), text)
        } else {
            text.clone()
        },
        is_error,
        metadata: Some(json!({
            "output": preview(&text),
            "command": command_to_run,
            "exit": exit_code,
            "description": description,
            "cwd": workdir,
            "yield_time_ms": yield_time_ms,
            "tty": true,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_shell_prefers_powershell_alias() {
        let spec = resolve_shell(Some("pwsh"), true);
        assert_eq!(spec.program, "powershell");
        assert_eq!(spec.args, &["-NoLogo", "-NoProfile", "-Command"]);
    }

    #[test]
    fn resolve_shell_prefers_cmd_alias() {
        let spec = resolve_shell(Some("cmd.exe"), true);
        assert_eq!(spec.program, "cmd");
        assert_eq!(spec.args, &["/C"]);
    }

    #[test]
    fn resolve_shell_defaults_to_platform_shell_login() {
        let expected = platform_shell(true);
        let spec = resolve_shell(None, true);
        assert_eq!(spec.program, expected.program);
        assert_eq!(spec.args, expected.args);
    }

    #[test]
    fn preview_truncates_long_text() {
        let long = "a".repeat(MAX_METADATA_LENGTH + 1);
        let result = preview(&long);
        assert!(result.ends_with("\n\n..."));
        assert_eq!(result.len(), MAX_METADATA_LENGTH + 5); // adds newline newline ...
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
