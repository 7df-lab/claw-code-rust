use devo_protocol::approx_bytes_for_tokens;
use portable_pty::{Child, CommandBuilder, ExitStatus, PtySize, native_pty_system};
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Instant;
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::events::ToolProgressSender;
use crate::invocation::FunctionToolOutput;

const MAX_METADATA_LENGTH: usize = 30_000;
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub(crate) const DEFAULT_YIELD_TIME_MS: u64 = 1_000;
pub(crate) const DEFAULT_MAX_OUTPUT_TOKENS: usize = 16_000;
const TRUNCATED_SUFFIX: &str = "\n\n... [truncated]";

#[cfg(not(unix))]
fn try_windows_sandbox_launch(
    sandbox_profile: Option<&str>,
    workdir: &std::path::Path,
    shell: &ShellSpec,
    command: &str,
) -> anyhow::Result<Option<devo_windows_sandbox::WindowsSandboxLaunch>> {
    use std::sync::Once;
    if !devo_windows_sandbox::should_wrap_profile(sandbox_profile) {
        return Ok(None);
    }
    let profile = sandbox_profile.expect("checked by should_wrap_profile");
    let profile_name = profile
        .parse::<devo_sandbox::ProfileName>()
        .map_err(|error| anyhow::anyhow!("invalid sandbox profile '{profile}': {error}"))?;
    let config = devo_sandbox::load_sandbox_config(workdir)?;
    let resolved = profile_name.resolve_profile(workdir, &config)?;
    let request = devo_windows_sandbox::WindowsSandboxRequest {
        command: command.to_string(),
        shell_program: shell.program.to_string(),
        shell_args: shell.args.iter().map(|arg| arg.to_string()).collect(),
        cwd: workdir.to_path_buf(),
        readable_roots: resolved.read_only,
        writable_roots: resolved.read_write,
        deny_read: resolved.deny,
        restrict_network: resolved.restrict_network,
    };
    match devo_windows_sandbox::prepare_windows_sandbox_launch(&request)? {
        Some(launch) => Ok(Some(launch)),
        None => {
            static WARNED: Once = Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "Windows sandbox profile is active but launch preparation is not wired yet; \
                     commands run unwrapped"
                );
            });
            Ok(None)
        }
    }
}

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

/// Resolved arguments for [`run_with_pty`] after `ShellExecRequest` has been
/// normalized: shell override/login → [`ShellSpec`], and the command possibly
/// rewritten (e.g. PowerShell UTF-8 prelude). Does not carry `tty` /
/// `shell_override` / `login` because those are already applied.
struct PtyRunConfig {
    shell: ShellSpec,
    command_to_run: String,
    workdir: PathBuf,
    description: String,
    timeout_ms: u64,
    yield_time_ms: u64,
    max_output_tokens: usize,
    sandbox_profile: Option<String>,
}

/// RAII guard around a PTY-spawned child process.
///
/// Ensures the child is killed if the guard is dropped while still armed
/// (timeout, cancel, or early return). Call [`Self::disarm`] after a clean
/// exit so [`Drop`] does not kill an already-reaped process.
struct PtyChildGuard {
    child: Option<Box<dyn Child + Send + Sync>>,
}

impl PtyChildGuard {
    /// Take ownership of `child` and keep the guard armed.
    fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self { child: Some(child) }
    }

    /// Non-blocking poll for exit status; panics if already disarmed.
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child
            .as_mut()
            .expect("PTY child guard must hold child while active")
            .try_wait()
    }

    /// Force-kill the child and wait for it to exit (best-effort).
    fn kill_and_wait(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Release ownership without killing; subsequent [`Drop`] is a no-op.
    fn disarm(mut self) {
        self.child.take();
    }
}

impl Drop for PtyChildGuard {
    /// Kill the child if the guard was dropped while still armed.
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }
}

/// Run a shell command from a [`ShellExecRequest`].
///
/// Resolves the shell and command, then either delegates to [`run_with_pty`]
/// when `tty` is set, or spawns a non-interactive pipe process (stdout/stderr
/// captured). Applies sandbox wrapping when a profile is set, waits for
/// completion (or cancel/timeout), and returns truncated tool output.
pub(crate) async fn execute_shell_command(
    request: ShellExecRequest,
    progress: Option<ToolProgressSender>,
    cancel_token: CancellationToken,
) -> anyhow::Result<FunctionToolOutput> {
    // --- Validate request & normalize shell/command ---
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
    // PowerShell often emits mojibake without an explicit UTF-8 console encoding.
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
        command
    };

    // --- PTY path (interactive / TTY) ---
    if tty {
        return run_with_pty(
            PtyRunConfig {
                shell,
                command_to_run,
                workdir,
                description,
                timeout_ms,
                yield_time_ms,
                max_output_tokens,
                sandbox_profile,
            },
            progress,
            cancel_token,
        )
        .await;
    }

    // --- Pipe path: sandbox wrap + build Command ---
    info!(command = %command_to_run, shell = shell.program, "executing shell command");
    let command_preview = preview(&command_to_run);

    // Unix (`cfg(unix)` covers Linux *and* macOS): decide whether to launch through
    // an OS sandbox wrapper. `wrap_command_for_profile` picks the launcher:
    // - macOS: `sandbox-exec` with a Seatbelt profile (full policy). Seatbelt is
    //   never applied via `pre_exec` after fork in a multithreaded process.
    // - Linux: Landlock/`pre_exec` usually carries the profile; `bwrap` is added
    //   only when PipeComposed needs what Landlock cannot express (deny paths,
    //   network restriction).
    // Windows uses the separate `try_windows_sandbox_launch` path below.
    #[cfg(unix)]
    let sandbox_wrap = match devo_sandbox::wrap_command_for_profile(
        sandbox_profile.as_deref(),
        &workdir,
        devo_sandbox::WrapMode::PipeComposed,
        &devo_sandbox::SandboxLogger::new(),
    ) {
        Ok(wrap) => wrap,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to set up sandbox: {error}"
            )));
        }
    };
    #[cfg(not(unix))]
    let sandbox_wrap = devo_sandbox::SandboxWrap::None;
    #[cfg(not(unix))]
    let windows_launch = match try_windows_sandbox_launch(
        sandbox_profile.as_deref(),
        &workdir,
        &shell,
        &command_to_run,
    ) {
        Ok(launch) => launch,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to set up Windows sandbox: {error}"
            )));
        }
    };

    // Prefer OS wrapper (`sandbox-exec` / `bwrap` / Windows launcher); else bare shell.
    let mut child = match &sandbox_wrap {
        devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
            let mut child = Command::new(&wrapped.program);
            child
                .args(&wrapped.prefix_args)
                .arg(shell.program)
                .args(shell.args)
                .arg(&command_to_run);
            child
        }
        devo_sandbox::SandboxWrap::None => {
            #[cfg(not(unix))]
            if let Some(launch) = &windows_launch {
                let mut child = Command::new(&launch.program);
                child.args(&launch.args);
                for (key, value) in &launch.env {
                    child.env(key, value);
                }
                child
            } else {
                let mut child = Command::new(shell.program);
                child.args(shell.args).arg(&command_to_run);
                child
            }
            #[cfg(unix)]
            {
                let mut child = Command::new(shell.program);
                child.args(shell.args).arg(&command_to_run);
                child
            }
        }
    };
    child
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(&workdir)
        .kill_on_drop(true);

    // --- Apply in-process sandbox (Unix pre_exec) and env ---
    #[cfg(unix)]
    {
        let sandbox_workspace = workdir.clone();
        // `requires_child_apply` is false on macOS (Seatbelt is only via
        // `sandbox-exec`) and when a Linux wrapper already enforces the full
        // policy. Otherwise resolve Landlock/seccomp for `pre_exec`.
        let sandbox_plan = if sandbox_wrap.requires_child_apply() {
            match devo_util_process::sandbox::resolve_profile_for_spawn(
                sandbox_profile.as_deref(),
                &sandbox_workspace,
            ) {
                Ok(plan) => plan,
                Err(error) => {
                    return Ok(FunctionToolOutput::error(format!(
                        "failed to resolve sandbox profile: {error}"
                    )));
                }
            }
        } else {
            None
        };
        unsafe {
            // `pre_exec` runs in the child after `fork`, before `exec`. Apply the
            // parent-resolved Landlock/seccomp plan here so only the spawned
            // command is sandboxed (parent stays unrestricted). Config must not
            // be loaded in this hook — resolve above in the parent. Skipped when
            // `sandbox_plan` is `None` (macOS / fully wrapped Linux).
            child.pre_exec(move || {
                devo_util_process::sandbox::apply_resolved_in_child(sandbox_plan.as_ref())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = &sandbox_profile;

    if cfg!(windows) {
        child.env("PYTHONUTF8", "1");
    }

    #[cfg(unix)]
    apply_sandbox_proxy_env(&mut child, sandbox_profile.as_deref(), &workdir);

    // --- Spawn and schedule sandbox placeholder cleanup ---
    let spawned = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to spawn process: {error}"
            )));
        }
    };
    // bwrap mounts are not up when spawn returns, so the placeholder directory
    // must outlive the launch; remove it after a delay instead.
    if let devo_sandbox::SandboxWrap::Wrapped(wrapped) = &sandbox_wrap
        && let Some(directory) = &wrapped.placeholder_dir
    {
        let directory = directory.clone();
        tokio::spawn(async move {
            tokio::time::sleep(devo_sandbox::PLACEHOLDER_CLEANUP_DELAY).await;
            devo_sandbox::remove_placeholder_dir(&directory);
        });
    }

    // --- Wait for exit, cancel, or timeout ---
    let result = tokio::select! {
        result = timeout(Duration::from_millis(timeout_ms), spawned.wait_with_output()) => result,
        _ = cancel_token.cancelled() => {
            return Ok(FunctionToolOutput::error("command cancelled"));
        }
    };

    // --- Build success / error tool output ---
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

#[cfg(test)]
pub(crate) fn platform_shell_program(login: bool) -> &'static str {
    platform_shell(login).program
}

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

#[cfg(unix)]
fn apply_sandbox_proxy_env(
    child: &mut Command,
    sandbox_profile: Option<&str>,
    workdir: &std::path::Path,
) {
    for (key, value) in devo_sandbox::proxy_env_for_sandbox_profile(sandbox_profile, workdir) {
        child.env(key, value);
    }
}

/// Run a command attached to a pseudo-terminal (PTY).
///
/// Used when [`ShellExecRequest::tty`] is true. Opens a PTY, optionally wraps
/// the spawn in an OS sandbox launcher (no `pre_exec` on this path), reads
/// master output on a background thread, and polls the child until exit,
/// timeout, or cancel. Returns truncated tool output with TTY metadata.
async fn run_with_pty(
    config: PtyRunConfig,
    progress: Option<ToolProgressSender>,
    cancel_token: CancellationToken,
) -> anyhow::Result<FunctionToolOutput> {
    // --- Open PTY ---
    let PtyRunConfig {
        shell,
        command_to_run,
        workdir,
        description,
        timeout_ms,
        yield_time_ms,
        max_output_tokens,
        sandbox_profile,
    } = config;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| anyhow::anyhow!("failed to open PTY: {error}"))?;

    // --- Sandbox wrap (OS launcher only; no nested in-child apply) ---
    // PTY spawns have no `pre_exec` hook. Unix: `wrap_command_for_profile(PtyOnly)`
    // wraps with macOS `sandbox-exec` or Linux `bwrap` carrying the full profile.
    // Windows: `try_windows_sandbox_launch` below. Do not also apply the profile
    // in-process (no nested sandboxes).
    #[cfg(unix)]
    let sandbox_wrap = match devo_sandbox::wrap_command_for_profile(
        sandbox_profile.as_deref(),
        &workdir,
        devo_sandbox::WrapMode::PtyOnly,
        &devo_sandbox::SandboxLogger::new(),
    ) {
        Ok(wrap) => wrap,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to set up sandbox: {error}"
            )));
        }
    };
    #[cfg(not(unix))]
    let sandbox_wrap = devo_sandbox::SandboxWrap::None;
    #[cfg(not(unix))]
    let windows_launch = match try_windows_sandbox_launch(
        sandbox_profile.as_deref(),
        &workdir,
        &shell,
        &command_to_run,
    ) {
        Ok(launch) => launch,
        Err(error) => {
            return Ok(FunctionToolOutput::error(format!(
                "failed to set up Windows sandbox: {error}"
            )));
        }
    };
    #[cfg(not(unix))]
    let _ = sandbox_profile;

    // --- Build CommandBuilder (wrapper or bare shell) ---
    let mut builder = match &sandbox_wrap {
        devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
            let mut builder = CommandBuilder::new(&wrapped.program);
            builder.args(&wrapped.prefix_args);
            builder.arg(shell.program);
            builder
        }
        devo_sandbox::SandboxWrap::None => {
            #[cfg(not(unix))]
            if let Some(launch) = &windows_launch {
                let mut builder = CommandBuilder::new(&launch.program);
                builder.args(
                    launch
                        .args
                        .iter()
                        .map(|arg| arg.as_str())
                        .collect::<Vec<_>>(),
                );
                for (key, value) in &launch.env {
                    builder.env(key, value);
                }
                builder
            } else {
                CommandBuilder::new(shell.program)
            }
            #[cfg(unix)]
            CommandBuilder::new(shell.program)
        }
    };
    // Windows sandbox launch already embeds the full command line.
    #[cfg(not(unix))]
    if windows_launch.is_none() {
        builder.args(shell.args);
        builder.arg(&command_to_run);
    }
    #[cfg(unix)]
    {
        builder.args(shell.args);
        builder.arg(&command_to_run);
    }
    builder.cwd(&workdir);
    if cfg!(windows) {
        builder.env("PYTHONUTF8", "1");
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
    }
    #[cfg(unix)]
    for (key, value) in
        devo_sandbox::proxy_env_for_sandbox_profile(sandbox_profile.as_deref(), &workdir)
    {
        builder.env(key, value);
    }

    // --- Spawn on slave, guard child, drop slave fd ---
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| anyhow::anyhow!("failed to spawn PTY command: {error}"))?;
    // bwrap mounts are not up when spawn returns, so the placeholder directory
    // must outlive the launch; remove it after a delay instead.
    if let devo_sandbox::SandboxWrap::Wrapped(wrapped) = &sandbox_wrap
        && let Some(directory) = &wrapped.placeholder_dir
    {
        let directory = directory.clone();
        tokio::spawn(async move {
            tokio::time::sleep(devo_sandbox::PLACEHOLDER_CLEANUP_DELAY).await;
            devo_sandbox::remove_placeholder_dir(&directory);
        });
    }
    let mut child = PtyChildGuard::new(child);
    drop(pair.slave);

    // --- Background reader: master → channel ---
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

    // --- Poll loop: drain output, wait for exit / timeout / cancel ---
    let started = Instant::now();
    let sleep_ms = yield_time_ms.max(10);
    let timeout = Duration::from_millis(timeout_ms);
    let mut output = Vec::new();
    let mut exit_code = None;
    let mut timed_out = false;
    let mut cancelled = false;

    loop {
        // Non-blocking drain so progress can stream while the child still runs.
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

    // --- Final drain + tool result ---
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
    // Clean exit: release ownership so Drop does not kill a finished process.
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

#[cfg(test)]
mod tests;
