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
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_YIELD_TIME_MS: u64 = 1_000;
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 16_000;
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

pub(crate) struct ShellExecRequest {
    pub command: String,
    pub workdir: PathBuf,
    pub description: String,
    pub shell_override: Option<String>,
    pub tty: bool,
    pub login: bool,
    pub timeout_ms: u64,
    pub yield_time_ms: u64,
    pub max_output_tokens: usize,
    pub sandbox_profile: Option<String>,
}

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
            let _ = child.kill();
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
            let _ = child.kill();
        }
    }
}

pub(crate) fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

pub(crate) fn default_yield_time_ms() -> u64 {
    DEFAULT_YIELD_TIME_MS
}

pub(crate) fn default_max_output_tokens() -> usize {
    DEFAULT_MAX_OUTPUT_TOKENS
}

#[allow(dead_code)]
pub(crate) fn windows_destructive_filesystem_guidance() -> &'static str {
    r#"Windows safety rules:
- Do not compose destructive filesystem commands across shells. Do not enumerate paths in PowerShell and then pass them to `cmd /c`, batch builtins, or another shell for deletion or moving. Use one shell end-to-end, prefer native PowerShell cmdlets such as `Remove-Item` / `Move-Item` with `-LiteralPath`, and avoid string-built shell commands for file operations.
- Before any recursive delete or move on Windows, verify the resolved absolute target paths stay within the intended workspace or explicitly named target directory. Never issue a recursive delete or move against a computed path if the final target has not been checked."#
}

#[allow(dead_code)]
pub(crate) fn shell_command_description() -> String {
    if cfg!(windows) {
        format!(
            r#"Runs a Powershell command (Windows) and returns its output.

Examples of valid command strings:

- ls -a (show hidden): "Get-ChildItem -Force"
- recursive find by name: "Get-ChildItem -Recurse -Filter *.py"
- recursive grep: "Get-ChildItem -Path C:\myrepo -Recurse | Select-String -Pattern 'TODO' -CaseSensitive"
- ps aux | grep python: "Get-Process | Where-Object {{ $_.ProcessName -like '*python*' }}"
- setting an env var: "$env:FOO='bar'; echo $env:FOO"
- running an inline Python script: "@'\nprint('Hello, world!')\n'@ | python -"

{}"#,
            windows_destructive_filesystem_guidance()
        )
    } else {
        "Runs a shell command and returns its output.\n- Always set the `workdir` param when using the shell_command function. Do not use `cd` unless absolutely necessary.".to_string()
    }
}

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

    info!(command = %command_to_run, shell = shell.program, "executing shell command");
    let command_preview = preview(&command_to_run);

    // Linux pipe spawns compose a bwrap wrapper with the pre_exec sandbox when
    // the profile needs enforcement Landlock cannot express (deny paths,
    // network restriction); everything else runs unwrapped.
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

    #[cfg(unix)]
    {
        let sandbox_workspace = workdir.clone();
        let helper_enforces = matches!(
            &sandbox_wrap,
            devo_sandbox::SandboxWrap::Wrapped(wrapped) if wrapped.helper_enforces
        );
        let sandbox_plan = if helper_enforces {
            None
        } else {
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
        };
        unsafe {
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

async fn run_with_pty(
    config: PtyRunConfig,
    progress: Option<ToolProgressSender>,
    cancel_token: CancellationToken,
) -> anyhow::Result<FunctionToolOutput> {
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

    // PTY spawns have no pre_exec hook: enforce the profile by wrapping the
    // command in the OS sandbox launcher (macOS sandbox-exec, Linux bwrap).
    // The wrapped child must NOT also apply the profile (no nested sandboxes).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContent;
    use pretty_assertions::assert_eq;
    use std::hint::black_box;
    use std::time::Instant;

    #[tokio::test]
    async fn execute_shell_command_non_tty_sends_progress() {
        let cmd = "echo stream_test";
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let result = execute_shell_command(
            ShellExecRequest {
                command: cmd.to_string(),
                workdir: std::env::current_dir().unwrap_or_default(),
                description: "test".into(),
                shell_override: None,
                tty: false,
                login: false,
                timeout_ms: 5000,
                yield_time_ms: 100,
                max_output_tokens: 100,
                sandbox_profile: None,
            },
            Some(tx),
            CancellationToken::new(),
        )
        .await;

        assert!(result.is_ok(), "command should succeed: {:?}", result.err());
        // Progress channel should have received output
        if let Ok(chunk) = rx.try_recv() {
            assert!(!chunk.is_empty(), "progress chunk should not be empty");
        }
    }

    #[tokio::test]
    async fn execute_shell_command_progress_none_does_not_crash() {
        let cmd = "echo test";
        let result = execute_shell_command(
            ShellExecRequest {
                command: cmd.to_string(),
                workdir: std::env::current_dir().unwrap_or_default(),
                description: "test".into(),
                shell_override: None,
                tty: false,
                login: false,
                timeout_ms: 5000,
                yield_time_ms: 100,
                max_output_tokens: 100,
                sandbox_profile: None,
            },
            None,
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_shell_command_cancels_non_tty_process() {
        let cancel_token = CancellationToken::new();
        let cancel_task_token = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_task_token.cancel();
        });

        let result = execute_shell_command(
            ShellExecRequest {
                command: "sleep 5; echo should_not_print".to_string(),
                workdir: std::env::current_dir().unwrap_or_default(),
                description: "cancel test".into(),
                shell_override: None,
                tty: false,
                login: false,
                timeout_ms: 10_000,
                yield_time_ms: 100,
                max_output_tokens: 100,
                sandbox_profile: None,
            },
            None,
            cancel_token,
        )
        .await
        .expect("execute shell command");

        assert!(result.is_error);
        assert_eq!(result.content.into_string(), "command cancelled");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn aborting_tty_command_kills_pty_child() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let started_marker = temp_dir.path().join("started");
        let delayed_marker = temp_dir.path().join("delayed");
        let quote_path = |path: &std::path::Path| {
            format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
        };
        let command = format!(
            "touch {}; sleep 2; touch {}",
            quote_path(&started_marker),
            quote_path(&delayed_marker)
        );
        let cancel_token = CancellationToken::new();
        let task_cancel_token = cancel_token.clone();
        let task = tokio::spawn(execute_shell_command(
            ShellExecRequest {
                command,
                workdir: temp_dir.path().to_path_buf(),
                description: "abort PTY test".into(),
                shell_override: Some("bash".to_string()),
                tty: true,
                login: false,
                timeout_ms: 10_000,
                yield_time_ms: 100,
                max_output_tokens: 100,
                sandbox_profile: None,
            },
            None,
            task_cancel_token,
        ));

        for _ in 0..50 {
            if started_marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(started_marker.exists(), "PTY command should have started");
        cancel_token.cancel();
        task.abort();
        let _ = task.await;
        tokio::time::sleep(Duration::from_millis(2_500)).await;

        assert!(
            !delayed_marker.exists(),
            "aborted PTY command should not reach delayed marker"
        );
    }

    #[tokio::test]
    async fn execute_shell_command_success_metadata_is_mixed() {
        let result = execute_shell_command(
            ShellExecRequest {
                command: "echo metadata_test".to_string(),
                workdir: std::env::current_dir().unwrap_or_default(),
                description: "metadata test".into(),
                shell_override: None,
                tty: false,
                login: false,
                timeout_ms: 5000,
                yield_time_ms: 100,
                max_output_tokens: 100,
                sandbox_profile: None,
            },
            None,
            CancellationToken::new(),
        )
        .await
        .expect("execute shell command");

        assert!(!result.is_error);
        match result.content {
            ToolContent::Mixed {
                text: Some(text),
                json: Some(metadata),
            } => {
                assert!(text.contains("metadata_test"));
                assert_eq!(metadata["description"], "metadata test");
            }
            content => panic!("expected mixed success output, got {content:?}"),
        }
    }

    #[tokio::test]
    async fn execute_shell_command_error_output_is_text_only() {
        let result = execute_shell_command(
            ShellExecRequest {
                command: "exit 7".to_string(),
                workdir: std::env::current_dir().unwrap_or_default(),
                description: "error test".into(),
                shell_override: None,
                tty: false,
                login: false,
                timeout_ms: 5000,
                yield_time_ms: 100,
                max_output_tokens: 100,
                sandbox_profile: None,
            },
            None,
            CancellationToken::new(),
        )
        .await
        .expect("execute shell command");

        assert!(result.is_error);
        assert!(matches!(result.content, ToolContent::Text(text) if text.contains("exit code 7")));
    }

    use super::{merge_streams, platform_shell_program, preview, resolve_shell, truncate_output};

    #[test]
    #[cfg(windows)]
    fn resolve_shell_prefers_powershell_alias() {
        let spec = resolve_shell(Some("pwsh"), true);
        assert_eq!(spec.program, "powershell");
        assert_eq!(spec.args, &["-NoLogo", "-NoProfile", "-Command"]);
    }

    #[test]
    #[cfg(windows)]
    fn resolve_shell_prefers_cmd_alias() {
        let spec = resolve_shell(Some("cmd.exe"), true);
        assert_eq!(spec.program, "cmd");
        assert_eq!(spec.args, &["/C"]);
    }

    #[test]
    fn resolve_shell_defaults_to_platform_shell_login() {
        let spec = resolve_shell(None, true);
        assert_eq!(spec.program, platform_shell_program(true));
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
    fn truncate_output_preserves_utf8_boundaries() {
        assert_eq!(truncate_output("😀😀😀", 1), "😀😀😀");
        assert_eq!(
            truncate_output("😀😀😀😀😀", 1),
            "😀😀😀😀\n\n... [truncated]"
        );
    }

    #[test]
    #[ignore]
    fn bench_truncate_output_ascii_no_truncation() {
        let input = "shell output line\n".repeat(256);
        let iterations = 200_000;
        let expected_len = input.len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(truncate_output(black_box(&input), black_box(2_000))).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "truncate_output_ascii_no_truncation iterations={iterations} bytes={expected_len} elapsed_ms={} per_call_us={:.2}",
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore]
    fn bench_truncate_output_ascii_large_truncation() {
        let input = "shell output line\n".repeat(8_192);
        let iterations = 50_000;
        let expected_len = truncate_output(&input, 1_000).len();
        let started = Instant::now();
        let mut total_len = 0usize;

        for _ in 0..iterations {
            total_len += black_box(truncate_output(black_box(&input), black_box(1_000))).len();
        }

        let elapsed = started.elapsed();
        assert_eq!(total_len, expected_len * iterations);
        println!(
            "truncate_output_ascii_large_truncation iterations={iterations} bytes={} elapsed_ms={} per_call_us={:.2}",
            input.len(),
            elapsed.as_secs_f64() * 1_000.0,
            elapsed.as_secs_f64() * 1_000_000.0 / iterations as f64
        );
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
        assert_eq!(merge_streams("", ""), "");
    }
}
