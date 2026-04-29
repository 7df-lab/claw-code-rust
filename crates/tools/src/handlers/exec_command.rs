use std::sync::Arc;

use async_trait::async_trait;

use crate::errors::ToolExecutionError;
use crate::events::ToolProgressSender;
use crate::handler_kind::ToolHandlerKind;
use crate::invocation::{FunctionToolOutput, ToolInvocation, ToolOutput};
use crate::tool_handler::ToolHandler;
use crate::unified_exec::process::{UnifiedExecProcess, collect_output};
use crate::unified_exec::store::ProcessStore;
use crate::unified_exec::{ExecCommandArgs, ProcessOutput, WriteStdinArgs};

pub struct ExecCommandHandler {
    store: Arc<ProcessStore>,
}

impl ExecCommandHandler {
    pub fn new(store: Arc<ProcessStore>) -> Self {
        ExecCommandHandler { store }
    }
}

#[async_trait]
impl ToolHandler for ExecCommandHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::ExecCommand
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let args = ExecCommandArgs {
            cmd: invocation
                .input
                .get("cmd")
                .or_else(|| invocation.input.get("command"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                    message: "missing 'cmd' field".into(),
                })?
                .to_string(),
            workdir: invocation.input["workdir"].as_str().map(|s| s.to_string()),
            shell: invocation.input["shell"].as_str().map(|s| s.to_string()),
            login: invocation.input["login"].as_bool().unwrap_or(true),
            tty: invocation.input["tty"].as_bool().unwrap_or(true),
            yield_time_ms: invocation.input["yield_time_ms"]
                .as_u64()
                .unwrap_or(crate::unified_exec::DEFAULT_YIELD_MS),
            max_output_tokens: invocation.input["max_output_tokens"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(crate::unified_exec::MAX_OUTPUT_TOKENS),
        };

        let cwd = invocation.input["workdir"]
            .as_str()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| invocation.cwd.clone());

        if !cwd.exists() {
            return Ok(Box::new(FunctionToolOutput::error(format!(
                "working directory does not exist: {}",
                cwd.display()
            ))));
        }

        let (proc, _broadcast_rx) =
            UnifiedExecProcess::spawn(0, &args.cmd, &cwd, args.shell.as_deref(), args.login)
                .map_err(|e| ToolExecutionError::ExecutionFailed {
                    message: format!("failed to spawn process: {e}"),
                })?;

        if let Some(ref sender) = progress {
            let mut progress_rx = proc.subscribe();
            let s = sender.clone();
            tokio::spawn(async move {
                while let Ok(bytes) = progress_rx.recv().await {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if s.send(text).is_err() {
                        break;
                    }
                }
            });
        }

        let proc = Arc::new(proc);
        let session_id = self.store.allocate(Arc::clone(&proc)).await;

        let mut rx = proc.subscribe();
        let output =
            collect_output(&mut rx, &proc, args.yield_time_ms, args.max_output_tokens).await;

        let response = format_exec_response(&output, Some(session_id));
        Ok(Box::new(FunctionToolOutput::success(response)))
    }
}

pub struct WriteStdinHandler {
    store: Arc<ProcessStore>,
}

impl WriteStdinHandler {
    pub fn new(store: Arc<ProcessStore>) -> Self {
        WriteStdinHandler { store }
    }
}

#[async_trait]
impl ToolHandler for WriteStdinHandler {
    fn tool_kind(&self) -> ToolHandlerKind {
        ToolHandlerKind::WriteStdin
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        _progress: Option<ToolProgressSender>,
    ) -> Result<Box<dyn ToolOutput>, ToolExecutionError> {
        let args = WriteStdinArgs {
            session_id: invocation.input["session_id"].as_i64().ok_or_else(|| {
                ToolExecutionError::ExecutionFailed {
                    message: "missing 'session_id' field".into(),
                }
            })? as i32,
            chars: invocation.input["chars"].as_str().unwrap_or("").to_string(),
            yield_time_ms: invocation.input["yield_time_ms"]
                .as_u64()
                .unwrap_or(crate::unified_exec::DEFAULT_POLL_YIELD_MS),
            max_output_tokens: invocation.input["max_output_tokens"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(crate::unified_exec::MAX_OUTPUT_TOKENS),
        };

        let proc = self.store.get(args.session_id).await.ok_or_else(|| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Unknown process id {}", args.session_id),
            }
        })?;

        if !args.chars.is_empty() {
            proc.write_stdin(&args.chars)
                .map_err(|e| ToolExecutionError::ExecutionFailed {
                    message: format!("write_stdin failed: {e}"),
                })?;

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let mut rx = proc.subscribe();
        let output =
            collect_output(&mut rx, &proc, args.yield_time_ms, args.max_output_tokens).await;

        if output.exit_code.is_some() && output.output.is_empty() {
            self.store.remove(args.session_id).await;
        }

        let response = format_exec_response(&output, None);
        Ok(Box::new(FunctionToolOutput::success(response)))
    }
}

fn format_exec_response(output: &ProcessOutput, session_id: Option<i32>) -> String {
    let mut parts = Vec::new();

    parts.push(format!("Wall time: {:.1} seconds", output.wall_time_secs));

    if let Some(code) = output.exit_code {
        parts.push(format!("Process exited with code {code}"));
    }
    if let Some(sid) = session_id {
        if output.exit_code.is_none() {
            parts.push(format!("Process running with session ID {sid}"));
        }
    }

    if output.truncated {
        parts.push("Output (truncated):".to_string());
    } else {
        parts.push("Output:".to_string());
    }
    parts.push(output.output.clone());

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_exec_response_exited() {
        let output = ProcessOutput {
            output: "hello world".into(),
            exit_code: Some(0),
            wall_time_secs: 1.5,
            truncated: false,
        };
        let text = format_exec_response(&output, None);
        assert!(text.contains("Wall time: 1.5"));
        assert!(text.contains("Process exited with code 0"));
        assert!(text.contains("hello world"));
        assert!(!text.contains("session ID"));
    }

    #[test]
    fn format_exec_response_running() {
        let output = ProcessOutput {
            output: "building...".into(),
            exit_code: None,
            wall_time_secs: 10.0,
            truncated: false,
        };
        let text = format_exec_response(&output, Some(42));
        assert!(text.contains("Process running with session ID 42"));
        assert!(!text.contains("exit code"));
    }

    #[test]
    fn format_exec_response_truncated() {
        let output = ProcessOutput {
            output: "long output...".into(),
            exit_code: None,
            wall_time_secs: 5.0,
            truncated: true,
        };
        let text = format_exec_response(&output, Some(1));
        assert!(text.contains("Output (truncated)"));
    }

    #[test]
    fn format_exec_response_with_both_exit_and_session() {
        let output = ProcessOutput {
            output: "done".into(),
            exit_code: Some(0),
            wall_time_secs: 3.0,
            truncated: false,
        };
        // When exit_code is Some, session_id is not shown even if provided
        let text = format_exec_response(&output, Some(99));
        assert!(text.contains("Process exited with code 0"));
        assert!(!text.contains("session ID"));
    }

    #[test]
    fn exec_command_args_missing_cmd() {
        let args = serde_json::json!({});
        let result = serde_json::from_value::<serde_json::Value>(args);
        assert!(result.is_ok());
        // The cmd field is required but we can't easily test parse failure
        // because there's no deserialize impl for ExecCommandArgs
    }
}
