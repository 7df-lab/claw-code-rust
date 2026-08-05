use devo_protocol::approx_bytes_for_tokens;
use devo_protocol::approx_tokens_from_byte_count;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Instant;

use devo_sandbox::SandboxPermissionOverlay;
use devo_util_process::SpawnedProcess;
pub use devo_util_process::TerminalSize;
use devo_util_process::combine_output_receivers;
use devo_util_process::spawn_process_no_stdin_sandboxed_with_overlay;
use devo_util_process::spawn_pty_process;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};

use super::ProcessOutput;
use super::buffer::HeadTailBuffer;

const PTY_ROWS: u16 = 24;
const PTY_COLS: u16 = 120;
const PTY_TRAILING_OUTPUT_GRACE_MS: u64 = 150;
const POWERSHELL_UTF8_OUTPUT_PREFIX: &str =
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;\n";
const UNIFIED_EXEC_ENV: [(&str, &str); 10] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("LANG", "C.UTF-8"),
    ("LC_CTYPE", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("DEVO_CI", "1"),
];

/// Max time (in seconds) a PTY process can live without any write_stdin interaction.
const IDLE_TIMEOUT_SECS: u64 = 1800;

#[derive(Debug, PartialEq, Eq)]
struct ShellSpec {
    program: String,
    args: Vec<String>,
}

fn resolve_shell(shell_override: Option<&str>, login: bool) -> ShellSpec {
    let default_shell = if cfg!(windows) {
        "powershell".to_string()
    } else {
        std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| "bash".to_string())
    };
    resolve_shell_with_default(shell_override, login, &default_shell)
}

fn resolve_shell_with_default(
    shell_override: Option<&str>,
    login: bool,
    default_shell: &str,
) -> ShellSpec {
    if let Some(shell) = shell_override {
        return ShellSpec {
            program: shell.to_string(),
            args: shell_args(shell, login),
        };
    }

    ShellSpec {
        program: default_shell.to_string(),
        args: shell_args(default_shell, login),
    }
}

fn shell_args(shell: &str, login: bool) -> Vec<String> {
    let shell_name = shell_name(shell);

    if is_powershell_name(&shell_name) {
        let mut args = Vec::new();
        if !login {
            args.push("-NoProfile".to_string());
        }
        args.push("-Command".to_string());
        return args;
    }

    if shell_name == "cmd" {
        return vec!["/c".to_string()];
    }

    vec![if login { "-lc" } else { "-c" }.to_string()]
}

fn interactive_shell_args(shell: &str, login: bool) -> Vec<String> {
    let shell_name = shell_name(shell);

    if is_powershell_name(&shell_name) {
        if login {
            return Vec::new();
        }
        return vec!["-NoProfile".to_string()];
    }

    if shell_name == "cmd" {
        return Vec::new();
    }

    if login {
        vec!["-l".to_string()]
    } else {
        Vec::new()
    }
}

fn shell_name(shell: &str) -> String {
    Path::new(shell)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase()
}

fn is_powershell_name(name: &str) -> bool {
    name == "powershell" || name == "pwsh"
}

fn command_for_shell(cmd: &str, shell_spec: &ShellSpec) -> String {
    if !is_powershell_name(&shell_name(&shell_spec.program)) {
        return cmd.to_string();
    }
    let trimmed = cmd.trim_start();
    if trimmed.starts_with(POWERSHELL_UTF8_OUTPUT_PREFIX) {
        cmd.to_string()
    } else {
        format!("{POWERSHELL_UTF8_OUTPUT_PREFIX}{cmd}")
    }
}

pub struct UnifiedExecProcess {
    session: Arc<devo_util_process::ProcessHandle>,
    exit_code: Arc<AtomicI32>,
    terminated_flag: Arc<AtomicBool>,
    stdin_writer: Option<mpsc::Sender<Vec<u8>>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    output_buffer: Arc<AsyncMutex<HeadTailBuffer>>,
    last_stdin_interaction: Arc<Mutex<Instant>>,
    process_id: i32,
    tty: bool,
}

impl UnifiedExecProcess {
    pub async fn spawn(
        process_id: i32,
        cmd: &str,
        cwd: &Path,
        shell: Option<&str>,
        login: bool,
        tty: bool,
    ) -> Result<(Self, broadcast::Receiver<Vec<u8>>), String> {
        Self::spawn_with_sandbox(process_id, cmd, cwd, shell, login, tty, None).await
    }

    pub async fn spawn_with_sandbox(
        process_id: i32,
        cmd: &str,
        cwd: &Path,
        shell: Option<&str>,
        login: bool,
        tty: bool,
        sandbox_profile: Option<String>,
    ) -> Result<(Self, broadcast::Receiver<Vec<u8>>), String> {
        Self::spawn_with_sandbox_overlay(
            process_id,
            cmd,
            cwd,
            shell,
            login,
            tty,
            sandbox_profile,
            None,
        )
        .await
    }

    pub async fn spawn_with_sandbox_overlay(
        process_id: i32,
        cmd: &str,
        cwd: &Path,
        shell: Option<&str>,
        login: bool,
        tty: bool,
        sandbox_profile: Option<String>,
        sandbox_overlay: Option<SandboxPermissionOverlay>,
    ) -> Result<(Self, broadcast::Receiver<Vec<u8>>), String> {
        let shell_spec = resolve_shell(shell, login);
        let command = command_for_shell(cmd, &shell_spec);
        let mut args = shell_spec.args.clone();
        args.push(command);
        let env = unified_exec_env(cwd, sandbox_profile.as_deref());

        let spawned = if tty {
            spawn_pty_with_sandbox_wrap(
                &shell_spec.program,
                &args,
                cwd,
                &env,
                TerminalSize {
                    rows: PTY_ROWS,
                    cols: PTY_COLS,
                },
                sandbox_profile.as_deref(),
                sandbox_overlay.as_ref(),
            )
            .await?
        } else {
            spawn_process_no_stdin_sandboxed_with_overlay(
                &shell_spec.program,
                &args,
                cwd,
                &env,
                /*arg0*/ &None,
                &[],
                sandbox_profile,
                sandbox_overlay,
            )
            .await
            .map_err(|error| format!("failed to spawn command: {error}"))?
        };

        Ok(Self::from_spawned_process(process_id, tty, spawned))
    }

    pub async fn spawn_interactive_shell(
        process_id: i32,
        cwd: &Path,
        shell: Option<&str>,
        login: bool,
        size: Option<TerminalSize>,
        sandbox_profile: Option<String>,
    ) -> Result<(Self, broadcast::Receiver<Vec<u8>>), String> {
        Self::spawn_interactive_shell_with_overlay(
            process_id,
            cwd,
            shell,
            login,
            size,
            sandbox_profile,
            None,
        )
        .await
    }

    pub async fn spawn_interactive_shell_with_overlay(
        process_id: i32,
        cwd: &Path,
        shell: Option<&str>,
        login: bool,
        size: Option<TerminalSize>,
        sandbox_profile: Option<String>,
        sandbox_overlay: Option<SandboxPermissionOverlay>,
    ) -> Result<(Self, broadcast::Receiver<Vec<u8>>), String> {
        let shell_spec = resolve_shell(shell, login);
        let args = interactive_shell_args(&shell_spec.program, login);
        let env = unified_exec_env(cwd, sandbox_profile.as_deref());

        let spawned = spawn_pty_with_sandbox_wrap(
            &shell_spec.program,
            &args,
            cwd,
            &env,
            size.unwrap_or(TerminalSize {
                rows: PTY_ROWS,
                cols: PTY_COLS,
            }),
            sandbox_profile.as_deref(),
            sandbox_overlay.as_ref(),
        )
        .await?;

        Ok(Self::from_spawned_process(
            process_id, /*tty*/ true, spawned,
        ))
    }

    fn from_spawned_process(
        process_id: i32,
        tty: bool,
        spawned: SpawnedProcess,
    ) -> (Self, broadcast::Receiver<Vec<u8>>) {
        let SpawnedProcess {
            session,
            stdout_rx,
            stderr_rx,
            exit_rx,
        } = spawned;
        let session = Arc::new(session);
        let stdin_writer = tty.then(|| session.writer_sender());
        let output_buffer = Arc::new(AsyncMutex::new(HeadTailBuffer::new()));
        let (output_tx, proc_output_rx) = broadcast::channel(256);
        let combined_rx = combine_output_receivers(stdout_rx, stderr_rx);
        forward_output(combined_rx, output_tx.clone(), Arc::clone(&output_buffer));

        let exit_code = Arc::new(AtomicI32::new(-1));
        let terminated_flag = Arc::new(AtomicBool::new(false));
        let last_stdin_interaction = Arc::new(Mutex::new(Instant::now()));

        monitor_exit(
            exit_rx,
            Arc::clone(&exit_code),
            Arc::clone(&terminated_flag),
        );
        if tty {
            monitor_idle_timeout(
                Arc::clone(&session),
                Arc::clone(&terminated_flag),
                Arc::clone(&last_stdin_interaction),
            );
        }

        (
            UnifiedExecProcess {
                session,
                exit_code,
                terminated_flag,
                stdin_writer,
                output_tx,
                output_buffer,
                last_stdin_interaction,
                process_id,
                tty,
            },
            proc_output_rx,
        )
    }

    pub fn write_stdin(&self, chars: &str) -> Result<(), String> {
        let Some(writer) = self.stdin_writer.as_ref() else {
            return Err("stdin is closed for this session".to_string());
        };

        writer
            .try_send(stdin_bytes_for_pty(chars))
            .map_err(|error| format!("failed to write to stdin: {error}"))?;
        *self
            .last_stdin_interaction
            .lock()
            .map_err(|error| format!("lock error: {error}"))? = Instant::now();
        Ok(())
    }

    pub fn terminate(&self) {
        self.terminated_flag.store(true, Ordering::SeqCst);
        self.session.request_terminate();
    }

    pub fn close_stdin(&self) {
        self.session.close_stdin();
    }

    pub fn resize(&self, size: TerminalSize) -> Result<(), String> {
        self.session
            .resize(size)
            .map_err(|error| format!("failed to resize PTY: {error}"))
    }

    pub fn exit_code(&self) -> Option<i32> {
        let code = self.exit_code.load(Ordering::SeqCst);
        if code >= 0 {
            Some(code)
        } else {
            self.session.exit_code().filter(|code| *code >= 0)
        }
    }

    pub fn is_running(&self) -> bool {
        !self.terminated_flag.load(Ordering::SeqCst)
    }

    pub fn process_id(&self) -> i32 {
        self.process_id
    }

    pub fn tty(&self) -> bool {
        self.tty
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }
}

/// Spawn a PTY process, enforcing `sandbox_profile` by wrapping the command
/// in the OS sandbox launcher (macOS `sandbox-exec`, Linux `bwrap`). PTY
/// spawns have no `pre_exec` hook, so the wrapper carries the full profile
/// policy and the wrapped child must NOT also apply it (no nested sandboxes).
async fn spawn_pty_with_sandbox_wrap(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    size: TerminalSize,
    sandbox_profile: Option<&str>,
    sandbox_overlay: Option<&SandboxPermissionOverlay>,
) -> Result<SpawnedProcess, String> {
    let wrap = devo_sandbox::wrap_command_for_profile_with_overlay(
        sandbox_profile,
        cwd,
        devo_sandbox::WrapMode::PtyOnly,
        &devo_sandbox::SandboxLogger::new(),
        sandbox_overlay,
    )
    .map_err(|error| format!("failed to set up sandbox: {error}"))?;
    let (program, args) = match &wrap {
        devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
            let mut wrapped_args = wrapped.prefix_args.clone();
            wrapped_args.push(program.to_string());
            wrapped_args.extend(args.iter().cloned());
            (wrapped.program.clone(), wrapped_args)
        }
        devo_sandbox::SandboxWrap::None => (program.to_string(), args.to_vec()),
    };
    let spawned = spawn_pty_process(&program, &args, cwd, env, /*arg0*/ &None, size)
        .await
        .map_err(|error| format!("failed to spawn PTY command: {error}"))?;
    // bwrap mounts are not up when spawn returns, so the placeholder directory
    // must outlive the launch; remove it after a delay instead.
    if let devo_sandbox::SandboxWrap::Wrapped(wrapped) = &wrap
        && let Some(directory) = &wrapped.placeholder_dir
    {
        let directory = directory.clone();
        tokio::spawn(async move {
            sleep(devo_sandbox::PLACEHOLDER_CLEANUP_DELAY).await;
            devo_sandbox::remove_placeholder_dir(&directory);
        });
    }
    Ok(spawned)
}

fn unified_exec_env(cwd: &Path, sandbox_profile: Option<&str>) -> HashMap<String, String> {
    let mut env = std::env::vars().collect::<HashMap<_, _>>();
    for (key, value) in UNIFIED_EXEC_ENV {
        env.insert(key.to_string(), value.to_string());
    }
    if cfg!(windows) {
        env.insert("PYTHONUTF8".to_string(), "1".to_string());
    }
    #[cfg(unix)]
    for (key, value) in devo_sandbox::proxy_env_for_sandbox_profile(sandbox_profile, cwd) {
        env.insert(key, value);
    }
    #[cfg(not(unix))]
    let _ = (cwd, sandbox_profile);
    env
}

fn forward_output(
    mut output_rx: broadcast::Receiver<Vec<u8>>,
    output_tx: broadcast::Sender<Vec<u8>>,
    output_buffer: Arc<AsyncMutex<HeadTailBuffer>>,
) {
    tokio::spawn(async move {
        loop {
            match output_rx.recv().await {
                Ok(bytes) => {
                    output_buffer.lock().await.push(&bytes);
                    let _ = output_tx.send(bytes);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

fn monitor_exit(
    mut exit_rx: tokio::sync::oneshot::Receiver<i32>,
    exit_code: Arc<AtomicI32>,
    terminated_flag: Arc<AtomicBool>,
) {
    tokio::spawn(async move {
        let code = (&mut exit_rx).await.unwrap_or(-1);
        sleep(Duration::from_millis(PTY_TRAILING_OUTPUT_GRACE_MS)).await;
        exit_code.store(code, Ordering::SeqCst);
        terminated_flag.store(true, Ordering::SeqCst);
    });
}

fn monitor_idle_timeout(
    session: Arc<devo_util_process::ProcessHandle>,
    terminated_flag: Arc<AtomicBool>,
    last_stdin_interaction: Arc<Mutex<Instant>>,
) {
    tokio::spawn(async move {
        let idle_timeout = Duration::from_secs(IDLE_TIMEOUT_SECS);
        loop {
            if terminated_flag.load(Ordering::SeqCst) {
                break;
            }

            let idle_for = last_stdin_interaction
                .lock()
                .map(|last| last.elapsed())
                .unwrap_or(idle_timeout);
            if idle_for >= idle_timeout {
                session.request_terminate();
                terminated_flag.store(true, Ordering::SeqCst);
                break;
            }

            sleep(Duration::from_millis(100)).await;
        }
    });
}

fn stdin_bytes_for_pty(chars: &str) -> Vec<u8> {
    #[cfg(windows)]
    {
        let mut bytes = Vec::with_capacity(chars.len());
        let mut previous_was_cr = false;
        for byte in chars.bytes() {
            if byte == b'\n' {
                if !previous_was_cr {
                    bytes.push(b'\r');
                }
                bytes.push(b'\n');
                previous_was_cr = false;
                continue;
            }
            bytes.push(byte);
            previous_was_cr = byte == b'\r';
        }
        bytes
    }

    #[cfg(not(windows))]
    {
        chars.as_bytes().to_vec()
    }
}

impl Drop for UnifiedExecProcess {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub async fn collect_output(
    output_rx: &mut broadcast::Receiver<Vec<u8>>,
    process: &UnifiedExecProcess,
    yield_time_ms: u64,
    max_output_tokens: usize,
) -> ProcessOutput {
    let started = Instant::now();
    let mut collected = Vec::new();
    let deadline = Duration::from_millis(yield_time_ms);

    loop {
        {
            let mut pending = process.output_buffer.lock().await;
            collected.extend_from_slice(&pending.drain_collect_bytes());
        }

        loop {
            match output_rx.try_recv() {
                Ok(_bytes) => {}
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => {
                    let _ = output_rx.try_recv();
                    break;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            }
        }

        let done = !process.is_running() || (process.exit_code().is_some() && output_rx.is_empty());

        if done {
            sleep(Duration::from_millis(50)).await;
            {
                let mut pending = process.output_buffer.lock().await;
                collected.extend_from_slice(&pending.drain_collect_bytes());
            }
            while let Ok(bytes) = output_rx.try_recv() {
                let _ = bytes;
            }
            break;
        }

        if started.elapsed() >= deadline {
            break;
        }

        sleep(Duration::from_millis(10)).await;
    }

    let original_token_count = approximate_token_count(collected.len());
    let raw_output = String::from_utf8_lossy(&collected).to_string();
    let (output, truncated) = formatted_truncate_tokens(&raw_output, max_output_tokens);

    ProcessOutput {
        output,
        exit_code: process.exit_code(),
        wall_time_secs: started.elapsed().as_secs_f64(),
        truncated,
        original_token_count,
    }
}

fn approximate_token_count(byte_len: usize) -> usize {
    approx_tokens_from_byte_count(byte_len)
        .try_into()
        .unwrap_or(usize::MAX)
}

fn formatted_truncate_tokens(content: &str, max_output_tokens: usize) -> (String, bool) {
    let max_bytes = approx_bytes_for_tokens(max_output_tokens);
    if content.len() <= max_bytes {
        return (content.to_string(), false);
    }

    let total_lines = content.lines().count();
    let truncated = truncate_middle_with_token_marker(content, max_bytes);
    (
        format!("Total output lines: {total_lines}\n\n{truncated}"),
        true,
    )
}

fn truncate_middle_with_token_marker(content: &str, max_bytes: usize) -> String {
    if max_bytes == 0 {
        return format!(
            "…{} tokens truncated…",
            approximate_token_count(content.len())
        );
    }

    let head_budget = max_bytes / 2;
    let tail_budget = max_bytes.saturating_sub(head_budget);
    let head_end = floor_char_boundary(content, head_budget);
    let tail_start = ceil_char_boundary(content, content.len().saturating_sub(tail_budget));
    let omitted_bytes = tail_start.saturating_sub(head_end);
    format!(
        "{}…{} tokens truncated…{}",
        &content[..head_end],
        approximate_token_count(omitted_bytes),
        &content[tail_start..]
    )
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[tokio::test]
    async fn process_spawn_and_exit() {
        let cmd = "echo hello";
        let (proc, mut rx) = UnifiedExecProcess::spawn(
            1,
            cmd,
            Path::new("."),
            /*shell*/ None,
            /*login*/ false,
            /*tty*/ false,
        )
        .await
        .expect("spawn should succeed");

        let mut waited = 0u64;
        while proc.is_running() && waited < 3000 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            waited += 100;
        }

        let _output = collect_output(&mut rx, &proc, 1000, 1000).await;
        if !proc.is_running() {
            assert!(
                proc.exit_code().is_some(),
                "process should have an exit code"
            );
        }
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn process_non_tty_captures_output_without_early_subscription() {
        let (proc, mut rx) = UnifiedExecProcess::spawn(
            4,
            "printf buffered-output",
            Path::new("."),
            /*shell*/ None,
            /*login*/ false,
            /*tty*/ false,
        )
        .await
        .expect("spawn should succeed");

        tokio::time::sleep(Duration::from_millis(100)).await;

        let output = collect_output(&mut rx, &proc, 250, 1000).await;

        assert_eq!(output.output, "buffered-output");
        assert_eq!(output.exit_code, Some(0));
    }

    #[tokio::test]
    async fn process_non_tty_rejects_stdin_write() {
        let (proc, _rx) = UnifiedExecProcess::spawn(
            5,
            "echo test",
            Path::new("."),
            /*shell*/ None,
            /*login*/ false,
            /*tty*/ false,
        )
        .await
        .expect("spawn should succeed");

        assert_eq!(
            proc.write_stdin("input\n"),
            Err("stdin is closed for this session".to_string())
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn process_non_tty_applies_devo_unified_exec_env() {
        let (proc, mut rx) = UnifiedExecProcess::spawn(
            6,
            "printf '%s|%s|%s' \"$NO_COLOR\" \"$TERM\" \"$PAGER\"",
            Path::new("."),
            /*shell*/ None,
            /*login*/ false,
            /*tty*/ false,
        )
        .await
        .expect("spawn should succeed");

        let output = collect_output(&mut rx, &proc, 1000, 1000).await;

        assert_eq!(output.output, "1|dumb|cat");
        assert_eq!(output.exit_code, Some(0));
    }

    #[test]
    fn formatted_truncate_tokens_keeps_head_tail_and_line_count() {
        let content = "alpha beta gamma delta epsilon\nzeta eta theta iota kappa";

        let (output, truncated) = formatted_truncate_tokens(content, 5);

        assert!(truncated);
        assert!(output.starts_with("Total output lines: 2\n\nalpha"));
        assert!(output.contains("tokens truncated"));
        assert!(output.ends_with("iota kappa"));
    }

    #[test]
    fn formatted_truncate_tokens_preserves_utf8_boundaries() {
        let content = "😀😀😀😀😀😀😀😀😀😀";

        let (output, truncated) = formatted_truncate_tokens(content, 2);

        assert!(truncated);
        assert!(output.contains("tokens truncated"));
    }

    #[test]
    fn resolve_shell_uses_user_shell_default_and_login_args() {
        assert_eq!(
            resolve_shell_with_default(
                /*shell_override*/ None, /*login*/ true, "/bin/zsh"
            ),
            ShellSpec {
                program: "/bin/zsh".to_string(),
                args: vec!["-lc".to_string()],
            }
        );
        assert_eq!(
            resolve_shell_with_default(
                /*shell_override*/ None, /*login*/ false, "/bin/zsh"
            ),
            ShellSpec {
                program: "/bin/zsh".to_string(),
                args: vec!["-c".to_string()],
            }
        );
    }

    #[test]
    fn resolve_shell_uses_powershell_profile_only_for_login() {
        assert_eq!(
            resolve_shell_with_default(
                /*shell_override*/ Some("pwsh"),
                /*login*/ true,
                "/bin/zsh",
            ),
            ShellSpec {
                program: "pwsh".to_string(),
                args: vec!["-Command".to_string()],
            }
        );
        assert_eq!(
            resolve_shell_with_default(
                /*shell_override*/ Some("pwsh"),
                /*login*/ false,
                "/bin/zsh",
            ),
            ShellSpec {
                program: "pwsh".to_string(),
                args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            }
        );
    }

    #[test]
    fn command_for_shell_prefixes_powershell_utf8_output() {
        let shell_spec = ShellSpec {
            program: "pwsh".to_string(),
            args: vec!["-Command".to_string()],
        };

        assert_eq!(
            command_for_shell("Write-Output hi", &shell_spec),
            format!("{POWERSHELL_UTF8_OUTPUT_PREFIX}Write-Output hi")
        );
        assert_eq!(
            command_for_shell(
                &format!("{POWERSHELL_UTF8_OUTPUT_PREFIX}Write-Output hi"),
                &shell_spec,
            ),
            format!("{POWERSHELL_UTF8_OUTPUT_PREFIX}Write-Output hi")
        );
    }

    #[test]
    fn command_for_shell_leaves_posix_shell_unchanged() {
        let shell_spec = ShellSpec {
            program: "/bin/zsh".to_string(),
            args: vec!["-lc".to_string()],
        };

        assert_eq!(command_for_shell("echo hi", &shell_spec), "echo hi");
    }

    #[cfg(windows)]
    #[test]
    fn stdin_bytes_for_windows_pty_uses_carriage_return() {
        assert_eq!(stdin_bytes_for_pty("Alice\n"), b"Alice\r\n");
        assert_eq!(stdin_bytes_for_pty("Alice\r\n"), b"Alice\r\n");
    }

    #[tokio::test]
    async fn process_terminate_works() {
        if cfg!(target_os = "linux") {
            let (proc, _rx) = UnifiedExecProcess::spawn(
                2,
                "sleep 60",
                Path::new("."),
                /*shell*/ None,
                /*login*/ false,
                /*tty*/ true,
            )
            .await
            .expect("spawn should succeed");
            assert!(proc.is_running());

            proc.terminate();
            let mut waited = 0u64;
            while proc.is_running() && waited < 5000 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                waited += 100;
            }

            assert!(!proc.is_running(), "process should have been terminated");
        }
    }

    #[tokio::test]
    async fn process_write_stdin_before_exit() {
        if cfg!(target_os = "linux") {
            let (proc, _rx) = UnifiedExecProcess::spawn(
                3,
                "cat",
                Path::new("."),
                /*shell*/ None,
                /*login*/ false,
                /*tty*/ true,
            )
            .await
            .expect("spawn should succeed");

            tokio::time::sleep(Duration::from_millis(300)).await;

            *proc
                .last_stdin_interaction
                .lock()
                .expect("last stdin interaction lock should not be poisoned") =
                Instant::now() - Duration::from_secs(60);
            let result = proc.write_stdin("test data\n");
            assert!(result.is_ok(), "write_stdin failed: {result:?}");
            let idle_for = proc
                .last_stdin_interaction
                .lock()
                .expect("last stdin interaction lock should not be poisoned")
                .elapsed();
            assert!(idle_for < Duration::from_secs(1));
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn process_windows_pty_echo_exits() {
        let (proc, mut rx) = UnifiedExecProcess::spawn(
            4,
            "Write-Output unified-pty-ok",
            Path::new("."),
            /*shell*/ Some("powershell"),
            /*login*/ false,
            /*tty*/ true,
        )
        .await
        .expect("spawn should succeed");

        let output = collect_output(&mut rx, &proc, 5_000, 1_000).await;

        assert_eq!(output.exit_code, Some(0));
        assert!(output.output.contains("unified-pty-ok"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn process_windows_pty_read_host_accepts_stdin() {
        let (proc, mut rx) = UnifiedExecProcess::spawn(
            5,
            "Write-Host \"Enter name:\"; $name = Read-Host; Write-Host \"Hello, $name\"",
            Path::new("."),
            /*shell*/ Some("powershell"),
            /*login*/ false,
            /*tty*/ true,
        )
        .await
        .expect("spawn should succeed");

        let initial = collect_output(&mut rx, &proc, 2_000, 1_000).await;
        assert!(initial.output.contains("Enter name:"));

        proc.write_stdin("Alice\n")
            .expect("stdin write should work");
        let output = collect_output(&mut rx, &proc, 5_000, 1_000).await;

        assert_eq!(output.exit_code, Some(0));
        assert!(
            output.output.contains("Hello, Alice"),
            "missing greeting in output: {:?}",
            output.output
        );
    }
}
