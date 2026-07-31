//! Pipe-backed process spawning for non-interactive commands.
//!
//! This backend keeps stdout and stderr split for callers that need structured
//! output while still sharing the same `ProcessHandle` lifecycle as PTY-backed
//! sessions.

use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::process::ChildTerminator;
use super::process::ProcessHandle;
use super::process::SpawnedProcess;

#[cfg(target_os = "linux")]
use libc;

struct PipeChildTerminator {
    #[cfg(windows)]
    pid: u32,
    #[cfg(unix)]
    process_group_id: u32,
}

impl ChildTerminator for PipeChildTerminator {
    fn kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            super::process_group::kill_process_group(self.process_group_id)
        }

        #[cfg(windows)]
        {
            kill_process(self.pid)
        }

        #[cfg(not(any(unix, windows)))]
        {
            Ok(())
        }
    }
}

#[cfg(windows)]
fn kill_process(pid: u32) -> io::Result<()> {
    unsafe {
        let handle = winapi::um::processthreadsapi::OpenProcess(
            winapi::um::winnt::PROCESS_TERMINATE,
            0,
            pid,
        );
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let success = winapi::um::processthreadsapi::TerminateProcess(handle, 1);
        let err = io::Error::last_os_error();
        winapi::um::handleapi::CloseHandle(handle);
        if success == 0 { Err(err) } else { Ok(()) }
    }
}

async fn read_output_stream<R>(mut reader: R, output_tx: mpsc::Sender<Vec<u8>>)
where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; 8_192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let _ = output_tx.send(buf[..n].to_vec()).await;
            }
            Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[derive(Clone, Copy)]
enum PipeStdinMode {
    Piped,
    Null,
}

#[allow(clippy::too_many_arguments)]
async fn spawn_process_with_stdin_mode(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
    stdin_mode: PipeStdinMode,
    inherited_fds: &[i32],
    sandbox_profile: Option<String>,
) -> Result<SpawnedProcess> {
    if program.is_empty() {
        anyhow::bail!("missing program for pipe spawn");
    }

    #[cfg(not(unix))]
    let _ = inherited_fds;

    // Linux pipe spawns compose a bwrap wrapper with the pre_exec sandbox when
    // the profile needs enforcement Landlock cannot express (deny paths,
    // network restriction); everything else runs unwrapped.
    #[cfg(unix)]
    let sandbox_wrap = devo_sandbox::wrap_command_for_profile(
        sandbox_profile.as_deref(),
        cwd,
        devo_sandbox::WrapMode::PipeComposed,
        &devo_sandbox::SandboxLogger::new(),
    )?;
    #[cfg(not(unix))]
    let sandbox_wrap = devo_sandbox::SandboxWrap::None;

    let (program, args): (String, Vec<String>) = match &sandbox_wrap {
        devo_sandbox::SandboxWrap::Wrapped(wrapped) => {
            let mut wrapped_args = wrapped.prefix_args.clone();
            wrapped_args.push(program.to_string());
            wrapped_args.extend(args.iter().cloned());
            (wrapped.program.clone(), wrapped_args)
        }
        devo_sandbox::SandboxWrap::None => (program.to_string(), args.to_vec()),
    };

    let mut command = Command::new(&program);
    #[cfg(unix)]
    if let Some(arg0) = arg0 {
        // argv[0] overrides only apply to a direct exec; a sandbox launcher
        // owns argv[0] of a wrapped invocation.
        if matches!(sandbox_wrap, devo_sandbox::SandboxWrap::None) {
            command.arg0(arg0);
        }
    }
    #[cfg(target_os = "linux")]
    let parent_pid = unsafe { libc::getpid() };
    #[cfg(unix)]
    let inherited_fds = inherited_fds.to_vec();
    #[cfg(unix)]
    let sandbox_workspace = cwd.to_path_buf();
    #[cfg(unix)]
    let sandbox_plan = if sandbox_wrap.requires_child_apply() {
        crate::sandbox::resolve_profile_for_spawn(sandbox_profile.as_deref(), &sandbox_workspace)?
    } else {
        None
    };
    #[cfg(unix)]
    unsafe {
        command.pre_exec(move || {
            super::process_group::detach_from_tty()?;
            #[cfg(target_os = "linux")]
            super::process_group::set_parent_death_signal(parent_pid)?;
            super::backend::close_inherited_fds_except(&inherited_fds);
            crate::sandbox::apply_resolved_in_child(sandbox_plan.as_ref())?;
            Ok(())
        });
    }
    #[cfg(not(unix))]
    let _ = (arg0, sandbox_profile);
    command.current_dir(cwd);
    command.env_clear();
    for (key, value) in env {
        command.env(key, value);
    }
    // Restricted profiles get ProxyOnly capability via wrap, but the child
    // still needs HTTP(S)_PROXY pointing at the managed localhost proxy.
    #[cfg(unix)]
    for (key, value) in devo_sandbox::proxy_env_for_sandbox_profile(sandbox_profile.as_deref(), cwd)
    {
        command.env(key, value);
    }
    for arg in &args {
        command.arg(arg);
    }
    match stdin_mode {
        PipeStdinMode::Piped => {
            command.stdin(Stdio::piped());
        }
        PipeStdinMode::Null => {
            command.stdin(Stdio::null());
        }
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn()?;
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
    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("missing child pid"))?;
    #[cfg(unix)]
    let process_group_id = pid;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);
    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(128);
    let (stderr_tx, stderr_rx) = mpsc::channel::<Vec<u8>>(128);
    let writer_handle = if let Some(stdin) = stdin {
        tokio::spawn(async move {
            let mut writer = stdin;
            while let Some(bytes) = writer_rx.recv().await {
                let _ = writer.write_all(&bytes).await;
                let _ = writer.flush().await;
            }
        })
    } else {
        drop(writer_rx);
        tokio::spawn(async {})
    };

    let stdout_handle = stdout.map(|stdout| {
        let stdout_tx = stdout_tx.clone();
        tokio::spawn(async move {
            read_output_stream(BufReader::new(stdout), stdout_tx).await;
        })
    });
    let stderr_handle = stderr.map(|stderr| {
        let stderr_tx = stderr_tx.clone();
        tokio::spawn(async move {
            read_output_stream(BufReader::new(stderr), stderr_tx).await;
        })
    });
    let mut reader_abort_handles = Vec::with_capacity(2);
    if let Some(handle) = stdout_handle.as_ref() {
        reader_abort_handles.push(handle.abort_handle());
    }
    if let Some(handle) = stderr_handle.as_ref() {
        reader_abort_handles.push(handle.abort_handle());
    }
    let reader_handle = tokio::spawn(async move {
        if let Some(handle) = stdout_handle {
            let _ = handle.await;
        }
        if let Some(handle) = stderr_handle {
            let _ = handle.await;
        }
    });

    let (exit_tx, exit_rx) = oneshot::channel::<i32>();
    let exit_status = Arc::new(AtomicBool::new(false));
    let wait_exit_status = Arc::clone(&exit_status);
    let exit_code = Arc::new(StdMutex::new(None));
    let wait_exit_code = Arc::clone(&exit_code);
    let wait_handle: JoinHandle<()> = tokio::spawn(async move {
        let code = match child.wait().await {
            Ok(status) => status.code().unwrap_or(-1),
            Err(_) => -1,
        };
        wait_exit_status.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut guard) = wait_exit_code.lock() {
            *guard = Some(code);
        }
        let _ = exit_tx.send(code);
    });

    let handle = ProcessHandle::new(
        writer_tx,
        Box::new(PipeChildTerminator {
            #[cfg(windows)]
            pid,
            #[cfg(unix)]
            process_group_id,
        }),
        reader_handle,
        reader_abort_handles,
        writer_handle,
        wait_handle,
        exit_status,
        exit_code,
        /*pty_handles*/ None,
        /*resizer*/ None,
    );

    Ok(SpawnedProcess {
        session: handle,
        stdout_rx,
        stderr_rx,
        exit_rx,
    })
}

/// Spawn a process using regular pipes (no PTY), returning handles for stdin, split output, and exit.
pub async fn spawn_process(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
) -> Result<SpawnedProcess> {
    spawn_process_with_stdin_mode(
        program,
        args,
        cwd,
        env,
        arg0,
        PipeStdinMode::Piped,
        &[],
        None,
    )
    .await
}

/// Spawn a process using regular pipes (no PTY), returning handles for stdin, split output, and exit.
/// The child process applies the named sandbox profile before `exec`.
pub async fn spawn_process_sandboxed(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
    sandbox_profile: Option<String>,
) -> Result<SpawnedProcess> {
    spawn_process_with_stdin_mode(
        program,
        args,
        cwd,
        env,
        arg0,
        PipeStdinMode::Piped,
        &[],
        sandbox_profile,
    )
    .await
}

/// Spawn a process using regular pipes, but close stdin immediately.
pub async fn spawn_process_no_stdin(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
) -> Result<SpawnedProcess> {
    spawn_process_no_stdin_with_inherited_fds(program, args, cwd, env, arg0, &[]).await
}

/// Spawn a process using regular pipes, close stdin immediately, and preserve
/// selected inherited file descriptors across exec on Unix.
pub async fn spawn_process_no_stdin_with_inherited_fds(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
    inherited_fds: &[i32],
) -> Result<SpawnedProcess> {
    spawn_process_with_stdin_mode(
        program,
        args,
        cwd,
        env,
        arg0,
        PipeStdinMode::Null,
        inherited_fds,
        None,
    )
    .await
}

/// Spawn a process using regular pipes, close stdin immediately, and preserve
/// selected inherited file descriptors across exec on Unix. The child process
/// applies the named sandbox profile before `exec`.
pub async fn spawn_process_no_stdin_sandboxed(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
    inherited_fds: &[i32],
    sandbox_profile: Option<String>,
) -> Result<SpawnedProcess> {
    spawn_process_with_stdin_mode(
        program,
        args,
        cwd,
        env,
        arg0,
        PipeStdinMode::Null,
        inherited_fds,
        sandbox_profile,
    )
    .await
}
