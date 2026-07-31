//! Local shell command execution (pipe and PTY).
//!
//! # Layout
//! - [`resolve`]: shell binary / command normalization
//! - [`launch`]: shared [`SandboxLaunchPlan`] (wrap / pre_exec / placeholder)
//! - [`pipe`]: non-TTY spawn + pipe-specific result formatting
//! - [`pty`]: TTY spawn + PTY-specific result formatting

mod launch;
mod pipe;
mod pty;
mod resolve;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use devo_protocol::approx_bytes_for_tokens;
use tokio_util::sync::CancellationToken;

use crate::events::ToolProgressSender;
use crate::invocation::FunctionToolOutput;

#[cfg(test)]
pub(crate) use launch::SandboxLaunchPlan;
#[cfg(test)]
pub(crate) use resolve::platform_shell_program;
use resolve::{normalize_command_for_shell, resolve_shell};

use pipe::run_with_pipes;
use pty::run_with_pty;
use resolve::ResolvedShellRun;

const MAX_METADATA_LENGTH: usize = 30_000;
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub(crate) const DEFAULT_YIELD_TIME_MS: u64 = 1_000;
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: usize = 16_000;
const TRUNCATED_SUFFIX: &str = "\n\n... [truncated]";

/// Input to [`execute_shell_command`]: the caller's raw request before shell
/// resolution or pipe/PTY branching.
///
/// `shell_override` / `login` select the interpreter; `tty` chooses the
/// execution path. Shared runtime knobs (workdir, timeouts, sandbox, …) are
/// forwarded into whichever path runs.
pub(crate) struct ShellExecRequest {
    pub command: String,
    pub workdir: PathBuf,
    pub description: String,
    /// Optional shell name/alias (`bash`, `pwsh`, `cmd`, …). `None` uses the
    /// platform default.
    pub shell_override: Option<String>,
    /// When true, run under a PTY via [`run_with_pty`]; otherwise pipe spawn.
    pub tty: bool,
    /// Prefer login-shell args (e.g. `bash -lc`) when resolving the shell.
    pub login: bool,
    pub timeout_ms: u64,
    pub yield_time_ms: u64,
    pub max_output_tokens: usize,
    pub sandbox_profile: Option<String>,
}

/// Run a shell command from a [`ShellExecRequest`].
///
/// Resolves the shell and command, then delegates to [`run_with_pty`] or
/// [`run_with_pipes`]. Applies sandbox wrapping when a profile is set.
pub(crate) async fn execute_shell_command(
    request: ShellExecRequest,
    progress: Option<ToolProgressSender>,
    cancel_token: CancellationToken,
) -> anyhow::Result<FunctionToolOutput> {
    let ShellExecRequest {
        command,
        workdir,
        description,
        shell_override,
        tty,
        login,
        timeout_ms,
        yield_time_ms,
        max_output_tokens,
        sandbox_profile,
    } = request;

    if !workdir.exists() {
        return Ok(FunctionToolOutput::error(format!(
            "working directory does not exist: {}",
            workdir.display()
        )));
    }

    let shell = resolve_shell(shell_override.as_deref(), login);
    let command_to_run = normalize_command_for_shell(&shell, command);
    let run = ResolvedShellRun {
        shell,
        command_to_run,
        workdir,
        description,
        timeout_ms,
        yield_time_ms,
        max_output_tokens,
        sandbox_profile,
    };

    if tty {
        run_with_pty(run, progress, cancel_token).await
    } else {
        run_with_pipes(run, progress, cancel_token).await
    }
}

// TODO: Preview truncation belongs on the client, not the server. Move
// `preview` (and callers that stuff truncated text into tool metadata) to
// the client side so the server returns full output / structured metadata.
pub(crate) fn preview(text: &str) -> String {
    if text.len() <= MAX_METADATA_LENGTH {
        return text.to_string();
    }
    format!("{}\n\n...", &text[..MAX_METADATA_LENGTH])
}

pub(crate) fn truncate_output(text: &str, max_output_tokens: usize) -> String {
    if max_output_tokens == 0 {
        return String::new();
    }
    let max_chars = approx_bytes_for_tokens(max_output_tokens);
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    if out.len() < text.len() {
        out.push_str(TRUNCATED_SUFFIX);
    }
    out
}
