//! PTY-backed shell execution path.

use std::sync::mpsc;
use std::time::Instant;

use portable_pty::{Child, ExitStatus, PtySize, native_pty_system};
use serde_json::json;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::events::ToolProgressSender;
use crate::invocation::FunctionToolOutput;

use super::launch::SandboxLaunchPlan;
use super::preview;
use super::resolve::ResolvedShellRun;
use super::truncate_output;

/// RAII guard around a PTY-spawned child process.
///
/// Ensures the child is killed if the guard is dropped while still armed
/// (timeout, cancel, or early return). Call [`Self::disarm`] after a clean
/// exit so [`Drop`] does not kill an already-reaped process.
struct PtyChildGuard {
    child: Option<Box<dyn Child + Send + Sync>>,
}

impl PtyChildGuard {
    fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child
            .as_mut()
            .expect("PTY child guard must hold child while active")
            .try_wait()
    }

    fn kill_and_wait(&mut self) {
        if let Some(child) = self.child.as_mut() {
            kill_pty_child(child);
            let _ = child.wait();
        }
    }

    fn disarm(mut self) {
        self.child.take();
    }
}

impl Drop for PtyChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            kill_pty_child(child);
        }
    }
}

/// Kill the PTY child and its process group.
///
/// `portable-pty` on Unix already runs `setsid()` in the child, so the shell is
/// the session/process-group leader. A direct `Child::kill` only targets that
/// PID; descendants such as `sleep` keep the PTY slave open. Signal the whole
/// group first, then fall back to the direct kill.
fn kill_pty_child(child: &mut Box<dyn Child + Send + Sync>) {
    if let Some(pid) = child.process_id() {
        let _ = devo_util_process::process_group::kill_process_group_by_pid(pid);
    }
    let _ = child.kill();
}

/// Run a command attached to a pseudo-terminal (PTY).
///
/// Opens a PTY, applies [`SandboxLaunchPlan::prepare_pty`], reads master output
/// on a background thread, and polls until exit, timeout, or cancel. Formats
/// PTY-specific tool output (single stream, `tty: true` metadata).
pub(crate) async fn run_with_pty(
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

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| anyhow::anyhow!("failed to open PTY: {error}"))?;

    // PTY spawns have no `pre_exec` hook. Unix: wrap with macOS `sandbox-exec`
    // or Linux `bwrap` carrying the full profile. Windows: launcher via plan.
    let plan = match SandboxLaunchPlan::prepare_pty(
        sandbox_profile.as_deref(),
        &workdir,
        &shell,
        &command_to_run,
    ) {
        Ok(plan) => plan,
        Err(error) => return Ok(FunctionToolOutput::error(error)),
    };

    let mut builder = plan.build_pty_command_builder(&shell, &command_to_run);
    builder.cwd(&workdir);
    if cfg!(windows) {
        builder.env("PYTHONUTF8", "1");
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
    }

    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| anyhow::anyhow!("failed to spawn PTY command: {error}"))?;
    plan.schedule_placeholder_cleanup();
    let mut child = PtyChildGuard::new(child);
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
    let mut cancelled = false;

    loop {
        while let Ok(chunk) = rx.try_recv() {
            output.extend_from_slice(&chunk);
            if let Some(ref sender) = progress {
                let text = String::from_utf8_lossy(&chunk).into_owned();
                let _ = sender.send(text);
            }
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
            child.kill_and_wait();
            break;
        }

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {}
            _ = cancel_token.cancelled() => {
                cancelled = true;
                child.kill_and_wait();
                break;
            }
        }
    }

    while let Ok(chunk) = rx.try_recv() {
        output.extend_from_slice(&chunk);
    }

    let mut text = String::from_utf8_lossy(&output).into_owned();
    text = truncate_output(&text, max_output_tokens);

    if timed_out {
        return Ok(FunctionToolOutput::error(format!(
            "command timed out after {timeout_ms}ms\n{text}"
        )));
    }
    if cancelled {
        return Ok(FunctionToolOutput::error(format!(
            "command cancelled\n{text}"
        )));
    }
    child.disarm();

    let is_error = exit_code.unwrap_or(1) != 0;
    let content = if is_error {
        let code = exit_code.unwrap_or(-1);
        devo_sandbox::shell_error_message(sandbox_profile.as_deref(), code, &text, "", &text)
    } else {
        text.clone()
    };
    if is_error {
        return Ok(FunctionToolOutput::error(content));
    }

    Ok(FunctionToolOutput::success_with_metadata(
        content,
        json!({
            "output": preview(&text),
            "command": command_to_run,
            "exit": exit_code,
            "description": description,
            "cwd": workdir,
            "yield_time_ms": yield_time_ms,
            "tty": true,
        }),
    ))
}
