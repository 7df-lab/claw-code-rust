//! Non-interactive pipe spawn path for shell_exec.

use std::process::Stdio;

use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::events::ToolProgressSender;
use crate::invocation::FunctionToolOutput;

use super::launch::SandboxLaunchPlan;
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
        output_capture,
        shell,
        command_to_run,
        workdir,
        description,
        timeout_ms,
        yield_time_ms,
        max_output_tokens,
        sandbox_profile,
        sandbox_permission_overlay,
    } = run;

    info!(command = %command_to_run, shell = shell.program, "executing shell command");

    let plan = match SandboxLaunchPlan::prepare_pipe(
        sandbox_profile.as_deref(),
        sandbox_permission_overlay.as_ref(),
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

    let mut command = process_wrap::tokio::CommandWrap::from(child);
    #[cfg(windows)]
    command.wrap(devo_util_process::windows_job::OwnedJob);
    let mut child = match command.spawn() {
        Ok(child) => super::pipe_child::PipeChild(child),
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to spawn process: {error}"
            )));
        }
    };
    plan.schedule_placeholder_cleanup();

    let stdout_task = spawn_stream_reader(
        child.0.stdout().take(),
        progress.clone(),
        output_capture.clone(),
    );
    let stderr_task =
        spawn_stream_reader(child.0.stderr().take(), progress, output_capture.clone());

    enum WaitOutcome {
        Exited(std::process::ExitStatus),
        Cancelled,
        TimedOut,
        WaitError(std::io::Error),
    }

    let outcome = tokio::select! {
        status = child.0.wait() => match status {
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
    let mut result_text = truncate_output(&merge_streams(&stdout, &stderr), max_output_tokens);
    super::append_capture_notice(&mut result_text, &output_capture);

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
                        "command": command_to_run,
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
    output_capture: Option<Result<devo_tools::output_store::SharedOutputCapture, String>>,
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
                    if let Some(Ok(capture)) = &output_capture {
                        let mut capture = capture.lock().expect("output capture lock");
                        if capture.append(&chunk[..n]).is_err() {
                            capture.mark_incomplete();
                        }
                    }
                    let keep = n.min((1024_usize * 1024).saturating_sub(buffer.len()));
                    buffer.extend_from_slice(&chunk[..keep]);
                }
                Err(_) => break,
            }
        }
        buffer
    })
}

async fn kill_and_wait(child: &mut super::pipe_child::PipeChild) {
    child.terminate_tree();
    let _ = child.0.wait().await;
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
