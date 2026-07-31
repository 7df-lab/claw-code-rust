//! Non-interactive pipe spawn path for shell_exec.

use std::process::Stdio;

use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::time::Duration;
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
/// Unix signal-aware sandbox errors). On cancel/timeout, already-written
/// stdout/stderr are drained and included in the error text (same shape as
/// the PTY path).
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

    // Own process group so cancel/timeout can SIGKILL the shell *and* its
    // descendants (e.g. `sleep`); otherwise pipes stay open until children exit.
    #[cfg(unix)]
    child.process_group(0);

    if cfg!(windows) {
        child.env("PYTHONUTF8", "1");
    }

    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to spawn process: {error}"
            )));
        }
    };
    plan.schedule_placeholder_cleanup();

    let stdout_task = spawn_stream_reader(child.stdout.take(), progress.clone());
    let stderr_task = spawn_stream_reader(child.stderr.take(), progress);

    enum WaitOutcome {
        Exited(std::process::ExitStatus),
        Cancelled,
        TimedOut,
        WaitError(std::io::Error),
    }

    let outcome = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => WaitOutcome::Exited(status),
            Err(error) => WaitOutcome::WaitError(error),
        },
        _ = cancel_token.cancelled() => WaitOutcome::Cancelled,
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => WaitOutcome::TimedOut,
    };

    match &outcome {
        WaitOutcome::Cancelled | WaitOutcome::TimedOut => {
            kill_and_wait(&mut child).await;
        }
        WaitOutcome::Exited(_) | WaitOutcome::WaitError(_) => {}
    }

    let stdout = String::from_utf8_lossy(&stdout_task.await.unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_task.await.unwrap_or_default()).into_owned();
    let result_text = truncate_output(&merge_streams(&stdout, &stderr), max_output_tokens);

    match outcome {
        WaitOutcome::Cancelled => Ok(FunctionToolOutput::error(format!(
            "command cancelled\n{result_text}"
        ))),
        WaitOutcome::TimedOut => Ok(FunctionToolOutput::error(format!(
            "command timed out after {timeout_ms}ms\n{result_text}"
        ))),
        WaitOutcome::WaitError(error) => Ok(FunctionToolOutput::error(format!(
            "failed to spawn process: {error}"
        ))),
        WaitOutcome::Exited(status) => {
            if status.success() {
                Ok(FunctionToolOutput::success_with_metadata(
                    result_text.clone(),
                    json!({
                        "output": preview(&result_text),
                        "command": command_preview,
                        "exit": status.code(),
                        "description": description,
                        "cwd": workdir,
                        "yield_time_ms": yield_time_ms,
                    }),
                ))
            } else {
                #[cfg(unix)]
                let unix_signal = {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal()
                };
                #[cfg(not(unix))]
                let unix_signal: Option<i32> = None;
                let error_message = devo_sandbox::shell_error_message_with_signal(
                    sandbox_profile.as_deref(),
                    status.code(),
                    unix_signal,
                    &stdout,
                    &stderr,
                    &result_text,
                );
                Ok(FunctionToolOutput::error(error_message))
            }
        }
    }
}

fn spawn_stream_reader<R>(
    pipe: Option<R>,
    progress: Option<ToolProgressSender>,
) -> tokio::task::JoinHandle<Vec<u8>>
where
    R: AsyncReadExt + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = Vec::new();
        let Some(mut pipe) = pipe else {
            return buffer;
        };
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    if let Some(ref sender) = progress {
                        let _ = sender.send(String::from_utf8_lossy(&chunk[..n]).into_owned());
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                Err(_) => break,
            }
        }
        buffer
    })
}

async fn kill_and_wait(child: &mut Child) {
    let _ = devo_util_process::process_group::kill_child_process_group(child);
    let _ = child.start_kill();
    let _ = child.wait().await;
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
