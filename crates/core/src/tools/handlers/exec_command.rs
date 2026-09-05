use std::sync::Arc;

use async_trait::async_trait;
use devo_protocol::SessionId;
#[cfg(test)]
use uuid::Uuid;

use crate::apply_patch::exec_apply_patch;
use crate::contracts::ToolCallError;
use crate::contracts::ToolContext;
use crate::contracts::ToolProgressSender;
use crate::contracts::ToolResult;
use crate::contracts::ToolResultContent;
use crate::invocation::ToolContent;
use crate::json_schema::JsonSchema;
use crate::tool_handler::ToolHandler;
use crate::tool_spec::ToolCapabilityTag;
use crate::tool_spec::ToolExecutionMode;
use crate::tool_spec::ToolOutputMode;
use crate::tool_spec::ToolSpec;
use crate::tools::background_tasks::BackgroundTaskStore;
use crate::unified_exec::ExecCommandArgs;
use crate::unified_exec::ProcessOutput;
use crate::unified_exec::WARNING_PROCESSES;
use crate::unified_exec::WriteStdinArgs;
use crate::unified_exec::process::UnifiedExecProcess;
use crate::unified_exec::process::collect_output;
use crate::unified_exec::store::ProcessStore;

#[allow(dead_code)]
const MAX_EXEC_OUTPUT_DELTAS_PER_CALL: usize = 10_000;
#[allow(dead_code)]
const UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES: usize = 8_192;

pub struct ExecCommandHandler {
    store: Arc<ProcessStore>,
    background_tasks: Arc<BackgroundTaskStore>,
    spec: ToolSpec,
}

impl ExecCommandHandler {
    pub(crate) fn new(
        store: Arc<ProcessStore>,
        background_tasks: Arc<BackgroundTaskStore>,
    ) -> Self {
        Self {
            store,
            background_tasks,
            spec: ToolSpec {
                name: "exec_command".into(),
                description: "Execute a command with PTY support and process management.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "cmd".to_string(),
                            JsonSchema::string(Some("The command to execute.")),
                        ),
                        (
                            "workdir".to_string(),
                            JsonSchema::string(Some("Working directory")),
                        ),
                        (
                            "shell".to_string(),
                            JsonSchema::string(Some("Shell override")),
                        ),
                        (
                            "login".to_string(),
                            JsonSchema::boolean(Some("Whether to use login shell")),
                        ),
                        (
                            "tty".to_string(),
                            JsonSchema::boolean(Some("Whether to use PTY")),
                        ),
                        (
                            "execution_mode".to_string(),
                            JsonSchema::string(Some("attached (default) or background")),
                        ),
                    ]),
                    Some(vec!["cmd".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for ExecCommandHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let args = ExecCommandArgs {
            cmd: input
                .get("cmd")
                .or_else(|| input.get("command"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolCallError::InvalidInput("missing 'cmd' field".into()))?
                .to_string(),
            workdir: input["workdir"].as_str().map(|s| s.to_string()),
            shell: input["shell"].as_str().map(|s| s.to_string()),
            login: input["login"].as_bool().unwrap_or(true),
            tty: input["tty"].as_bool().unwrap_or(false),
            yield_time_ms: input["yield_time_ms"]
                .as_u64()
                .unwrap_or(crate::unified_exec::DEFAULT_YIELD_MS),
            max_output_tokens: input["max_output_tokens"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(crate::unified_exec::MAX_OUTPUT_TOKENS),
        };
        let execution_mode = match input["execution_mode"].as_str().unwrap_or("attached") {
            "attached" => ExecExecutionMode::Attached,
            "background" => ExecExecutionMode::Background,
            value => {
                return Err(ToolCallError::InvalidInput(format!(
                    "execution_mode must be attached or background, got {value}"
                )));
            }
        };

        let cwd = input["workdir"]
            .as_str()
            .map(|path| {
                let path = std::path::PathBuf::from(path);
                if path.is_absolute() {
                    path
                } else {
                    ctx.workspace_root.join(path)
                }
            })
            .unwrap_or_else(|| ctx.workspace_root.clone());

        if !cwd.exists() {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!(
                    "working directory does not exist: {}",
                    cwd.display()
                )),
                "Invalid workdir",
                ToolCallError::ExecutionFailed(format!(
                    "working directory does not exist: {}",
                    cwd.display()
                )),
            ));
        }

        if is_raw_apply_patch_body(&args.cmd) {
            return Ok(ToolResult::error(
                ToolResultContent::Text("apply_patch verification failed: patch detected without explicit call to apply_patch.".into()),
                "Invalid command",
                ToolCallError::InvalidInput("apply_patch verification failed".into()),
            ));
        }

        if let Some((patch_cwd, patch_text)) = apply_patch_command(&args.cmd, &cwd) {
            let output = exec_apply_patch(
                &patch_cwd,
                // &ctx.session_id.to_string(),
                serde_json::json!({ "patchText": patch_text }),
            )
            .await
            .map_err(|e| ToolCallError::ExecutionFailed(e.to_string()))?;
            let (text, metadata) = match output.content {
                ToolContent::Text(text) => (text, None),
                ToolContent::Json(json) => (json.to_string(), Some(json)),
                ToolContent::Mixed { text, json } => (text.unwrap_or_default(), json),
            };
            let content = text;
            return if output.is_error {
                Ok(ToolResult::error(
                    ToolResultContent::Text(content.clone()),
                    "Patch failed",
                    ToolCallError::ExecutionFailed(content),
                ))
            } else if metadata.is_some() {
                Ok(ToolResult::success(
                    ToolResultContent::Mixed {
                        text: Some(content),
                        json: metadata,
                    },
                    "Patch applied",
                ))
            } else {
                Ok(ToolResult::success(
                    ToolResultContent::Text(content),
                    "Patch applied",
                ))
            };
        }

        let Some(process_id) = self.store.reserve_process_id().await else {
            return Ok(ToolResult::error(
                ToolResultContent::Text(format!(
                    "max unified exec processes ({}) reached; cannot allocate process",
                    crate::unified_exec::MAX_PROCESSES
                )),
                "Process limit reached",
                ToolCallError::ExecutionFailed(format!(
                    "max unified exec processes ({}) reached",
                    crate::unified_exec::MAX_PROCESSES
                )),
            ));
        };

        let spawned_process = UnifiedExecProcess::spawn_with_sandbox_overlay(
            process_id,
            &args.cmd,
            &cwd,
            args.shell.as_deref(),
            args.login,
            args.tty,
            crate::unified_exec::process::SandboxExecutionOptions {
                sandbox_profile: ctx.sandbox_profile.clone(),
                sandbox_overlay: crate::tools::sandbox_overlay_for_spawn(
                    ctx.sandbox_permission_overlay.as_ref(),
                ),
            },
        )
        .await;
        let (proc, _broadcast_rx) = match spawned_process {
            Ok(spawned) => spawned,
            Err(error) => {
                self.store.release_reserved(process_id).await;
                return Err(ToolCallError::ExecutionFailed(format!(
                    "failed to spawn process: {error}"
                )));
            }
        };

        let proc = Arc::new(proc);
        self.store
            .insert_reserved(process_id, Arc::clone(&proc))
            .await;

        if execution_mode == ExecExecutionMode::Background {
            let owner_session_id =
                SessionId::try_from(ctx.session_id.as_str()).map_err(|error| {
                    ToolCallError::InvalidInput(format!("invalid current session id: {error}"))
                })?;
            let task = self
                .background_tasks
                .register_command(owner_session_id, process_id, args.cmd, Arc::clone(&proc))
                .await;
            let task_id = task.task_id.clone();
            let background_tasks = Arc::clone(&self.background_tasks);
            tokio::spawn(async move {
                while proc.is_running() && proc.exit_code().is_none() {
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                let mut rx = proc.subscribe();
                let output = collect_output(&mut rx, &proc, 100, args.max_output_tokens).await;
                let response =
                    format_exec_response(&output, Some(process_id), /*warning*/ None);
                background_tasks
                    .complete_command(&task_id, output.exit_code, response)
                    .await;
            });
            return Ok(ToolResult::success(
                ToolResultContent::Mixed {
                    text: Some(format!(
                        "Command running as background task {}",
                        task.task_id.0
                    )),
                    json: Some(
                        serde_json::to_value(task)
                            .map_err(|error| ToolCallError::InternalError(error.to_string()))?,
                    ),
                },
                "Command started in background",
            ));
        }

        let mut cancellation_guard = crate::unified_exec::cancellation::CancelProcessOnDrop::new(
            Arc::clone(&proc),
            Arc::clone(&self.store),
            process_id,
        );
        let cancel_token = ctx.cancel_token.clone();
        let store_for_cancel = Arc::clone(&self.store);
        let proc_for_cancel = Arc::clone(&proc);
        tokio::spawn(crate::unified_exec::cancellation::watch_turn(
            cancel_token,
            proc_for_cancel,
            store_for_cancel,
            process_id,
        ));

        let mut rx = proc.subscribe();
        let output = tokio::select! {
            output = collect_output(
                &mut rx,
                &proc,
                crate::unified_exec::clamp_exec_yield_time(args.yield_time_ms),
                args.max_output_tokens,
            ) => output,
            _ = ctx.cancel_token.cancelled() => {
                proc.terminate();
                self.store.remove(process_id).await;

                return Err(ToolCallError::Cancelled);
            }
        };
        cancellation_guard.disarm();
        let warning = if output.exit_code.is_some() {
            self.store.remove(process_id).await;
            None
        } else {
            let process_count = self.store.len().await;
            (process_count >= WARNING_PROCESSES).then(|| open_process_warning(process_count))
        };

        let response = format_exec_response(&output, Some(process_id), warning.as_deref());
        Ok(ToolResult::success(
            ToolResultContent::Text(response),
            "Command executed",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecExecutionMode {
    Attached,
    Background,
}

pub struct WriteStdinHandler {
    store: Arc<ProcessStore>,
    spec: ToolSpec,
}

impl WriteStdinHandler {
    pub fn new(store: Arc<ProcessStore>) -> Self {
        Self {
            store,
            spec: ToolSpec {
                name: "write_stdin".into(),
                description: "Write to stdin of a running process.".into(),
                input_schema: JsonSchema::object(
                    std::collections::BTreeMap::from([
                        (
                            "process_id".to_string(),
                            JsonSchema::integer(Some("Running process ID")),
                        ),
                        (
                            "chars".to_string(),
                            JsonSchema::string(Some("Characters to write to stdin")),
                        ),
                    ]),
                    Some(vec!["process_id".to_string(), "chars".to_string()]),
                    None,
                ),
                output_mode: ToolOutputMode::Mixed,
                execution_mode: ToolExecutionMode::Mutating,
                capability_tags: vec![ToolCapabilityTag::ExecuteProcess],
                supports_parallel: false,
                preparation_feedback: crate::tool_spec::ToolPreparationFeedback::None,
                display_name: None,
                supports_cancellation: None,
                supports_streaming: None,
            },
        }
    }
}

#[async_trait]
impl ToolHandler for WriteStdinHandler {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn handle(
        &self,
        _ctx: ToolContext,
        input: serde_json::Value,
        _progress: Option<ToolProgressSender>,
    ) -> Result<ToolResult, ToolCallError> {
        let args = WriteStdinArgs {
            process_id: input
                .get("process_id")
                .or_else(|| input.get("session_id"))
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| ToolCallError::InvalidInput("missing 'process_id' field".into()))?
                as i32,
            chars: input["chars"].as_str().unwrap_or("").to_string(),
            yield_time_ms: input["yield_time_ms"]
                .as_u64()
                .unwrap_or(crate::unified_exec::DEFAULT_POLL_YIELD_MS),
            max_output_tokens: input["max_output_tokens"]
                .as_u64()
                .map(|v| v as usize)
                .unwrap_or(crate::unified_exec::MAX_OUTPUT_TOKENS),
        };

        let proc = self.store.get(args.process_id).await.ok_or_else(|| {
            ToolCallError::ExecutionFailed(format!("Unknown process id {}", args.process_id))
        })?;

        let mut cancellation_guard = crate::unified_exec::cancellation::CancelProcessOnDrop::new(
            Arc::clone(&proc),
            Arc::clone(&self.store),
            args.process_id,
        );

        if !args.chars.is_empty() {
            if !proc.tty() {
                cancellation_guard.disarm();
                return Err(ToolCallError::ExecutionFailed(
                    "stdin is closed for this session".to_string(),
                ));
            }
            if let Err(error) = proc.write_stdin(&args.chars)
                && proc.is_running()
                && proc.exit_code().is_none()
            {
                cancellation_guard.disarm();
                return Err(ToolCallError::ExecutionFailed(format!(
                    "write_stdin failed: {error}"
                )));
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let mut rx = proc.subscribe();
        let output = collect_output(
            &mut rx,
            &proc,
            crate::unified_exec::clamp_write_stdin_yield_time(args.yield_time_ms, &args.chars),
            args.max_output_tokens,
        )
        .await;

        if output.exit_code.is_some() {
            self.store.remove(args.process_id).await;
        }

        cancellation_guard.disarm();
        let response = format_exec_response(&output, Some(args.process_id), /*warning*/ None);
        Ok(ToolResult::success(
            ToolResultContent::Text(response),
            "Input written",
        ))
    }
}

fn format_exec_response(
    output: &ProcessOutput,
    process_id: Option<i32>,
    warning: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if let Some(code) = output.exit_code {
        parts.push(format!("Process exited with code {code}"));
    }
    if let Some(process_id) = process_id
        && output.exit_code.is_none()
    {
        parts.push(format!("Process running with process ID {process_id}"));
    }
    if let Some(warning) = warning {
        parts.push(warning.to_string());
    }
    if !output.output.is_empty() {
        parts.push(output.output.clone());
    }

    parts.join("\n")
}

fn open_process_warning(process_count: usize) -> String {
    format!(
        "Warning: The maximum number of unified exec processes you can keep open is {WARNING_PROCESSES} and you currently have {process_count} processes open. Reuse older processes or close them to prevent automatic pruning of old processes"
    )
}

#[allow(dead_code)]
fn progress_delta_chunks(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut chunks = Vec::new();
    let mut remaining = text.as_ref();
    while !remaining.is_empty() {
        let take = floor_char_boundary(
            remaining,
            remaining.len().min(UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES),
        );
        let take = if take == 0 {
            remaining
                .char_indices()
                .nth(1)
                .map_or(remaining.len(), |(index, _)| index)
        } else {
            take
        };
        chunks.push(remaining[..take].to_string());
        remaining = &remaining[take..];
    }
    chunks
}

#[allow(dead_code)]
fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn is_raw_apply_patch_body(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed.starts_with("*** Begin Patch") && trimmed.contains("*** End Patch")
}

fn apply_patch_command(
    command: &str,
    cwd: &std::path::Path,
) -> Option<(std::path::PathBuf, String)> {
    let trimmed = command.trim();
    if let Some(argv) = shlex::split(trimmed)
        && let [cmd, patch_text] = argv.as_slice()
        && (cmd == "apply_patch" || cmd == "applypatch")
    {
        return Some((cwd.to_path_buf(), patch_text.clone()));
    }

    let (effective_cwd, script) = if let Some((cd_command, rest)) = trimmed.split_once("&&") {
        let argv = shlex::split(cd_command.trim())?;
        match argv.as_slice() {
            [cmd, dir] if cmd == "cd" => {
                let path = std::path::PathBuf::from(dir);
                let path = if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                };
                (path, rest.trim())
            }
            _ => (cwd.to_path_buf(), trimmed),
        }
    } else {
        (cwd.to_path_buf(), trimmed)
    };

    let mut lines = script.lines();
    let first_line = lines.next()?.trim();
    let command_name = first_line.split_whitespace().next()?;
    if command_name != "apply_patch" && command_name != "applypatch" {
        return None;
    }
    let heredoc_index = first_line.find("<<")?;
    let delimiter = first_line[heredoc_index + 2..].trim();
    let delimiter = delimiter
        .strip_prefix('-')
        .unwrap_or(delimiter)
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    if delimiter.is_empty() {
        return None;
    }

    let mut patch_lines = Vec::new();
    while let Some(line) = lines.next() {
        if line.trim() == delimiter {
            if lines.any(|remaining| !remaining.trim().is_empty()) {
                return None;
            }
            return Some((effective_cwd, patch_lines.join("\n")));
        }
        patch_lines.push(line);
    }

    None
}

#[cfg(test)]
mod tests {
    include!("exec_cancellation_tests.rs");
    use super::*;
    use devo_tools::contracts::ToolBudgets;
    use pretty_assertions::assert_eq;

    fn test_exec_handler() -> ExecCommandHandler {
        let process_store = Arc::new(ProcessStore::new());
        let background_tasks = Arc::new(BackgroundTaskStore::new(Arc::clone(&process_store)));
        ExecCommandHandler::new(process_store, background_tasks)
    }
    use tokio_util::sync::CancellationToken;

    fn result_text_and_metadata(
        content: &crate::contracts::ToolResultContent,
    ) -> (String, Option<&serde_json::Value>) {
        match content {
            crate::contracts::ToolResultContent::Text(text) => (text.clone(), None),
            crate::contracts::ToolResultContent::Json(json) => (json.to_string(), Some(json)),
            crate::contracts::ToolResultContent::Mixed { text, json } => {
                (text.clone().unwrap_or_default(), json.as_ref())
            }
        }
    }

    fn test_ctx(cwd: std::path::PathBuf) -> crate::contracts::ToolContext {
        crate::contracts::ToolContext {
            tool_call_id: crate::invocation::ToolCallId("test".into()),
            session_id: "test-session".into(),
            turn_id: Some("test-turn".into()),
            workspace_root: cwd,
            // permission_profile: crate::contracts::ToolPermissionProfile {
            //     can_read_workspace: true,
            //     can_write_workspace: true,
            //     can_execute_commands: true,
            //     network_enabled: true,
            // },
            // tool_registry: std::sync::Arc::new(crate::contracts::NoopToolRegistry),
            budgets: ToolBudgets {
                wall_time_limit_ms: Some(6_000),
                output_limit_bytes: 32 * 1024,
            },
            cancel_token: CancellationToken::new(),
            agent_scope: crate::contracts::ToolAgentScope::Parent,
            collaboration_mode: devo_protocol::CollaborationMode::Build,
            agent_coordinator: None,
            client_filesystem: None,
            file_read_ledger: None,
            network_proxy: None,
            network_no_proxy: None,
            sandbox_permission_overlay: None,
            sandbox_profile: None,
        }
    }

    #[test]
    fn format_exec_response_exited() {
        let output = ProcessOutput {
            output: "hello world".into(),
            exit_code: Some(0),
            wall_time_secs: 1.5,
            truncated: false,
            original_token_count: 3,
        };
        let text = format_exec_response(&output, None, /*warning*/ None);
        assert_eq!(text, "Process exited with code 0\nhello world");
    }

    #[test]
    fn format_exec_response_running() {
        let output = ProcessOutput {
            output: "building...".into(),
            exit_code: None,
            wall_time_secs: 10.0,
            truncated: false,
            original_token_count: 3,
        };
        let text = format_exec_response(&output, Some(42), /*warning*/ None);
        assert_eq!(text, "Process running with process ID 42\nbuilding...");
    }

    #[test]
    fn format_exec_response_truncated() {
        let output = ProcessOutput {
            output: "long output...".into(),
            exit_code: None,
            wall_time_secs: 5.0,
            truncated: true,
            original_token_count: 3,
        };
        let text = format_exec_response(&output, Some(1), /*warning*/ None);
        assert_eq!(text, "Process running with process ID 1\nlong output...");
    }

    #[test]
    fn format_exec_response_with_both_exit_and_process_id() {
        let output = ProcessOutput {
            output: "done".into(),
            exit_code: Some(0),
            wall_time_secs: 3.0,
            truncated: false,
            original_token_count: 1,
        };
        let text = format_exec_response(&output, Some(99), /*warning*/ None);
        assert_eq!(text, "Process exited with code 0\ndone");
    }

    #[test]
    fn format_exec_response_includes_open_process_warning() {
        let output = ProcessOutput {
            output: "building...".into(),
            exit_code: None,
            wall_time_secs: 10.0,
            truncated: false,
            original_token_count: 3,
        };

        let text = format_exec_response(
            &output,
            Some(42),
            Some(&open_process_warning(WARNING_PROCESSES)),
        );

        assert_eq!(
            text,
            format!(
                "Process running with process ID 42\n{}\nbuilding...",
                open_process_warning(WARNING_PROCESSES)
            )
        );
    }

    #[tokio::test]
    async fn write_stdin_accepts_legacy_session_id() {
        let handler = WriteStdinHandler::new(Arc::new(ProcessStore::new()));
        let error = handler
            .handle(
                test_ctx(std::env::temp_dir()),
                serde_json::json!({ "session_id": 42, "chars": "" }),
                /*_progress*/ None,
            )
            .await
            .expect_err("unknown legacy process id");
        let ToolCallError::ExecutionFailed(message) = error else {
            panic!("legacy session_id should be parsed as a process id");
        };

        assert_eq!(message, "Unknown process id 42");
    }

    #[tokio::test]
    async fn write_stdin_prefers_process_id_over_legacy_session_id() {
        let handler = WriteStdinHandler::new(Arc::new(ProcessStore::new()));
        let error = handler
            .handle(
                test_ctx(std::env::temp_dir()),
                serde_json::json!({
                    "process_id": 42,
                    "session_id": 99,
                    "chars": ""
                }),
                /*_progress*/ None,
            )
            .await
            .expect_err("unknown process id");
        let ToolCallError::ExecutionFailed(message) = error else {
            panic!("process_id should take precedence over legacy session_id");
        };

        assert_eq!(message, "Unknown process id 42");
    }

    #[test]
    fn progress_delta_chunks_caps_chunk_size_on_utf8_boundary() {
        let text = "a".repeat(UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES - 1) + "😀tail";

        let chunks = progress_delta_chunks(text.as_bytes());

        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].len() <= UNIFIED_EXEC_OUTPUT_DELTA_MAX_BYTES);
        assert_eq!(chunks.join(""), text);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exec_command_streams_progress_before_final_output() {
        let root = std::env::temp_dir().join(format!("devo-exec-stream-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp test dir");
        let handler = test_exec_handler();

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({
                    "cmd": "printf 'first\\n'; sleep 0.05; printf 'second\\n'",
                    "login": false,
                    "yield_time_ms": 1000,
                    "max_output_tokens": 1000,
                }),
                None,
            )
            .await
            .expect("handle exec command");

        let (text, _) = result_text_and_metadata(&output.content);
        assert!(
            text.contains("first"),
            "output should contain initial output: {text:?}"
        );
        assert!(
            text.contains("second"),
            "output should contain final output: {text:?}"
        );
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }

    #[test]
    fn exec_command_args_missing_cmd() {
        let args = serde_json::json!({});
        let result = serde_json::from_value::<serde_json::Value>(args);
        assert!(result.is_ok());
        // The cmd field is required but we can't easily test parse failure
        // because there's no deserialize impl for ExecCommandArgs
    }

    #[test]
    fn apply_patch_command_extracts_heredoc() {
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nPATCH\n";

        let parsed = apply_patch_command(command, std::path::Path::new("/tmp/root"));

        assert_eq!(
            parsed,
            Some((
                std::path::PathBuf::from("/tmp/root"),
                "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch".to_string()
            ))
        );
    }

    #[test]
    fn apply_patch_command_extracts_cd_heredoc() {
        let command = "cd sub && apply_patch <<EOF\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nEOF";

        let parsed = apply_patch_command(command, std::path::Path::new("/tmp/root"));

        assert_eq!(
            parsed,
            Some((
                std::path::PathBuf::from("/tmp/root/sub"),
                "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch".to_string()
            ))
        );
    }

    #[test]
    fn apply_patch_command_extracts_direct_body() {
        let command =
            "apply_patch '*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch'";

        let parsed = apply_patch_command(command, std::path::Path::new("/tmp/root"));

        assert_eq!(
            parsed,
            Some((
                std::path::PathBuf::from("/tmp/root"),
                "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch".to_string()
            ))
        );
    }

    #[test]
    fn apply_patch_command_rejects_trailing_commands_after_heredoc() {
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nPATCH\necho done";

        assert_eq!(
            apply_patch_command(command, std::path::Path::new("/tmp/root")),
            None
        );
    }

    #[tokio::test]
    async fn exec_command_rejects_raw_apply_patch_body() {
        let root = std::env::temp_dir().join(format!("devo-apply-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp test dir");
        let handler = test_exec_handler();
        let command = "*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\n";

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({ "cmd": command }),
                None,
            )
            .await
            .expect("handle exec command");

        assert!(matches!(
            output.structured_status,
            crate::contracts::ToolTerminalStatus::Failed(_)
        ));
        let text = match &output.content {
            crate::contracts::ToolResultContent::Text(t) => t.as_str(),
            _ => "",
        };
        assert!(text.contains("patch detected without explicit call to apply_patch"));
        assert!(!root.join("file.txt").exists());
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }

    #[tokio::test]
    async fn exec_command_intercepts_apply_patch_heredoc() {
        let root = std::env::temp_dir().join(format!("devo-apply-patch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp test dir");
        let handler = test_exec_handler();
        let command = "apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: file.txt\n+hello\n*** End Patch\nPATCH\n";

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({ "cmd": command }),
                None,
            )
            .await
            .expect("handle exec command");

        let (text, metadata) = result_text_and_metadata(&output.content);
        assert!(text.starts_with("Success. Updated the following files:"));
        assert!(text.contains("Success. Updated the following files:"));
        assert!(!text.contains("\"diagnostics\""));
        assert_eq!(
            metadata
                .and_then(|json| json.get("files"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).expect("read patched file"),
            "hello\n"
        );
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }

    #[tokio::test]
    async fn exec_command_intercepts_apply_patch_after_cd() {
        let root = std::env::temp_dir().join(format!("devo-apply-patch-{}", Uuid::new_v4()));
        let subdir = root.join("sub");
        std::fs::create_dir_all(&subdir).expect("create temp test dir");
        let handler = test_exec_handler();
        let command = "cd sub && apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: nested.txt\n+hello\n*** End Patch\nPATCH\n";

        let output = handler
            .handle(
                test_ctx(root.clone()),
                serde_json::json!({ "cmd": command }),
                None,
            )
            .await
            .expect("handle exec command");

        let (text, metadata) = result_text_and_metadata(&output.content);
        assert!(text.starts_with("Success. Updated the following files:"));
        assert!(text.contains("Success. Updated the following files:"));
        assert!(!text.contains("\"diagnostics\""));
        assert_eq!(
            metadata
                .and_then(|json| json.get("files"))
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            std::fs::read_to_string(subdir.join("nested.txt")).expect("read patched file"),
            "hello\n"
        );
        std::fs::remove_dir_all(root).expect("cleanup temp test dir");
    }
}
