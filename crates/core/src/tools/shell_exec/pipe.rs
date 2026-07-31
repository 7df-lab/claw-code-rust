//! Non-interactive pipe spawn path for shell_exec.

use std::process::Stdio;

use serde_json::json;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::events::ToolProgressSender;
use crate::invocation::FunctionToolOutput;

use super::launch::SandboxLaunchPlan;
use super::preview;
use super::resolve::ResolvedShellRun;
use super::truncate_output;

/// Run a command with piped stdout/stderr (non-TTY).
///
/// Applies [`SandboxLaunchPlan::prepare_pipe`], waits for completion (or
/// cancel/timeout), and formats pipe-specific tool output (merged streams,
/// Unix signal-aware sandbox errors).
pub(crate) async fn run_with_pipes(
    run: ResolvedShellRun,
    progress: Option<ToolProgressSender>,
    cancel_token: CancellationToken,
) -> anyhow::Result<FunctionToolOutput> {
    let ResolvedShellRun {
        shell,
        command_to_run,
        workdir,
        description,
        timeout_ms,
        yield_time_ms,
        max_output_tokens,
        sandbox_profile,
    } = run;

    info!(command = %command_to_run, shell = shell.program, "executing shell command");
    let command_preview = preview(&command_to_run);

    let plan = match SandboxLaunchPlan::prepare_pipe(
        sandbox_profile.as_deref(),
        &workdir,
        &shell,
        &command_to_run,
    ) {
        Ok(plan) => plan,
        Err(error) => return Ok(FunctionToolOutput::error(error)),
    };

    let mut child = plan.build_tokio_command(&shell, &command_to_run);
    child
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&workdir)
        .kill_on_drop(true);

    if cfg!(windows) {
        child.env("PYTHONUTF8", "1");
    }

    let spawned = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to spawn process: {error}"
            )));
        }
    };
    plan.schedule_placeholder_cleanup();

    let result = tokio::select! {
        result = timeout(Duration::from_millis(timeout_ms), spawned.wait_with_output()) => result,
        _ = cancel_token.cancelled() => {
            return Ok(FunctionToolOutput::error("command cancelled"));
        }
    };

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let result_text = merge_streams(&stdout, &stderr);
            if let Some(ref sender) = progress {
                let _ = sender.send(result_text.clone());
            }
            let result_text = truncate_output(&result_text, max_output_tokens);
            if output.status.success() {
                Ok(FunctionToolOutput::success_with_metadata(
                    result_text.clone(),
                    json!({
                        "output": preview(&result_text),
                        "command": command_preview,
                        "exit": output.status.code(),
                        "description": description,
                        "cwd": workdir,
                        "yield_time_ms": yield_time_ms,
                    }),
                ))
            } else {
                #[cfg(unix)]
                let unix_signal = {
                    use std::os::unix::process::ExitStatusExt;
                    output.status.signal()
                };
                #[cfg(not(unix))]
                let unix_signal: Option<i32> = None;
                let error_message = devo_sandbox::shell_error_message_with_signal(
                    sandbox_profile.as_deref(),
                    output.status.code(),
                    unix_signal,
                    &stdout,
                    &stderr,
                    &result_text,
                );
                Ok(FunctionToolOutput::error(error_message))
            }
        }
        Ok(Err(error)) => Ok(FunctionToolOutput::error(format!(
            "failed to spawn process: {error}"
        ))),
        Err(_) => Ok(FunctionToolOutput::error(format!(
            "command timed out after {timeout_ms}ms"
        ))),
    }
}

pub(crate) fn merge_streams(stdout: &str, stderr: &str) -> String {
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
    result
}
